//! Read-only SPARQL helpers shared by Full and standard SHACL constraints.
use anyhow::{Context, Result, ensure};
use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;
use serde_json::{Map, Value, json};
#[derive(Clone)]
struct Token {
    word: String,
    start: usize,
    end: usize,
}

fn tokens(text: &str) -> Vec<Token> {
    let b = text.as_bytes();
    let mut i = 0;
    let mut result = vec![];
    while i < b.len() {
        if b[i] == b'#' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == b'\'' || b[i] == b'"' {
            let q = b[i];
            let n = if i + 2 < b.len() && b[i + 1] == q && b[i + 2] == q {
                3
            } else {
                1
            };
            i += n;
            while i < b.len() {
                if b[i] == b'\\' {
                    i = (i + 2).min(b.len());
                } else if b[i..].starts_with(&vec![q; n]) {
                    i += n;
                    break;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        if b[i] == b'<'
            && let Some(n) = b[i + 1..].iter().position(|x| *x == b'>')
            && !b[i + 1..i + 1 + n].iter().any(|c| c.is_ascii_whitespace())
        {
            i += n + 2;
            continue;
        }
        let start = i;
        if b[i] == b'?' || b[i] == b'$' || b[i].is_ascii_alphabetic() || b[i] == b'_' {
            i += 1;
            while i < b.len() {
                if b[i].is_ascii_alphanumeric() || b[i] == b'_' {
                    i += 1;
                } else if b[i] >= 128 {
                    i += text[i..]
                        .chars()
                        .next()
                        .expect("UTF-8 character")
                        .len_utf8();
                } else {
                    break;
                }
            }
        } else if b"{}()".contains(&b[i]) {
            i += 1;
        } else {
            i += 1;
            continue;
        }
        result.push(Token {
            word: text[start..i].into(),
            start,
            end: i,
        });
    }
    result
}

pub fn local_query(text: &str) -> Result<()> {
    let ts = tokens(text);
    ensure!(
        !ts.iter().any(|t| ["FROM", "SERVICE"]
            .iter()
            .any(|w| t.word.eq_ignore_ascii_case(w))
            && text.as_bytes().get(t.start.wrapping_sub(1)) != Some(&b':')),
        "external query I/O is disabled"
    );
    ensure!(
        matches!(
            spargebra::SparqlParser::new().parse_query(text)?,
            spargebra::Query::Select { dataset: None, .. }
                | spargebra::Query::Ask { dataset: None, .. }
        ),
        "only local SELECT/ASK queries are supported"
    );
    Ok(())
}

pub fn contains_variable(text: &str, name: &str) -> bool {
    tokens(text)
        .iter()
        .any(|token| token.word.strip_prefix(['?', '$']) == Some(name))
}

pub fn local_select(text: &str) -> Result<()> {
    let ts = tokens(text);
    ensure!(
        !ts.iter().any(|t| ["FROM", "SERVICE"]
            .iter()
            .any(|w| t.word.eq_ignore_ascii_case(w))
            && text.as_bytes().get(t.start.wrapping_sub(1)) != Some(&b':')),
        "external query I/O is disabled"
    );
    ensure!(
        matches!(
            spargebra::SparqlParser::new().parse_query(text)?,
            spargebra::Query::Select { dataset: None, .. }
        ),
        "profile requires a local SELECT query"
    );
    Ok(())
}

pub fn shacl_path(text: &str, path: Option<&str>) -> Result<String> {
    let mut output = text.to_owned();
    for token in tokens(text)
        .into_iter()
        .rev()
        .filter(|t| t.word == "$PATH" || t.word == "?PATH")
    {
        output.replace_range(
            token.start..token.end,
            path.context("SHACL PATH requires a PropertyShape path")?,
        );
    }
    Ok(output)
}

pub fn term_json(term: &Term) -> Value {
    match term {
        Term::NamedNode(n) => json!({"type":"uri","value":n.as_str()}),
        Term::BlankNode(n) => json!({"type":"bnode","value":n.as_str()}),
        Term::Literal(l) => {
            let mut item = json!({"type":"literal","value":l.value()});
            if let Some(lang) = l.language() {
                item["xml:lang"] = json!(lang);
            } else {
                item["datatype"] = json!(l.datatype().as_str());
            }
            item
        }
        #[allow(unreachable_patterns)]
        _ => json!({"type":"unsupported","value":term.to_string()}),
    }
}

pub fn collect(
    results: QueryResults<'_>,
    max_rows: usize,
    max_bytes: usize,
    view: &str,
) -> Result<Value> {
    match results {
        QueryResults::Boolean(value) => Ok(
            json!({"boolean":value,"variables":[],"items":[],"complete":true,"truncated":false,"query_type":"ASK","view":view}),
        ),
        QueryResults::Solutions(rows) => {
            let vars = rows
                .variables()
                .iter()
                .map(|v| v.as_str().to_owned())
                .collect::<Vec<_>>();
            let mut items = Vec::new();
            let mut bytes = 0;
            for row in rows {
                ensure!(items.len() < max_rows, "query output row budget exceeded");
                let row = row?;
                let mut item = Map::new();
                for (var, term) in row.iter() {
                    item.insert(var.as_str().into(), term_json(term));
                }
                bytes += serde_json::to_vec(&item)?.len();
                ensure!(bytes <= max_bytes, "query output byte budget exceeded");
                items.push(Value::Object(item));
            }
            Ok(
                json!({"variables":vars,"items":items,"complete":true,"truncated":false,"query_type":"SELECT","view":view}),
            )
        }
        _ => anyhow::bail!("only local SELECT/ASK queries are supported"),
    }
}
