//! Differential checks against the unchanged in-memory RDF implementation.
use super::*;
use crate::shacl_sqlite::SqliteGraph;
use oxigraph::io::{RdfFormat, RdfParser};
use rusqlite::{Connection, params};

const PREFIXES: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex: <urn:example:> .\n";

fn graph(text: &str) -> Graph {
    let text = format!("{PREFIXES}{text}");
    let mut graph = Graph::default();
    for quad in RdfParser::from_format(RdfFormat::Turtle).for_reader(text.as_bytes()) {
        let quad = quad.unwrap();
        graph.triples.insert([
            quad.subject.to_string(),
            quad.predicate.to_string(),
            quad.object.to_string(),
        ]);
    }
    graph
}

fn store(graph: &Graph) -> SqliteGraph {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE terms (id INTEGER PRIMARY KEY, n TEXT NOT NULL UNIQUE);
         CREATE TABLE facts (g INTEGER, s INTEGER, p INTEGER, o INTEGER, explicit INTEGER,
           PRIMARY KEY (g,s,p,o)) WITHOUT ROWID;
         CREATE INDEX facts_spo ON facts (s,p,o);
         CREATE INDEX facts_pos ON facts (p,o,s);",
        )
        .unwrap();
    for (index, triple) in graph.triples.iter().enumerate() {
        let mut ids = Vec::new();
        for term in triple {
            connection
                .execute("INSERT OR IGNORE INTO terms(n) VALUES (?)", [term])
                .unwrap();
            ids.push(
                connection
                    .query_row("SELECT id FROM terms WHERE n=?", [term], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
            );
        }
        // Spread facts over named/default graphs and both provenance kinds;
        // duplicate SPOs must not change targets, counts or native constraints.
        for graph in [index % 3, index % 3 + 3] {
            connection
                .execute(
                    "INSERT INTO facts VALUES (?,?,?,?,?)",
                    params![graph as i64, ids[0], ids[1], ids[2], (index % 2) as i64],
                )
                .unwrap();
        }
    }
    SqliteGraph::new(connection).unwrap()
}

fn compare(data: &str, shapes: &str, selected: &[&str], focus: Option<&str>) -> Value {
    let data = graph(data);
    let shapes = graph(shapes);
    let selected = selected
        .iter()
        .map(|name| iri(&format!("urn:example:{name}")))
        .collect();
    let expected = validate_with_depth(&data, &shapes, &selected, focus, 64, true).unwrap();
    let actual = validate_sqlite(&store(&data), &shapes, &selected, focus, 64).unwrap();
    assert_eq!(actual, expected);
    actual
}

#[test]
fn native_core_reads_preserve_closed_peer_and_literal_constraints() {
    let report = compare(
        "ex:a a ex:Person; ex:score 3; ex:ceiling 4; ex:same 3; ex:other 9;
             ex:label \"hello\"@en, \"bonjour\"@fr; ex:link ex:b; ex:extra true .
         ex:b a ex:Child . ex:Child rdfs:subClassOf ex:Person .",
        "ex:Root a sh:NodeShape; sh:targetNode ex:a; sh:closed true;
           sh:ignoredProperties (rdf:type ex:ceiling ex:same ex:other);
           sh:property ex:Score, ex:Label, ex:Link .
         ex:Score sh:path ex:score; sh:minCount 1; sh:maxCount 1;
           sh:datatype xsd:integer; sh:minInclusive 1; sh:maxExclusive 4;
           sh:equals ex:same; sh:disjoint ex:other; sh:lessThan ex:ceiling .
         ex:Label sh:path ex:label; sh:uniqueLang true; sh:languageIn (\"en\" \"fr\");
           sh:minLength 2; sh:pattern \"^[a-z]+$\" .
         ex:Link sh:path ex:link; sh:nodeKind sh:IRI; sh:class ex:Person .",
        &["Root"],
        None,
    );
    assert_eq!(report["target_count"], 1);
    assert_eq!(report["conforms"], false);
    assert_eq!(report["violations"].as_array().unwrap().len(), 1);
}

#[test]
fn target_transitivity_and_pinned_class_constraint_are_distinct() {
    let report = compare(
        "ex:a a ex:Base . ex:b a ex:Child . ex:c a ex:Grandchild .
         ex:Child rdfs:subClassOf ex:Base .
         ex:Grandchild rdfs:subClassOf ex:Child, ex:Peer .
         ex:Peer rdfs:subClassOf ex:Grandchild .",
        "ex:Root a sh:NodeShape; sh:targetClass ex:Base; sh:class ex:Base .",
        &["Root"],
        None,
    );
    assert_eq!(report["target_count"], 3);
    // Preserve this pinned Core implementation's one-hop class check, while
    // explicit targetClass still follows the transitive subclass closure.
    assert_eq!(report["violations"].as_array().unwrap().len(), 1);
    assert_eq!(report["violations"][0]["focus_node"], "urn:example:c");
}

#[test]
fn nested_and_qualified_constraints_visit_the_same_dependencies() {
    let report = compare(
        "ex:a ex:child ex:b, ex:c . ex:b ex:value 2 . ex:c ex:value 7 .",
        "ex:Root a sh:NodeShape; sh:targetNode ex:a; sh:property ex:Children .
         ex:Children sh:path ex:child; sh:node ex:Value;
           sh:qualifiedValueShape [sh:hasValue ex:b]; sh:qualifiedMinCount 2 .
         ex:Value sh:and (
           [sh:property [sh:path ex:value; sh:minCount 1]]
           [sh:xone ([sh:hasValue ex:b] [sh:hasValue ex:c])]);
           sh:not [sh:hasValue ex:absent] .",
        &["Root"],
        None,
    );
    assert_eq!(report["target_count"], 1);
    assert_eq!(
        report["violations"].as_array().unwrap().len(),
        1,
        "{report}"
    );
    assert_eq!(
        report["violations"][0]["source_constraint_component"],
        "http://www.w3.org/ns/shacl#QualifiedMinCountConstraintComponent"
    );
}

#[test]
fn literal_targets_and_lexical_forms_survive_union_navigation() {
    let report = compare(
        "ex:a ex:edge \"01\"^^xsd:integer, \"hello\"@en, ex:b .
         ex:b ex:edge ex:c . _:a..b ex:label \"blank\" .",
        "ex:Objects a sh:NodeShape; sh:targetObjectsOf ex:edge; sh:nodeKind sh:Literal .
         ex:Subjects a sh:NodeShape; sh:targetSubjectsOf ex:edge; sh:nodeKind sh:IRI .",
        &["Objects", "Subjects"],
        None,
    );
    assert_eq!(report["target_count"], 6);
    assert_eq!(report["violations"].as_array().unwrap().len(), 2);
}

#[test]
fn global_queries_keep_union_default_graph_and_bindings() {
    let report = compare(
        "ex:a ex:link ex:b . ex:b ex:label \"B\" .
         ex:unrelated ex:blocked true .",
        "ex:Root a sh:NodeShape; sh:targetNode ex:a;
           sh:property [sh:path (ex:link ex:label); sh:hasValue \"B\"];
           sh:sparql [sh:select \"SELECT $this WHERE { ?else <urn:example:blocked> true }\"],
                     [sh:select \"SELECT $this WHERE { GRAPH ?g { ?s ?p ?o } }\"] .",
        &["Root"],
        None,
    );
    assert_eq!(report["conforms"], false);
    assert_eq!(report["violations"].as_array().unwrap().len(), 1);
    assert_eq!(report["violations"][0]["focus_node"], "urn:example:a");
}

#[test]
fn native_read_failure_never_becomes_a_conformance_report() {
    let mut data = graph("ex:unrelated ex:p true .");
    data.triples.insert([
        iri("urn:example:a"),
        iri(&format!("{RDF}type")),
        "<invalid iri>".into(),
    ]);
    let data = store(&data);
    let shapes = graph("ex:Root a sh:NodeShape; sh:class ex:Person .");
    let error = validate_sqlite(
        &data,
        &shapes,
        &BTreeSet::from([iri("urn:example:Root")]),
        Some("<urn:example:a>"),
        64,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("SHACL input failed"),
        "{error:#}"
    );
    assert!(data.check_error().is_err());
}

#[test]
fn unused_queries_remain_lazy() {
    // A poisoned but unrelated term proves that skipping target queries is
    // behavioral, not merely an eager parse followed by discarded results.
    let mut data = graph("ex:a ex:p true .");
    data.triples.insert([
        "<invalid iri>".into(),
        iri("urn:example:q"),
        iri("urn:example:o"),
    ]);
    let data = store(&data);
    let shapes = graph(
        "ex:Root a sh:NodeShape; sh:nodeKind sh:IRI;
        sh:target [a sh:SPARQLTarget; sh:select \"SELECT ?this WHERE {?this ?p ?o}\"] .
        ex:Inactive a sh:NodeShape; sh:deactivated true;
        sh:sparql [sh:select \"SELECT $this WHERE {?s ?p ?o}\"] .",
    );
    let report = validate_sqlite(
        &data,
        &shapes,
        &BTreeSet::from([iri("urn:example:Root"), iri("urn:example:Inactive")]),
        Some("<urn:example:a>"),
        64,
    )
    .unwrap();
    assert_eq!(report["conforms"], true);
    assert_eq!(report["target_count"], 2);
    let empty = validate_sqlite(&data, &shapes, &BTreeSet::new(), None, 64).unwrap();
    assert_eq!(empty["conforms"], true);
    assert_eq!(empty["target_count"], 0);
}

type LexicalQueryResult = (Option<bool>, Vec<BTreeMap<String, String>>);

fn compare_prepared_query(
    memory: &PreparedQueries<'_>,
    sqlite: &PreparedQueries<'_>,
    text: &str,
    bindings: &[(&str, &str)],
) -> LexicalQueryResult {
    let bindings = bindings
        .iter()
        .map(|(name, term)| ((*name).to_owned(), (*term).to_owned()))
        .collect();
    let run = |queries: &PreparedQueries<'_>| {
        let (boolean, rows) = queries.query_bindings(text, &bindings).unwrap();
        (
            boolean,
            rows.into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|(name, term)| (name, term.to_string()))
                        .collect::<BTreeMap<_, _>>()
                })
                .collect::<Vec<_>>(),
        )
    };
    let expected = run(memory);
    let actual = run(sqlite);
    // Multi-row cases order their output explicitly. Keep duplicate solutions
    // and every auxiliary prebinding column visible to the comparison.
    assert_eq!(actual, expected, "query: {text}; bindings: {bindings:?}");
    actual
}

fn query_column<'a>(result: &'a LexicalQueryResult, name: &str) -> Vec<&'a str> {
    result.1.iter().map(|row| row[name].as_str()).collect()
}

#[test]
fn prepared_queries_preserve_cycles_alternatives_inverse_and_zero_length_paths() {
    let data = graph(
        "ex:a ex:p ex:b; ex:q ex:d . ex:b ex:p ex:c . ex:c ex:p ex:a .
         ex:x ex:p ex:b . ex:isolated ex:label \"only a label\" .",
    );
    let sqlite = store(&data);
    let view = ValidationData::Sqlite(&sqlite);
    let memory_queries = data.prepare_queries().unwrap();
    let sqlite_queries = view.prepare_queries().unwrap();
    for (path, focus, expected) in [
        (
            "<urn:example:p>+",
            "<urn:example:a>",
            vec!["<urn:example:a>", "<urn:example:b>", "<urn:example:c>"],
        ),
        (
            "<urn:example:p>*",
            "<urn:example:a>",
            vec!["<urn:example:a>", "<urn:example:b>", "<urn:example:c>"],
        ),
        (
            "(<urn:example:p>|<urn:example:q>)",
            "<urn:example:a>",
            vec!["<urn:example:b>", "<urn:example:d>"],
        ),
        (
            "^<urn:example:p>",
            "<urn:example:b>",
            vec!["<urn:example:a>", "<urn:example:x>"],
        ),
        (
            "<urn:example:p>/<urn:example:p>",
            "<urn:example:a>",
            vec!["<urn:example:c>"],
        ),
        (
            "<urn:example:p>?",
            "<urn:example:a>",
            vec!["<urn:example:a>", "<urn:example:b>"],
        ),
        (
            "<urn:example:p>*",
            "<urn:example:isolated>",
            vec!["<urn:example:isolated>"],
        ),
        // The pinned evaluator checks graph membership before zero-length
        // paths. Changing the backing store must preserve that behavior.
        ("<urn:example:p>*", "<urn:example:absent>", vec![]),
    ] {
        let text = format!("SELECT ?v WHERE {{ $this {path} ?v }} ORDER BY ?v");
        let result =
            compare_prepared_query(&memory_queries, &sqlite_queries, &text, &[("this", focus)]);
        assert_eq!(query_column(&result, "v"), expected, "{path}: {focus}");
    }
}

#[test]
fn prepared_query_cache_keeps_prebindings_values_and_subquery_scopes_separate() {
    let data = graph("ex:a ex:score 2 . ex:b ex:score 7 .");
    let sqlite = store(&data);
    let view = ValidationData::Sqlite(&sqlite);
    let memory_queries = data.prepare_queries().unwrap();
    let sqlite_queries = view.prepare_queries().unwrap();
    let text = "SELECT ?value WHERE {
        $this <urn:example:score> ?value .
        FILTER(?value >= $minimum && (!BOUND($maximum) || ?value <= $maximum))
    } ORDER BY ?value";
    let minimum = "\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>";
    let maximum = "\"5\"^^<http://www.w3.org/2001/XMLSchema#integer>";
    let result = compare_prepared_query(
        &memory_queries,
        &sqlite_queries,
        text,
        &[("this", "<urn:example:a>"), ("minimum", minimum)],
    );
    assert_eq!(
        query_column(&result, "value"),
        ["\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>"]
    );
    assert!(!result.1[0].contains_key("maximum"));
    let result = compare_prepared_query(
        &memory_queries,
        &sqlite_queries,
        text,
        &[
            ("this", "<urn:example:b>"),
            ("minimum", minimum),
            ("maximum", maximum),
        ],
    );
    assert!(result.1.is_empty());
    let result = compare_prepared_query(
        &memory_queries,
        &sqlite_queries,
        text,
        &[("this", "<urn:example:b>"), ("minimum", minimum)],
    );
    assert_eq!(
        query_column(&result, "value"),
        ["\"7\"^^<http://www.w3.org/2001/XMLSchema#integer>"]
    );
    assert!(!result.1[0].contains_key("maximum"));

    let result = compare_prepared_query(
        &memory_queries,
        &sqlite_queries,
        "SELECT ?value WHERE {
            VALUES (?this ?value) { (<urn:example:a> 1) (<urn:example:b> 2) }
        } ORDER BY ?value",
        &[("this", "<urn:example:b>")],
    );
    assert_eq!(
        query_column(&result, "value"),
        ["\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>"]
    );
    let result = compare_prepared_query(
        &memory_queries,
        &sqlite_queries,
        "SELECT ?inner WHERE {
            { SELECT (?this AS ?inner) WHERE { VALUES ?this { <urn:example:b> } } }
            FILTER(sameTerm($this, <urn:example:a>))
        }",
        &[("this", "<urn:example:a>")],
    );
    assert_eq!(query_column(&result, "inner"), ["<urn:example:b>"]);
}

#[test]
fn prepared_queries_preserve_ask_exists_empty_results_and_expression_errors() {
    let data = graph("ex:a ex:p ex:b . ex:bad ex:number \"invalid\"^^xsd:integer .");
    let sqlite = store(&data);
    let view = ValidationData::Sqlite(&sqlite);
    let memory_queries = data.prepare_queries().unwrap();
    let sqlite_queries = view.prepare_queries().unwrap();
    for (text, expected) in [
        ("ASK {}", true),
        ("ASK { FILTER(BOUND($this)) }", true),
        ("ASK { FILTER($this = <urn:example:a>) }", true),
        ("ASK { FILTER($this = <urn:example:b>) }", false),
        ("ASK { FILTER(BOUND($this)) } LIMIT 0", false),
        ("ASK { FILTER EXISTS { $this <urn:example:p> ?o } }", true),
        (
            "ASK { FILTER NOT EXISTS { $this <urn:example:missing> ?o } }",
            true,
        ),
        (
            "ASK { FILTER NOT EXISTS { $this <urn:example:p> ?o } }",
            false,
        ),
    ] {
        let result = compare_prepared_query(
            &memory_queries,
            &sqlite_queries,
            text,
            &[("this", "<urn:example:a>")],
        );
        assert_eq!(result, (Some(expected), vec![]), "{text}");
    }
    for text in [
        "SELECT ?v WHERE { <urn:example:missing> ?p ?v }",
        "SELECT ?v WHERE { ?s ?p ?v } LIMIT 0",
        "SELECT ?v WHERE { <urn:example:bad> <urn:example:number> ?v FILTER(?v > 0) }",
        "SELECT ?v WHERE { FILTER(?unbound) }",
    ] {
        let result = compare_prepared_query(&memory_queries, &sqlite_queries, text, &[]);
        assert_eq!(result, (None, vec![]), "{text}");
    }
    // Ordinary SPARQL expression errors leave a binding absent; they are not
    // storage failures and must not poison a later query on the same store.
    let result = compare_prepared_query(
        &memory_queries,
        &sqlite_queries,
        "SELECT ((1 / 0) AS ?v) WHERE {}",
        &[],
    );
    assert_eq!(result, (None, vec![BTreeMap::new()]));
    let result = compare_prepared_query(
        &memory_queries,
        &sqlite_queries,
        "ASK { <urn:example:a> <urn:example:p> <urn:example:b> }",
        &[],
    );
    assert_eq!(result.0, Some(true));
    sqlite.check_error().unwrap();
}

#[test]
fn prepared_queries_preserve_union_counts_bags_limits_and_empty_named_graphs() {
    let data = graph("ex:a ex:p ex:b, ex:c . ex:b ex:p ex:a .");
    let sqlite = store(&data);
    let view = ValidationData::Sqlite(&sqlite);
    let memory_queries = data.prepare_queries().unwrap();
    let sqlite_queries = view.prepare_queries().unwrap();
    for (text, expected) in [
        (
            "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }",
            "\"3\"^^<http://www.w3.org/2001/XMLSchema#integer>",
        ),
        (
            "SELECT (COUNT(*) AS ?n) WHERE { ?s <urn:example:absent> ?o }",
            "\"0\"^^<http://www.w3.org/2001/XMLSchema#integer>",
        ),
        (
            "SELECT (COUNT(DISTINCT ?o) AS ?n) WHERE {
                <urn:example:a> <urn:example:p> ?o VALUES ?copy { 1 1 }
            }",
            "\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>",
        ),
    ] {
        let result = compare_prepared_query(&memory_queries, &sqlite_queries, text, &[]);
        assert_eq!(query_column(&result, "n"), [expected]);
    }
    let bag_query = "SELECT ?o WHERE {
        <urn:example:a> <urn:example:p> ?o VALUES ?copy { 1 1 }
    } ORDER BY ?o";
    let result = compare_prepared_query(&memory_queries, &sqlite_queries, bag_query, &[]);
    assert_eq!(
        query_column(&result, "o"),
        [
            "<urn:example:b>",
            "<urn:example:b>",
            "<urn:example:c>",
            "<urn:example:c>"
        ]
    );
    let result = compare_prepared_query(
        &memory_queries,
        &sqlite_queries,
        &format!("{bag_query} OFFSET 1 LIMIT 2"),
        &[],
    );
    assert_eq!(
        query_column(&result, "o"),
        ["<urn:example:b>", "<urn:example:c>"]
    );
    for text in [
        "SELECT ?g ?s WHERE { GRAPH ?g { ?s ?p ?o } }",
        "SELECT ?s WHERE { GRAPH <urn:example:a> { ?s ?p ?o } }",
    ] {
        let result = compare_prepared_query(&memory_queries, &sqlite_queries, text, &[]);
        assert_eq!(result, (None, vec![]));
    }
    for text in ["ASK { GRAPH ?g {} }", "ASK { GRAPH <urn:example:a> {} }"] {
        let result = compare_prepared_query(&memory_queries, &sqlite_queries, text, &[]);
        assert_eq!(result, (Some(false), vec![]));
    }
}

#[test]
fn prepared_queries_preserve_rdf_term_identity_and_document_blank_labels() {
    let data = graph(
        "ex:a ex:number \"01\"^^xsd:integer, \"1\"^^xsd:integer;
            ex:huge \"184467440737095516160\"^^xsd:integer;
            ex:label \"bonjour\"@fr; ex:edge _:a..b .
         _:a..b ex:edge _:c..d . _:c..d ex:label \"leaf\" .",
    );
    let sqlite = store(&data);
    let view = ValidationData::Sqlite(&sqlite);
    let memory_queries = data.prepare_queries().unwrap();
    let sqlite_queries = view.prepare_queries().unwrap();
    let result = compare_prepared_query(
        &memory_queries,
        &sqlite_queries,
        "SELECT ?n (STR(?n) AS ?lexical) (DATATYPE(?n) AS ?type) WHERE {
            <urn:example:a> <urn:example:number> ?n FILTER(?n = 1)
        } ORDER BY STR(?n)",
        &[],
    );
    assert_eq!(query_column(&result, "lexical"), ["\"01\"", "\"1\""]);
    let result = compare_prepared_query(
        &memory_queries,
        &sqlite_queries,
        "SELECT (COUNT(DISTINCT ?n) AS ?count) WHERE { <urn:example:a> <urn:example:number> ?n }",
        &[],
    );
    assert_eq!(
        query_column(&result, "count"),
        ["\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>"]
    );
    let result = compare_prepared_query(
        &memory_queries,
        &sqlite_queries,
        "SELECT ?n WHERE {
            <urn:example:a> <urn:example:number> ?n
            FILTER(sameTerm(?n, \"01\"^^<http://www.w3.org/2001/XMLSchema#integer>))
        }",
        &[],
    );
    assert_eq!(
        query_column(&result, "n"),
        ["\"01\"^^<http://www.w3.org/2001/XMLSchema#integer>"]
    );
    let result = compare_prepared_query(
        &memory_queries,
        &sqlite_queries,
        "ASK {
            <urn:example:a> <urn:example:huge> ?n
            FILTER(?n > 184467440737095516159 && ?n + 1 = 184467440737095516161)
        }",
        &[],
    );
    assert_eq!(result.0, Some(true));
    let result = compare_prepared_query(
        &memory_queries,
        &sqlite_queries,
        "SELECT ?node ?next WHERE {
            <urn:example:a> <urn:example:edge> ?node .
            ?node <urn:example:edge> ?next . ?next <urn:example:label> \"leaf\"
        }",
        &[],
    );
    assert_eq!(query_column(&result, "node"), ["_:a..b"]);
    assert_eq!(query_column(&result, "next"), ["_:c..d"]);
    let result = compare_prepared_query(
        &memory_queries,
        &sqlite_queries,
        "SELECT (STR(?label) AS ?text) (LANG(?label) AS ?language) WHERE {
            <urn:example:a> <urn:example:label> ?label
        }",
        &[],
    );
    assert_eq!(query_column(&result, "text"), ["\"bonjour\""]);
    assert_eq!(query_column(&result, "language"), ["\"fr\""]);

    // Computed terms must rejoin stored terms by RDF identity. Terms absent
    // from the dictionary remain usable in VALUES, expressions and results.
    let result = compare_prepared_query(
        &memory_queries,
        &sqlite_queries,
        "SELECT ?s ?n WHERE {
            BIND(STRDT(\"01\", <http://www.w3.org/2001/XMLSchema#integer>) AS ?n)
            ?s <urn:example:number> ?n
        }",
        &[],
    );
    assert_eq!(query_column(&result, "s"), ["<urn:example:a>"]);
    let result = compare_prepared_query(
        &memory_queries,
        &sqlite_queries,
        "SELECT ?v WHERE { VALUES ?v { <urn:example:unrecorded> \"computed\" } } ORDER BY ?v",
        &[],
    );
    assert_eq!(result.1.len(), 2);
    let result = compare_prepared_query(
        &memory_queries,
        &sqlite_queries,
        "ASK { BIND(BNODE(\"same\") AS ?a) BIND(BNODE(\"same\") AS ?b) FILTER(sameTerm(?a, ?b)) }",
        &[],
    );
    assert_eq!(result.0, Some(true));
}
