//! RDF graph navigation for standard constraints; no domain vocabulary or pack metadata.
use anyhow::{Result, ensure};
use oxrdf::Term;
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
pub const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
pub const SH: &str = "http://www.w3.org/ns/shacl#";
pub fn iri(s: &str) -> String {
    format!("<{s}>")
}
pub fn value(s: &str) -> String {
    match s.parse::<Term>() {
        Ok(Term::NamedNode(n)) => n.as_str().into(),
        Ok(Term::BlankNode(n)) => format!("_:{}", n.as_str()),
        Ok(Term::Literal(l)) => l.value().into(),
        _ => s.into(),
    }
}
#[derive(Clone, Default)]
pub struct Graph {
    pub triples: BTreeSet<[String; 3]>,
}
type BoundQueryResult = (Option<bool>, Vec<BTreeMap<String, Term>>);
impl Graph {
    pub fn scan(&self) -> BTreeSet<[String; 3]> {
        self.triples.clone()
    }
    pub fn objects(&self, s: &str, p: &str) -> BTreeSet<String> {
        self.triples
            .iter()
            .filter(|t| t[0] == s && t[1] == p)
            .map(|t| t[2].clone())
            .collect()
    }
    pub fn subjects(&self, p: &str, o: &str) -> BTreeSet<String> {
        self.triples
            .iter()
            .filter(|t| t[1] == p && t[2] == o)
            .map(|t| t[0].clone())
            .collect()
    }
    pub fn has(&self, s: &str, p: &str, o: &str) -> bool {
        self.triples
            .contains(&[s.to_owned(), p.to_owned(), o.to_owned()])
    }
    pub fn has_type(&self, s: &str, class: &str) -> bool {
        let mut pending = self
            .objects(s, &iri(&format!("{RDF}type")))
            .into_iter()
            .collect::<Vec<_>>();
        let mut seen = HashSet::new();
        while let Some(t) = pending.pop() {
            if t == class {
                return true;
            }
            if seen.insert(t.clone()) {
                pending.extend(self.objects(&t, &iri(&format!("{RDFS}subClassOf"))));
            }
        }
        false
    }
    pub fn one(&self, s: &str, p: &str) -> Result<String> {
        let values = self.objects(s, p);
        ensure!(values.len() == 1, "Expected one value for {p}");
        Ok(values.into_iter().next().expect("one value"))
    }
    pub fn nt(&self) -> String {
        self.triples
            .iter()
            .map(|t| format!("{} {} {} .\n", t[0], t[1], t[2]))
            .collect()
    }
    pub fn select_focus(&self, text: &str, focus: &str) -> Result<Vec<BTreeMap<String, Term>>> {
        crate::query::local_select(text)?;
        Ok(self
            .query_bindings(text, &BTreeMap::from([("this".into(), focus.into())]))?
            .1)
    }
    pub fn query_bindings(
        &self,
        text: &str,
        bindings: &BTreeMap<String, String>,
    ) -> Result<BoundQueryResult> {
        use oxigraph::{
            model::Variable,
            sparql::{QueryResults, SparqlEvaluator},
        };
        crate::query::local_query(text)?;
        let bindings = bindings
            .iter()
            .filter(|(name, _)| crate::query::contains_variable(text, name))
            .map(|(name, term)| (name.clone(), term.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut dataset = oxrdf::Dataset::new();
        for quad in oxigraph::io::RdfParser::from_format(oxigraph::io::RdfFormat::NTriples)
            .for_reader(self.nt().as_bytes())
        {
            dataset.insert(&quad?);
        }
        let mut parsed = spargebra::SparqlParser::new().parse_query(text)?;
        let was_ask = matches!(parsed, spargebra::Query::Ask { .. });
        if let spargebra::Query::Ask {
            dataset,
            pattern,
            base_iri,
        } = parsed
        {
            let pattern = match pattern {
                spargebra::algebra::GraphPattern::Project { inner, .. } => *inner,
                pattern => pattern,
            };
            parsed = spargebra::Query::Select {
                dataset,
                base_iri,
                pattern: spargebra::algebra::GraphPattern::Slice {
                    inner: Box::new(spargebra::algebra::GraphPattern::Project {
                        inner: Box::new(pattern),
                        variables: bindings
                            .keys()
                            .map(Variable::new)
                            .collect::<std::result::Result<Vec<_>, _>>()?,
                    }),
                    start: 0,
                    length: Some(1),
                },
            };
        }
        if let spargebra::Query::Select { pattern, .. } = &mut parsed {
            // SEP-0007 accepts substitutions only for projected variables.
            // Expose the SHACL prebindings internally; callers consume only the
            // original this/value columns and never this auxiliary projection.
            fn expose(
                pattern: &mut spargebra::algebra::GraphPattern,
                bindings: &BTreeMap<String, String>,
            ) -> Result<()> {
                use spargebra::algebra::GraphPattern;
                match pattern {
                    GraphPattern::Project { variables, .. } => {
                        for name in bindings.keys() {
                            let variable = Variable::new(name)?;
                            if !variables.contains(&variable) {
                                variables.push(variable);
                            }
                        }
                    }
                    GraphPattern::OrderBy { inner, .. }
                    | GraphPattern::Slice { inner, .. }
                    | GraphPattern::Distinct { inner }
                    | GraphPattern::Reduced { inner } => expose(inner, bindings)?,
                    _ => anyhow::bail!("SHACL SELECT projection is unavailable"),
                }
                Ok(())
            }
            expose(pattern, &bindings)?;
        }
        // SHACL prebindings can appear only in FILTER/BOUND expressions.
        // Optimizing before applying them would fold such variables as unbound.
        let mut query = SparqlEvaluator::new()
            .without_optimizations()
            .for_query(parsed);
        for (name, term) in &bindings {
            query = query.substitute_variable(Variable::new(name)?, term.parse::<Term>()?);
        }

        let result = query.on_queryable_dataset(&dataset).execute()?;
        if let QueryResults::Boolean(answer) = result {
            return Ok((Some(answer), vec![]));
        }
        let QueryResults::Solutions(result) = result else {
            anyhow::bail!("SHACL requires SELECT");
        };
        if was_ask {
            return Ok((
                Some(result.into_iter().next().transpose()?.is_some()),
                vec![],
            ));
        }
        let mut rows = vec![];
        let mut bytes = 0;
        for row in result {
            let row = row?;
            let row = row
                .iter()
                .map(|(variable, term)| (variable.as_str().to_owned(), term.clone()))
                .collect::<BTreeMap<_, _>>();
            bytes += row.values().map(|t| t.to_string().len()).sum::<usize>();
            ensure!(
                rows.len() < 4096 && bytes <= 1024 * 1024,
                "SHACL SELECT result budget exceeded"
            );
            rows.push(row);
        }
        Ok((None, rows))
    }
}
