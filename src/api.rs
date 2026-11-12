//! Versioned consumer boundary. Storage and worker protocols are private adapters.
use anyhow::{Context, Result, bail, ensure};
use ontokiln_reasoner::{Reasoner, parameters, sparql};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub const CONTRACT: &str = "ontokiln-api/v1";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub contract: String,
    pub request_id: String,
    pub operation: Operation,
    #[serde(default)]
    pub limits: Limits,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Capabilities {
        dataset: Option<PathBuf>,
    },
    Compile {
        sources: Vec<Source>,
        output: PathBuf,
        profile: String,
        #[serde(default)]
        rules: Vec<RuleFile>,
    },
    Materialize {
        dataset: PathBuf,
        output: PathBuf,
    },
    Query {
        dataset: PathBuf,
        query: String,
        #[serde(default)]
        parameters: BTreeMap<String, parameters::Parameter>,
        #[serde(default)]
        view: View,
        revision: Option<String>,
        #[serde(default)]
        require: Vec<String>,
    },
    Validate {
        dataset: PathBuf,
        #[serde(default)]
        shapes: Vec<Source>,
        #[serde(default)]
        shapes_from_dataset: bool,
        selected_shapes: Option<Vec<String>>,
        focus_node: Option<String>,
    },
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    #[default]
    Asserted,
    Inferred,
    Closure,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub path: PathBuf,
    pub document_id: String,
    pub format: String,
    pub expected_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_iri: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuleFile {
    pub path: PathBuf,
    pub expected_sha256: String,
}

#[derive(Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub memory_bytes: u64,
    pub scratch_bytes: u64,
    pub timeout_ms: u64,
    pub max_rows: usize,
    pub max_bytes: usize,
    pub max_iterations: u64,
    pub max_derived_facts: u64,
    pub max_intermediate_rows: u64,
    pub max_validation_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_bytes: 512 * 1024 * 1024,
            scratch_bytes: 4 * 1024 * 1024 * 1024,
            timeout_ms: 300_000,
            max_rows: 100_000,
            max_bytes: 4 * 1024 * 1024,
            max_iterations: 100,
            max_derived_facts: 1_000_000,
            max_intermediate_rows: 10_000_000,
            max_validation_depth: 64,
        }
    }
}

fn invalid(error: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("INVALID_REQUEST|{error}")
}

impl Request {
    fn internal(&self) -> Result<Value> {
        ensure!(
            self.contract == CONTRACT,
            "UNSUPPORTED_CONTRACT|expected {CONTRACT}"
        );
        ensure!(
            !self.request_id.is_empty() && self.request_id.len() <= 256,
            "INVALID_REQUEST|request_id must contain 1..256 UTF-8 bytes"
        );
        let l = &self.limits;
        ensure!(
            l.memory_bytes > 0
                && l.scratch_bytes > 0
                && l.timeout_ms > 0
                && l.max_rows <= sparql::MAX_ROWS
                && l.max_bytes <= sparql::MAX_BYTES
                && l.max_validation_depth > 0,
            "INVALID_REQUEST|invalid limits"
        );
        let mut v = json!({"operation":"api", "_api_request":self,
            "memory_budget_bytes":l.memory_bytes, "disk_budget_bytes":l.scratch_bytes,
            "timeout_seconds":l.timeout_ms as f64 / 1000.0});
        match &self.operation {
            Operation::Compile {
                sources,
                output,
                profile,
                rules,
            } => {
                ensure!(
                    [
                        "no-rules-v1",
                        "datalog-v1",
                        "rdfs-supported-v1",
                        "rdfs-property-chain-v1"
                    ]
                    .contains(&profile.as_str()),
                    "UNSUPPORTED_CAPABILITY|unknown rule profile"
                );
                v["operation"] = json!("compile");
                v["sources"] = json!(sources);
                v["max_term_bytes"] = json!(1024 * 1024);
                v["artifact"] = json!({"output":output,"profile_id":profile,"index_profile":"core-v1","rules_files":rules});
            }
            Operation::Materialize { dataset, output } => {
                v["operation"] = json!("materialize");
                v["artifact"] = json!({"compiled":dataset,"output":output});
                v["max_iterations"] = json!(l.max_iterations);
                v["max_derived_facts"] = json!(l.max_derived_facts);
                v["max_intermediate_rows"] = json!(l.max_intermediate_rows);
            }
            _ => {}
        }
        Ok(v)
    }
}

fn failure(code: &str, message: &str) -> Value {
    let (code, status) = match code {
        "INVALID_REQUEST" | "SOURCE_INVALID" | "RULE_INVALID" => ("INVALID_REQUEST", "FAILED"),
        "UNSUPPORTED_CONTRACT" => ("UNSUPPORTED_CONTRACT", "UNSUPPORTED"),
        "UNSUPPORTED_CAPABILITY" | "UNSUPPORTED_RULE_PROFILE" | "FORMAT_VERSION_UNSUPPORTED" => {
            ("UNSUPPORTED_CAPABILITY", "UNSUPPORTED")
        }
        "INVALID_DATASET" | "MANIFEST_CORRUPT" => ("INVALID_DATASET", "FAILED"),
        "INVALID_QUERY" | "QUERY_FAILED" => ("INVALID_QUERY", "FAILED"),
        "CANCELLED" => ("CANCELLED", "CANCELLED"),
        "RESOURCE_LIMIT_EXCEEDED" | "BUDGET_EXHAUSTED" | "FIXPOINT_LIMIT_EXCEEDED" => {
            ("RESOURCE_EXHAUSTED", "BUDGET_EXHAUSTED")
        }
        _ => ("EXECUTION_FAILED", "FAILED"),
    };
    json!({"status":status,"result":null,"error":{"code":code,"message":message}})
}

/// Execute the stable file protocol through the existing supervised worker.
/// Domain failures are terminal JSON responses; result-file I/O errors are process failures.
pub fn run_files(request: &Path, result: &Path) -> Result<()> {
    ensure!(
        fs::metadata(request)?.len() <= 4 * 1024 * 1024,
        "API request exceeds 4 MiB"
    );
    let bytes = fs::read(request)?;
    let raw: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let internal = serde_json::from_slice::<Request>(&bytes)
        .map_err(invalid)
        .and_then(|req| req.internal());
    let internal = match internal {
        Ok(value) => value,
        Err(error) => {
            let text = error.to_string();
            let (code, message) = text.split_once('|').unwrap_or(("EXECUTION_FAILED", &text));
            return crate::full_execution::write_atomic(
                result,
                &envelope(
                    failure(code, message),
                    &raw,
                    &bytes,
                    &json!({"worker_reaped":true,"worker_pid":null}),
                ),
            );
        }
    };
    let directory = tempfile::tempdir()?;
    let input = directory.path().join("request.json");
    fs::write(&input, serde_json::to_vec(&internal)?)?;
    crate::full_execution::run_files_with(&input, result, |reply, path| {
        let execution = &reply["execution"];
        ensure!(
            execution["worker_reaped"] == true,
            "API worker was not reaped"
        );
        let response = if let Some(error) = reply.get("error") {
            failure(
                error["code"].as_str().unwrap_or("EXECUTION_FAILED"),
                error["message"].as_str().unwrap_or("execution failed"),
            )
        } else {
            ensure!(
                execution["status"] == "COMPLETE" && reply["api_result"].is_object(),
                "invalid API worker terminal record"
            );
            json!({"status":"COMPLETE","result":reply["api_result"],"error":null})
        };
        crate::full_execution::write_atomic(path, &envelope(response, &raw, &bytes, execution))
    })
}

fn envelope(mut response: Value, raw: &Value, bytes: &[u8], execution: &Value) -> Value {
    response["contract"] = json!(CONTRACT);
    response["request_id"] = raw
        .get("request_id")
        .filter(|v| v.is_string())
        .cloned()
        .unwrap_or(Value::Null);
    response["request_sha256"] = json!(crate::full_artifact::digest(bytes));
    response["execution"] =
        json!({"worker_reaped":execution["worker_reaped"],"worker_pid":execution["worker_pid"]});
    response["kernel"] = crate::identity::kernel();
    response
}

fn dataset(path: &Path) -> Result<(Value, Option<ontokiln_pack::Pack>)> {
    if path.is_file() {
        let pack =
            ontokiln_pack::Pack::load(path).map_err(|e| anyhow::anyhow!("INVALID_DATASET|{e}"))?;
        Ok((
            json!({"semantics":"pack-rdfs-hierarchy/v1","capabilities":["select-ask/v1","views/v1","document-graphs/v1","revision/v1"],
            "default_graph":"union","views":["asserted","inferred","closure"],"profile":"rdfs-hierarchy-v1","facts":pack.triple_count()}),
            Some(pack),
        ))
    } else {
        let manifest = crate::full_artifact::verify(path)
            .map_err(|e| anyhow::anyhow!("INVALID_DATASET|{e}"))?;
        ensure!(
            matches!(manifest["kind"].as_str(), Some("compiled" | "reasoned")),
            "INVALID_DATASET|unknown snapshot kind"
        );
        ensure!(
            manifest["kind"] != "reasoned" || manifest["fixpoint_reached"] == true,
            "INVALID_DATASET|reasoned snapshot has no completed fixpoint"
        );
        let views = if manifest["kind"] == "reasoned" || manifest["profile_id"] == "no-rules-v1" {
            json!(["asserted", "inferred", "closure"])
        } else {
            json!(["asserted"])
        };
        Ok((
            json!({"semantics":"rdf-dataset-union/v1","capabilities":["select-ask/v1","views/v1","rdf-named-graphs/v1","shacl/v1"],
            "default_graph":"union","views":views,"profile":manifest["profile_id"],"manifest":manifest}),
            None,
        ))
    }
}

fn snapshot(path: &Path, meta: &Value, target: &Path) -> Result<()> {
    crate::full_artifact::freeze(
        &path.join("facts.sqlite"),
        &target.join("facts.sqlite"),
        meta["manifest"]["files"]["facts.sqlite"]["sha256"]
            .as_str()
            .context("snapshot digest")?,
    )
    .map_err(|e| anyhow::anyhow!("INVALID_DATASET|{e}"))?;
    Ok(())
}

/// Called only by the supervised worker; not an alternative public execution API.
pub(crate) fn execute_worker(value: Value) -> Result<Value> {
    let req: Request = serde_json::from_value(value["_api_request"].clone()).map_err(invalid)?;
    req.internal()?;
    let limits = &req.limits;
    let result = match &req.operation {
        Operation::Compile { output, .. } | Operation::Materialize { output, .. } => {
            let mut reply = crate::full_artifact::compute(value.clone())?;
            reply["api_result"] = json!({"dataset":output,"stats":{
                "explicit_facts":reply["stats"]["explicit_facts"],
                "inferred_facts":reply["stats"].get("inferred_facts").cloned().unwrap_or(json!(0))},
                "fixpoint_reached":reply.get("fixpoint_reached").cloned().unwrap_or(json!(false)),
                "proof_coverage":{"per_fact":"not_recorded"}});
            return Ok(reply);
        }
        Operation::Capabilities { dataset: path } => {
            let mut info = json!({"operations":["capabilities","compile","materialize","query","validate"],
                "default_view":"asserted","proof_coverage":{"per_fact":"not_recorded"}});
            if let Some(path) = path {
                let (mut meta, _) = dataset(path)?;
                meta.as_object_mut()
                    .context("dataset metadata")?
                    .remove("manifest");
                info["dataset"] = meta;
            }
            info
        }
        Operation::Query {
            dataset: path,
            query,
            parameters,
            view,
            revision,
            require,
        } => {
            crate::query::local_query(query).map_err(|e| anyhow::anyhow!("INVALID_QUERY|{e}"))?;
            let parsed =
                parameters::bind(sparql::SparqlParser::new().parse_query(query)?, parameters)
                    .map_err(invalid)?;
            let (meta, pack) = dataset(path)?;
            ensure!(
                meta["views"]
                    .as_array()
                    .context("dataset views")?
                    .contains(&json!(view)),
                "UNSUPPORTED_CAPABILITY|materialize this dataset before querying inferred or closure views"
            );
            for capability in require {
                ensure!(
                    meta["capabilities"]
                        .as_array()
                        .context("capabilities")?
                        .contains(&json!(capability)),
                    "UNSUPPORTED_CAPABILITY|required {capability}"
                );
            }
            let mut data = if let Some(pack) = pack {
                let engine = Reasoner::new(pack);
                let options = sparql::Options {
                    view: serde_json::from_value(json!(view))?,
                    revision: revision.clone(),
                    max_rows: limits.max_rows,
                    ..Default::default()
                };
                let mut bytes = Vec::new();
                engine
                    .write_sparql(
                        &parsed,
                        vec![],
                        &Default::default(),
                        &options,
                        false,
                        &mut bytes,
                    )
                    .map_err(|e| {
                        if e.to_string().contains("budget") {
                            anyhow::anyhow!("BUDGET_EXHAUSTED|{e}")
                        } else {
                            anyhow::anyhow!("INVALID_QUERY|{e}")
                        }
                    })?;
                let reply: Value = serde_json::from_slice(&bytes)?;
                json!({"query_type":if reply.get("boolean").is_some(){"ASK"}else{"SELECT"},
                    "variables":reply["head"].get("vars").cloned().unwrap_or(json!([])),
                    "items":reply["results"].get("bindings").cloned().unwrap_or(json!([])),"boolean":reply["boolean"]})
            } else {
                ensure!(
                    revision.is_none(),
                    "UNSUPPORTED_CAPABILITY|revision selection is not available for this dataset"
                );
                let copy = tempfile::tempdir()?;
                snapshot(path, &meta, copy.path())?;
                crate::full::execute(
                    json!({"operation":"query","dataset":copy.path().join("facts.sqlite"),
                    "query":query,"parameters":parameters,"view":view,"max_rows":limits.max_rows,
                    "max_bytes":limits.max_bytes,"query_seconds":null}),
                )
                .map_err(anyhow::Error::msg)?
            };
            normalize_query(&mut data, limits)?;
            json!({"query_type":data["query_type"],"variables":data["variables"],"items":data["items"],
                "boolean":data["boolean"],"view":view,"complete":true,"truncated":false,
                "proof_coverage":{"per_fact":"not_recorded"}})
        }
        Operation::Validate {
            dataset: path,
            shapes,
            shapes_from_dataset,
            selected_shapes,
            focus_node,
        } => {
            ensure!(
                !path.is_file(),
                "UNSUPPORTED_CAPABILITY|SHACL requires an RDF dataset artifact"
            );
            ensure!(
                !*shapes_from_dataset || shapes.is_empty(),
                "INVALID_REQUEST|choose dataset shapes or external shapes"
            );
            let (meta, pack) = dataset(path)?;
            ensure!(
                pack.is_none(),
                "UNSUPPORTED_CAPABILITY|SHACL requires an RDF dataset artifact"
            );
            let copy = tempfile::tempdir()?;
            snapshot(path, &meta, copy.path())?;
            let mut frozen = Vec::new();
            for (i, source) in shapes.iter().enumerate() {
                let dest = copy.path().join(format!("shape-{i}.rdf"));
                crate::full_artifact::freeze(&source.path, &dest, &source.expected_sha256)
                    .map_err(invalid)?;
                let mut s = json!({"file":dest,"format":source.format});
                if let Some(base) = &source.base_iri {
                    s["base_iri"] = json!(base);
                }
                frozen.push(s);
            }
            let mut input = json!({"operation":"validate","dataset":copy.path(),"shapes":frozen,
                "shapes_from_dataset":shapes_from_dataset,"max_validation_depth":limits.max_validation_depth});
            if let Some(selected) = selected_shapes {
                input["selected_shapes"] = json!(selected);
            }
            if let Some(focus) = focus_node {
                input["focus_node"] = json!(focus);
            }
            let reply = crate::full::execute(input).map_err(anyhow::Error::msg)?;
            json!({"conforms":reply["conforms"],"checked_shapes":reply["checked_shapes"],
                "report":serde_json::from_str::<Value>(reply["report"].as_str().context("SHACL report")?)?})
        }
    };
    Ok(json!({"api_result":result,"metrics":{}}))
}

fn normalize_query(data: &mut Value, limits: &Limits) -> Result<()> {
    let rows = data["items"].as_array_mut().context("query rows")?;
    ensure!(
        rows.len() <= limits.max_rows,
        "BUDGET_EXHAUSTED|output row limit"
    );
    for row in rows.iter_mut() {
        for term in row.as_object_mut().context("query row")?.values_mut() {
            match term["type"].as_str() {
                Some("uri" | "bnode") => {}
                Some("literal") => {
                    if term.get("datatype").is_none() && term.get("xml:lang").is_none() {
                        term["datatype"] = json!("http://www.w3.org/2001/XMLSchema#string");
                    }
                }
                _ => bail!("UNSUPPORTED_CAPABILITY|unsupported result RDF term"),
            }
        }
    }
    if data["query_type"] == "ASK" {
        ensure!(limits.max_rows > 0, "BUDGET_EXHAUSTED|ASK output row limit");
    }
    let payload = json!({"query_type":data["query_type"],"variables":data["variables"],
        "items":data["items"],"boolean":data["boolean"]});
    ensure!(
        serde_json::to_vec(&payload)?.len() <= limits.max_bytes,
        "BUDGET_EXHAUSTED|output byte limit"
    );
    Ok(())
}
