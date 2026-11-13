//! Request-scoped SHACL navigation over a verified, read-only Full snapshot.
//!
//! The graph is the same distinct union of all stored facts used by Full
//! validation. Rudof retains its existing RDF term types and Core validators;
//! Core neighborhoods and local SPARQL now read SQLite without a full RDF copy.
use crate::rdf_graph::{Graph, ShaclBudgetExceeded};
use anyhow::{Result, anyhow, bail, ensure};
use iri_s::IriS;
use oxrdf::{BlankNode, NamedOrBlankNode, Term, Triple};
use prefixmap::{PrefixMap, PrefixMapError};
use rudof_rdf::rdf_core::{
    Matcher, NeighsRDF, RDFError, Rdf,
    term::{
        IriOrBlankNode, Object,
        literal::{ConcreteLiteral, Lang},
    },
};
use rudof_rdf::rdf_impl::InMemoryGraph;
use rusqlite::{Connection, OptionalExtension, params_from_iter};
use spareval::{InternalQuad, QueryableDataset};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::{fmt, io};

const FROM_FACTS: &str = " FROM facts AS f JOIN terms AS s ON s.id=f.s \
    JOIN terms AS p ON p.id=f.p JOIN terms AS o ON o.id=f.o";
const CACHE_ENTRIES: usize = 4096;
const CACHE_BYTES: usize = 4 * 1024 * 1024;
const NEIGHBORHOOD_OBJECTS: usize = 4096;
const NEIGHBORHOOD_BYTES: usize = 1024 * 1024;
const QUERY_PAGE_ROWS: usize = 256;

/// Full's strict compiler writes one canonical N-Triples serialization per RDF
/// term into the UNIQUE terms.n dictionary. Consequently a stored term and the
/// same query constant always resolve to the same ID; expression-created terms
/// absent from that dictionary retain their RDF identity as External values.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum QueryTerm {
    Stored(i64),
    External(Term),
}

struct QueryTermCache {
    entries: HashMap<i64, Term>,
    bytes: usize,
}

impl QueryTermCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
        }
    }

    fn get(&self, id: i64) -> Option<Term> {
        self.entries.get(&id).cloned()
    }

    fn insert(&mut self, id: i64, term: Term, serialized_bytes: usize) {
        let bytes = serialized_bytes.saturating_mul(2).saturating_add(128);
        if bytes > CACHE_BYTES {
            return;
        }
        if self.entries.len() >= CACHE_ENTRIES || self.bytes.saturating_add(bytes) > CACHE_BYTES {
            self.entries.clear();
            self.bytes = 0;
        }
        if self.entries.insert(id, term).is_none() {
            self.bytes += bytes;
        }
    }
}

/// Composed validation reads a property's values immediately before Rudof asks
/// for the same neighborhood. Retain only that most recent complete read.
struct Neighborhood {
    subject: String,
    predicate: String,
    objects: Vec<String>,
    bytes: usize,
}

impl Neighborhood {
    fn new(subject: &str, predicate: &str) -> Option<Self> {
        let bytes = subject
            .len()
            .saturating_add(predicate.len())
            .saturating_add(std::mem::size_of::<Self>());
        (bytes <= NEIGHBORHOOD_BYTES).then(|| Self {
            subject: subject.to_owned(),
            predicate: predicate.to_owned(),
            objects: Vec::new(),
            bytes,
        })
    }

    fn push(&mut self, object: &str) -> bool {
        // Allow for spare Vec capacity as well as each String's allocation.
        let bytes = self
            .bytes
            .saturating_add(object.len())
            .saturating_add(2 * std::mem::size_of::<String>());
        if self.objects.len() >= NEIGHBORHOOD_OBJECTS || bytes > NEIGHBORHOOD_BYTES {
            return false;
        }
        self.objects.push(object.to_owned());
        self.bytes = bytes;
        true
    }
}

/// These caches are bounded and die with the worker. The byte allowance is a
/// conservative term-size estimate, separate from the worker's measured RSS.
struct TermCache<T> {
    entries: HashMap<String, T>,
    bytes: usize,
}

impl<T: Clone> TermCache<T> {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
        }
    }

    fn get(&self, term: &str) -> Option<T> {
        self.entries.get(term).cloned()
    }

    fn insert(&mut self, term: &str, value: T) {
        let bytes = term.len().saturating_mul(2).saturating_add(128);
        if bytes > CACHE_BYTES {
            return;
        }
        if self.entries.len() >= CACHE_ENTRIES || self.bytes.saturating_add(bytes) > CACHE_BYTES {
            self.entries.clear();
            self.bytes = 0;
        }
        if self.entries.insert(term.to_owned(), value).is_none() {
            self.bytes += bytes;
        }
    }
}

struct ReadFailure {
    message: String,
    budget: bool,
}

impl ReadFailure {
    fn error(&self) -> anyhow::Error {
        if self.budget {
            anyhow::Error::new(ShaclBudgetExceeded("SHACL SQLite resource budget exceeded"))
                .context(self.message.clone())
        } else {
            anyhow!(self.message.clone())
        }
    }
}

fn sqlite_error(error: rusqlite::Error) -> anyhow::Error {
    let budget = matches!(error, rusqlite::Error::SqliteFailure(ref cause, _)
        if matches!(cause.code, rusqlite::ErrorCode::DiskFull | rusqlite::ErrorCode::OperationInterrupted));
    ReadFailure {
        message: format!("SHACL SQLite read failed: {error}"),
        budget,
    }
    .error()
}

pub(crate) struct SqliteGraph {
    db: Connection,
    display: InMemoryGraph,
    ids: RefCell<TermCache<Option<i64>>>,
    terms: RefCell<TermCache<Term>>,
    query_terms: RefCell<QueryTermCache>,
    neighborhood: RefCell<Option<Neighborhood>>,
    /// Pinned Rudof validators and SPARQL expressions swallow some store errors.
    /// Never let those failures become empty matches or successful reports.
    failure: RefCell<Option<ReadFailure>>,
}

impl fmt::Debug for SqliteGraph {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteGraph")
            .field("cached_ids", &self.ids.borrow().entries.len())
            .field("cached_terms", &self.terms.borrow().entries.len())
            .field(
                "cached_query_terms",
                &self.query_terms.borrow().entries.len(),
            )
            .field("cached_neighborhood", &self.neighborhood.borrow().is_some())
            .field("read_failed", &self.failure.borrow().is_some())
            .finish()
    }
}

impl SqliteGraph {
    pub(crate) fn new(db: Connection) -> Result<Self> {
        db.execute_batch(
            "PRAGMA query_only=ON; PRAGMA mmap_size=0; \
             PRAGMA cache_size=-4096; PRAGMA temp_store=FILE;",
        )
        .map_err(sqlite_error)?;
        db.set_prepared_statement_cache_capacity(32);
        // Preserve the old loader's schema admission even when no selected
        // shape asks for data. Preparing LIMIT 0 validates names without a scan.
        drop(
            db.prepare_cached(&format!("SELECT DISTINCT s.n,p.n,o.n{FROM_FACTS} LIMIT 0"))
                .map_err(sqlite_error)?,
        );
        Ok(Self {
            db,
            display: InMemoryGraph::new(),
            ids: RefCell::new(TermCache::new()),
            terms: RefCell::new(TermCache::new()),
            query_terms: RefCell::new(QueryTermCache::new()),
            neighborhood: RefCell::new(None),
            failure: RefCell::new(None),
        })
    }

    /// Check after native validation or query consumption and before publishing.
    /// The first failure remains present for the entire lifetime of this store.
    pub(crate) fn check_error(&self) -> Result<()> {
        match self.failure.borrow().as_ref() {
            Some(failure) => Err(failure.error()),
            None => Ok(()),
        }
    }

    fn remember<T>(&self, result: Result<T>) -> Result<T> {
        if let Err(error) = &result {
            let mut failure = self.failure.borrow_mut();
            if failure.is_none() {
                *failure = Some(ReadFailure {
                    message: error.to_string(),
                    budget: error.is::<ShaclBudgetExceeded>(),
                });
            }
        }
        result
    }

    fn sql<T>(&self, result: rusqlite::Result<T>) -> Result<T> {
        self.remember(result.map_err(sqlite_error))
    }

    fn term_id(&self, term: &str) -> Result<Option<i64>> {
        self.check_error()?;
        if let Some(id) = self.ids.borrow().get(term) {
            return Ok(id);
        }
        let mut statement = self.sql(self.db.prepare_cached("SELECT id FROM terms WHERE n=?"))?;
        let id = self.sql(statement.query_row([term], |row| row.get(0)).optional())?;
        self.ids.borrow_mut().insert(term, id);
        Ok(id)
    }

    /// An absent fixed term makes the pattern empty; it does not request a scan.
    fn pattern_filter(&self, terms: [Option<&str>; 3]) -> Result<Option<(String, Vec<i64>)>> {
        self.check_error()?;
        let mut filters = Vec::new();
        let mut values = Vec::new();
        for (column, term) in ["s", "p", "o"].into_iter().zip(terms) {
            if let Some(term) = term {
                let Some(id) = self.term_id(term)? else {
                    return Ok(None);
                };
                filters.push(format!("f.{column}=?"));
                values.push(id);
            }
        }
        let filter = if filters.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", filters.join(" AND "))
        };
        Ok(Some((filter, values)))
    }

    fn visit_column(
        &self,
        column: &str,
        terms: [Option<&str>; 3],
        mut visit: impl FnMut(&str) -> Result<()>,
    ) -> Result<()> {
        let Some((filter, values)) = self.pattern_filter(terms)? else {
            return Ok(());
        };
        // Column names are internal constants. All RDF values use bindings.
        let query = format!("SELECT DISTINCT {column}.n{FROM_FACTS}{filter}");
        let mut statement = self.sql(self.db.prepare_cached(&query))?;
        let mut rows = self.sql(statement.query(params_from_iter(values)))?;
        while let Some(row) = self.sql(rows.next())? {
            let term: String = self.sql(row.get(0))?;
            // A caller's budget rejection is already returned directly. It is
            // not a database failure and must preserve its original error type.
            visit(&term)?;
        }
        Ok(())
    }

    pub(crate) fn visit_objects(
        &self,
        subject: &str,
        predicate: &str,
        mut visit: impl FnMut(&str) -> Result<()>,
    ) -> Result<()> {
        // Release the old entry before accumulating a replacement. A failed or
        // oversized visit must not retain a previous or partial neighborhood.
        drop(self.neighborhood.borrow_mut().take());
        let mut candidate = Neighborhood::new(subject, predicate);
        self.visit_column("o", [Some(subject), Some(predicate), None], |object| {
            visit(object)?;
            if let Some(neighborhood) = &mut candidate
                && !neighborhood.push(object)
            {
                candidate = None;
            }
            Ok(())
        })?;
        self.check_error()?;
        *self.neighborhood.borrow_mut() = candidate;
        Ok(())
    }

    fn native_objects(&self, subject: &str, predicate: &str) -> Result<HashSet<Term>> {
        // Cached data never hides an earlier failure swallowed by a validator.
        self.check_error()?;
        if let Some(neighborhood) = self.neighborhood.borrow().as_ref()
            && neighborhood.subject == subject
            && neighborhood.predicate == predicate
        {
            return neighborhood
                .objects
                .iter()
                .map(|object| self.parsed_term(object))
                .collect();
        }
        let mut objects = HashSet::new();
        self.visit_column("o", [Some(subject), Some(predicate), None], |object| {
            objects.insert(self.parsed_term(object)?);
            Ok(())
        })?;
        self.check_error()?;
        Ok(objects)
    }

    pub(crate) fn subjects(&self, predicate: &str, object: &str) -> Result<BTreeSet<String>> {
        let mut terms = BTreeSet::new();
        self.visit_column("s", [None, Some(predicate), Some(object)], |term| {
            terms.insert(term.to_owned());
            Ok(())
        })?;
        Ok(terms)
    }

    pub(crate) fn predicate_subjects(&self, predicate: &str) -> Result<BTreeSet<String>> {
        let mut terms = BTreeSet::new();
        self.visit_column("s", [None, Some(predicate), None], |term| {
            terms.insert(term.to_owned());
            Ok(())
        })?;
        Ok(terms)
    }

    pub(crate) fn predicate_objects(&self, predicate: &str) -> Result<BTreeSet<String>> {
        let mut terms = BTreeSet::new();
        self.visit_column("o", [None, Some(predicate), None], |term| {
            terms.insert(term.to_owned());
            Ok(())
        })?;
        Ok(terms)
    }

    fn visit_rows(
        &self,
        terms: [Option<&str>; 3],
        mut visit: impl FnMut([String; 3]) -> Result<()>,
    ) -> Result<()> {
        let Some((filter, values)) = self.pattern_filter(terms)? else {
            return Ok(());
        };
        let query = format!("SELECT DISTINCT s.n,p.n,o.n{FROM_FACTS}{filter}");
        let mut statement = self.sql(self.db.prepare_cached(&query))?;
        let mut rows = self.sql(statement.query(params_from_iter(values)))?;
        while let Some(row) = self.sql(rows.next())? {
            visit([
                self.sql(row.get(0))?,
                self.sql(row.get(1))?,
                self.sql(row.get(2))?,
            ])?;
        }
        Ok(())
    }

    pub(crate) fn load_graph(&self) -> Result<Graph> {
        let mut graph = Graph::default();
        self.visit_rows([None, None, None], |row| {
            graph.triples.insert(row);
            Ok(())
        })?;
        Ok(graph)
    }

    fn parsed_term(&self, text: &str) -> Result<Term> {
        self.check_error()?;
        if let Some(term) = self.terms.borrow().get(text) {
            return Ok(term);
        }
        let parsed = (|| {
            if let Some(label) = text.strip_prefix("_:") {
                // Match native_graph: standalone Term parsing can truncate
                // consecutive dots, while this constructor permits colons.
                ensure!(
                    !label.contains(':'),
                    "SHACL input failed: invalid blank node label"
                );
                BlankNode::new(label)
                    .map(Term::BlankNode)
                    .map_err(|error| anyhow!("SHACL input failed: {error}"))
            } else {
                text.parse::<Term>()
                    .map_err(|error| anyhow!("SHACL input failed: {error}"))
            }
        })();
        let parsed = self.remember(parsed)?;
        self.terms.borrow_mut().insert(text, parsed.clone());
        Ok(parsed)
    }

    fn parsed_triple(&self, row: [String; 3]) -> Result<Triple> {
        let parsed = (|| {
            let subject = match self.parsed_term(&row[0])? {
                Term::NamedNode(node) => NamedOrBlankNode::NamedNode(node),
                Term::BlankNode(node) => NamedOrBlankNode::BlankNode(node),
                _ => bail!("SHACL input failed: subject must be an IRI or blank node"),
            };
            let predicate = match self.parsed_term(&row[1])? {
                Term::NamedNode(node) => node,
                _ => bail!("SHACL input failed: predicate must be an IRI"),
            };
            Ok(Triple::new(subject, predicate, self.parsed_term(&row[2])?))
        })();
        self.remember(parsed)
    }

    fn matching_triples(&self, terms: [Option<&str>; 3]) -> Result<Vec<Triple>> {
        // Eagerly finish one matching neighborhood so SQL and decode failures
        // cannot disappear inside NeighsRDF's infallible iterator items.
        let mut triples = Vec::new();
        self.visit_rows(terms, |row| {
            triples.push(self.parsed_triple(row)?);
            Ok(())
        })?;
        Ok(triples)
    }

    fn query_term(&self, id: i64) -> Result<Term> {
        self.check_error()?;
        if let Some(term) = self.query_terms.borrow().get(id) {
            return Ok(term);
        }
        let mut statement = self.sql(self.db.prepare_cached("SELECT n FROM terms WHERE id=?"))?;
        let text: String = self.sql(statement.query_row([id], |row| row.get(0)))?;
        // Match Graph::query_dataset's strict N-Triples parser rather than the
        // standalone Term parser used by Native Core. The former also keeps
        // complete blank labels and rejects trailing tokens or lax syntax.
        let parsed = (|| -> Result<Term> {
            let input = format!("<urn:ontokiln:query-term> <urn:ontokiln:query-value> {text} .\n");
            let mut quads = oxigraph::io::RdfParser::from_format(oxigraph::io::RdfFormat::NTriples)
                .for_reader(input.as_bytes());
            let quad = quads
                .next()
                .transpose()
                .map_err(|error| anyhow!("SHACL query input failed: {error}"))?
                .ok_or_else(|| anyhow!("SHACL query input failed: missing RDF term"))?;
            ensure!(
                quads.next().is_none(),
                "SHACL query input failed: trailing RDF term content"
            );
            Ok(quad.object)
        })();
        let term = self.remember(parsed)?;
        self.query_terms
            .borrow_mut()
            .insert(id, term.clone(), text.len());
        Ok(term)
    }

    fn query_order(fixed: &[Option<i64>; 3]) -> [usize; 3] {
        let mut best = [0, 1, 2];
        let mut best_prefix = 0;
        for order in [[0, 1, 2], [1, 2, 0], [2, 0, 1]] {
            let prefix = order
                .iter()
                .take_while(|&&column| fixed[column].is_some())
                .count();
            if prefix > best_prefix {
                best = order;
                best_prefix = prefix;
            }
        }
        best
    }

    fn query_page(
        &self,
        fixed: [Option<i64>; 3],
        order: [usize; 3],
        after: Option<[i64; 3]>,
    ) -> Result<Vec<[i64; 3]>> {
        self.check_error()?;
        let columns = ["f.s", "f.p", "f.o"];
        let mut filters = Vec::new();
        let mut bindings = Vec::new();
        for (column, value) in columns.into_iter().zip(fixed) {
            if let Some(value) = value {
                filters.push(format!("{column}=?"));
                bindings.push(value);
            }
        }
        let ordered = order.map(|column| columns[column]).join(",");
        if let Some(after) = after {
            filters.push(format!("({ordered})>(?,?,?)"));
            bindings.extend(order.map(|column| after[column]));
        }
        let filter = if filters.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", filters.join(" AND "))
        };
        let query = format!(
            "SELECT DISTINCT f.s,f.p,f.o{FROM_FACTS}{filter} ORDER BY {ordered} LIMIT {QUERY_PAGE_ROWS}"
        );
        let mut statement = self.sql(self.db.prepare_cached(&query))?;
        let mut rows = self.sql(statement.query(params_from_iter(bindings)))?;
        let mut result = Vec::new();
        while let Some(row) = self.sql(rows.next())? {
            result.push([
                self.sql(row.get(0))?,
                self.sql(row.get(1))?,
                self.sql(row.get(2))?,
            ]);
        }
        Ok(result)
    }
}

impl<'a> QueryableDataset<'a> for &'a SqliteGraph {
    type InternalTerm = QueryTerm;
    type Error = io::Error;

    fn internal_quads_for_pattern(
        &self,
        subject: Option<&QueryTerm>,
        predicate: Option<&QueryTerm>,
        object: Option<&QueryTerm>,
        graph_name: Option<Option<&QueryTerm>>,
    ) -> impl Iterator<Item = io::Result<InternalQuad<QueryTerm>>> + use<'a> {
        let graph = *self;
        let mut fixed = [None; 3];
        // None means all named graphs in QueryableDataset, not the default
        // graph. SHACL exposes only the distinct physical-graph union below.
        let mut last_page = graph_name != Some(None);
        for (index, term) in [subject, predicate, object].into_iter().enumerate() {
            match term {
                Some(QueryTerm::Stored(id)) => fixed[index] = Some(*id),
                Some(QueryTerm::External(_)) => last_page = true,
                None => {}
            }
        }
        let order = SqliteGraph::query_order(&fixed);
        let mut after = None;
        let mut page = Vec::<[i64; 3]>::new().into_iter();
        let mut finished = false;
        std::iter::from_fn(move || {
            loop {
                if finished {
                    return None;
                }
                if let Err(error) = graph.check_error() {
                    finished = true;
                    return Some(Err(io::Error::other(error)));
                }
                if let Some([subject, predicate, object]) = page.next() {
                    return Some(Ok(InternalQuad {
                        subject: QueryTerm::Stored(subject),
                        predicate: QueryTerm::Stored(predicate),
                        object: QueryTerm::Stored(object),
                        graph_name: None,
                    }));
                }
                if last_page {
                    finished = true;
                    return None;
                }
                match graph.query_page(fixed, order, after) {
                    Ok(rows) => {
                        last_page = rows.len() < QUERY_PAGE_ROWS;
                        after = rows.last().copied();
                        page = rows.into_iter();
                    }
                    Err(error) => {
                        finished = true;
                        return Some(Err(io::Error::other(error)));
                    }
                }
            }
        })
    }

    fn internal_named_graphs(&self) -> impl Iterator<Item = io::Result<QueryTerm>> + use<'a> {
        self.check_error()
            .err()
            .map(|error| Err(io::Error::other(error)))
            .into_iter()
    }

    fn contains_internal_graph_name(&self, _: &QueryTerm) -> io::Result<bool> {
        self.check_error().map_err(io::Error::other)?;
        Ok(false)
    }

    fn internalize_term(&self, term: Term) -> io::Result<QueryTerm> {
        self.term_id(&term.to_string())
            .map(|id| id.map_or_else(|| QueryTerm::External(term), QueryTerm::Stored))
            .map_err(io::Error::other)
    }

    fn externalize_term(&self, term: QueryTerm) -> io::Result<Term> {
        self.check_error().map_err(io::Error::other)?;
        match term {
            QueryTerm::Stored(id) => self.query_term(id).map_err(io::Error::other),
            QueryTerm::External(term) => Ok(term),
        }
    }
}

impl Rdf for SqliteGraph {
    type Subject = <InMemoryGraph as Rdf>::Subject;
    type IRI = <InMemoryGraph as Rdf>::IRI;
    type Term = <InMemoryGraph as Rdf>::Term;
    type BNode = <InMemoryGraph as Rdf>::BNode;
    type Literal = <InMemoryGraph as Rdf>::Literal;
    type Triple = <InMemoryGraph as Rdf>::Triple;
    type Err = anyhow::Error;

    fn qualify_iri(&self, iri: &Self::IRI) -> String {
        self.display.qualify_iri(iri)
    }

    fn qualify_subject(&self, subject: &Self::Subject) -> String {
        self.display.qualify_subject(subject)
    }

    fn qualify_term(&self, term: &Self::Term) -> String {
        self.display.qualify_term(term)
    }

    fn prefixmap(&self) -> Option<PrefixMap> {
        <InMemoryGraph as Rdf>::prefixmap(&self.display)
    }

    fn resolve_prefix_local(&self, prefix: &str, local: &str) -> Result<IriS, PrefixMapError> {
        self.display.resolve_prefix_local(prefix, local)
    }

    fn term_as_literal(term: &Self::Term) -> Result<Self::Literal, RDFError> {
        <InMemoryGraph as Rdf>::term_as_literal(term)
    }

    fn term_as_sliteral(term: &Self::Term) -> Result<ConcreteLiteral, RDFError> {
        <InMemoryGraph as Rdf>::term_as_sliteral(term)
    }

    fn term_as_subject(term: &Self::Term) -> Result<Self::Subject, RDFError> {
        <InMemoryGraph as Rdf>::term_as_subject(term)
    }

    fn subject_as_term(subject: &Self::Subject) -> Self::Term {
        <InMemoryGraph as Rdf>::subject_as_term(subject)
    }

    fn triple_as_term(triple: &Self::Triple) -> Self::Term {
        <InMemoryGraph as Rdf>::triple_as_term(triple)
    }

    fn iris_as_term(iri: &IriS) -> Self::Term {
        <InMemoryGraph as Rdf>::iris_as_term(iri)
    }

    fn term_as_iri(term: &Self::Term) -> Result<Self::IRI, RDFError> {
        <InMemoryGraph as Rdf>::term_as_iri(term)
    }

    fn iri_or_bnode_as_term(value: &IriOrBlankNode) -> Self::Term {
        <InMemoryGraph as Rdf>::iri_or_bnode_as_term(value)
    }

    fn term_as_bnode(term: &Self::Term) -> Result<Self::BNode, RDFError> {
        <InMemoryGraph as Rdf>::term_as_bnode(term)
    }

    fn term_as_iris(term: &Self::Term) -> Result<IriS, RDFError> {
        <InMemoryGraph as Rdf>::term_as_iris(term)
    }

    fn term_as_object(term: &Self::Term) -> Result<Object, RDFError> {
        <InMemoryGraph as Rdf>::term_as_object(term)
    }

    fn object_as_term(object: &Object) -> Self::Term {
        <InMemoryGraph as Rdf>::object_as_term(object)
    }

    fn subject_as_node(subject: &Self::Subject) -> Result<Object, RDFError> {
        <InMemoryGraph as Rdf>::subject_as_node(subject)
    }

    fn term_as_lang(term: &Self::Term) -> Result<Lang, RDFError> {
        <InMemoryGraph as Rdf>::term_as_lang(term)
    }
}

impl NeighsRDF for SqliteGraph {
    fn objects_for(
        &self,
        subject: &Self::Term,
        predicate: &Self::IRI,
    ) -> Result<HashSet<Self::Term>, RDFError> {
        // Keep the existing RDF conversion failure for literal subjects. It is
        // not a store read failure and must not poison subsequent validation.
        let subject_node = Self::term_as_subject(subject)?;
        self.native_objects(&subject_node.to_string(), &predicate.to_string())
            .map_err(|error| RDFError::ErrorObjectsFor {
                subject: subject.to_string(),
                predicate: predicate.to_string(),
                error: error.to_string(),
            })
    }

    fn triples(&self) -> Result<impl Iterator<Item = Self::Triple>, Self::Err> {
        Ok(self.matching_triples([None, None, None])?.into_iter())
    }

    fn triples_matching<S, P, O>(
        &self,
        subject: &S,
        predicate: &P,
        object: &O,
    ) -> Result<impl Iterator<Item = Self::Triple> + '_, Self::Err>
    where
        S: Matcher<Self::Subject>,
        P: Matcher<Self::IRI>,
        O: Matcher<Self::Term>,
    {
        let subject = subject.value().map(ToString::to_string);
        let predicate = predicate.value().map(ToString::to_string);
        let object = object.value().map(ToString::to_string);
        Ok(self
            .matching_triples([subject.as_deref(), predicate.as_deref(), object.as_deref()])?
            .into_iter())
    }
}

#[cfg(test)]
#[path = "../tests/shacl/sqlite_store.rs"]
mod tests;
