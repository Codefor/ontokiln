//! RDF graph navigation for standard constraints; no domain vocabulary or pack metadata.
use anyhow::{Result, ensure};
use oxrdf::Term;
use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Resource exhaustion during SHACL evaluation, distinct from invalid RDF or Shapes.
#[derive(Debug)]
pub(crate) struct ShaclBudgetExceeded(pub(crate) &'static str);

impl std::fmt::Display for ShaclBudgetExceeded {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for ShaclBudgetExceeded {}

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
        self.object_terms(s, p).map(str::to_owned).collect()
    }
    pub fn object_terms<'a>(&'a self, s: &'a str, p: &'a str) -> impl Iterator<Item = &'a str> {
        self.triples
            .range([s.to_owned(), p.to_owned(), String::new()]..)
            .take_while(move |t| t[0] == s && t[1] == p)
            .map(|t| t[2].as_str())
    }
    pub fn outgoing<'a>(&'a self, s: &'a str) -> impl Iterator<Item = &'a [String; 3]> {
        self.triples
            .range([s.to_owned(), String::new(), String::new()]..)
            .take_while(move |t| t[0] == s)
    }
    /// Borrow one reverse index for a validation; never copy the graph's terms.
    /// The immutable borrow prevents mutation from invalidating its row references.
    pub fn indexed(&self) -> IndexedGraph<'_> {
        IndexedGraph {
            graph: self,
            pos: OnceCell::new(),
        }
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
        self.prepare_queries()?.select_focus(text, focus)
    }
    pub fn query_bindings(
        &self,
        text: &str,
        bindings: &BTreeMap<String, String>,
    ) -> Result<BoundQueryResult> {
        self.prepare_queries()?.query_bindings(text, bindings)
    }
    pub fn prepare_queries(&self) -> Result<PreparedQueries<'_>> {
        Ok(PreparedQueries {
            source: self,
            dataset: RefCell::new(None),
            queries: RefCell::new(BTreeMap::new()),
        })
    }
    fn query_dataset(&self) -> Result<oxrdf::Dataset> {
        struct TripleReader<I>(I);
        impl<I: Iterator<Item = u8>> std::io::Read for TripleReader<I> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let mut count = 0;
                for (slot, byte) in buffer.iter_mut().zip(self.0.by_ref()) {
                    *slot = byte;
                    count += 1;
                }
                Ok(count)
            }
        }
        let input = TripleReader(
            self.triples
                .iter()
                .flat_map(|t| format!("{} {} {} .\n", t[0], t[1], t[2]).into_bytes()),
        );
        let mut dataset = oxrdf::Dataset::new();
        for quad in oxigraph::io::RdfParser::from_format(oxigraph::io::RdfFormat::NTriples)
            .for_reader(input)
        {
            dataset.insert(&quad?);
        }
        Ok(dataset)
    }
}

/// SPO stays in the canonical set; POS is a sorted vector of borrowed rows.
/// Its memory cost is one pointer per triple, independent of RDF term length.
pub struct IndexedGraph<'a> {
    graph: &'a Graph,
    pos: OnceCell<Vec<&'a [String; 3]>>,
}
impl std::ops::Deref for IndexedGraph<'_> {
    type Target = Graph;

    fn deref(&self) -> &Self::Target {
        self.graph
    }
}
impl<'a> IndexedGraph<'a> {
    fn pos(&self) -> &[&'a [String; 3]] {
        self.pos.get_or_init(|| {
            let mut rows = self.graph.triples.iter().collect::<Vec<_>>();
            rows.sort_unstable_by(|a, b| (&a[1], &a[2], &a[0]).cmp(&(&b[1], &b[2], &b[0])));
            rows
        })
    }
    pub fn subjects(&self, p: &str, o: &str) -> BTreeSet<String> {
        let pos = self.pos();
        let start = pos.partition_point(|t| (t[1].as_str(), t[2].as_str()) < (p, o));
        pos[start..]
            .iter()
            .take_while(|t| t[1] == p && t[2] == o)
            .map(|t| t[0].clone())
            .collect()
    }
    pub fn with_predicate(&self, p: &str) -> impl Iterator<Item = &'a [String; 3]> {
        let pos = self.pos();
        let start = pos.partition_point(|t| t[1].as_str() < p);
        let end = pos.partition_point(|t| t[1].as_str() <= p);
        pos[start..end].iter().copied()
    }
    /// SHACL instances include all transitive subclasses, including cycles.
    pub fn instances_of(&self, class: &str) -> BTreeSet<String> {
        let rdf_type = iri(&format!("{RDF}type"));
        let subclass = iri(&format!("{RDFS}subClassOf"));
        let mut pending = vec![class.to_owned()];
        let mut seen = HashSet::new();
        let mut instances = BTreeSet::new();
        while let Some(class) = pending.pop() {
            if seen.insert(class.clone()) {
                instances.extend(self.subjects(&rdf_type, &class));
                pending.extend(self.subjects(&subclass, &class));
            }
        }
        instances
    }
}

/// SHACL consumes only the immutable in-memory graph.
pub(crate) enum ValidationData<'a> {
    Memory(IndexedGraph<'a>),
}

impl ValidationData<'_> {
    pub(crate) fn prepare_queries(&self) -> Result<PreparedQueries<'_>> {
        let Self::Memory(graph) = self;
        graph.graph.prepare_queries()
    }

    pub(crate) fn visit_objects(
        &self,
        subject: &str,
        predicate: &str,
        mut visit: impl FnMut(&str) -> Result<()>,
    ) -> Result<()> {
        let Self::Memory(graph) = self;
        for object in graph.object_terms(subject, predicate) {
            visit(object)?;
        }
        Ok(())
    }

    pub(crate) fn predicate_subjects(&self, predicate: &str) -> Result<BTreeSet<String>> {
        let Self::Memory(graph) = self;
        Ok(graph.with_predicate(predicate).map(|t| t[0].clone()).collect())
    }

    pub(crate) fn predicate_objects(&self, predicate: &str) -> Result<BTreeSet<String>> {
        let Self::Memory(graph) = self;
        Ok(graph.with_predicate(predicate).map(|t| t[2].clone()).collect())
    }

    pub(crate) fn instances_of(&self, class: &str) -> Result<BTreeSet<String>> {
        let Self::Memory(graph) = self;
        Ok(graph.instances_of(class))
    }

    pub(crate) fn check_error(&self) -> Result<()> {
        Ok(())
    }
}

/// One immutable in-memory SHACL data graph and per-validation query plans.
pub struct PreparedQueries<'a> {
    source: &'a Graph,
    dataset: RefCell<Option<oxrdf::Dataset>>,
    queries: RefCell<BTreeMap<String, spargebra::Query>>,
}
impl PreparedQueries<'_> {
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
        use oxigraph::{model::Variable, sparql::SparqlEvaluator};
        crate::query::local_query(text)?;
        let bindings = bindings
            .iter()
            .filter(|(name, _)| crate::query::contains_variable(text, name))
            .map(|(name, term)| (name.clone(), term.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut plans = self.queries.borrow_mut();
        if !plans.contains_key(text) {
            plans.insert(
                text.to_owned(),
                spargebra::SparqlParser::new().parse_query(text)?,
            );
        }
        // Exposing bindings mutates the projection; never mutate the cached plan.
        let mut parsed = plans[text].clone();
        drop(plans);
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
        // Binding-safe quad joins remain available as a physical execution
        // choice without enabling those logical expression rewrites.
        let mut query = SparqlEvaluator::new()
            .without_optimizations()
            .for_query(parsed);
        for (name, term) in &bindings {
            query = query.substitute_variable(Variable::new(name)?, term.parse::<Term>()?);
        }

        let mut dataset = self.dataset.borrow_mut();
        if dataset.is_none() {
            *dataset = Some(self.source.query_dataset()?);
        }
        Self::collect_query_results(
            query.on_queryable_dataset(dataset.as_ref().expect("SHACL dataset")).execute()?,
            was_ask,
        )
    }

    fn collect_query_results(
        result: oxigraph::sparql::QueryResults<'_>,
        was_ask: bool,
    ) -> Result<BoundQueryResult> {
        use oxigraph::sparql::QueryResults;
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
                ShaclBudgetExceeded("SHACL SELECT result budget exceeded")
            );
            rows.push(row);
        }
        Ok((None, rows))
    }
}
