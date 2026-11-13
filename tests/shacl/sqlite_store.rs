//! Storage boundary tests, included inside the request-scoped SQLite adapter.
use super::*;
use oxrdf::NamedNode;
use rudof_rdf::rdf_core::Any;
use rusqlite::{ffi, params};

type Fact<'a> = (&'a str, &'a str, &'a str, Option<&'a str>, bool);

fn term_id(db: &Connection, text: &str) -> i64 {
    db.execute("INSERT OR IGNORE INTO terms(n) VALUES (?)", [text])
        .expect("insert term");
    db.query_row("SELECT id FROM terms WHERE n=?", [text], |row| row.get(0))
        .expect("term id")
}

fn database(facts: &[Fact<'_>]) -> Connection {
    let db = Connection::open_in_memory().expect("memory database");
    db.execute_batch(
        "CREATE TABLE terms(id INTEGER PRIMARY KEY, n TEXT NOT NULL UNIQUE);
         CREATE TABLE facts(g INTEGER NOT NULL,s INTEGER NOT NULL,p INTEGER NOT NULL,
            o INTEGER NOT NULL,explicit INTEGER NOT NULL,PRIMARY KEY(g,s,p,o)) WITHOUT ROWID;
         CREATE INDEX facts_spo ON facts(s,p,o,g,explicit);
         CREATE INDEX facts_pos ON facts(p,o,s,g);
         CREATE INDEX facts_osp ON facts(o,s,p,g);",
    )
    .expect("read-contract tables");
    for &(subject, predicate, object, graph, explicit) in facts {
        let subject = term_id(&db, subject);
        let predicate = term_id(&db, predicate);
        let object = term_id(&db, object);
        let graph = graph.map_or(0, |graph| term_id(&db, graph));
        db.execute(
            "INSERT INTO facts(g,s,p,o,explicit) VALUES (?,?,?,?,?)",
            params![graph, subject, predicate, object, i64::from(explicit)],
        )
        .expect("insert fact");
    }
    db
}

fn error<T>(result: Result<T>) -> anyhow::Error {
    match result {
        Ok(_) => panic!("expected a storage error"),
        Err(error) => error,
    }
}

fn rows(triples: impl Iterator<Item = Triple>) -> BTreeSet<[String; 3]> {
    triples
        .map(|triple| {
            [
                triple.subject.to_string(),
                triple.predicate.to_string(),
                triple.object.to_string(),
            ]
        })
        .collect()
}

fn pattern<S, P, O>(
    graph: &SqliteGraph,
    subject: &S,
    predicate: &P,
    object: &O,
) -> BTreeSet<[String; 3]>
where
    S: Matcher<NamedOrBlankNode>,
    P: Matcher<NamedNode>,
    O: Matcher<Term>,
{
    rows(
        graph
            .triples_matching(subject, predicate, object)
            .expect("matching triples"),
    )
}

fn expected(values: &[[&str; 3]]) -> BTreeSet<[String; 3]> {
    values.iter().map(|row| row.map(str::to_owned)).collect()
}

#[test]
fn all_graphs_and_views_form_one_distinct_union_for_every_pattern() {
    let graph = SqliteGraph::new(database(&[
        ("<urn:s>", "<urn:p>", "\"one\"", None, true),
        ("<urn:s>", "<urn:p>", "\"one\"", Some("<urn:g1>"), true),
        ("<urn:s>", "<urn:p>", "\"one\"", Some("<urn:g2>"), false),
        ("<urn:s>", "<urn:p>", "\"two\"", Some("<urn:g1>"), false),
        ("<urn:s>", "<urn:q>", "\"one\"", Some("<urn:g2>"), true),
        ("<urn:t>", "<urn:p>", "\"one\"", None, false),
        (
            "_:subject..tail",
            "<urn:p>",
            "_:value..tail",
            Some("<urn:g2>"),
            true,
        ),
    ]))
    .expect("SQLite graph");
    let union = expected(&[
        ["<urn:s>", "<urn:p>", "\"one\""],
        ["<urn:s>", "<urn:p>", "\"two\""],
        ["<urn:s>", "<urn:q>", "\"one\""],
        ["<urn:t>", "<urn:p>", "\"one\""],
        ["_:subject..tail", "<urn:p>", "_:value..tail"],
    ]);
    assert_eq!(graph.load_graph().expect("union graph").triples, union);
    assert_eq!(rows(graph.triples().expect("native graph")), union);

    let subject = NamedOrBlankNode::NamedNode(NamedNode::new("urn:s").expect("subject"));
    let predicate = NamedNode::new("urn:p").expect("predicate");
    let object = Term::from(oxrdf::Literal::new_simple_literal("one"));
    assert_eq!(
        graph
            .objects_for(&Term::from(subject.clone()), &predicate)
            .expect("native object projection"),
        HashSet::from([
            object.clone(),
            Term::from(oxrdf::Literal::new_simple_literal("two")),
        ])
    );
    let fixed = ["<urn:s>", "<urn:p>", "\"one\""];
    let patterns = [
        (0, pattern(&graph, &Any, &Any, &Any)),
        (1, pattern(&graph, &subject, &Any, &Any)),
        (2, pattern(&graph, &Any, &predicate, &Any)),
        (3, pattern(&graph, &subject, &predicate, &Any)),
        (4, pattern(&graph, &Any, &Any, &object)),
        (5, pattern(&graph, &subject, &Any, &object)),
        (6, pattern(&graph, &Any, &predicate, &object)),
        (7, pattern(&graph, &subject, &predicate, &object)),
    ];
    for (mask, actual) in patterns {
        let expected: BTreeSet<_> = union
            .iter()
            .filter(|row| {
                (0..3).all(|column| mask & (1 << column) == 0 || row[column] == fixed[column])
            })
            .cloned()
            .collect();
        assert_eq!(actual, expected, "fixed-position mask {mask}");
    }
    let missing = NamedNode::new("urn:missing").expect("missing predicate");
    assert!(pattern(&graph, &Any, &missing, &Any).is_empty());
    assert_eq!(
        graph.subjects("<urn:p>", "\"one\"").expect("subjects"),
        BTreeSet::from(["<urn:s>".into(), "<urn:t>".into()])
    );
    assert_eq!(
        graph
            .predicate_subjects("<urn:p>")
            .expect("target subjects"),
        BTreeSet::from(["<urn:s>".into(), "<urn:t>".into(), "_:subject..tail".into()])
    );
    assert_eq!(
        graph.predicate_objects("<urn:p>").expect("target objects"),
        BTreeSet::from(["\"one\"".into(), "\"two\"".into(), "_:value..tail".into()])
    );
    graph.check_error().expect("complete reads");
}

#[test]
fn native_terms_preserve_blank_labels_and_literal_lexical_forms() {
    let integer = "\"007\"^^<http://www.w3.org/2001/XMLSchema#integer>";
    let graph = SqliteGraph::new(database(&[
        ("_:subject..tail", "<urn:p>", "_:value..tail", None, true),
        ("_:subject..tail", "<urn:p>", integer, None, true),
        ("_:subject..tail", "<urn:p>", "\"Label\"@en", None, false),
    ]))
    .expect("SQLite graph");
    let subject =
        NamedOrBlankNode::BlankNode(BlankNode::new("subject..tail").expect("blank subject"));
    let object = Term::BlankNode(BlankNode::new("value..tail").expect("blank object"));
    let predicate = NamedNode::new("urn:p").expect("predicate");
    assert_eq!(
        pattern(&graph, &subject, &predicate, &object),
        expected(&[["_:subject..tail", "<urn:p>", "_:value..tail"]])
    );
    assert_eq!(
        pattern(&graph, &subject, &predicate, &Any),
        expected(&[
            ["_:subject..tail", "<urn:p>", "_:value..tail"],
            ["_:subject..tail", "<urn:p>", integer],
            ["_:subject..tail", "<urn:p>", "\"Label\"@en"],
        ])
    );
}

#[test]
fn construction_rejects_missing_read_schema_before_any_shape_access() {
    let absent = Connection::open_in_memory().expect("empty database");
    let missing_tables = error(SqliteGraph::new(absent));
    assert!(missing_tables.to_string().contains("no such table"));
    let wrong_columns = Connection::open_in_memory().expect("wrong schema database");
    wrong_columns
        .execute_batch(
            "CREATE TABLE terms(id INTEGER PRIMARY KEY,n TEXT);
         CREATE TABLE facts(s INTEGER,p INTEGER);",
        )
        .expect("incomplete facts schema");
    let missing_column = error(SqliteGraph::new(wrong_columns));
    assert!(missing_column.to_string().contains("no such column"));
}

#[test]
fn sql_failure_is_sticky_even_after_the_schema_is_repaired() {
    let graph = SqliteGraph::new(database(&[])).expect("SQLite graph");
    graph
        .db
        .execute_batch("PRAGMA query_only=OFF; DROP TABLE terms; PRAGMA query_only=ON;")
        .expect("inject schema failure after admission");
    let first = error(graph.predicate_subjects("<urn:p>"));
    assert!(!first.is::<ShaclBudgetExceeded>());
    graph
        .db
        .execute_batch(
            "PRAGMA query_only=OFF;
         CREATE TABLE terms(id INTEGER PRIMARY KEY,n TEXT);
         PRAGMA query_only=ON;",
        )
        .expect("repair source for sticky-error regression");
    for _ in 0..2 {
        assert_eq!(error(graph.check_error()).to_string(), first.to_string());
        assert_eq!(error(graph.load_graph()).to_string(), first.to_string());
    }
}

#[test]
fn row_conversion_failure_is_sticky_after_the_value_is_repaired() {
    let db = database(&[("<urn:s>", "<urn:p>", "\"ok\"", None, true)]);
    let object = term_id(&db, "\"ok\"");
    db.execute(
        "UPDATE terms SET n=? WHERE id=?",
        params![vec![0_u8, 1_u8], object],
    )
    .expect("inject non-text term column");
    let graph = SqliteGraph::new(db).expect("SQLite graph");
    let first = error(graph.predicate_objects("<urn:p>"));
    assert!(!first.is::<ShaclBudgetExceeded>());
    graph
        .db
        .execute_batch("PRAGMA query_only=OFF")
        .expect("repair access");
    graph
        .db
        .execute("UPDATE terms SET n=? WHERE id=?", params!["\"ok\"", object])
        .expect("repair term storage class");
    graph
        .db
        .execute_batch("PRAGMA query_only=ON")
        .expect("restore read-only");
    assert_eq!(error(graph.check_error()).to_string(), first.to_string());
    assert_eq!(
        error(graph.predicate_objects("<urn:p>")).to_string(),
        first.to_string()
    );
    assert_eq!(error(graph.check_error()).to_string(), first.to_string());
}

#[test]
fn native_decode_failures_include_blank_label_rules_and_remain_sticky() {
    for bad_term in ["_:bad:label", "<broken iri>"] {
        let graph = SqliteGraph::new(database(&[("<urn:s>", "<urn:p>", bad_term, None, true)]))
            .expect("SQLite graph");
        let first = error(graph.triples());
        assert!(first.to_string().contains("SHACL input failed"));
        assert!(!first.is::<ShaclBudgetExceeded>());
        assert_eq!(error(graph.check_error()).to_string(), first.to_string());
        assert_eq!(error(graph.triples()).to_string(), first.to_string());
        assert_eq!(error(graph.check_error()).to_string(), first.to_string());
    }
}

#[test]
fn literal_subject_conversion_is_an_rdf_result_not_a_store_failure() {
    let graph = SqliteGraph::new(database(&[("<urn:s>", "<urn:p>", "\"ok\"", None, true)]))
        .expect("SQLite graph");
    let literal = Term::from(oxrdf::Literal::new_simple_literal("literal focus"));
    let predicate = NamedNode::new("urn:p").expect("predicate");
    assert!(<SqliteGraph as Rdf>::term_as_subject(&literal).is_err());
    assert!(graph.objects_for(&literal, &predicate).is_err());
    graph.check_error().expect("no I/O error");
    assert_eq!(rows(graph.triples().expect("still readable")).len(), 1);
}

#[test]
fn visitor_budget_preserves_its_type_and_stops_before_the_next_value() {
    let graph = SqliteGraph::new(database(&[
        ("<urn:s>", "<urn:p>", "\"one\"", None, true),
        ("<urn:s>", "<urn:p>", "\"one\"", Some("<urn:g>"), true),
        ("<urn:s>", "<urn:p>", "\"two\"", None, true),
        ("<urn:s>", "<urn:p>", "\"three\"", None, false),
    ]))
    .expect("SQLite graph");
    let mut visited = 0;
    let budget = error(graph.visit_objects("<urn:s>", "<urn:p>", |_| {
        visited += 1;
        Err(ShaclBudgetExceeded("test callback budget").into())
    }));
    assert_eq!(visited, 1);
    assert!(budget.is::<ShaclBudgetExceeded>());
    graph
        .check_error()
        .expect("caller error did not poison store");
    let mut values = BTreeSet::new();
    graph
        .visit_objects("<urn:s>", "<urn:p>", |value| {
            values.insert(value.to_owned());
            Ok(())
        })
        .expect("subsequent complete read");
    assert_eq!(
        values,
        BTreeSet::from(["\"one\"".into(), "\"two\"".into(), "\"three\"".into()])
    );
}

#[test]
fn a_complete_recent_neighborhood_reuses_values_without_sql_but_never_hides_failure() {
    let mut graph = SqliteGraph::new(database(&[
        ("<urn:s>", "<urn:p>", "\"one\"", None, true),
        ("<urn:s>", "<urn:p>", "\"one\"", Some("<urn:g>"), false),
        ("<urn:s>", "<urn:p>", "_:value..tail", None, false),
    ]))
    .expect("SQLite graph");
    let subject = Term::from(NamedNode::new("urn:s").expect("subject"));
    let predicate = NamedNode::new("urn:p").expect("predicate");
    let mut visited = BTreeSet::new();
    graph
        .visit_objects("<urn:s>", "<urn:p>", |value| {
            visited.insert(value.to_owned());
            Ok(())
        })
        .expect("complete neighborhood");
    assert_eq!(visited.len(), 2);
    graph
        .db
        .progress_handler(1, Some(|| true))
        .expect("reject any new SQLite work");
    let reused = graph
        .objects_for(&subject, &predicate)
        .expect("complete neighborhood must not execute SQL");
    assert_eq!(
        reused
            .into_iter()
            .map(|term| term.to_string())
            .collect::<BTreeSet<_>>(),
        visited
    );
    graph.check_error().expect("cache hit did not run SQL");

    let failure = error(graph.predicate_objects("<urn:uncached-predicate>"));
    assert!(failure.is::<ShaclBudgetExceeded>());
    graph
        .db
        .progress_handler(0, None::<fn() -> bool>)
        .expect("restore successful database access");
    assert!(graph.objects_for(&subject, &predicate).is_err());
    assert!(error(graph.check_error()).is::<ShaclBudgetExceeded>());
}

#[test]
fn neighborhood_count_bound_does_not_truncate_a_complete_read() {
    for count in [4096, 4097] {
        let objects = (0..count)
            .map(|index| format!("<urn:object:{index}>"))
            .collect::<Vec<_>>();
        let facts = objects
            .iter()
            .map(|object| ("<urn:s>", "<urn:p>", object.as_str(), None, true))
            .collect::<Vec<_>>();
        let mut graph = SqliteGraph::new(database(&facts)).expect("SQLite graph");
        let mut visited = 0;
        graph
            .visit_objects("<urn:s>", "<urn:p>", |_| {
                visited += 1;
                Ok(())
            })
            .expect("complete large neighborhood");
        assert_eq!(visited, count);
        graph
            .db
            .progress_handler(1, Some(|| true))
            .expect("reject SQL for cache probe");
        let subject = Term::from(NamedNode::new("urn:s").expect("subject"));
        let predicate = NamedNode::new("urn:p").expect("predicate");
        if count == 4096 {
            assert_eq!(
                graph
                    .objects_for(&subject, &predicate)
                    .expect("bounded complete cache")
                    .len(),
                count
            );
            graph.check_error().expect("no SQL on hit");
        } else {
            assert!(graph.objects_for(&subject, &predicate).is_err());
            assert!(error(graph.check_error()).is::<ShaclBudgetExceeded>());
        }
    }
}

#[test]
fn neighborhood_byte_bound_does_not_truncate_large_values() {
    let first = format!("\"{}\"", "x".repeat(600 * 1024));
    let second = format!("\"{}\"", "y".repeat(600 * 1024));
    let mut graph = SqliteGraph::new(database(&[
        ("<urn:s>", "<urn:p>", &first, None, true),
        ("<urn:s>", "<urn:p>", &second, None, true),
    ]))
    .expect("SQLite graph");
    let mut bytes = 0;
    let mut count = 0;
    graph
        .visit_objects("<urn:s>", "<urn:p>", |value| {
            bytes += value.len();
            count += 1;
            Ok(())
        })
        .expect("all large values remain visible");
    assert_eq!(count, 2);
    assert_eq!(bytes, first.len() + second.len());
    graph
        .db
        .progress_handler(1, Some(|| true))
        .expect("reject SQL for cache probe");
    let subject = Term::from(NamedNode::new("urn:s").expect("subject"));
    let predicate = NamedNode::new("urn:p").expect("predicate");
    assert!(graph.objects_for(&subject, &predicate).is_err());
    assert!(error(graph.check_error()).is::<ShaclBudgetExceeded>());
}

#[test]
fn failed_visitors_discard_both_previous_and_partial_neighborhoods() {
    let mut graph = SqliteGraph::new(database(&[
        ("<urn:s>", "<urn:p>", "\"one\"", None, true),
        ("<urn:s>", "<urn:p>", "\"two\"", None, true),
        ("<urn:s>", "<urn:p>", "\"three\"", None, true),
    ]))
    .expect("SQLite graph");
    graph
        .visit_objects("<urn:s>", "<urn:p>", |_| Ok(()))
        .expect("initial complete cache");
    let mut visited = 0;
    let interrupted = error(graph.visit_objects("<urn:s>", "<urn:p>", |_| {
        visited += 1;
        if visited == 2 {
            Err(ShaclBudgetExceeded("test interrupted visitor").into())
        } else {
            Ok(())
        }
    }));
    assert_eq!(visited, 2);
    assert!(interrupted.is::<ShaclBudgetExceeded>());
    graph
        .check_error()
        .expect("callback failure preserves its own type");
    graph
        .db
        .progress_handler(1, Some(|| true))
        .expect("reject SQL for cache probe");
    let subject = Term::from(NamedNode::new("urn:s").expect("subject"));
    let predicate = NamedNode::new("urn:p").expect("predicate");
    assert!(graph.objects_for(&subject, &predicate).is_err());
    assert!(error(graph.check_error()).is::<ShaclBudgetExceeded>());
}

#[test]
fn native_object_projection_and_cache_both_keep_decode_errors_sticky() {
    for cache in [false, true] {
        let graph = SqliteGraph::new(database(&[(
            "<urn:s>",
            "<urn:p>",
            "_:invalid:label",
            None,
            true,
        )]))
        .expect("SQLite graph");
        if cache {
            graph
                .visit_objects("<urn:s>", "<urn:p>", |_| Ok(()))
                .expect("complete lexical read");
        }
        let subject = Term::from(NamedNode::new("urn:s").expect("subject"));
        let predicate = NamedNode::new("urn:p").expect("predicate");
        assert!(graph.objects_for(&subject, &predicate).is_err());
        let failure = error(graph.check_error());
        assert!(failure.to_string().contains("invalid blank node label"));
        assert!(!failure.is::<ShaclBudgetExceeded>());
        assert!(graph.objects_for(&subject, &predicate).is_err());
        assert_eq!(error(graph.check_error()).to_string(), failure.to_string());
    }
}

#[test]
fn sqlite_full_and_interrupt_remain_typed_in_the_sticky_error() {
    for code in [ffi::SQLITE_FULL, ffi::SQLITE_INTERRUPT] {
        let graph = SqliteGraph::new(database(&[])).expect("SQLite graph");
        // A deterministic native error avoids filling the host's temporary disk.
        let first = error(graph.sql::<()>(Err(rusqlite::Error::SqliteFailure(
            ffi::Error::new(code),
            Some("injected resource failure".into()),
        ))));
        assert!(first.is::<ShaclBudgetExceeded>());
        // A later unrelated fault cannot replace the first fault or its category.
        let _ = graph.sql::<()>(Err(rusqlite::Error::SqliteFailure(
            ffi::Error::new(ffi::SQLITE_CORRUPT),
            Some("later fault".into()),
        )));
        for _ in 0..2 {
            let remembered = error(graph.check_error());
            assert!(remembered.is::<ShaclBudgetExceeded>());
            assert_eq!(remembered.to_string(), first.to_string());
        }
        assert!(error(graph.load_graph()).is::<ShaclBudgetExceeded>());
    }
}

#[test]
fn a_real_sqlite_interrupt_cannot_become_an_empty_result() {
    let mut graph = SqliteGraph::new(database(&[("<urn:s>", "<urn:p>", "\"ok\"", None, true)]))
        .expect("SQLite graph");
    graph
        .db
        .progress_handler(1, Some(|| true))
        .expect("interrupt handler");
    let first = error(graph.predicate_objects("<urn:p>"));
    assert!(first.is::<ShaclBudgetExceeded>());
    graph
        .db
        .progress_handler(0, None::<fn() -> bool>)
        .expect("disable interruption");
    assert!(error(graph.check_error()).is::<ShaclBudgetExceeded>());
    assert_eq!(error(graph.load_graph()).to_string(), first.to_string());
}
