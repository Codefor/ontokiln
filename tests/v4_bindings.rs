//! Initial bindings keep the existing public API and SEP-0007 scope semantics.
use ontokiln::engine::{DerivedOptions, OntologyEngine};
use ontokiln_compiler::v4::Source;
use ontokiln_pack::v4::View;
use ontokiln_reasoner::parameters::{Parameter, ValuesTable};
use oxigraph::io::{RdfFormat, RdfParser};
use oxrdf::Dataset;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use spareval::{QueryEvaluator, QueryResults};
use spargebra::SparqlParser;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use tempfile::tempdir;

const DATA: &str = r#"
    <urn:a> <urn:p> 1; <urn:name> "A"; <urn:link> <urn:b> .
    <urn:b> <urn:p> 2; <urn:name> "B" .
    <urn:c> <urn:p> 3 .
"#;

fn engine() -> OntologyEngine {
    let root = tempdir().unwrap();
    let path = root.path().join("data.ttl");
    fs::write(&path, DATA).unwrap();
    OntologyEngine::build(
        &[Source {
            path,
            document_id: "synthetic".into(),
            format: "turtle".into(),
            expected_sha256: format!("{:x}", Sha256::digest(DATA.as_bytes())),
            base_iri: None,
        }],
        "no-rules-v1",
        &[],
    )
    .unwrap()
    .0
}

fn bindings(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect()
}

fn query(engine: &OntologyEngine, text: &str, pairs: &[(&str, &str)]) -> Value {
    serde_json::from_slice(
        &engine
            .query_bound(
                text,
                View::Asserted,
                &BTreeMap::new(),
                &bindings(pairs),
                &[],
            )
            .unwrap(),
    )
    .unwrap()
}

// A deliberately unoptimized evaluator is the pre-change reference. Fixed
// expected answers below protect against both implementations sharing a bug.
fn reference(text: &str, pairs: &[(&str, &str)]) -> Value {
    let mut dataset = Dataset::new();
    for quad in RdfParser::from_format(RdfFormat::Turtle).for_reader(DATA.as_bytes()) {
        dataset.insert(&quad.unwrap());
    }
    let mut parsed = SparqlParser::new().parse_query(text).unwrap();
    let columns = if matches!(parsed, spargebra::Query::Select { .. }) {
        ontokiln::query::expose_bindings(&mut parsed, pairs.iter().map(|(k, _)| (*k).into()))
            .unwrap()
    } else {
        None
    };
    let evaluator = QueryEvaluator::new().without_optimizations();
    let mut prepared = evaluator.prepare(&parsed);
    for (name, value) in pairs {
        prepared = prepared.substitute_variable(
            oxrdf::Variable::new(*name).unwrap(),
            value.parse::<oxrdf::Term>().unwrap(),
        );
    }
    let result = prepared.execute(&dataset).unwrap();
    let serializer =
        sparesults::QueryResultsSerializer::from_format(sparesults::QueryResultsFormat::Json);
    let mut bytes = Vec::new();
    match result {
        QueryResults::Boolean(value) => {
            serializer
                .serialize_boolean_to_writer(&mut bytes, value)
                .unwrap();
        }
        QueryResults::Solutions(solutions) => {
            let mut writer = serializer
                .serialize_solutions_to_writer(&mut bytes, solutions.variables().to_vec())
                .unwrap();
            for solution in solutions {
                writer.serialize(&solution.unwrap()).unwrap();
            }
            writer.finish().unwrap();
        }
        _ => panic!("not a SELECT/ASK query"),
    }
    let mut result: Value = serde_json::from_slice(&bytes).unwrap();
    if let Some(columns) = columns {
        let keep: BTreeSet<_> = columns.iter().collect();
        for row in result["results"]["bindings"].as_array_mut().unwrap() {
            row.as_object_mut().unwrap().retain(|k, _| keep.contains(k));
        }
        result["head"]["vars"] = json!(columns);
    }
    result
}

fn uri(value: &str) -> Value {
    json!({"type":"uri", "value":value})
}

fn integer(value: &str) -> Value {
    json!({"type":"literal", "value":value,
        "datatype":"http://www.w3.org/2001/XMLSchema#integer"})
}

#[test]
fn filter_only_inputs_and_bound_preserve_public_columns_and_term_equality() {
    let engine = engine();
    let text = "SELECT ?s WHERE { ?s <urn:p> ?n FILTER(?n = ?input && BOUND(?input)) } ORDER BY ?s";
    let input = [(
        "input",
        "\"01\"^^<http://www.w3.org/2001/XMLSchema#integer>",
    )];
    let result = query(&engine, text, &input);
    assert_eq!(result, reference(text, &input));
    assert_eq!(result["head"]["vars"], json!(["s"]));
    assert_eq!(result["results"]["bindings"], json!([{"s":uri("urn:a")} ]));
    let absent = query(&engine, text, &[]);
    assert_eq!(absent["results"]["bindings"], json!([]));

    let same = "SELECT ?s WHERE { ?s <urn:p> ?n FILTER(sameTerm(?n, ?input)) }";
    assert_eq!(
        query(&engine, same, &input)["results"]["bindings"],
        json!([])
    );
    let literal = "SELECT ?copy WHERE { BIND(?input AS ?copy) }";
    let input = [("input", "\"bonjour\"@fr")];
    assert_eq!(query(&engine, literal, &input), reference(literal, &input));
    assert_eq!(
        query(&engine, literal, &input)["results"]["bindings"],
        json!([{"copy":{"type":"literal","value":"bonjour","xml:lang":"fr"}}])
    );
}

#[test]
fn scopes_optional_union_minus_exists_and_aggregates_match_fixed_answers() {
    let engine = engine();
    let input = [("input", "<urn:a>")];
    let cases = [
        (
            "SELECT ?s WHERE { ?s <urn:p> ?n FILTER(?s = ?input) }",
            json!([{"s":uri("urn:a")} ]),
        ),
        (
            "SELECT ?s ?v WHERE { ?s <urn:p> ?n OPTIONAL { ?s <urn:name> ?v FILTER(?s = ?input) } } ORDER BY ?s",
            json!([{"s":uri("urn:a"),"v":{"type":"literal","value":"A"}}, {"s":uri("urn:b")}, {"s":uri("urn:c")} ]),
        ),
        (
            "SELECT ?s WHERE { { ?s <urn:p> 1 FILTER(?s = ?input) } UNION { ?s <urn:p> 2 FILTER(BOUND(?input)) } } ORDER BY ?s",
            json!([{"s":uri("urn:a")}, {"s":uri("urn:b")} ]),
        ),
        (
            "SELECT ?s WHERE { ?s <urn:p> ?n FILTER EXISTS { ?input <urn:link> ?s } }",
            json!([{"s":uri("urn:b")} ]),
        ),
        (
            "SELECT ?s WHERE { ?s <urn:p> ?n FILTER NOT EXISTS { ?input <urn:link> ?s } } ORDER BY ?s",
            json!([{"s":uri("urn:a")}, {"s":uri("urn:c")} ]),
        ),
        (
            "SELECT ?s WHERE { ?s <urn:p> ?n MINUS { ?s <urn:p> 2 FILTER(BOUND(?input)) } } ORDER BY ?s",
            json!([{"s":uri("urn:a")}, {"s":uri("urn:c")} ]),
        ),
        // Non-projected input is hidden inside this subquery.
        (
            "SELECT ?s WHERE { { SELECT ?s WHERE { ?s <urn:p> ?n FILTER(!BOUND(?input)) } } } ORDER BY ?s",
            json!([{"s":uri("urn:a")}, {"s":uri("urn:b")}, {"s":uri("urn:c")} ]),
        ),
        // Projection explicitly admits the same input in this subquery.
        (
            "SELECT ?s WHERE { { SELECT ?s ?input WHERE { ?s <urn:p> ?n FILTER(?s = ?input) } } }",
            json!([{"s":uri("urn:a")} ]),
        ),
        (
            "SELECT DISTINCT ?s WHERE { VALUES ?s { <urn:a> <urn:a> <urn:b> } FILTER(BOUND(?input)) } ORDER BY ?s LIMIT 1 OFFSET 1",
            json!([{"s":uri("urn:b")} ]),
        ),
        (
            "SELECT (COUNT(*) AS ?n) WHERE { VALUES ?s { <urn:a> <urn:a> <urn:b> } FILTER(BOUND(?input)) }",
            json!([{"n":integer("3")} ]),
        ),
        (
            "SELECT ?s (COUNT(*) AS ?n) WHERE { VALUES ?s { <urn:a> <urn:a> <urn:b> } FILTER(BOUND(?input)) } GROUP BY ?s ORDER BY ?s",
            json!([{"s":uri("urn:a"), "n":integer("2")}, {"s":uri("urn:b"), "n":integer("1")} ]),
        ),
    ];
    for (text, expected) in cases {
        let result = query(&engine, text, &input);
        assert_eq!(result, reference(text, &input), "{text}");
        assert_eq!(result["results"]["bindings"], expected, "{text}");
    }
}

#[test]
fn ask_preserves_existing_binding_admission_and_results() {
    let engine = engine();
    for (text, expected) in [
        ("ASK { ?input <urn:p> ?n FILTER(BOUND(?input)) }", true),
        ("ASK { ?input <urn:p> ?n FILTER(!BOUND(?input)) }", false),
        (
            "ASK { ?input <urn:p> ?n BIND(?input AS ?s) FILTER(?s = <urn:a>) }",
            true,
        ),
        (
            "ASK { ?input <urn:p> ?n FILTER EXISTS { ?input <urn:link> <urn:b> } }",
            true,
        ),
    ] {
        let input = [("input", "<urn:a>")];
        let result = query(&engine, text, &input);
        assert_eq!(result, reference(text, &input), "{text}");
        assert_eq!(result["boolean"], expected, "{text}");
    }
    assert_eq!(
        query(&engine, "ASK { FILTER(BOUND(?input)) }", &[])["boolean"],
        false
    );
    assert!(
        engine
            .query_bound(
                "ASK { FILTER(BOUND(?input)) }",
                View::Asserted,
                &BTreeMap::new(),
                &bindings(&[("input", "<urn:a>")]),
                &[]
            )
            .is_err()
    );
    assert!(
        engine
            .query_bound(
                "ASK {}",
                View::Asserted,
                &BTreeMap::new(),
                &bindings(&[("missing", "<urn:a>")]),
                &[]
            )
            .is_err()
    );
}

#[test]
fn overwrites_undef_and_conflicting_values_preserve_bags_and_rdf_terms() {
    let engine = engine();
    for (text, input, expected) in [
        (
            "SELECT ?input WHERE { BIND(2 AS ?input) }",
            "<urn:a>",
            json!([]),
        ),
        (
            "SELECT ?s WHERE { BIND(2 AS ?input) BIND(?input AS ?s) }",
            "<urn:a>",
            json!([]),
        ),
        (
            "SELECT ?input WHERE { VALUES ?input { UNDEF <urn:a> <urn:a> <urn:b> } }",
            "<urn:a>",
            json!([{"input":uri("urn:a")}, {"input":uri("urn:a")}, {"input":uri("urn:a")} ]),
        ),
        (
            "SELECT ?input WHERE { BIND((1 / 0) AS ?input) }",
            "<urn:a>",
            json!([{"input":uri("urn:a")} ]),
        ),
    ] {
        let pairs = [("input", input)];
        let result = query(&engine, text, &pairs);
        assert_eq!(result, reference(text, &pairs), "{text}");
        assert_eq!(result["results"]["bindings"], expected, "{text}");
    }
}

#[test]
fn parameters_values_and_derived_overlay_keep_the_existing_api() {
    let engine = engine();
    let parameters = BTreeMap::from([(
        "predicate".into(),
        Parameter::Iri {
            value: "urn:p".into(),
        },
    )]);
    let values = [ValuesTable {
        variables: vec!["chosen".into()],
        rows: vec![
            vec![Some(Parameter::Iri {
                value: "urn:a".into(),
            })],
            vec![Some(Parameter::Iri {
                value: "urn:a".into(),
            })],
        ],
    }];
    let bytes = engine.query_bound(
        "SELECT ?chosen WHERE { VALUES ?chosen {} ?chosen <urn:ontokiln:param:predicate> ?n FILTER(BOUND(?input)) }",
        View::Asserted, &parameters, &bindings(&[("input", "<urn:a>")]), &values,
    ).unwrap();
    let result: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(result["head"]["vars"], json!(["chosen"]));
    assert_eq!(
        result["results"]["bindings"],
        json!([{"chosen":uri("urn:a")}, {"chosen":uri("urn:a")} ])
    );

    let derived = [
        "CONSTRUCT { ?input <urn:derived> ?n } WHERE { ?input <urn:p> ?n FILTER(BOUND(?input)) }"
            .into(),
    ];
    let bytes = engine
        .query_with_derived(
            "SELECT ?n WHERE { ?input <urn:derived> ?n }",
            View::Asserted,
            &BTreeMap::new(),
            &bindings(&[("input", "<urn:a>")]),
            &[],
            DerivedOptions {
                queries: &derived,
                max_derived_facts: 8,
                max_intermediate_rows: 8,
            },
        )
        .unwrap();
    let result: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(result["head"]["vars"], json!(["n"]));
    assert_eq!(result["results"]["bindings"], json!([{"n":integer("1")} ]));
    assert_eq!(
        query(&engine, "ASK { ?s <urn:derived> ?n }", &[])["boolean"],
        false
    );
    assert!(
        engine
            .query_with_derived(
                "ASK {}",
                View::Asserted,
                &BTreeMap::new(),
                &bindings(&[("input", "<urn:a>")]),
                &[],
                DerivedOptions {
                    queries: &derived,
                    max_derived_facts: 0,
                    max_intermediate_rows: 8
                }
            )
            .is_err()
    );
}

// Count actual dataset rows visited rather than enforcing a wall-clock limit.
struct CountedDataset<'a> {
    dataset: &'a Dataset,
    rows: std::rc::Rc<std::cell::Cell<usize>>,
}
impl<'a> spareval::QueryableDataset<'a> for CountedDataset<'a> {
    type InternalTerm = <&'a Dataset as spareval::QueryableDataset<'a>>::InternalTerm;
    type Error = std::convert::Infallible;

    fn internal_quads_for_pattern(
        &self,
        subject: Option<&Self::InternalTerm>,
        predicate: Option<&Self::InternalTerm>,
        object: Option<&Self::InternalTerm>,
        graph_name: Option<Option<&Self::InternalTerm>>,
    ) -> impl Iterator<Item = Result<spareval::InternalQuad<Self::InternalTerm>, Self::Error>> + use<'a>
    {
        let rows = self.rows.clone();
        self.dataset
            .internal_quads_for_pattern(subject, predicate, object, graph_name)
            .inspect(move |_| rows.set(rows.get() + 1))
    }
    fn internalize_term(&self, term: oxrdf::Term) -> Result<Self::InternalTerm, Self::Error> {
        self.dataset.internalize_term(term)
    }
    fn externalize_term(&self, term: Self::InternalTerm) -> Result<oxrdf::Term, Self::Error> {
        self.dataset.externalize_term(term)
    }
}

#[test]
fn nested_lateral_rhs_uses_left_bindings_without_repeated_full_scans() {
    const N: usize = 64;
    let mut rdf = String::new();
    for i in 0..N {
        rdf += &format!("<urn:s:{i}> <urn:edge> <urn:t:{i}>; <urn:source> <urn:e:{i}> .\n");
        rdf += &format!("<urn:t:{i}> <urn:label> <urn:v:{i}> .\n");
        rdf += &format!("<urn:e:{i}> <urn:status> \"confirmed\"; <urn:revision> \"v1\" .\n");
    }
    let mut dataset = Dataset::new();
    for quad in RdfParser::from_format(RdfFormat::Turtle).for_reader(rdf.as_bytes()) {
        dataset.insert(&quad.unwrap());
    }
    let text = "SELECT ?s ?v WHERE { { ?s <urn:edge> ?t; <urn:source> ?e . ?t <urn:label> ?v BIND(1 AS ?kind) } ?e <urn:status> \"confirmed\"; <urn:revision> ?revision FILTER(?revision = ?input) } ORDER BY ?s";
    let mut parsed = SparqlParser::new().parse_query(text).unwrap();
    ontokiln::query::expose_bindings(&mut parsed, std::iter::once("input".into())).unwrap();
    let rows = std::rc::Rc::new(std::cell::Cell::new(0));
    let evaluator = QueryEvaluator::new();
    let result = evaluator
        .prepare(&parsed)
        .substitute_variable(
            oxrdf::Variable::new("input").unwrap(),
            oxrdf::Literal::new_simple_literal("v1"),
        )
        .execute(CountedDataset {
            dataset: &dataset,
            rows: rows.clone(),
        })
        .unwrap();
    let QueryResults::Solutions(solutions) = result else {
        panic!("SELECT required")
    };
    let expected: BTreeSet<_> = (0..N)
        .map(|i| (format!("<urn:s:{i}>"), format!("<urn:v:{i}>")))
        .collect();
    let actual = solutions
        .map(|s| {
            let s = s.unwrap();
            (
                s.get("s").unwrap().to_string(),
                s.get("v").unwrap().to_string(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(actual.len(), N);
    assert_eq!(actual.into_iter().collect::<BTreeSet<_>>(), expected);
    assert!(
        rows.get() <= 8 * N,
        "{} rows visited for {N} linked entities",
        rows.get()
    );
}
