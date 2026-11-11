//! Engine and artifact identity contain only generic compilation inputs.
use anyhow::{Result, ensure};
use ontokiln_pack::{PACK_VERSION, Pack};
use serde_json::{Value, json};
use std::path::Path;

pub fn kernel() -> Value {
    json!({
        "contract_version":"ontokiln-core/v3",
        "version":env!("CARGO_PKG_VERSION"),
        "source_revision":env!("ONTOKILN_BUILD_REVISION"),
        "kernel_sha256":env!("ONTOKILN_KERNEL_SHA256"),
        "pack_version":PACK_VERSION,
        "capabilities":["rdf-pack/v3", "sparql/v1", "positive-rules/v1", "proofs/v1",
            "full-source-doc/v2", "full-execution/v1", "shacl-core-and-local-select/v1"]
    })
}

pub fn inspect(path: &Path) -> Result<Value> {
    let pack = Pack::load(path)?;
    describe(&pack)
}

pub fn describe(pack: &Pack) -> Result<Value> {
    // Hash the verified private snapshot, not a possibly replaced source path.
    use sha2::{Digest, Sha256};
    use std::io::{Read, Seek, SeekFrom};
    let mut input = pack.snapshot_file().expect("loaded snapshot").try_clone()?;
    input.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(
        json!({"schema_version":"ontokiln-pack-identity/v1", "kernel":kernel(),
        "pack_version":PACK_VERSION, "pack_sha256":format!("{:x}",hash.finalize()),
        "terms":pack.term_count(), "triples":pack.triple_count(), "sources":pack.documents()}),
    )
}

pub fn check(path: &Path, expected: &Path) -> Result<Value> {
    let actual = inspect(path)?;
    let expected: Value = serde_json::from_slice(&std::fs::read(expected)?)?;
    ensure!(actual == expected, "engine or pack identity mismatch");
    Ok(json!({"compatible":true,"identity":actual}))
}
