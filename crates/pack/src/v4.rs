//! Pack v4: a compact, immutable, fully materialized RDF dataset.
//!
//! This module intentionally does not reinterpret source documents as RDF named
//! graphs. Source provenance and the RDF graph column are independent.
//! It is an in-memory representation with a deterministic, checksummed portable
//! serialization. The v3 pack stays available during semantic migration.
use anyhow::{Context, Result, ensure};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use tempfile::NamedTempFile;

/// Reserved graph ID representing the physical default graph. All other graph
/// IDs refer to terms in the same dictionary as subjects and objects.
pub const DEFAULT_GRAPH: u32 = u32::MAX;
pub const FORMAT_VERSION: u32 = 4;
const MAGIC: &[u8; 8] = b"ONTOKILN";
const HEADER_BYTES: usize = 8 + 4 + 8 + 32;

/// The four columns are (graph, subject, predicate, object).
/// IDs are *local* to a Pack. They must never be compared between builds.
pub type Quad = [u32; 4];

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub document_id: String,
    pub sha256: String,
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_iri: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    /// The logical dataset remains stable as storage implementations change.
    pub semantics: String,
    pub profile_id: String,
    /// Inferred and closure views may only be offered when this is true.
    pub fixpoint_reached: bool,
    pub source_tree_digest: String,
    pub semantic_fingerprint: String,
    pub documents: Vec<Document>,
}

impl Metadata {
    pub fn new(profile_id: impl Into<String>, fixpoint_reached: bool) -> Self {
        Self {
            semantics: "rdf-dataset-union/v1".into(),
            profile_id: profile_id.into(),
            fixpoint_reached,
            source_tree_digest: String::new(),
            semantic_fingerprint: String::new(),
            documents: vec![],
        }
    }
}

/// Input record. Multiple records with the same physical quad are combined:
/// asserted wins, while every original asserting document is retained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Statement {
    pub quad: Quad,
    pub asserted: bool,
    pub sources: Vec<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum View {
    Asserted,
    Inferred,
    Closure,
}

/// All arrays use integer IDs; neither source RDF text nor a second decoded
/// graph is retained in the runtime representation.
pub struct PackV4 {
    terms: Vec<String>,
    quads: Vec<Quad>,
    asserted: Vec<u8>, // one bit per quad
    spo: Vec<u32>,
    pos: Vec<u32>,
    osp: Vec<u32>,
    source_offsets: Vec<u64>,
    source_ids: Vec<u32>,
    metadata: Metadata,
}

impl PackV4 {
    pub fn from_statements(
        terms: Vec<String>,
        mut statements: Vec<Statement>,
        metadata: Metadata,
    ) -> Result<Self> {
        ensure!(
            terms.len() < u32::MAX as usize && statements.len() < u32::MAX as usize,
            "Pack v4 term/row ID space exhausted"
        );
        ensure!(!metadata.profile_id.is_empty(), "empty rule profile");
        ensure!(
            metadata.semantics == "rdf-dataset-union/v1",
            "unsupported Pack v4 dataset semantics"
        );
        for pair in terms.windows(2) {
            ensure!(pair[0] < pair[1], "Pack v4 terms must be sorted and unique");
        }
        let parsed = terms
            .iter()
            .map(|term| term.parse::<oxrdf::Term>())
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("invalid Pack v4 RDF term")?;
        let mut documents = HashSet::new();
        for document in &metadata.documents {
            ensure!(
                !document.document_id.is_empty() && documents.insert(document.document_id.as_str()),
                "missing or duplicate source document ID"
            );
        }
        for statement in &statements {
            let [g, s, p, o] = statement.quad;
            for id in [s, p, o]
                .into_iter()
                .chain((g != DEFAULT_GRAPH).then_some(g))
            {
                ensure!(
                    (id as usize) < parsed.len(),
                    "Pack v4 term ID out of bounds"
                );
            }
            ensure!(
                !matches!(parsed[s as usize], oxrdf::Term::Literal(_)),
                "RDF subject cannot be a literal"
            );
            ensure!(
                matches!(parsed[p as usize], oxrdf::Term::NamedNode(_)),
                "RDF predicate must be an IRI"
            );
            if g != DEFAULT_GRAPH {
                ensure!(
                    !matches!(parsed[g as usize], oxrdf::Term::Literal(_)),
                    "RDF graph cannot be a literal"
                );
            }
            ensure!(
                statement.asserted || statement.sources.is_empty(),
                "inferred facts cannot invent asserting documents"
            );
            for source in &statement.sources {
                ensure!(
                    (*source as usize) < metadata.documents.len(),
                    "Pack v4 source document ID out of bounds"
                );
            }
        }
        statements.par_sort_unstable_by_key(|record| record.quad);
        let mut merged: Vec<Statement> = Vec::with_capacity(statements.len());
        for statement in statements {
            if let Some(previous) = merged.last_mut()
                && previous.quad == statement.quad
            {
                previous.asserted |= statement.asserted;
                previous.sources.extend(statement.sources);
            } else {
                merged.push(statement);
            }
        }
        if merged.iter().any(|record| !record.asserted) {
            ensure!(
                metadata.fixpoint_reached,
                "Pack v4 inferred facts require a completed rule fixpoint"
            );
        }
        let count = merged.len();
        let mut quads = Vec::with_capacity(count);
        let mut asserted = vec![0u8; count.div_ceil(8)];
        let mut source_offsets = Vec::with_capacity(count + 1);
        let mut source_ids = Vec::new();
        source_offsets.push(0);
        for (i, mut record) in merged.into_iter().enumerate() {
            quads.push(record.quad);
            if record.asserted {
                asserted[i / 8] |= 1 << (i % 8);
            }
            record.sources.sort_unstable();
            record.sources.dedup();
            source_ids.extend(record.sources);
            source_offsets.push(source_ids.len() as u64);
        }
        let mut spo = sequence(count)?;
        let mut pos = spo.clone();
        let mut osp = spo.clone();
        spo.par_sort_unstable_by_key(|&i| key(quads[i as usize], 0));
        pos.par_sort_unstable_by_key(|&i| key(quads[i as usize], 1));
        osp.par_sort_unstable_by_key(|&i| key(quads[i as usize], 2));
        let pack = Self {
            terms,
            quads,
            asserted,
            spo,
            pos,
            osp,
            source_offsets,
            source_ids,
            metadata,
        };
        pack.validate()?;
        Ok(pack)
    }

    /// Borrow the canonical dictionary; IDs are local to this Pack.
    pub fn terms(&self) -> &[String] {
        &self.terms
    }

    /// Iterate the physical quads with asserted/source provenance.
    pub fn statements(&self) -> impl Iterator<Item = Statement> + '_ {
        self.quads
            .iter()
            .copied()
            .enumerate()
            .map(|(row, quad)| Statement {
                quad,
                asserted: self.is_asserted(row),
                sources: self
                    .sources(row)
                    .expect("validated provenance offsets")
                    .to_vec(),
            })
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    pub fn term_count(&self) -> usize {
        self.terms.len()
    }
    pub fn quad_count(&self) -> usize {
        self.quads.len()
    }
    pub fn term(&self, id: u32) -> Option<&str> {
        self.terms.get(id as usize).map(String::as_str)
    }
    pub fn term_id(&self, term: &str) -> Option<u32> {
        self.terms
            .binary_search_by(|value| value.as_str().cmp(term))
            .ok()
            .map(|index| index as u32)
    }
    pub fn quad(&self, row: usize) -> Option<Quad> {
        self.quads.get(row).copied()
    }
    pub fn is_asserted(&self, row: usize) -> bool {
        self.asserted
            .get(row / 8)
            .is_some_and(|v| v & (1 << (row % 8)) != 0)
    }
    pub fn sources(&self, row: usize) -> Option<&[u32]> {
        let start = *self.source_offsets.get(row)? as usize;
        let end = *self.source_offsets.get(row + 1)? as usize;
        self.source_ids.get(start..end)
    }

    /// Indexed physical quad access. The caller chooses whether to request a
    /// physical named/default graph or the union of all graphs.
    pub fn matching(
        &self,
        subject: Option<u32>,
        predicate: Option<u32>,
        object: Option<u32>,
        graph: Option<u32>,
        view: View,
    ) -> Matches<'_> {
        // Pick the index with the longest bound prefix. Choosing POS merely
        // because P is bound forces an O(N_p) scan for every S+P lookup in
        // joins: use SPO for S+P, POS for P+O and OSP for O+S.
        let (permutation, order) = if subject.is_some() && predicate.is_some()
            || subject.is_some() && object.is_none()
            || subject.is_none() && predicate.is_none() && object.is_none()
        {
            (&self.spo, 0)
        } else if predicate.is_some() && object.is_some()
            || predicate.is_some() && subject.is_none()
        {
            (&self.pos, 1)
        } else {
            (&self.osp, 2)
        };
        let bound = match order {
            1 => [predicate, object, subject],
            2 => [object, subject, predicate],
            _ => [subject, predicate, object],
        };
        let prefix = bound.iter().take_while(|value| value.is_some()).count();
        let low = [
            bound[0].unwrap_or(0),
            bound[1].unwrap_or(0),
            bound[2].unwrap_or(0),
            0,
        ];
        let high = [
            bound[0].unwrap_or(u32::MAX),
            bound[1].unwrap_or(u32::MAX),
            bound[2].unwrap_or(u32::MAX),
            u32::MAX,
        ];
        let (start, end) = if prefix == 0 {
            (0, permutation.len())
        } else {
            (
                permutation.partition_point(|&row| key(self.quads[row as usize], order) < low),
                permutation.partition_point(|&row| key(self.quads[row as usize], order) <= high),
            )
        };
        Matches {
            pack: self,
            permutation,
            next: start,
            end,
            subject,
            predicate,
            object,
            graph,
            view,
        }
    }

    /// The SPARQL default graph and SHACL data graph are the distinct SPO union
    /// of selected physical quads; named graph identities are not invented from
    /// source documents.
    pub fn union_triples(
        &self,
        s: Option<u32>,
        p: Option<u32>,
        o: Option<u32>,
        view: View,
    ) -> impl Iterator<Item = [u32; 3]> + '_ {
        let mut previous = None;
        self.matching(s, p, o, None, view)
            .filter_map(move |(_, quad)| {
                let triple = [quad[1], quad[2], quad[3]];
                if previous == Some(triple) {
                    None
                } else {
                    previous = Some(triple);
                    Some(triple)
                }
            })
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let directory = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(directory)?;
        let metadata = serde_json::to_vec(&self.metadata)?;
        let mut strings = Vec::new();
        let mut term_offsets = Vec::with_capacity(self.terms.len() + 1);
        term_offsets.push(0u64);
        for term in &self.terms {
            strings.extend_from_slice(term.as_bytes());
            term_offsets.push(strings.len() as u64);
        }
        let mut body = Vec::new();
        for n in [
            self.terms.len() as u64,
            self.quads.len() as u64,
            self.source_ids.len() as u64,
            strings.len() as u64,
            metadata.len() as u64,
        ] {
            put_u64(&mut body, n);
        }
        for offset in term_offsets {
            put_u64(&mut body, offset);
        }
        body.extend_from_slice(&strings);
        for quad in &self.quads {
            for id in quad {
                put_u32(&mut body, *id);
            }
        }
        body.extend_from_slice(&self.asserted);
        for order in [&self.spo, &self.pos, &self.osp] {
            for id in order {
                put_u32(&mut body, *id);
            }
        }
        for offset in &self.source_offsets {
            put_u64(&mut body, *offset);
        }
        for id in &self.source_ids {
            put_u32(&mut body, *id);
        }
        body.extend_from_slice(&metadata);
        let mut file = NamedTempFile::new_in(directory)?;
        file.write_all(MAGIC)?;
        file.write_all(&FORMAT_VERSION.to_le_bytes())?;
        file.write_all(&(body.len() as u64).to_le_bytes())?;
        file.write_all(&Sha256::digest(&body))?;
        file.write_all(&body)?;
        file.as_file_mut().sync_all()?;
        file.persist_noclobber(path)
            .context("publish immutable Pack v4")?;
        File::open(directory)?.sync_all()?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        ensure!(bytes.len() >= HEADER_BYTES, "truncated Pack v4 header");
        ensure!(&bytes[..8] == MAGIC, "invalid Pack v4 header");
        let version = u32::from_le_bytes(bytes[8..12].try_into()?);
        ensure!(
            version == FORMAT_VERSION,
            "unsupported Pack format v{version}"
        );
        let size = u64::from_le_bytes(bytes[12..20].try_into()?);
        ensure!(
            size == (bytes.len() - HEADER_BYTES) as u64,
            "Pack v4 body size mismatch"
        );
        let body = &bytes[HEADER_BYTES..];
        ensure!(
            Sha256::digest(body).as_slice() == &bytes[20..HEADER_BYTES],
            "Pack v4 checksum mismatch"
        );
        let mut input = Input { body, offset: 0 };
        let n_terms = usize::try_from(input.u64()?)?;
        let n_quads = usize::try_from(input.u64()?)?;
        let n_sources = usize::try_from(input.u64()?)?;
        let string_bytes = usize::try_from(input.u64()?)?;
        let metadata_bytes = usize::try_from(input.u64()?)?;
        ensure!(
            n_terms < u32::MAX as usize && n_quads < u32::MAX as usize,
            "Pack v4 ID space exhausted"
        );
        let mut term_offsets =
            Vec::with_capacity(n_terms.checked_add(1).context("term count overflow")?);
        for _ in 0..=n_terms {
            term_offsets.push(input.u64()?);
        }
        let strings = input.take(string_bytes)?;
        ensure!(
            term_offsets.first() == Some(&0) && term_offsets.last() == Some(&(string_bytes as u64)),
            "invalid Pack v4 term offsets"
        );
        let mut terms = Vec::with_capacity(n_terms);
        for offsets in term_offsets.windows(2) {
            let (a, b) = (usize::try_from(offsets[0])?, usize::try_from(offsets[1])?);
            ensure!(a < b && b <= strings.len(), "invalid Pack v4 term boundary");
            terms.push(std::str::from_utf8(&strings[a..b])?.to_owned());
        }
        let mut quads = Vec::with_capacity(n_quads);
        for _ in 0..n_quads {
            quads.push([input.u32()?, input.u32()?, input.u32()?, input.u32()?]);
        }
        let asserted = input.take(n_quads.div_ceil(8))?.to_vec();
        let spo = input.u32s(n_quads)?;
        let pos = input.u32s(n_quads)?;
        let osp = input.u32s(n_quads)?;
        let mut source_offsets = Vec::with_capacity(n_quads + 1);
        for _ in 0..=n_quads {
            source_offsets.push(input.u64()?);
        }
        let source_ids = input.u32s(n_sources)?;
        let metadata: Metadata = serde_json::from_slice(input.take(metadata_bytes)?)?;
        ensure!(input.offset == body.len(), "Pack v4 trailing bytes");
        let pack = Self {
            terms,
            quads,
            asserted,
            spo,
            pos,
            osp,
            source_offsets,
            source_ids,
            metadata,
        };
        pack.validate()?;
        Ok(pack)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.metadata.semantics == "rdf-dataset-union/v1"
                && !self.metadata.profile_id.is_empty(),
            "invalid Pack v4 semantic metadata"
        );
        ensure!(
            self.quads.len() < u32::MAX as usize && self.terms.len() < u32::MAX as usize,
            "Pack v4 ID space exhausted"
        );
        for pair in self.terms.windows(2) {
            ensure!(pair[0] < pair[1], "Pack v4 term order is invalid");
        }
        let terms = self
            .terms
            .iter()
            .map(|t| t.parse::<oxrdf::Term>())
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("invalid Pack v4 RDF dictionary")?;
        for (row, quad) in self.quads.iter().enumerate() {
            if row > 0 {
                ensure!(
                    self.quads[row - 1] < *quad,
                    "unsorted or duplicate Pack v4 quads"
                );
            }
            let [g, s, p, o] = *quad;
            for id in [s, p, o]
                .into_iter()
                .chain((g != DEFAULT_GRAPH).then_some(g))
            {
                ensure!((id as usize) < terms.len(), "invalid Pack v4 quad term ID");
            }
            ensure!(
                !matches!(terms[s as usize], oxrdf::Term::Literal(_))
                    && matches!(terms[p as usize], oxrdf::Term::NamedNode(_))
                    && (g == DEFAULT_GRAPH
                        || !matches!(terms[g as usize], oxrdf::Term::Literal(_))),
                "invalid Pack v4 RDF quad"
            );
            if !self.is_asserted(row) {
                ensure!(
                    self.metadata.fixpoint_reached,
                    "incomplete inferred Pack v4"
                );
                ensure!(
                    self.sources(row).is_some_and(|s| s.is_empty()),
                    "inferred quad has asserted sources"
                );
            }
        }
        ensure!(
            self.asserted.len() == self.quads.len().div_ceil(8),
            "invalid Pack v4 assertion bitmap"
        );
        if !self.quads.len().is_multiple_of(8) && !self.asserted.is_empty() {
            let used = self.quads.len() % 8;
            ensure!(
                self.asserted.last().is_some_and(|v| v >> used == 0),
                "nonzero Pack v4 unused assertion bits"
            );
        }
        let mut seen = HashSet::new();
        for document in &self.metadata.documents {
            ensure!(
                !document.document_id.is_empty() && seen.insert(document.document_id.as_str()),
                "duplicate or empty Pack v4 document ID"
            );
        }
        ensure!(
            self.source_offsets.len() == self.quads.len() + 1
                && self.source_offsets.first() == Some(&0)
                && self.source_offsets.last() == Some(&(self.source_ids.len() as u64)),
            "invalid Pack v4 provenance offsets"
        );
        for (row, offsets) in self.source_offsets.windows(2).enumerate() {
            let (a, b) = (usize::try_from(offsets[0])?, usize::try_from(offsets[1])?);
            ensure!(a <= b && b <= self.source_ids.len(), "invalid source range");
            let source_ids = &self.source_ids[a..b];
            ensure!(
                self.is_asserted(row) || source_ids.is_empty(),
                "inferred quad has asserting sources"
            );
            for (i, source) in source_ids.iter().enumerate() {
                ensure!(
                    (*source as usize) < self.metadata.documents.len(),
                    "Pack v4 provenance document out of bounds"
                );
                if i > 0 {
                    ensure!(source_ids[i - 1] < *source, "duplicate provenance source");
                }
            }
        }
        for (order, permutation) in [(&self.spo, 0), (&self.pos, 1), (&self.osp, 2)] {
            ensure!(
                order.len() == self.quads.len(),
                "invalid Pack v4 index length"
            );
            let mut visited = vec![false; self.quads.len()];
            let mut last = None;
            for &id in order {
                ensure!(
                    (id as usize) < self.quads.len(),
                    "Pack v4 index ID out of bounds"
                );
                let index = id as usize;
                ensure!(!visited[index], "duplicate Pack v4 index row");
                visited[index] = true;
                let current = key(self.quads[index], permutation);
                if let Some(previous) = last {
                    ensure!(previous < current, "Pack v4 index is out of order");
                }
                last = Some(current);
            }
        }
        Ok(())
    }
}

pub struct Matches<'a> {
    pack: &'a PackV4,
    permutation: &'a [u32],
    next: usize,
    end: usize,
    subject: Option<u32>,
    predicate: Option<u32>,
    object: Option<u32>,
    graph: Option<u32>,
    view: View,
}
impl Iterator for Matches<'_> {
    type Item = (usize, Quad);
    fn next(&mut self) -> Option<Self::Item> {
        while self.next < self.end {
            let i = self.permutation[self.next] as usize;
            self.next += 1;
            let q = self.pack.quads[i];
            if self.subject.is_some_and(|s| q[1] != s)
                || self.predicate.is_some_and(|p| q[2] != p)
                || self.object.is_some_and(|o| q[3] != o)
                || self.graph.is_some_and(|g| q[0] != g)
                || (self.view == View::Asserted && !self.pack.is_asserted(i))
                || (self.view == View::Inferred && self.pack.is_asserted(i))
            {
                continue;
            }
            return Some((i, q));
        }
        None
    }
}
fn key(quad: Quad, order: usize) -> [u32; 4] {
    match order {
        1 => [quad[2], quad[3], quad[1], quad[0]],
        2 => [quad[3], quad[1], quad[2], quad[0]],
        _ => [quad[1], quad[2], quad[3], quad[0]],
    }
}
fn sequence(len: usize) -> Result<Vec<u32>> {
    (0..len)
        .map(|n| u32::try_from(n).map_err(Into::into))
        .collect()
}
fn put_u32(buffer: &mut Vec<u8>, n: u32) {
    buffer.extend_from_slice(&n.to_le_bytes());
}
fn put_u64(buffer: &mut Vec<u8>, n: u64) {
    buffer.extend_from_slice(&n.to_le_bytes());
}
struct Input<'a> {
    body: &'a [u8],
    offset: usize,
}
impl<'a> Input<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .context("Pack v4 section size overflow")?;
        let chunk = self
            .body
            .get(self.offset..end)
            .context("truncated Pack v4 section")?;
        self.offset = end;
        Ok(chunk)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
    fn u32s(&mut self, count: usize) -> Result<Vec<u32>> {
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(self.u32()?);
        }
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> PackV4 {
        let terms = vec![
            "<urn:g>".to_owned(),
            "<urn:o>".to_owned(),
            "<urn:p>".to_owned(),
            "<urn:s>".to_owned(),
        ];
        let metadata = Metadata {
            documents: vec![Document {
                document_id: "a.ttl".into(),
                sha256: "sha".into(),
                format: "turtle".into(),
                base_iri: None,
            }],
            ..Metadata::new("rdfs-supported-v1", true)
        };
        let facts = vec![
            Statement {
                quad: [DEFAULT_GRAPH, 3, 2, 1],
                asserted: true,
                sources: vec![0],
            },
            Statement {
                quad: [0, 3, 2, 1],
                asserted: false,
                sources: vec![],
            },
            Statement {
                quad: [0, 3, 2, 1],
                asserted: true,
                sources: vec![0, 0],
            },
        ];
        PackV4::from_statements(terms, facts, metadata).unwrap()
    }

    #[test]
    fn v4_roundtrip_preserves_graphs_assertions_provenance_and_union() {
        let pack = fixture();
        assert_eq!(pack.quad_count(), 2);
        assert_eq!(pack.sources(0), Some(&[0][..]));
        assert_eq!(pack.sources(1), Some(&[0][..]));
        assert_eq!(
            pack.union_triples(None, None, None, View::Closure).count(),
            1
        );
        assert_eq!(
            pack.union_triples(None, None, None, View::Asserted).count(),
            1
        );
        assert_eq!(
            pack.union_triples(None, None, None, View::Inferred).count(),
            0
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("graph.pack");
        pack.save(&path).unwrap();
        let restored = PackV4::load(&path).unwrap();
        assert_eq!(restored.metadata(), pack.metadata());
        assert_eq!(restored.quad_count(), 2);
        assert_eq!(
            restored
                .matching(None, None, None, Some(DEFAULT_GRAPH), View::Closure)
                .count(),
            1
        );
        assert_eq!(
            restored
                .matching(None, None, None, Some(0), View::Closure)
                .count(),
            1
        );
        assert_eq!(
            restored
                .union_triples(None, None, None, View::Closure)
                .collect::<Vec<_>>(),
            pack.union_triples(None, None, None, View::Closure)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn v4_detects_corrupt_snapshot() {
        let pack = fixture();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.pack");
        pack.save(&path).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&path, bytes).unwrap();
        assert!(PackV4::load(&path).is_err());
    }

    #[test]
    fn v4_rejects_unfinished_inference() {
        let terms = vec!["<urn:o>".into(), "<urn:p>".into(), "<urn:s>".into()];
        assert!(
            PackV4::from_statements(
                terms,
                vec![Statement {
                    quad: [DEFAULT_GRAPH, 2, 1, 0],
                    asserted: false,
                    sources: vec![]
                }],
                Metadata::new("datalog-v1", false)
            )
            .is_err()
        );
    }

    #[test]
    fn index_selector_prefers_longest_bound_prefix_for_joins() {
        let pack = fixture();
        let selected = |subject, predicate, object| {
            pack.matching(subject, predicate, object, None, View::Closure)
                .permutation
                .as_ptr()
        };
        assert_eq!(selected(Some(3), Some(2), None), pack.spo.as_ptr());
        assert_eq!(selected(Some(3), None, Some(1)), pack.osp.as_ptr());
        assert_eq!(selected(None, Some(2), Some(1)), pack.pos.as_ptr());
        assert_eq!(selected(Some(3), None, None), pack.spo.as_ptr());
        assert_eq!(selected(None, Some(2), None), pack.pos.as_ptr());
        assert_eq!(selected(None, None, Some(1)), pack.osp.as_ptr());
        assert_eq!(selected(None, None, None), pack.spo.as_ptr());
    }

    #[test]
    fn v4_indexed_lookup_and_union_preserve_independent_graphs() {
        let pack = fixture();
        assert_eq!(
            pack.matching(Some(3), Some(2), Some(1), None, View::Closure)
                .count(),
            2
        );
        assert_eq!(
            pack.matching(Some(3), Some(2), Some(1), None, View::Asserted)
                .count(),
            2
        );
        assert_eq!(
            pack.matching(Some(3), Some(2), Some(1), None, View::Inferred)
                .count(),
            0
        );
        assert_eq!(
            pack.union_triples(Some(3), Some(2), Some(1), View::Closure)
                .count(),
            1
        );
    }
}
