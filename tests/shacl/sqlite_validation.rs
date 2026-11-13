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
