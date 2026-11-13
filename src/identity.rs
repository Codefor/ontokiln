//! Immutable Pack v4 identity and runtime capabilities.
use anyhow::Result;
use ontokiln_pack::v4::{FORMAT_VERSION, PackV4};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

pub fn kernel() -> Value {
    json!({
        "contract_version": "ontokiln-core/v4",
        "version": env!("CARGO_PKG_VERSION"),
        "source_revision": env!("ONTOKILN_BUILD_REVISION"),
        "kernel_sha256": env!("ONTOKILN_KERNEL_SHA256"),
        "pack_version": FORMAT_VERSION,
        "capabilities": [
            "rdf-pack/v4", "rdf-dataset-union/v1",
            "sparql/v1", "shacl/v1", "positive-rules/v1"
        ]
    })
}
pub fn inspect(path: &Path) -> Result<Value> {
    let pack = PackV4::load(path)?;
    let digest = format!("{:x}", Sha256::digest(std::fs::read(path)?));
    Ok(json!({
        "pack_version": FORMAT_VERSION,
        "pack_sha256":digest,
        "terms":pack.term_count(),
        "quads":pack.quad_count(),
        "metadata":pack.metadata(),
        "kernel":kernel()
    }))
}
