//! Public local adapters and golden answers; no alternate engine or oracle.
use ontokiln::engine::{DerivedOptions, OntologyEngine};
use ontokiln::local::{input_digest, load_rules, load_sources, rebuild, source_file};
use ontokiln_pack::v4::{Metadata, PackV4, View};
use ontokiln_reasoner::parameters::{Parameter, ValuesTable};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::tempdir;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/local")
}
fn build(input: &Path) -> OntologyEngine {
    OntologyEngine::build(
        &load_sources(input).unwrap(),
        "rdfs-property-chain-v1",
        &load_rules(Some(&fixture().join("rules.json"))).unwrap(),
    )
    .unwrap()
    .0
}
fn answer(engine: &OntologyEngine, query: &str, view: View) -> Value {
    serde_json::from_slice(&engine.query(query, view).unwrap()).unwrap()
}
fn golden(engine: &OntologyEngine) {
    let cases: BTreeMap<String, Value> =
        serde_json::from_slice(&fs::read(fixture().join("golden.json")).unwrap()).unwrap();
    for (file, expected) in cases {
        let query = fs::read_to_string(fixture().join("queries").join(&file)).unwrap();
        assert_eq!(answer(engine, &query, View::Closure), expected, "{file}");
    }
    let rdfs = fs::read_to_string(fixture().join("queries/rdfs.rq")).unwrap();
    assert_eq!(answer(engine, &rdfs, View::Asserted)["boolean"], false);
    assert_eq!(answer(engine, &rdfs, View::Inferred)["boolean"], true);
    let lineage = fs::read_to_string(fixture().join("queries/lineage.rq")).unwrap();
    assert_eq!(
        answer(engine, &lineage, View::Asserted)["results"]["bindings"],
        json!([])
    );
    assert_eq!(
        answer(engine, &lineage, View::Inferred),
        answer(engine, &lineage, View::Closure)
    );
}

#[test]
fn directory_manifest_resident_and_pack_reload_have_the_same_golden_semantics() {
    let root = tempdir().unwrap();
    let local = build(&fixture().join("ontology"));
    // FaaS/other consumers directly call this identical public Engine entry.
    let embedded = build(&fixture().join("sources.json"));
    assert_eq!(local.pack().metadata(), embedded.pack().metadata());
    assert!(local.pack().metadata().fixpoint_reached);
    let pack = root.path().join("golden.pack");
    local.save_pack(&pack).unwrap();
    let loaded = OntologyEngine::load_pack(&pack).unwrap();
    for engine in [&local, &embedded, &loaded] {
        golden(engine);
    }
    assert_eq!(local.pack().metadata(), loaded.pack().metadata());
    assert!(local.save_pack(&pack).is_err());
}

#[test]
fn bound_parameters_rdf_bindings_and_values_preserve_public_columns() {
    let engine = build(&fixture().join("sources.json"));
    let query = fs::read_to_string(fixture().join("queries/bound.rq")).unwrap();
    let expected = answer(
        &engine,
        &query.replace("?component", "<urn:local:cardTitle>"),
        View::Closure,
    );
    let empty = BTreeMap::new();
    let iri = Parameter::Iri {
        value: "urn:local:cardTitle".into(),
    };
    let parameters = BTreeMap::from([("component".into(), iri.clone())]);
    let parameter_query = query.replace("?component", "<urn:ontokiln:param:component>");
    let actual: Value = serde_json::from_slice(
        &engine
            .query_bound(&parameter_query, View::Closure, &parameters, &empty, &[])
            .unwrap(),
    )
    .unwrap();
    assert_eq!(actual, expected);
    let bindings = BTreeMap::from([("component".into(), "<urn:local:cardTitle>".into())]);
    let actual: Value = serde_json::from_slice(
        &engine
            .query_bound(&query, View::Closure, &BTreeMap::new(), &bindings, &[])
            .unwrap(),
    )
    .unwrap();
    assert_eq!(actual, expected);
    let values = [ValuesTable {
        variables: vec!["component".into()],
        rows: vec![vec![Some(iri)]],
    }];
    let actual: Value = serde_json::from_slice(
        &engine
            .query_bound(
                &query.replace("WHERE {", "WHERE { VALUES ?component {}"),
                View::Closure,
                &BTreeMap::new(),
                &empty,
                &values,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(actual, expected);
    let literals = BTreeMap::from([(
        "expected".into(),
        Parameter::Literal {
            value: "Raw title".into(),
            language: Some("en".into()),
            datatype: None,
        },
    )]);
    let actual: Value = serde_json::from_slice(
        &engine
            .query_bound(
                "ASK { <urn:local:rawTitle> <urn:local:value> <urn:ontokiln:param:expected> }",
                View::Closure,
                &literals,
                &BTreeMap::new(),
                &[],
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(actual["boolean"], true);
}

#[test]
fn core_and_sparql_shapes_use_inferred_and_named_graph_facts_after_reload() {
    let root = tempdir().unwrap();
    let engine = build(&fixture().join("sources.json"));
    let pack = root.path().join("shacl.pack");
    engine.save_pack(&pack).unwrap();
    let loaded = OntologyEngine::load_pack(&pack).unwrap();
    for engine in [&engine, &loaded] {
        for (file, conforms) in [
            ("core-pass.ttl", true),
            ("core-fail.ttl", false),
            ("sparql-pass.ttl", true),
            ("sparql-fail.ttl", false),
        ] {
            let source = source_file(fixture().join("shapes").join(file), file.into()).unwrap();
            let result = engine
                .validate_shape_sources(&[source], None, None, 64)
                .unwrap();
            assert_eq!(result["conforms"], conforms, "{file}: {result}");
            assert_eq!(result["checked_shapes"], 1);
            assert_eq!(result["violations"], u64::from(!conforms));
        }
        assert_eq!(
            engine.validate(None, None, None, 64).unwrap()["conforms"],
            true
        );
    }
}

#[test]
fn derived_lineage_projection_is_request_scoped() {
    let engine = build(&fixture().join("sources.json"));
    let query = "ASK { <urn:local:cardTitle> <urn:local:projectedValue> \"Raw title\"@en }";
    let derived = ["CONSTRUCT { ?s <urn:local:projectedValue> ?value } WHERE { ?s <urn:local:dependsOn> ?p . ?p <urn:local:value> ?value }".into()];
    let actual: Value = serde_json::from_slice(
        &engine
            .query_with_derived(
                query,
                View::Closure,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &[],
                DerivedOptions {
                    queries: &derived,
                    max_derived_facts: 10,
                    max_intermediate_rows: 10,
                },
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(actual["boolean"], true);
    assert_eq!(answer(&engine, query, View::Closure)["boolean"], false);
}

#[test]
fn malformed_rdf_digest_documents_rules_and_pack_fail_explicitly() {
    let root = tempdir().unwrap();
    let source_path = root.path().join("data.ttl");
    fs::write(&source_path, "<urn:a> <urn:p> <urn:b> .").unwrap();
    let source = source_file(source_path.clone(), "data".into()).unwrap();
    let mut bad = source.clone();
    bad.expected_sha256 = "0".repeat(64);
    let error = OntologyEngine::build(&[bad], "no-rules-v1", &[])
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("SHA-256 mismatch"));
    assert!(OntologyEngine::build(&[source.clone(), source.clone()], "no-rules-v1", &[]).is_err());
    fs::write(&source_path, "not valid RDF {{{").unwrap();
    let bad = source_file(source_path, "bad".into()).unwrap();
    assert!(OntologyEngine::build(&[bad], "no-rules-v1", &[]).is_err());
    for document in [
        json!({"schema_version":"wrong", "rules":[]}),
        json!({"schema_version":"ontology-rule-ir/v1", "rules":[], "ignored":true}),
    ] {
        let path = root.path().join("rules.json");
        fs::write(&path, document.to_string()).unwrap();
        assert!(load_rules(Some(&path)).is_err());
    }
    let mut rules = load_rules(Some(&fixture().join("rules.json"))).unwrap();
    rules[0].head[0] = "?unbound".into();
    let error = OntologyEngine::build(
        &load_sources(&fixture().join("sources.json")).unwrap(),
        "rdfs-property-chain-v1",
        &rules,
    )
    .err()
    .unwrap();
    assert!(format!("{error:#}").contains("UNSAFE_RULE"));
    let engine = build(&fixture().join("ontology"));
    let pack = root.path().join("bad.pack");
    engine.save_pack(&pack).unwrap();
    let mut bytes = fs::read(&pack).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    fs::write(&pack, bytes).unwrap();
    assert!(OntologyEngine::load_pack(&pack).is_err());
    fs::write(&pack, b"bad").unwrap();
    assert!(OntologyEngine::load_pack(&pack).is_err());
    let incomplete =
        PackV4::from_statements(vec![], vec![], Metadata::new("datalog-v1", false)).unwrap();
    assert!(OntologyEngine::from_pack(incomplete).is_err());
}

#[test]
fn full_rebuild_failure_preserves_old_engine_and_success_reloads_a_fresh_pack() {
    let root = tempdir().unwrap();
    let sources = root.path().join("rdf");
    fs::create_dir(&sources).unwrap();
    let file = sources.join("data.ttl");
    fs::write(&file, "<urn:a> <urn:p> <urn:old> .").unwrap();
    let (mut current, _) =
        rebuild(&sources, "no-rules-v1", None, &root.path().join("one.pack")).unwrap();
    let old_pack = fs::read(root.path().join("one.pack")).unwrap();
    let before = input_digest(&sources, None).unwrap();
    fs::write(&file, "broken {").unwrap();
    assert_ne!(input_digest(&sources, None).unwrap(), before);
    assert!(rebuild(&sources, "no-rules-v1", None, &root.path().join("two.pack")).is_err());
    assert!(!root.path().join("two.pack").exists());
    assert_eq!(
        answer(&current, "ASK { <urn:a> <urn:p> <urn:old> }", View::Closure)["boolean"],
        true
    );
    assert_eq!(fs::read(root.path().join("one.pack")).unwrap(), old_pack);
    fs::write(&file, "<urn:a> <urn:p> <urn:new> .").unwrap();
    let (next, _) = rebuild(&sources, "no-rules-v1", None, &root.path().join("two.pack")).unwrap();
    current = next;
    assert_eq!(
        answer(&current, "ASK { <urn:a> <urn:p> <urn:old> }", View::Closure)["boolean"],
        false
    );
    assert_eq!(
        answer(&current, "ASK { <urn:a> <urn:p> <urn:new> }", View::Closure)["boolean"],
        true
    );
    assert_eq!(fs::read(root.path().join("one.pack")).unwrap(), old_pack);
}

#[test]
fn cli_manifest_is_independent_of_the_working_directory() {
    let root = tempdir().unwrap();
    let pack = root.path().join("cli.pack");
    let output = Command::new(env!("CARGO_BIN_EXE_ontokiln"))
        .current_dir(root.path())
        .args(["build", "--sources"])
        .arg(fixture().join("sources.json"))
        .args(["--rules"])
        .arg(fixture().join("rules.json"))
        .args(["--profile", "rdfs-property-chain-v1", "--pack"])
        .arg(&pack)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    golden(&OntologyEngine::load_pack(&pack).unwrap());
}

#[cfg(unix)]
#[test]
fn symlinks_are_rejected_instead_of_omitting_rdf_documents() {
    let root = tempdir().unwrap();
    let file = root.path().join("data.ttl");
    fs::write(&file, "<urn:a> <urn:p> <urn:b> .").unwrap();
    let link = root.path().join("link.ttl");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert!(source_file(link, "link".into()).is_err());
    assert!(load_sources(root.path()).is_err());
}
