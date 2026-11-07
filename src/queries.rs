use crate::worker::{Job, Operation};
use crate::{ApiError, App, error};
use anyhow::{Context, Result, ensure};
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use ontokiln_reasoner::{
    parameters::Parameter,
    sparql::{self, Options, Query, SparqlParser},
};
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path, sync::Arc};

pub struct Template {
    text: String,
    media: &'static str,
}
pub type Templates = BTreeMap<String, Arc<Template>>;

pub fn templates(dir: Option<&Path>) -> Result<Templates> {
    let mut templates = Templates::new();
    if let Some(dir) = dir {
        for entry in dir
            .read_dir()
            .with_context(|| format!("open query directory {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_file() || path.extension().is_none_or(|e| e != "rq") {
                continue;
            }
            let name = path
                .file_stem()
                .and_then(|n| n.to_str())
                .context("invalid template filename")?;
            let text = std::fs::read_to_string(&path)?;
            let parsed = SparqlParser::new()
                .parse_query(&text)
                .with_context(|| format!("parse {}", path.display()))?;
            templates.insert(
                name.to_owned(),
                Arc::new(Template {
                    text,
                    media: sparql::media(&parsed, false),
                }),
            );
        }
        ensure!(
            !templates.is_empty(),
            "query directory contains no .rq templates"
        );
    }
    Ok(templates)
}

pub fn read_query(path: &Path) -> Result<Query> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    SparqlParser::new()
        .parse_query(&text)
        .with_context(|| format!("parse {}", path.display()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryRequest {
    template: String,
    #[serde(default)]
    bindings: BTreeMap<String, String>,
    #[serde(default)]
    parameters: BTreeMap<String, Parameter>,
    #[serde(default)]
    options: Options,
    format: Option<String>,
    request_id: Option<String>,
    memory_bytes: Option<usize>,
    deadline_ms: Option<u64>,
}

pub async fn cancel(
    State(app): State<App>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<StatusCode, ApiError> {
    if app.jobs.cancel(&id) {
        Ok(StatusCode::ACCEPTED)
    } else {
        Err(error(StatusCode::NOT_FOUND, "unknown active query"))
    }
}
pub async fn sources(State(app): State<App>) -> impl IntoResponse {
    Json(
        app.engine
            .documents()
            .iter()
            .map(|d| {
                let mut v = serde_json::json!(d);
                v["graph"] = d.graph().into();
                v
            })
            .collect::<Vec<_>>(),
    )
}

pub async fn sparql(
    State(app): State<App>,
    Json(req): Json<QueryRequest>,
) -> Result<Response, ApiError> {
    let deadline = crate::execution::timeout(req.deadline_ms.unwrap_or(5000))?;
    let heap = crate::execution::heap(req.memory_bytes)?;
    let invalid = |e: anyhow::Error| error(StatusCode::BAD_REQUEST, e.to_string());
    let q = app
        .templates
        .get(&req.template)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "unknown SPARQL template"))?;
    let jsonl = match req.format.as_deref() {
        None | Some("json") => false,
        Some("jsonl") => true,
        _ => {
            return Err(error(
                StatusCode::BAD_REQUEST,
                "format must be json or jsonl",
            ));
        }
    };
    req.options.validate(jsonl).map_err(invalid)?;
    let media = if (req.options.profile || req.options.counters) && !jsonl {
        "application/json"
    } else if jsonl {
        "application/x-ndjson"
    } else {
        q.media
    };
    let job = crate::worker::Job {
        operation: Operation::Sparql {
            query: q.text.clone(),
            bindings: req.bindings,
            parameters: req.parameters,
        },
        options: req.options,
        jsonl,
    };
    crate::execution::run(app, job, req.request_id, heap, deadline, media).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityRequest {
    #[serde(flatten)]
    controls: Controls,
    revision: Option<String>,
    types: Vec<String>,
    #[serde(default)]
    subclasses: bool,
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    100
}

pub async fn entities(
    State(app): State<App>,
    Json(req): Json<EntityRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if req.types.is_empty() || req.types.len() > 32 || req.limit > 1000 {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "expected 1..32 types and limit <= 1000",
        ));
    }
    let types = req
        .types
        .iter()
        .map(|s| crate::iri(s))
        .collect::<Result<Vec<_>, _>>()?;
    native(
        app,
        Operation::Entities {
            types,
            subclasses: req.subclasses,
            limit: req.limit,
        },
        req.revision,
        req.controls,
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchRequest {
    #[serde(flatten)]
    controls: Controls,
    #[serde(default)]
    view: ontokiln_reasoner::View,
    revision: Option<String>,
    subjects: Vec<String>,
    predicate: String,
    #[serde(default)]
    explain: bool,
}

pub async fn batch(
    State(app): State<App>,
    Json(req): Json<BatchRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if !(1..=16).contains(&req.subjects.len()) {
        return Err(error(StatusCode::BAD_REQUEST, "expected 1..16 subjects"));
    }
    let subjects = req
        .subjects
        .iter()
        .map(|s| crate::iri(s))
        .collect::<Result<Vec<_>, _>>()?;
    let predicate = crate::iri(&req.predicate)?;
    native(
        app,
        Operation::Subjects {
            subjects,
            predicate,
            explain: req.explain,
            view: req.view,
            batch: true,
        },
        req.revision,
        req.controls,
    )
    .await
}

#[derive(Default, Deserialize)]
pub struct Controls {
    pub request_id: Option<String>,
    pub deadline_ms: Option<u64>,
    pub memory_bytes: Option<usize>,
    pub max_bytes: Option<usize>,
    pub max_visits: Option<usize>,
    pub max_rows: Option<usize>,
}
pub async fn native(
    app: App,
    operation: Operation,
    revision: Option<String>,
    limits: Controls,
) -> Result<Response, ApiError> {
    let deadline = crate::execution::timeout(limits.deadline_ms.unwrap_or(5000))?;
    let heap = crate::execution::heap(limits.memory_bytes)?;
    let options = Options {
        revision,
        max_bytes: limits.max_bytes.unwrap_or(sparql::MAX_BYTES),
        max_rows: limits.max_rows.unwrap_or(sparql::MAX_ROWS),
        max_visits: limits.max_visits.unwrap_or(sparql::MAX_VISITS),
        ..Default::default()
    };
    options
        .validate(false)
        .map_err(|e| error(StatusCode::BAD_REQUEST, e.to_string()))?;
    crate::execution::run(
        app,
        Job {
            operation,
            options,
            jsonl: false,
        },
        limits.request_id,
        heap,
        deadline,
        "application/json",
    )
    .await
}
