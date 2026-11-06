use ontokiln_compiler::compile_directory;
use ontokiln_pack::Pack;
use ontokiln_reasoner::{RDF_TYPE, Reasoner, SUB_CLASS, SUB_PROPERTY};
use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn directory_and_standalone_pack_give_the_same_proofs() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("ontology");
    fs::create_dir_all(src.join("entities")).unwrap();
    fs::write(src.join("schema.ttl"), "@prefix r: <http://www.w3.org/2000/01/rdf-schema#> . <urn:A> r:subClassOf <urn:B> . <urn:B> r:subClassOf <urn:C> .").unwrap();
    fs::write(
        src.join("entities/item.TTL"),
        "<urn:item> a <urn:A>, <urn:A> .",
    )
    .unwrap();
    let built = compile_directory(&src).unwrap();
    assert_eq!(
        (built.files, built.parsed, built.pack.triple_count()),
        (2, 4, 3)
    );
    let path = dir.path().join("only.pack");
    built.pack.save(&path).unwrap();
    fs::remove_dir_all(&src).unwrap();
    let from_dir = Reasoner::new(built.pack);
    let from_pack = Reasoner::new(Pack::load(&path).unwrap());
    let answers = from_pack.query("<urn:item>", RDF_TYPE, true).unwrap();
    assert_eq!(
        answers,
        from_dir.query("<urn:item>", RDF_TYPE, true).unwrap()
    );
    assert_eq!(
        answers
            .iter()
            .map(|a| a.object.as_str())
            .collect::<Vec<_>>(),
        ["<urn:A>", "<urn:B>", "<urn:C>"]
    );
    assert!(!answers[0].inferred);
    assert!(answers[2].inferred);
    assert_eq!(
        answers[2]
            .proof
            .as_ref()
            .unwrap()
            .iter()
            .map(|s| s.rule)
            .collect::<Vec<_>>(),
        ["asserted", "inherited_type", "inherited_type"]
    );
    let class = from_pack.query("<urn:A>", SUB_CLASS, false).unwrap();
    assert_eq!(class.len(), 2);
    assert!(class[1].inferred && class[1].proof.is_none());
}

#[test]
fn property_cycles_terminate_and_explicit_facts_take_precedence() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("cycle.ttl"),
        r#"
        @prefix r: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix ex: <urn:> .
        ex:p r:subPropertyOf ex:q . ex:q r:subPropertyOf ex:z .
        ex:z r:subPropertyOf ex:p .
        ex:s ex:p "text"@en ; ex:q "text"@en .
    "#,
    )
    .unwrap();
    let engine = Reasoner::new(compile_directory(dir.path()).unwrap().pack);
    let query = |p| engine.query("<urn:s>", p, true).unwrap();
    assert_eq!(query("<urn:z>").len(), 1);
    assert_eq!(query("<urn:z>")[0].object, r#""text"@en"#);
    assert!(query("<urn:z>")[0].inferred);
    assert_eq!(query("<urn:z>")[0].proof.as_ref().unwrap().len(), 2);
    assert!(!query("<urn:q>")[0].inferred);
    assert_eq!(
        engine.query("<urn:p>", SUB_PROPERTY, false).unwrap().len(),
        3
    );
    assert!(query("<urn:missing>").is_empty());
}

#[test]
fn blank_nodes_are_document_scoped_and_pack_is_reproducible() {
    let dir = tempdir().unwrap();
    let graph = r#"@base <https://example.org/> . <item> <p> _:x, [ <q> "a" ] . "#;
    for name in ["a.ttl", "b.ttl"] {
        fs::write(dir.path().join(name), graph).unwrap();
    }
    let compile = || compile_directory(dir.path()).unwrap().pack;
    let first = compile();
    let file = dir.path().join("bundle.pack");
    first.save(&file).unwrap();
    let expected = fs::read(&file).unwrap();
    compile().save(&file).unwrap();
    assert_eq!(fs::read(&file).unwrap(), expected);
    let objects = Reasoner::new(first)
        .query(
            "<https://example.org/item>",
            "<https://example.org/p>",
            false,
        )
        .unwrap();
    assert_eq!(objects.len(), 4);
}

#[test]
fn invalid_sources_and_corrupt_packs_fail_without_replacing_previous_output() {
    let dir = tempdir().unwrap();
    assert!(compile_directory(dir.path()).is_err());
    let ttl = dir.path().join("data.ttl");
    let path = dir.path().join("bundle.pack");
    fs::write(&ttl, "<urn:s> <urn:p> <urn:o> .").unwrap();
    compile_directory(dir.path())
        .unwrap()
        .pack
        .save(&path)
        .unwrap();
    let good = fs::read(&path).unwrap();
    fs::write(ttl, "<broken").unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_ontokiln"))
        .arg("compile")
        .arg("--ttl")
        .arg(dir.path())
        .arg("--out")
        .arg(&path)
        .output()
        .unwrap();
    assert!(!status.status.success());
    assert!(String::from_utf8_lossy(&status.stderr).contains("data.ttl"));
    assert_eq!(fs::read(&path).unwrap(), good);
    let mut corrupt = good.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    fs::write(&path, corrupt).unwrap();
    assert!(
        Pack::load(&path)
            .unwrap_err()
            .to_string()
            .contains("checksum")
    );
    for unsupported in [1_u32, 3] {
        let mut bytes = good.clone();
        bytes[8..12].copy_from_slice(&unsupported.to_le_bytes());
        fs::write(&path, bytes).unwrap();
        let error = Pack::load(&path).unwrap_err().to_string();
        assert!(error.contains(&format!("unsupported pack version {unsupported}")));
        assert!(error.contains("rebuild"));
    }
    fs::write(&path, b"bad").unwrap();
    assert!(Pack::load(&path).is_err());
}

#[test]
fn pack_size_override_is_opt_in_and_checked_on_compile_and_load() {
    let dir = tempdir().unwrap();
    let ttl = dir.path().join("large.nt");
    let pack = dir.path().join("large.pack");
    fs::write(
        ttl,
        format!("<urn:s> <urn:p> \"{}\" .", "a".repeat(1024 * 1024)),
    )
    .unwrap();
    let binary = env!("CARGO_BIN_EXE_ontokiln");
    let compile = |limit| {
        Command::new(binary)
            .env("ONTOKILN_MAX_PACK_MIB", limit)
            .arg("compile")
            .arg("--ttl")
            .arg(dir.path())
            .arg("--out")
            .arg(&pack)
            .output()
            .unwrap()
    };
    assert!(compile("2").status.success());
    let good = fs::read(&pack).unwrap();
    for limit in ["1", "0", "invalid", "18446744073709551615"] {
        assert!(!compile(limit).status.success());
        assert_eq!(fs::read(&pack).unwrap(), good);
    }
    let rejected = Command::new(binary)
        .env("ONTOKILN_MAX_PACK_MIB", "1")
        .arg("serve")
        .arg("--pack")
        .arg(&pack)
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("configured size limit"));
}
