use crate::{ApiError, App, error};
use anyhow::{Context, Result, ensure};
use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use ontokiln_reasoner::sparql::{CancellationToken, Query, SparqlParser};
use oxrdf::{Term, Variable};
use serde::Deserialize;
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};

pub type Templates = BTreeMap<String, Arc<Query>>;

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
            let query = read_query(&path)?;
            templates.insert(name.to_owned(), Arc::new(query));
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
}

struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub async fn sparql(
    State(app): State<App>,
    Json(req): Json<QueryRequest>,
) -> Result<Response, ApiError> {
    let query = app
        .templates
        .get(&req.template)
        .cloned()
        .ok_or_else(|| error(StatusCode::NOT_FOUND, "unknown SPARQL template"))?;
    let bindings = req
        .bindings
        .into_iter()
        .map(|(name, value)| {
            let var = Variable::new(name)
                .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid variable name"))?;
            let term = value
                .parse::<Term>()
                .map_err(|_| error(StatusCode::BAD_REQUEST, "binding must be an N-Triples term"))?;
            Ok((var, term))
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    let slot = app.slot()?;
    let guard = CancelOnDrop(CancellationToken::new());
    let token = guard.0.clone();
    let worker = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        app.engine.sparql(&query, bindings, token)
    });
    let output = tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .map_err(|_| error(StatusCode::REQUEST_TIMEOUT, "SPARQL deadline exceeded"))?
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "query worker failed"))?
        .map_err(|e| error(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
    Ok(([(header::CONTENT_TYPE, output.media_type)], output.bytes).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityRequest {
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
    let slot = app.slot()?;
    let result = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        app.engine.entities(&types, req.subclasses, req.limit)
    })
    .await
    .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "query worker failed"))?
    .map_err(|e| error(StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
    Ok(Json(result))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchRequest {
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
    let slot = app.slot()?;
    let answers = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        app.engine.query_batch(&subjects, &predicate, req.explain)
    })
    .await
    .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "query worker failed"))?
    .map_err(|e| error(StatusCode::UNPROCESSABLE_ENTITY, e))?;
    Ok(Json(
        answers
            .into_iter()
            .map(|results| crate::Response { results })
            .collect::<Vec<_>>(),
    ))
}
