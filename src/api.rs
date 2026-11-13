//! Versioned consumer boundary; optional read artifacts have their own format contract.
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
pub const READ_CONTRACT: &str = "ontokiln-sqlite-read/v1";
const CONTENT_DIGEST_CAPABILITY: &str = "dataset-content-digest/v1";
const MANIFEST_DIGEST_CAPABILITY: &str = "dataset-manifest-digest/v1";

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
    PrepareQuery {
        query: String,
    },
    Rules {
        profile: String,
        files: Vec<RuleFile>,
    },
    Compile {
        sources: Vec<Source>,
        output: PathBuf,
        profile: String,
        #[serde(default)]
        rules: Vec<RuleFile>,
        import_lock: Option<BTreeMap<String, ImportTargets>>,
        expected_source_tree_digest: Option<String>,
    },
    Materialize {
        dataset: PathBuf,
        output: PathBuf,
    },
    Query {
        dataset: PathBuf,
        #[serde(skip_serializing_if = "Option::is_none")]
        expected_content_digest: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        expected_manifest_sha256: Option<String>,
        query: String,
        #[serde(default)]
        parameters: BTreeMap<String, parameters::Parameter>,
        #[serde(default)]
        rdf_bindings: BTreeMap<String, String>,
        #[serde(default)]
        values: Vec<parameters::ValuesTable>,
        #[serde(default)]
        derived_queries: Vec<String>,
        #[serde(default)]
        view: View,
        revision: Option<String>,
        #[serde(default)]
        require: Vec<String>,
    },
    Validate {
        dataset: PathBuf,
        #[serde(skip_serializing_if = "Option::is_none")]
        expected_content_digest: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        expected_manifest_sha256: Option<String>,
        #[serde(default)]
        shapes: Vec<Source>,
        #[serde(default)]
        shapes_from_dataset: bool,
        selected_shapes: Option<Vec<String>>,
        focus_node: Option<String>,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
pub enum ImportTargets {
    One(String),
    Many(Vec<String>),
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
    pub memory_bytes: Option<u64>,
    pub scratch_bytes: Option<u64>,
    pub timeout_ms: Option<f64>,
    pub max_rows: usize,
    pub max_bytes: usize,
    pub max_iterations: u64,
    pub max_derived_facts: u64,
    pub max_intermediate_rows: u64,
    pub max_validation_depth: usize,
    pub max_term_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_bytes: Some(512 * 1024 * 1024),
            scratch_bytes: Some(4 * 1024 * 1024 * 1024),
            timeout_ms: Some(300_000.0),
            max_rows: 100_000,
            max_bytes: 4 * 1024 * 1024,
            max_iterations: 100,
            max_derived_facts: 1_000_000,
            max_intermediate_rows: 10_000_000,
            max_validation_depth: 64,
            max_term_bytes: 1024 * 1024,
        }
    }
}

fn invalid(error: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("INVALID_REQUEST|{error}")
}

// Negotiate the envelope before applying a version-specific operation schema.
// Keep deserializing v1 from the original bytes so duplicate fields are rejected.
fn parse_request(bytes: &[u8], raw: &Value) -> Result<Request> {
    let contract = raw
        .get("contract")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("contract must be a string"))?;
    ensure!(
        contract == CONTRACT,
        "UNSUPPORTED_CONTRACT|expected {CONTRACT}"
    );
    serde_json::from_slice(bytes).map_err(invalid)
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
            l.memory_bytes.is_none_or(|v| v > 0)
                && l.scratch_bytes.is_none_or(|v| v > 0)
                && l.timeout_ms.is_none_or(|v| v.is_finite() && v > 0.0)
                && l.max_term_bytes > 0
                && l.max_validation_depth > 0,
            "INVALID_REQUEST|invalid limits"
        );
        if let Operation::Query {
            expected_content_digest,
            expected_manifest_sha256,
            ..
        }
        | Operation::Validate {
            expected_content_digest,
            expected_manifest_sha256,
            ..
        } = &self.operation
        {
            for (field, expected) in [
                ("expected_content_digest", expected_content_digest),
                ("expected_manifest_sha256", expected_manifest_sha256),
            ] {
                if let Some(expected) = expected {
                    ensure!(
                        expected.len() == 64
                            && expected
                                .bytes()
                                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
                        "INVALID_REQUEST|{field} must be 64 lowercase hexadecimal digits"
                    );
                }
            }
        }
        let mut v = json!({"operation":"api", "_api_request":self,
            "memory_budget_bytes":l.memory_bytes, "disk_budget_bytes":l.scratch_bytes,
            "timeout_seconds":l.timeout_ms.map(|v| v / 1000.0)});
        match &self.operation {
            Operation::Compile {
                sources,
                output,
                profile,
                rules,
                import_lock,
                expected_source_tree_digest,
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
                v["max_term_bytes"] = json!(l.max_term_bytes);
                v["import_lock"] = json!(import_lock);
                v["artifact"] = json!({"output":output,"profile_id":profile,"index_profile":"core-v1","rules_files":rules,"expected_source_tree_digest":expected_source_tree_digest});
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
    let reason = code;
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
    json!({"status":status,"result":null,"error":{"code":code,"message":message,"reason":reason}})
}

/// Execute the stable file protocol through the existing supervised worker.
/// Domain failures are terminal JSON responses; result-file I/O errors are process failures.
pub fn run_files(request: &Path, result: &Path) -> Result<()> {
    run_files_on_snapshot(request, result, None)
}

/// The session adapter owns a verified snapshot; original v1 bytes remain the
/// consumer identity and are never rewritten to an internal descriptor path.
pub(crate) fn run_files_on_snapshot(
    request: &Path,
    result: &Path,
    snapshot: Option<&crate::full_snapshot::FullSnapshot>,
) -> Result<()> {
    let bytes = fs::read(request)?;
    let raw: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let internal = parse_request(&bytes, &raw).and_then(|req| {
        let mut internal = req.internal()?;
        if let Some(snapshot) = snapshot {
            let path = match &req.operation {
                Operation::Query { dataset, .. } | Operation::Validate { dataset, .. } => {
                    Some(dataset)
                }
                Operation::Capabilities { dataset } => dataset.as_ref(),
                Operation::PrepareQuery { .. } => None,
                _ => bail!("UNSUPPORTED_CAPABILITY|sessions support read operations only"),
            };
            if let Some(path) = path {
                ensure!(
                    std::path::absolute(path).map_err(invalid)? == snapshot.source_path(),
                    "INVALID_DATASET|request dataset differs from the mounted snapshot"
                );
            }
            internal["_mounted_snapshot"] = json!({
                "source_path": snapshot.source_path(),
                // Readers only need semantic view metadata. Do not serialize
                // the entire source/file inventory for every hot request.
                "manifest": {
                    "kind": snapshot.manifest()["kind"],
                    "profile_id": snapshot.manifest()["profile_id"],
                    "fixpoint_reached": snapshot.manifest()["fixpoint_reached"],
                    "content_digest": snapshot.manifest()["content_digest"],
                },
                "manifest_sha256": snapshot.manifest_sha256(),
                "descriptor": snapshot.descriptor(),
                "bytes": snapshot.database_bytes(),
                "strict_terms": snapshot.strict_terms(),
            });
        }
        Ok(internal)
    });
    let internal = match internal {
        Ok(mut value) => {
            value["_api_request_sha256"] = json!(crate::full_artifact::digest(&bytes));
            value
        }
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
    crate::full_execution::run_files_with_snapshot(&input, result, snapshot, |reply, path| {
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
        let mut response = envelope(response, &raw, &bytes, execution);
        response["execution"]["metrics"] = reply["metrics"].clone();
        if execution["artifact"].is_object() {
            response["result"]["artifact"] = execution["artifact"].clone();
            response["result"]["read_contract"] = json!(READ_CONTRACT);
        }
        crate::full_execution::write_atomic(path, &response)
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
    response["execution"] = json!({"worker_reaped":execution["worker_reaped"],"worker_pid":execution["worker_pid"],
            "supervisor_pid":execution["supervisor_pid"],"integrity_checked":execution["integrity_checked"]});
    response["kernel"] = crate::identity::kernel();
    response
}

struct Dataset {
    meta: Value,
    pack: Option<ontokiln_pack::Pack>,
    pack_v4: Option<ontokiln_pack::v4::PackV4>,
    manifest_sha256: Option<String>,
}

fn dataset(path: &Path) -> Result<Dataset> {
    if path.is_file() {
        use std::io::Read;
        // V3 and V4 share magic but are different physical layouts.
        let mut header = [0_u8; 12];
        fs::File::open(path)?.read_exact(&mut header)?;
        if &header[..8] == b"ONTOKILN" && u32::from_le_bytes(header[8..12].try_into()?) == 4 {
            let pack = ontokiln_pack::v4::PackV4::load(path)
                .map_err(|e| anyhow::anyhow!("INVALID_DATASET|{e}"))?;
            let metadata = json!({
                "semantics":"rdf-dataset-union/v1",
                "capabilities":["select-ask/v1","views/v1","rdf-named-graphs/v1",
                    "shacl/v1","rdf-bindings/v1","values-bindings/v1",
                    "derived-query-data/v1"],
                "default_graph":"union",
                "views":["asserted","inferred","closure"],
                "profile":pack.metadata().profile_id,
                "format":"ontokiln-pack/v4",
                "facts":pack.quad_count()
            });
            return Ok(Dataset {
                meta: metadata,
                pack: None,
                pack_v4: Some(pack),
                manifest_sha256: None,
            });
        }
        let pack =
            ontokiln_pack::Pack::load(path).map_err(|e| anyhow::anyhow!("INVALID_DATASET|{e}"))?;
        Ok(Dataset {
            meta: json!({"semantics":"pack-rdfs-hierarchy/v1","capabilities":["select-ask/v1","views/v1","document-graphs/v1","revision/v1"],
            "default_graph":"union","views":["asserted","inferred","closure"],"profile":"rdfs-hierarchy-v1","facts":pack.triple_count()}),
            pack: Some(pack),
            pack_v4: None,
            manifest_sha256: None,
        })
    } else {
        let (manifest, manifest_sha256) = crate::full_artifact::verify_read_manifest(path)
            .map_err(|e| anyhow::anyhow!("INVALID_DATASET|{e}"))?;
        Ok(Dataset {
            meta: full_metadata(manifest)?,
            pack: None,
            pack_v4: None,
            manifest_sha256: Some(manifest_sha256),
        })
    }
}

fn full_metadata(manifest: Value) -> Result<Value> {
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
    Ok(
        json!({"semantics":"rdf-dataset-union/v1","capabilities":["select-ask/v1","views/v1","rdf-named-graphs/v1","shacl/v1","rdf-bindings/v1","values-bindings/v1","derived-query-data/v1",READ_CONTRACT,CONTENT_DIGEST_CAPABILITY,MANIFEST_DIGEST_CAPABILITY],
        "default_graph":"union","views":views,"profile":manifest["profile_id"],"read_contract":READ_CONTRACT,"manifest":manifest}),
    )
}

fn worker_dataset(path: &Path, request: &Value) -> Result<Dataset> {
    if let Some(mounted) = request.get("_mounted_snapshot") {
        ensure!(
            std::path::absolute(path)?
                == Path::new(mounted["source_path"].as_str().context("mounted dataset")?),
            "INVALID_DATASET|request dataset differs from the mounted snapshot"
        );
        let descriptor = mounted["descriptor"]
            .as_i64()
            .context("snapshot descriptor")?;
        let descriptor = i32::try_from(descriptor).context("snapshot descriptor range")?;
        crate::full_snapshot::validate_descriptor(
            descriptor,
            mounted["bytes"].as_u64().context("snapshot bytes")?,
        )?;
        Ok(Dataset {
            meta: full_metadata(mounted["manifest"].clone())?,
            pack: None,
            pack_v4: None,
            manifest_sha256: Some(
                mounted["manifest_sha256"]
                    .as_str()
                    .context("mounted manifest digest")?
                    .to_owned(),
            ),
        })
    } else {
        dataset(path)
    }
}

struct ReadSnapshot {
    _owned: Option<crate::full_snapshot::FullSnapshot>,
    database: PathBuf,
    strict_terms: bool,
}

fn read_snapshot(
    path: &Path,
    meta: &Value,
    manifest_sha256: Option<&str>,
    request: &Value,
    max_bytes: u64,
) -> Result<ReadSnapshot> {
    if let Some(mounted) = request.get("_mounted_snapshot") {
        let descriptor = mounted["descriptor"]
            .as_i64()
            .context("snapshot descriptor")?;
        Ok(ReadSnapshot {
            _owned: None,
            database: format!("ontokiln-fd:{descriptor}").into(),
            strict_terms: mounted["strict_terms"].as_bool() == Some(true),
        })
    } else {
        let reservation = Path::new(
            request["_request_directory"]
                .as_str()
                .context("supervisor request directory")?,
        )
        .join("snapshot-reservation.json");
        let snapshot = crate::full_snapshot::FullSnapshot::mount_verified(
            path,
            meta["manifest"].clone(),
            manifest_sha256
                .context("snapshot manifest digest")?
                .to_owned(),
            max_bytes,
            Some(&reservation),
        )?;
        Ok(ReadSnapshot {
            database: snapshot.database_locator().into(),
            strict_terms: snapshot.strict_terms(),
            _owned: Some(snapshot),
        })
    }
}

fn check_content_digest(meta: &Value, expected: Option<&str>) -> Result<()> {
    if let Some(expected) = expected {
        let actual = meta["manifest"]["content_digest"].as_str().ok_or_else(|| {
            anyhow::anyhow!("UNSUPPORTED_CAPABILITY|required {CONTENT_DIGEST_CAPABILITY}")
        })?;
        ensure!(
            actual == expected,
            "INVALID_DATASET|dataset content digest does not match expected_content_digest"
        );
    }
    Ok(())
}

fn check_manifest_digest(actual: Option<&str>, expected: Option<&str>) -> Result<()> {
    if let Some(expected) = expected {
        let actual = actual.ok_or_else(|| {
            anyhow::anyhow!("UNSUPPORTED_CAPABILITY|required {MANIFEST_DIGEST_CAPABILITY}")
        })?;
        ensure!(
            actual == expected,
            "INVALID_DATASET|dataset manifest digest does not match expected_manifest_sha256"
        );
    }
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
        Operation::PrepareQuery { query } => {
            crate::query::local_query(query)
                .or_else(|_| crate::query::local_derived_query(query))
                .map_err(|e| anyhow::anyhow!("INVALID_QUERY|{e}"))?;
            json!({"valid":true})
        }
        Operation::Rules { profile, files } => {
            let copy = tempfile::tempdir()?;
            let mut paths = Vec::new();
            for (i, rule) in files.iter().enumerate() {
                let suffix = rule
                    .path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("json");
                let path = copy.path().join(format!("rules-{i}.{suffix}"));
                crate::full_artifact::freeze(&rule.path, &path, &rule.expected_sha256)
                    .map_err(invalid)?;
                paths.push(path);
            }
            let result =
                crate::full::execute(json!({"operation":"rules","profile":profile,"files":paths}))
                    .map_err(anyhow::Error::msg)?;
            json!({"rules":result["rules"]})
        }
        Operation::Capabilities { dataset: path } => {
            let mut info = json!({"operations":["capabilities","compile","materialize","query","validate","rules","prepare_query"],
                "read_contracts":[READ_CONTRACT],"rule_contract":"ontology-rule-ir/v1",
                "runtime":crate::full::runtime_info(),"default_view":"asserted","proof_coverage":{"per_fact":"not_recorded"}});
            if let Some(path) = path {
                let Dataset { mut meta, .. } = worker_dataset(path, &value)?;
                meta.as_object_mut()
                    .context("dataset metadata")?
                    .remove("manifest");
                info["dataset"] = meta;
            }
            info
        }
        Operation::Query {
            dataset: path,
            expected_content_digest,
            expected_manifest_sha256,
            query,
            parameters,
            rdf_bindings,
            values,
            derived_queries,
            view,
            revision,
            require,
        } => {
            crate::query::local_query(query).map_err(|e| anyhow::anyhow!("INVALID_QUERY|{e}"))?;
            let parsed = parameters::bind_tables(
                sparql::SparqlParser::new().parse_query(query)?,
                parameters,
                values,
            )
            .map_err(invalid)?;
            let Dataset {
                meta,
                pack,
                pack_v4,
                manifest_sha256,
            } = worker_dataset(path, &value)?;
            check_content_digest(&meta, expected_content_digest.as_deref())?;
            check_manifest_digest(
                manifest_sha256.as_deref(),
                expected_manifest_sha256.as_deref(),
            )?;
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
                ensure!(
                    values.is_empty() && derived_queries.is_empty(),
                    "UNSUPPORTED_CAPABILITY|derived query data requires a Full dataset"
                );
                ensure!(
                    rdf_bindings.is_empty(),
                    "UNSUPPORTED_CAPABILITY|pack does not support variable bindings"
                );
                ensure!(
                    limits.max_rows <= sparql::MAX_ROWS && limits.max_bytes <= sparql::MAX_BYTES,
                    "UNSUPPORTED_CAPABILITY|pack result limits exceed supported maximum"
                );
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
            } else if let Some(pack) = pack_v4 {
                ensure!(
                    revision.is_none(),
                    "UNSUPPORTED_CAPABILITY|revision selection is unavailable for Pack v4"
                );
                let engine = crate::engine::OntologyEngine::from_pack(pack)
                    .map_err(|e| anyhow::anyhow!("INVALID_DATASET|{e}"))?;
                let view = match view {
                    View::Asserted => ontokiln_pack::v4::View::Asserted,
                    View::Inferred => ontokiln_pack::v4::View::Inferred,
                    View::Closure => ontokiln_pack::v4::View::Closure,
                };
                let result = engine
                    .query_with_derived(
                        query,
                        view,
                        parameters,
                        rdf_bindings,
                        values,
                        crate::engine::DerivedOptions {
                            queries: derived_queries,
                            max_derived_facts: limits.max_derived_facts,
                            max_intermediate_rows: limits.max_intermediate_rows,
                        },
                    )
                    .map_err(|e| {
                        if e.to_string().contains("BUDGET_EXHAUSTED") {
                            anyhow::anyhow!("BUDGET_EXHAUSTED|{e}")
                        } else {
                            anyhow::anyhow!("INVALID_QUERY|{e}")
                        }
                    })?;
                let reply: Value = serde_json::from_slice(&result)?;
                json!({
                    "query_type":if reply.get("boolean").is_some(){"ASK"}else{"SELECT"},
                    "variables":reply["head"].get("vars").cloned().unwrap_or(json!([])),
                    "items":reply["results"].get("bindings").cloned().unwrap_or(json!([])),
                    "boolean":reply["boolean"]
                })
            } else {
                ensure!(
                    revision.is_none(),
                    "UNSUPPORTED_CAPABILITY|revision selection is not available for this dataset"
                );
                ensure!(
                    derived_queries.len() <= 32,
                    "INVALID_REQUEST|at most 32 derived queries are allowed"
                );
                for text in derived_queries {
                    crate::query::local_derived_query(text)
                        .map_err(|e| anyhow::anyhow!("INVALID_QUERY|{e}"))?;
                }
                let snapshot = read_snapshot(
                    path,
                    &meta,
                    manifest_sha256.as_deref(),
                    &value,
                    limits.scratch_bytes.unwrap_or(u64::MAX),
                )?;
                let mut overlay = Vec::new();
                let mut unique = std::collections::BTreeSet::new();
                let mut intermediate_rows = 0u64;
                for text in derived_queries {
                    let rows = crate::disk::query(&json!({"dataset":snapshot.database,
                        "query":text,"parameters":parameters,"rdf_bindings":rdf_bindings,"values":values,"derived_data":true,
                        "view":view,"overlay":overlay,"max_rows":limits.max_intermediate_rows,
                        "max_bytes":limits.memory_bytes.unwrap_or(u64::MAX),"query_seconds":null}))
                        .map_err(|e| if e.to_string().contains("budget") {
                            anyhow::anyhow!("BUDGET_EXHAUSTED|{e}")
                        } else { anyhow::anyhow!("INVALID_QUERY|{e}") })?;
                    ensure!(
                        (rows["query_type"] == "SELECT" || rows["query_type"] == "CONSTRUCT")
                            && rows["variables"]
                                .as_array()
                                .is_some_and(|vars| ["s", "p", "o"]
                                    .iter()
                                    .all(|v| vars.contains(&json!(v)))),
                        "INVALID_QUERY|derived queries must SELECT s p o RDF terms"
                    );
                    for row in rows["items"].as_array().context("derived query rows")? {
                        intermediate_rows += 1;
                        ensure!(
                            intermediate_rows <= limits.max_intermediate_rows,
                            "BUDGET_EXHAUSTED|derived query intermediate row limit"
                        );
                        let triple = crate::query::rdf_triple(row)
                            .map_err(|e| anyhow::anyhow!("INVALID_QUERY|{e}"))?;
                        if unique.insert(triple.clone()) {
                            ensure!(
                                unique.len() as u64 <= limits.max_derived_facts,
                                "BUDGET_EXHAUSTED|derived query fact limit"
                            );
                            overlay.push(triple);
                        }
                    }
                }
                crate::full::execute(
                    json!({"operation":"query","dataset":snapshot.database,
                    "query":query,"parameters":parameters,"rdf_bindings":rdf_bindings,"values":values,
                    "overlay":overlay,"view":view,"max_rows":limits.max_rows,
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
            expected_content_digest,
            expected_manifest_sha256,
            shapes,
            shapes_from_dataset,
            selected_shapes,
            focus_node,
        } => {
            ensure!(
                !*shapes_from_dataset || shapes.is_empty(),
                "INVALID_REQUEST|choose dataset shapes or external shapes"
            );
            let Dataset {
                meta,
                pack,
                pack_v4,
                manifest_sha256,
            } = worker_dataset(path, &value)?;
            check_content_digest(&meta, expected_content_digest.as_deref())?;
            check_manifest_digest(
                manifest_sha256.as_deref(),
                expected_manifest_sha256.as_deref(),
            )?;
            if let Some(pack) = pack_v4 {
                let engine = crate::engine::OntologyEngine::from_pack(pack)
                    .map_err(|e| anyhow::anyhow!("INVALID_DATASET|{e}"))?;
                let selected: Option<std::collections::BTreeSet<String>> = selected_shapes
                    .as_ref()
                    .map(|shapes| shapes.iter().cloned().collect());
                let selected = selected.as_ref();
                let report = if *shapes_from_dataset {
                    engine.validate(
                        None,
                        selected,
                        focus_node.as_deref(),
                        limits.max_validation_depth,
                    )
                } else {
                    let temp = tempfile::tempdir()?;
                    let mut frozen = Vec::new();
                    for (index, shape) in shapes.iter().enumerate() {
                        let destination = temp.path().join(format!("shape-{index}.rdf"));
                        crate::full_artifact::freeze(
                            &shape.path,
                            &destination,
                            &shape.expected_sha256,
                        )
                        .map_err(invalid)?;
                        frozen.push(ontokiln_compiler::v4::Source {
                            path: destination,
                            document_id: shape.document_id.clone(),
                            format: shape.format.clone(),
                            expected_sha256: shape.expected_sha256.clone(),
                            base_iri: shape.base_iri.clone(),
                        });
                    }
                    engine.validate_shape_sources(
                        &frozen,
                        selected,
                        focus_node.as_deref(),
                        limits.max_validation_depth,
                    )
                }
                .map_err(|e| anyhow::anyhow!("EXECUTION_FAILED|{e}"))?;
                json!({
                    "conforms":report["conforms"],
                    "checked_shapes":report["checked_shapes"],
                    "violations":report["violations"],
                    "report":report["report"]
                })
            } else {
                ensure!(
                    pack.is_none(),
                    "UNSUPPORTED_CAPABILITY|SHACL requires an RDF dataset artifact"
                );
                let copy = tempfile::tempdir()?;
                let snapshot = read_snapshot(
                    path,
                    &meta,
                    manifest_sha256.as_deref(),
                    &value,
                    limits.scratch_bytes.unwrap_or(u64::MAX),
                )?;
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
                let mut input = json!({"operation":"validate","dataset":path,"_snapshot_database":snapshot.database,"shapes":frozen,
                "_strict_dataset_terms":snapshot.strict_terms,
                "shapes_from_dataset":shapes_from_dataset,"max_validation_depth":limits.max_validation_depth});
                if let Some(selected) = selected_shapes {
                    input["selected_shapes"] = json!(selected);
                }
                if let Some(focus) = focus_node {
                    input["focus_node"] = json!(focus);
                }
                let reply = crate::full::validate_api(input).map_err(anyhow::Error::msg)?;
                json!({"conforms":reply["conforms"],"checked_shapes":reply["checked_shapes"],
                "violations":reply["violations"],"report":serde_json::from_str::<Value>(reply["report"].as_str().context("SHACL report")?)?})
            }
        }
    };
    let mut reply = json!({"api_result":result,"metrics":{}});
    if let Some(mounted) = value.get("_mounted_snapshot") {
        reply["integrity_checked"] = json!(true);
        reply["metrics"]["snapshot_reused"] = json!(true);
        reply["metrics"]["snapshot_bytes"] = mounted["bytes"].clone();
        reply["metrics"]["snapshot_bytes_copied"] = json!(0);
    }
    Ok(reply)
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
