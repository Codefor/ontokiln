use ontokiln_compiler::{
    compile_directory,
    rules::{Engine, Rule, materialize},
};
use ontokiln_pack::Pack;
use ontokiln_reasoner::{
    Reasoner,
    sparql::{CancellationToken, SparqlParser},
};
use std::{collections::BTreeSet, fs, path::Path, process::Command};
use tempfile::tempdir;

#[test]
fn mixed_formats_and_thread_counts_preserve_v2_bytes_and_blank_scopes() {
    let d = tempdir().unwrap();
    fs::write(
        d.path().join("a.ttl"),
        "@prefix ex: <urn:> . ex:s ex:p _:b .",
    )
    .unwrap();
    fs::write(
        d.path().join("b.nt"),
        "<urn:s> <urn:p> _:b .\n<urn:s> <urn:p> <urn:o> .\n",
    )
    .unwrap();
    let build = |threads: &str| {
        let path = d.path().join(format!("{threads}.pack"));
        assert!(
            Command::new(env!("CARGO_BIN_EXE_ontokiln"))
                .env("RAYON_NUM_THREADS", threads)
                .arg("compile")
                .arg("--ttl")
                .arg(d.path())
                .arg("--out")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        fs::read(path).unwrap()
    };
    assert_eq!(build("1"), build("4"));
    let p = Pack::load(&d.path().join("4.pack")).unwrap();
    assert_eq!(p.triple_count(), 3);
}

// Variable-width text must never affect the alignment of the mapped SPO rows.
#[test]
fn mapped_v2_pack_survives_in_place_source_truncation() {
    use std::io::Write;
    let d = tempdir().unwrap();
    for n in 0..4 {
        let file = d.path().join("snapshot.pack");
        let term = format!("<urn:a{}>", "x".repeat(n));
        let pack = Pack::from_sorted(vec![term.clone()], vec![[0, 0, 0]]).unwrap();
        pack.save(&file).unwrap();
        let before = fs::read(&file).unwrap();
        assert_eq!(&before[..12], b"ONTOKILN\x02\x00\x00\x00");
        // One term, one row: the two offsets and the SPO row precede all text.
        assert_eq!(&before[44..48], &1_u32.to_le_bytes());
        assert_eq!(&before[48..56], &1_u64.to_le_bytes());
        assert_eq!(&before[56..64], &0_u64.to_le_bytes());
        assert_eq!(&before[64..72], &(term.len() as u64).to_le_bytes());
        assert_eq!(&before[72..84], &[0; 12]);
        assert_eq!(&before[84..], term.as_bytes());
        let mapped = Pack::load(&file).unwrap();
        fs::File::create(&file)
            .unwrap()
            .write_all(b"changed")
            .unwrap();
        assert_eq!(mapped.term(0), term);
        assert_eq!(mapped.edges(0, 0), &[[0, 0, 0]]);
        mapped.save(&file).unwrap();
        assert_eq!(fs::read(file).unwrap(), before);
    }
}

fn fixture(dir: &Path) -> Pack {
    fs::write(
        dir.join("facts.ttl"),
        r#"
        @prefix e: <urn:> . @prefix r: <http://www.w3.org/2000/01/rdf-schema#> .
        e:x a e:A,e:X ; e:p e:y ; e:label "hello"@en .
        e:y a e:B ; e:p e:z .
        e:z e:p e:x .
        e:A r:subClassOf e:B . e:B r:subClassOf e:A .
    "#,
    )
    .unwrap();
    compile_directory(dir).unwrap().pack
}

#[test]
fn sparql_matches_on_owned_and_mapped_pack_preserving_bags_values_and_graph_scope() {
    let d = tempdir().unwrap();
    let owned = fixture(d.path());
    let path = d.path().join("graph.pack");
    owned.save(&path).unwrap();
    for pack in [owned, Pack::load(&path).unwrap()] {
        let engine = Reasoner::new(pack);
        let run = |text: &str| {
            let q = SparqlParser::new().parse_query(text).unwrap();
            engine.sparql(&q, vec![], CancellationToken::new()).unwrap()
        };
        let json = |q: &str| serde_json::from_slice::<serde_json::Value>(&run(q).bytes).unwrap();
        let rows = json(
            "SELECT ?o ?v ?unbound WHERE { VALUES ?s { <urn:x> <urn:x> } ?s <urn:p>/<urn:p> ?o . BIND(1+2 AS ?v) OPTIONAL { ?s <urn:absent> ?unbound } }",
        );
        let rows = rows["results"]["bindings"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], rows[1]);
        assert_eq!(rows[0]["o"]["value"], "urn:z");
        assert_eq!(rows[0]["v"]["value"], "3");
        assert!(rows[0].get("unbound").is_none());
        assert_eq!(json("ASK { GRAPH ?g { ?s ?p ?o } }")["boolean"], false);
        assert_eq!(json("ASK { <urn:x> a <urn:B> }")["boolean"], false);
        assert_eq!(
            json("ASK { <urn:x> <urn:label> \"hello\"@en }")["boolean"],
            true
        );
        assert_eq!(
            json("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }")["results"]["bindings"][0]["n"]["value"],
            "9"
        );
        let o = run("CONSTRUCT { ?s <urn:alias> ?o } WHERE { ?s <urn:label> ?o }");
        assert_eq!(o.media_type, "application/n-triples");
        assert_eq!(
            std::str::from_utf8(&o.bytes).unwrap(),
            "<urn:x> <urn:alias> \"hello\"@en .\n"
        );
        assert!(
            SparqlParser::new()
                .parse_query("INSERT DATA {<urn:s> <urn:p> 1}")
                .is_err()
        );
        let kill = CancellationToken::new();
        kill.cancel();
        assert!(
            engine
                .sparql(
                    &SparqlParser::new().parse_query("ASK { ?s ?p ?o }").unwrap(),
                    vec![],
                    kill
                )
                .is_err()
        );
    }
}

#[test]
fn bitmap_intersection_respects_inheritance_cycles_and_limit_and_batch_matches_single() {
    let d = tempdir().unwrap();
    let engine = Reasoner::new(fixture(d.path()));
    let types = ["<urn:B>".into(), "<urn:X>".into()];
    assert_eq!(
        engine.entities(&types, true, 10).unwrap().entities,
        ["<urn:x>"]
    );
    assert_eq!(engine.entities(&types, false, 10).unwrap().total, 0);
    let limited = engine.entities(&types[..1], true, 0).unwrap();
    assert_eq!(limited.total, 2);
    assert!(limited.entities.is_empty());
    assert_eq!(
        engine
            .entities(&["<urn:missing>".into()], true, 10)
            .unwrap()
            .total,
        0
    );
    let subjects = [
        "<urn:y>".into(),
        "<urn:x>".into(),
        "<urn:absent>".into(),
        "<urn:x>".into(),
    ];
    let batch = engine.query_batch(&subjects, "<urn:p>", true).unwrap();
    for (i, s) in subjects.iter().enumerate() {
        assert_eq!(batch[i], engine.query(s, "<urn:p>", true).unwrap());
    }
    assert!(engine.query_batch(&[], "<urn:p>", false).is_err());
}

#[test]
fn scheduled_recursive_rules_match_independent_closure_and_other_backend() {
    let d = tempdir().unwrap();
    let pack = fixture(d.path());
    // Rules deliberately precede their prerequisites. a and b are mutually recursive.
    let rule = |h: &str, body: &[&str]| Rule {
        head: format!("urn:{h}"),
        body: body.iter().map(|s| format!("urn:{s}")).collect(),
    };
    let rules = [
        rule("out", &["a"]),
        rule("a", &["b", "p"]),
        rule("b", &["a"]),
        rule("b", &["p"]),
    ];
    let run = |engine| {
        let mut out = Vec::new();
        let summary = materialize(&pack, &rules, engine, 256, &mut out).unwrap();
        assert_eq!(summary.added, 27);
        assert_eq!(summary.stages.len(), 2);
        assert!(
            summary.stages[0].contains(&"urn:b".into())
                && summary.stages[0].contains(&"urn:a".into())
        );
        assert_eq!(summary.stages[1], ["urn:out"]);
        String::from_utf8(out).unwrap()
    };
    let mut rejected = Vec::new();
    assert!(materialize(&pack, &rules, Engine::Datafrog, 2, &mut rejected).is_err());
    assert!(rejected.is_empty());
    let frog = run(Engine::Datafrog);
    let mut expected = BTreeSet::new();
    for s in ["x", "y", "z"] {
        for o in ["x", "y", "z"] {
            for p in ["a", "b", "out"] {
                expected.insert(format!("<urn:{s}> <urn:{p}> <urn:{o}> ."));
            }
        }
    }
    assert_eq!(
        frog.lines().map(str::to_owned).collect::<BTreeSet<_>>(),
        expected
    );
    #[cfg(feature = "ascent-engine")]
    assert_eq!(run(Engine::Ascent), frog);
    assert!(
        materialize(
            &pack,
            &[rule("bad", &[])],
            Engine::Datafrog,
            256,
            Vec::new()
        )
        .is_err()
    );
}
