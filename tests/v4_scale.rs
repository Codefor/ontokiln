//! Production-scale Pack v4 smoke, with independent fixed expectations.
//! Run intentionally with --ignored to keep ordinary contract tests fast.
use ontokiln::engine::OntologyEngine;
use ontokiln_compiler::v4::{Source, compile_sources};
use ontokiln_compiler::v4_materialize::materialize;
use ontokiln_pack::v4::View;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::time::Instant;
use tempfile::tempdir;

#[test]
#[ignore = "million-fact capacity acceptance, not an ordinary unit test"]
fn one_million_explicit_facts_build_save_reload_and_count() {
    let temp = tempdir().unwrap();
    let input = temp.path().join("million.nt");
    {
        let mut writer = BufWriter::new(File::create(&input).unwrap());
        for n in 0..1_000_000_u32 {
            writeln!(writer, "<urn:s{n:08}> <urn:p> <urn:o{}> .", n % 100).unwrap();
        }
        writer.flush().unwrap();
    }
    let raw = fs::read(&input).unwrap();
    let source = Source {
        path: input,
        document_id: "million.nt".into(),
        format: "ntriples".into(),
        expected_sha256: format!("{:x}", Sha256::digest(&raw)),
        base_iri: None,
    };

    let started = Instant::now();
    let asserted = compile_sources(&[source]).unwrap();
    let compile_seconds = started.elapsed().as_secs_f64();
    let (materialized, stats) = materialize(asserted, "no-rules-v1", &[]).unwrap();
    let materialize_seconds = started.elapsed().as_secs_f64() - compile_seconds;
    let engine = OntologyEngine::from_pack(materialized).unwrap();
    let build_seconds = started.elapsed().as_secs_f64();
    assert_eq!(stats.explicit_facts, 1_000_000);
    assert_eq!(stats.inferred_facts, 0);
    assert_eq!(engine.pack().quad_count(), 1_000_000);

    let query = "SELECT (COUNT(?s) AS ?n) WHERE { ?s <urn:p> ?o }";
    let answer: Value =
        serde_json::from_slice(&engine.query(query, View::Closure).unwrap()).unwrap();
    assert_eq!(answer["results"]["bindings"][0]["n"]["value"], "1000000");

    let pack = temp.path().join("million.pack");
    let start = Instant::now();
    engine.save_pack(&pack).unwrap();
    let save_seconds = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let reloaded = OntologyEngine::load_pack(&pack).unwrap();
    let load_seconds = start.elapsed().as_secs_f64();

    assert_eq!(reloaded.pack().quad_count(), 1_000_000);
    assert_eq!(
        reloaded.query(query, View::Closure).unwrap(),
        engine.query(query, View::Closure).unwrap()
    );
    assert_eq!(
        fs::read(&pack).unwrap().len() as u64,
        fs::metadata(&pack).unwrap().len()
    );
    eprintln!(
        "v4 million-fact acceptance: compile={compile_seconds:.3}s materialize={materialize_seconds:.3}s build={build_seconds:.3}s save={save_seconds:.3}s load={load_seconds:.3}s pack_bytes={}",
        fs::metadata(&pack).unwrap().len()
    );
}
