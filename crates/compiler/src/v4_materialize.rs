//! In-memory positive-rule fixpoint over physical RDF quads.
//!
//! The rule representation and built-in RDFS rules follow Full's admitted
//! ontology-rule-ir/v1 profile, but runtime relationships remain integer IDs.
//! These rules operate on the physical default graph unless an atom explicitly
//! declares a graph. The SPARQL default-graph union is NOT a rule input scope.
use anyhow::{Context, Result, bail, ensure};
use ontokiln_pack::v4::{DEFAULT_GRAPH, Metadata, PackV4, Quad, Statement};
use oxrdf::Term;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub op: String,
    pub var: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub rule_id: String,
    pub head: Vec<String>,
    pub body: Vec<Vec<String>>,
    #[serde(default)]
    pub filters: Vec<Filter>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Materialization {
    pub explicit_facts: usize,
    pub inferred_facts: usize,
    pub fixpoint_iterations: usize,
    pub executed_rule_evaluations: usize,
}

#[derive(Clone, Copy)]
enum Cell {
    Fixed(u32),
    Var(usize),
}

#[derive(Clone, Copy)]
enum Check {
    Iri,
    Resource,
    Literal,
}

struct Program {
    head: [Cell; 4],
    body: Vec<[Cell; 4]>,
    filters: Vec<(Check, usize)>,
    variables: usize,
}

#[derive(Default)]
struct Relation {
    rows: Vec<Quad>,
    by_p: HashMap<u32, Vec<Quad>>,
    by_s: HashMap<u32, Vec<Quad>>,
    by_sp: HashMap<(u32, u32), Vec<Quad>>,
    by_po: HashMap<(u32, u32), Vec<Quad>>,
}
impl Relation {
    fn insert(&mut self, q: Quad) {
        self.rows.push(q);
        self.by_p.entry(q[2]).or_default().push(q);
        self.by_s.entry(q[1]).or_default().push(q);
        self.by_sp.entry((q[1], q[2])).or_default().push(q);
        self.by_po.entry((q[2], q[3])).or_default().push(q);
    }
    fn candidates(&self, pattern: [Option<u32>; 4]) -> &[Quad] {
        let [_, s, p, o] = pattern;
        if let (Some(s), Some(p)) = (s, p) {
            self.by_sp.get(&(s, p)).map_or(&[], Vec::as_slice)
        } else if let (Some(p), Some(o)) = (p, o) {
            self.by_po.get(&(p, o)).map_or(&[], Vec::as_slice)
        } else if let Some(p) = p {
            self.by_p.get(&p).map_or(&[], Vec::as_slice)
        } else if let Some(s) = s {
            self.by_s.get(&s).map_or(&[], Vec::as_slice)
        } else {
            &self.rows
        }
    }
}
fn var(s: &str) -> bool {
    s.starts_with('?')
        && s.len() > 1
        && s[1..]
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
}
fn atom(values: &[String]) -> Result<[String; 4]> {
    ensure!(
        values.len() == 3 || values.len() == 4,
        "RULE_INVALID: atoms need 3 or 4 terms"
    );
    let cells: Vec<_> = if values.len() == 3 {
        std::iter::once("DEFAULT".to_owned())
            .chain(values.iter().cloned())
            .collect()
    } else {
        values.to_vec()
    };
    let mut canonical = Vec::with_capacity(4);
    for (column, value) in cells.iter().enumerate() {
        if value == "DEFAULT" {
            ensure!(column == 0, "RULE_INVALID: DEFAULT only in graph position");
            canonical.push(value.clone());
        } else if var(value) {
            canonical.push(value.clone());
        } else {
            ensure!(!value.starts_with('?'), "RULE_INVALID: invalid variable");
            let parsed = value
                .parse::<Term>()
                .context("RULE_INVALID: invalid RDF term")?;
            ensure!(
                !(column == 0 || column == 1) || !matches!(parsed, Term::Literal(_)),
                "RULE_INVALID: subject/graph cannot be a literal"
            );
            ensure!(
                column != 2 || matches!(parsed, Term::NamedNode(_)),
                "RULE_INVALID: predicate must be an IRI"
            );
            canonical.push(parsed.to_string());
        }
    }
    Ok(canonical.try_into().expect("exactly four cells"))
}

fn normalize_rules(rules: &[Rule]) -> Result<Vec<Rule>> {
    let mut ids = HashSet::new();
    let mut result = Vec::with_capacity(rules.len());
    for rule in rules {
        ensure!(
            !rule.rule_id.is_empty() && ids.insert(rule.rule_id.as_str()),
            "RULE_INVALID: duplicate or empty rule id"
        );
        ensure!(
            !rule.body.is_empty() && rule.body.len() <= 16,
            "RULE_INVALID: rule requires 1..16 atoms"
        );
        let body = rule
            .body
            .iter()
            .map(|a| atom(a).map(|a| a.to_vec()))
            .collect::<Result<Vec<_>>>()?;
        let head = atom(&rule.head)?.to_vec();
        let bound: HashSet<_> = body.iter().flatten().filter(|v| var(v)).collect();
        for h in head.iter().filter(|v| var(v)) {
            ensure!(bound.contains(h), "UNSAFE_RULE: unbound head variable {h}");
        }
        for f in &rule.filters {
            ensure!(
                bound.contains(&f.var),
                "UNSUPPORTED_RULE_PROFILE: unbound filter variable"
            );
            ensure!(
                ["isIRI", "isResource", "isLiteral"].contains(&f.op.as_str()),
                "UNSUPPORTED_RULE_PROFILE: unsupported term-kind filter"
            );
        }
        result.push(Rule {
            rule_id: rule.rule_id.clone(),
            head,
            body,
            filters: rule.filters.clone(),
        });
    }
    Ok(result)
}

fn compile_rules(rules: &[Rule], terms: &HashMap<String, u32>) -> Result<Vec<Program>> {
    let mut output = Vec::new();
    for rule in rules {
        let mut vars = HashMap::<String, usize>::new();
        for term in rule.body.iter().flatten().filter(|v| var(v)) {
            let next = vars.len();
            vars.entry(term.clone()).or_insert(next);
        }
        let encode = |cells: &[String]| -> Result<[Cell; 4]> {
            cells
                .iter()
                .map(|value| {
                    if var(value) {
                        Ok(Cell::Var(
                            *vars.get(value).context("UNSAFE_RULE: unbound variable")?,
                        ))
                    } else if value == "DEFAULT" {
                        Ok(Cell::Fixed(DEFAULT_GRAPH))
                    } else {
                        Ok(Cell::Fixed(
                            *terms.get(value).context("missing normalized rule term")?,
                        ))
                    }
                })
                .collect::<Result<Vec<_>>>()?
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid atom"))
        };
        let filters = rule
            .filters
            .iter()
            .map(|f| {
                Ok((
                    match f.op.as_str() {
                        "isIRI" => Check::Iri,
                        "isResource" => Check::Resource,
                        "isLiteral" => Check::Literal,
                        _ => bail!("unsupported filter"),
                    },
                    *vars.get(&f.var).context("missing filter variable")?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        output.push(Program {
            head: encode(&rule.head)?,
            body: rule
                .body
                .iter()
                .map(|a| encode(a))
                .collect::<Result<Vec<_>>>()?,
            filters,
            variables: vars.len(),
        });
    }
    Ok(output)
}

fn resolve(cell: Cell, env: &[Option<u32>]) -> Option<u32> {
    match cell {
        Cell::Fixed(id) => Some(id),
        Cell::Var(i) => env[i],
    }
}
fn evaluate(
    rule: &Program,
    index: usize,
    pivot: usize,
    env: &mut [Option<u32>],
    all: &Relation,
    delta: &Relation,
    kind: &[u8],
    output: &mut HashSet<Quad>,
) -> Result<()> {
    if index == rule.body.len() {
        for (check, variable) in &rule.filters {
            let id = env[*variable].context("unbound term-kind filter")?;
            let tag = kind.get(id as usize).copied().unwrap_or(b'D');
            if !match check {
                Check::Iri => tag == b'I',
                Check::Resource => tag == b'I' || tag == b'B',
                Check::Literal => tag == b'L',
            } {
                return Ok(());
            }
        }
        let result: Quad = rule
            .head
            .map(|cell| resolve(cell, env).expect("safe head variables were bound by the body"));
        ensure!(
            result[1] != DEFAULT_GRAPH && result[2] != DEFAULT_GRAPH && result[3] != DEFAULT_GRAPH,
            "RULE_INVALID: DEFAULT only in graph position"
        );
        let term_kind = |id: u32| kind.get(id as usize).copied().unwrap_or(b'D');
        ensure!(
            term_kind(result[1]) != b'L'
                && term_kind(result[2]) == b'I'
                && (result[0] == DEFAULT_GRAPH || term_kind(result[0]) != b'L'),
            "RULE_INVALID: derived quad has invalid RDF term kinds"
        );
        output.insert(result);
        return Ok(());
    }
    let atom = &rule.body[index];
    let bound = atom.map(|cell| resolve(cell, env));
    let relation = if index == pivot { delta } else { all };
    for &quad in relation.candidates(bound) {
        let mut assigned = Vec::new();
        let mut compatible = true;
        for (position, cell) in atom.iter().enumerate() {
            match cell {
                Cell::Fixed(id) if *id != quad[position] => {
                    compatible = false;
                    break;
                }
                Cell::Var(i) => match env[*i] {
                    Some(id) if id != quad[position] => {
                        compatible = false;
                        break;
                    }
                    None => {
                        env[*i] = Some(quad[position]);
                        assigned.push(*i);
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        if compatible {
            evaluate(rule, index + 1, pivot, env, all, delta, kind, output)?;
        }
        for variable in assigned {
            env[variable] = None;
        }
    }
    Ok(())
}

fn build(id: &str, head: &[&str], body: &[&[&str]], filters: &[(&str, &str)]) -> Rule {
    Rule {
        rule_id: format!("rdfs-v1:{id}"),
        head: head.iter().map(|x| (*x).into()).collect(),
        body: body
            .iter()
            .map(|row| row.iter().map(|x| (*x).into()).collect())
            .collect(),
        filters: filters
            .iter()
            .map(|(op, var)| Filter {
                op: (*op).into(),
                var: (*var).into(),
            })
            .collect(),
    }
}

/// Exactly the six RDFS rule templates and three inverse/symmetric templates
/// already used by Full. Property-chain RDF list extraction is a separate step.
pub fn profile_rules(profile: &str) -> Result<Vec<Rule>> {
    ensure!(
        [
            "no-rules-v1",
            "datalog-v1",
            "rdfs-supported-v1",
            "rdfs-property-chain-v1"
        ]
        .contains(&profile),
        "UNSUPPORTED_RULE_PROFILE: unknown profile"
    );
    if !profile.starts_with("rdfs-") {
        return Ok(vec![]);
    }
    let rdf = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    let sc = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
    let sp = "<http://www.w3.org/2000/01/rdf-schema#subPropertyOf>";
    let domain = "<http://www.w3.org/2000/01/rdf-schema#domain>";
    let range = "<http://www.w3.org/2000/01/rdf-schema#range>";
    let mut rules = vec![
        build(
            "subclass-transitive",
            &["?a", sc, "?c"],
            &[&["?a", sc, "?b"], &["?b", sc, "?c"]],
            &[],
        ),
        build(
            "type",
            &["?x", rdf, "?b"],
            &[&["?x", rdf, "?a"], &["?a", sc, "?b"]],
            &[],
        ),
        build(
            "subproperty-transitive",
            &["?a", sp, "?c"],
            &[&["?a", sp, "?b"], &["?b", sp, "?c"]],
            &[],
        ),
        build(
            "property",
            &["?x", "?q", "?y"],
            &[&["?p", sp, "?q"], &["?x", "?p", "?y"]],
            &[],
        ),
        build(
            "domain",
            &["?x", rdf, "?c"],
            &[&["?p", domain, "?c"], &["?x", "?p", "?y"]],
            &[],
        ),
        build(
            "range",
            &["?y", rdf, "?c"],
            &[&["?p", range, "?c"], &["?x", "?p", "?y"]],
            &[("isResource", "?y")],
        ),
    ];
    if profile == "rdfs-property-chain-v1" {
        let inv = "<http://www.w3.org/2002/07/owl#inverseOf>";
        let symmetric = "<http://www.w3.org/2002/07/owl#SymmetricProperty>";
        rules.extend([
            build(
                "inverse-forward",
                &["?y", "?q", "?x"],
                &[&["?p", inv, "?q"], &["?x", "?p", "?y"]],
                &[("isIRI", "?p"), ("isIRI", "?q"), ("isResource", "?y")],
            ),
            build(
                "inverse-reverse",
                &["?y", "?p", "?x"],
                &[&["?p", inv, "?q"], &["?x", "?q", "?y"]],
                &[("isIRI", "?p"), ("isIRI", "?q"), ("isResource", "?y")],
            ),
            build(
                "symmetric",
                &["?y", "?p", "?x"],
                &[&["?p", rdf, symmetric], &["?x", "?p", "?y"]],
                &[("isResource", "?y")],
            ),
        ]);
    }
    Ok(rules)
}

/// Compute a complete, immutable closure without an SQLite file or external
/// process. A failed rule or incomplete fixpoint publishes no Pack.
pub fn materialize(
    pack: PackV4,
    profile: &str,
    user_rules: &[Rule],
) -> Result<(PackV4, Materialization)> {
    let mut rules = profile_rules(profile)?;
    ensure!(
        profile != "no-rules-v1" || user_rules.is_empty(),
        "UNSUPPORTED_RULE_PROFILE: no-rules does not accept rules"
    );
    // Explicitly reject unimplemented property-chain lists, rather than return
    // a false successful closure for the requested profile.
    if profile == "rdfs-property-chain-v1" {
        let p = "<http://www.w3.org/2002/07/owl#propertyChainAxiom>";
        if let Some(id) = pack.term_id(p) {
            ensure!(
                pack.matching(
                    None,
                    Some(id),
                    None,
                    Some(DEFAULT_GRAPH),
                    ontokiln_pack::v4::View::Asserted
                )
                .next()
                .is_none(),
                "UNSUPPORTED_RULE_PROFILE: Pack v4 propertyChainAxiom extraction pending"
            );
        }
    }
    rules.extend(user_rules.iter().cloned());
    let rules = normalize_rules(&rules)?;
    let mut vocabulary = BTreeSet::new();
    for value in pack.terms() {
        vocabulary.insert(value.clone());
    }
    for rule in &rules {
        for constant in rule.head.iter().chain(rule.body.iter().flatten()) {
            if constant != "DEFAULT" && !var(constant) {
                vocabulary.insert(constant.clone());
            }
        }
    }
    let vocabulary: Vec<_> = vocabulary.into_iter().collect();
    let dictionary: HashMap<_, _> = vocabulary
        .iter()
        .enumerate()
        .map(|(i, term)| Ok((term.clone(), u32::try_from(i)?)))
        .collect::<Result<_>>()?;
    let mapping: Vec<u32> = pack.terms().iter().map(|term| dictionary[term]).collect();
    let remap = |id: u32| {
        if id == DEFAULT_GRAPH {
            id
        } else {
            mapping[id as usize]
        }
    };
    let mut rows = pack
        .statements()
        .map(|mut statement| {
            statement.quad = statement.quad.map(remap);
            statement
        })
        .collect::<Vec<Statement>>();
    ensure!(
        rows.iter().all(|r| r.asserted),
        "Pack v4 materialization requires asserted-only input"
    );
    let mut all = Relation::default();
    let mut known = HashSet::<Quad>::with_capacity(rows.len());
    for record in &rows {
        if known.insert(record.quad) {
            all.insert(record.quad);
        }
    }
    let mut delta = Relation::default();
    for &fact in &known {
        delta.insert(fact);
    }
    let kind: Vec<u8> = vocabulary
        .iter()
        .map(|term| {
            let value = term
                .parse::<Term>()
                .expect("Pack vocabulary is already validated");
            if matches!(value, Term::NamedNode(_)) {
                b'I'
            } else if matches!(value, Term::BlankNode(_)) {
                b'B'
            } else if matches!(value, Term::Literal(_)) {
                b'L'
            } else {
                b'T'
            }
        })
        .collect();
    let program = compile_rules(&rules, &dictionary)?;
    let mut stats = Materialization {
        explicit_facts: known.len(),
        ..Materialization::default()
    };
    if !program.is_empty() {
        loop {
            stats.fixpoint_iterations += 1;
            let mut next = HashSet::new();
            for rule in &program {
                for pivot in 0..rule.body.len() {
                    stats.executed_rule_evaluations += 1;
                    evaluate(
                        rule,
                        0,
                        pivot,
                        &mut vec![None; rule.variables],
                        &all,
                        &delta,
                        &kind,
                        &mut next,
                    )?;
                }
            }
            let mut new: Vec<_> = next.into_iter().filter(|q| !known.contains(q)).collect();
            if new.is_empty() {
                break;
            }
            new.sort_unstable();
            let mut upcoming = Relation::default();
            for q in new {
                known.insert(q);
                all.insert(q);
                upcoming.insert(q);
                rows.push(Statement {
                    quad: q,
                    asserted: false,
                    sources: vec![],
                });
                stats.inferred_facts += 1;
            }
            delta = upcoming;
        }
    }
    let mut metadata: Metadata = pack.metadata().clone();
    metadata.profile_id = profile.into();
    metadata.fixpoint_reached = true;
    let recipe = serde_json::to_vec(&(profile, &rules))?;
    metadata.semantic_fingerprint = format!(
        "{:x}",
        Sha256::digest([&metadata.source_tree_digest.as_bytes()[..], &recipe].concat())
    );
    Ok((PackV4::from_statements(vocabulary, rows, metadata)?, stats))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ontokiln_pack::v4::{Statement, View};

    fn fixture(facts: &[(&str, &str, &str)], graph: bool) -> PackV4 {
        let mut vocab = BTreeSet::new();
        for &(s, p, o) in facts {
            vocab.extend([s.to_owned(), p.to_owned(), o.to_owned()]);
        }
        if graph {
            vocab.insert("<urn:g>".into());
        }
        let words: Vec<_> = vocab.into_iter().collect();
        let ids: HashMap<_, _> = words
            .iter()
            .enumerate()
            .map(|(i, t)| (t.clone(), i as u32))
            .collect();
        let rows = facts
            .iter()
            .map(|(s, p, o)| Statement {
                quad: [
                    if graph { ids["<urn:g>"] } else { DEFAULT_GRAPH },
                    ids[*s],
                    ids[*p],
                    ids[*o],
                ],
                asserted: true,
                sources: vec![],
            })
            .collect();
        PackV4::from_statements(words, rows, Metadata::new("no-rules-v1", true)).unwrap()
    }
    #[test]
    fn datalog_recursive_joins_reach_fixpoint_and_preserve_asserted() {
        let input = fixture(
            &[
                ("<urn:a>", "<urn:edge>", "<urn:b>"),
                ("<urn:b>", "<urn:edge>", "<urn:c>"),
                ("<urn:c>", "<urn:edge>", "<urn:a>"),
            ],
            false,
        );
        let rules = vec![
            Rule {
                rule_id: "seed".into(),
                head: vec!["?x".into(), "<urn:reach>".into(), "?y".into()],
                body: vec![vec!["?x".into(), "<urn:edge>".into(), "?y".into()]],
                filters: vec![],
            },
            Rule {
                rule_id: "transitive".into(),
                head: vec!["?x".into(), "<urn:reach>".into(), "?z".into()],
                body: vec![
                    vec!["?x".into(), "<urn:reach>".into(), "?y".into()],
                    vec!["?y".into(), "<urn:edge>".into(), "?z".into()],
                ],
                filters: vec![],
            },
        ];
        let (pack, stats) = materialize(input, "datalog-v1", &rules).unwrap();
        assert_eq!(stats.explicit_facts, 3);
        assert_eq!(stats.inferred_facts, 9);
        assert!(stats.fixpoint_iterations > 1);
        let p = pack.term_id("<urn:reach>").unwrap();
        assert_eq!(
            pack.union_triples(None, Some(p), None, View::Closure)
                .count(),
            9
        );
        assert_eq!(
            pack.union_triples(None, Some(p), None, View::Asserted)
                .count(),
            0
        );
        assert_eq!(
            pack.union_triples(None, Some(p), None, View::Inferred)
                .count(),
            9
        );
    }
    #[test]
    fn rdfs_rules_have_full_domain_range_and_graph_scope() {
        let input = fixture(
            &[
                (
                    "<urn:dog>",
                    "<http://www.w3.org/2000/01/rdf-schema#subClassOf>",
                    "<urn:animal>",
                ),
                (
                    "<urn:animal>",
                    "<http://www.w3.org/2000/01/rdf-schema#subClassOf>",
                    "<urn:life>",
                ),
                (
                    "<urn:fido>",
                    "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>",
                    "<urn:dog>",
                ),
            ],
            false,
        );
        let (pack, _) = materialize(input, "rdfs-supported-v1", &[]).unwrap();
        let t = pack
            .term_id("<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>")
            .unwrap();
        let s = pack.term_id("<urn:fido>").unwrap();
        let o = pack.term_id("<urn:life>").unwrap();
        assert_eq!(
            pack.union_triples(Some(s), Some(t), Some(o), View::Inferred)
                .count(),
            1
        );

        let isolated = fixture(
            &[
                (
                    "<urn:fido>",
                    "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>",
                    "<urn:dog>",
                ),
                (
                    "<urn:dog>",
                    "<http://www.w3.org/2000/01/rdf-schema#subClassOf>",
                    "<urn:life>",
                ),
            ],
            true,
        );
        let (pack, stats) = materialize(isolated, "rdfs-supported-v1", &[]).unwrap();
        assert_eq!(stats.inferred_facts, 0);
        assert_eq!(pack.quad_count(), 2);
    }
    #[test]
    fn rejects_invalid_or_unsafe_rule_without_publishing_pack() {
        let pack = fixture(&[("<urn:a>", "<urn:p>", "<urn:b>")], false);
        let rule = Rule {
            rule_id: "unsafe".into(),
            head: vec!["?missing".into(), "<urn:x>".into(), "?o".into()],
            body: vec![vec!["?s".into(), "<urn:p>".into(), "?o".into()]],
            filters: vec![],
        };
        assert!(materialize(pack, "datalog-v1", &[rule]).is_err());
    }
    #[test]
    fn inverse_and_symmetric_rule_profiles_match_full_scope() {
        let input = fixture(
            &[
                (
                    "<urn:p>",
                    "<http://www.w3.org/2002/07/owl#inverseOf>",
                    "<urn:q>",
                ),
                ("<urn:s>", "<urn:p>", "<urn:o>"),
                (
                    "<urn:q>",
                    "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>",
                    "<http://www.w3.org/2002/07/owl#SymmetricProperty>",
                ),
            ],
            false,
        );
        let (pack, _) = materialize(input, "rdfs-property-chain-v1", &[]).unwrap();
        let p = pack.term_id("<urn:q>").unwrap();
        let s = pack.term_id("<urn:o>").unwrap();
        let o = pack.term_id("<urn:s>").unwrap();
        assert_eq!(
            pack.union_triples(Some(s), Some(p), Some(o), View::Inferred)
                .count(),
            1
        );
    }
}
