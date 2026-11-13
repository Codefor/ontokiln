//! Explicit RDF compilation into the immutable in-memory Pack v4.
//!
//! This entry point has no rule materialization. It is deliberately restricted
//! to the no-rules profile until the Full rule semantics are migrated, so an
//! incomplete reasoned snapshot is never silently published as a complete one.
use anyhow::{Context, Result, ensure};
use ontokiln_pack::v4::{DEFAULT_GRAPH, Document, Metadata, PackV4, Statement};
use oxrdf::{BlankNode, GraphName, NamedOrBlankNode, Term};
use oxrdfio::{RdfFormat, RdfParser};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub path: PathBuf,
    pub document_id: String,
    pub format: String,
    pub expected_sha256: String,
    #[serde(default)]
    pub base_iri: Option<String>,
}

struct HashReader<R> {
    reader: R,
    hash: Sha256,
}
impl<R: Read> Read for HashReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let bytes = self.reader.read(buffer)?;
        self.hash.update(&buffer[..bytes]);
        Ok(bytes)
    }
}

pub fn compile_sources(sources: &[Source]) -> Result<PackV4> {
    ensure!(!sources.is_empty(), "no RDF sources supplied");
    let mut seen = BTreeSet::new();
    let mut docs = Vec::new();
    let mut terms = BTreeSet::new();
    let mut parsed = Vec::new();
    for (document, source) in sources.iter().enumerate() {
        ensure!(!source.document_id.is_empty() && seen.insert(source.document_id.clone()),
            "duplicate or empty source document ID");
        ensure!(
            source.expected_sha256.len() == 64
                && source.expected_sha256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "invalid expected source SHA-256"
        );
        let metadata = fs::symlink_metadata(&source.path)
            .with_context(|| format!("source {}", source.path.display()))?;
        ensure!(metadata.file_type().is_file(), "source must be a regular non-symlink file");
        let format = match source.format.as_str() {
            "turtle" => RdfFormat::Turtle,
            "ntriples" => RdfFormat::NTriples,
            "trig" => RdfFormat::TriG,
            "nquads" => RdfFormat::NQuads,
            _ => anyhow::bail!("unsupported RDF source format"),
        };
        let file = File::open(&source.path)?;
        let mut hash = HashReader { reader: file, hash: Sha256::new() };
        let mut parser = RdfParser::from_format(format);
        if let Some(base) = &source.base_iri {
            parser = parser.with_base_iri(base)?;
        } else if matches!(format, RdfFormat::Turtle | RdfFormat::TriG) {
            let base = url::Url::from_file_path(&source.path.canonicalize()?)
                .map_err(|_| anyhow::anyhow!("invalid RDF source file URL"))?;
            parser = parser.with_base_iri(base.as_str())?;
        }
        let mut blanks = HashMap::<BlankNode, usize>::new();
        let mut scope = |blank: BlankNode| {
            let next = blanks.len();
            let local = *blanks.entry(blank).or_insert(next);
            format!("_:f{document}n{local}")
        };
        for quad in parser.for_reader(BufReader::new(&mut hash)) {
            let quad = quad.with_context(|| format!("parse RDF document {}", source.document_id))?;
            let graph = match quad.graph_name {
                GraphName::DefaultGraph => None,
                GraphName::NamedNode(name) => Some(name.to_string()),
                GraphName::BlankNode(blank) => Some(scope(blank)),
            };
            let subject = match quad.subject {
                NamedOrBlankNode::NamedNode(name) => name.to_string(),
                NamedOrBlankNode::BlankNode(blank) => scope(blank),
            };
            let predicate = quad.predicate.to_string();
            let object = match quad.object {
                Term::BlankNode(blank) => scope(blank),
                other => other.to_string(),
            };
            if let Some(graph) = &graph { terms.insert(graph.clone()); }
            for value in [&subject, &predicate, &object] { terms.insert(value.clone()); }
            parsed.push((graph, subject, predicate, object, u32::try_from(document)?));
        }
        let actual = format!("{:x}", hash.hash.finalize());
        ensure!(actual == source.expected_sha256,
            "RDF source SHA-256 mismatch: {}", source.document_id);
        docs.push(Document {
            document_id: source.document_id.clone(), sha256: actual,
            format: source.format.clone(), base_iri: source.base_iri.clone(),
        });
    }
    ensure!(terms.len() < u32::MAX as usize, "Pack v4 term ID space exhausted");
    let vocabulary: Vec<String> = terms.into_iter().collect();
    let dictionary: HashMap<&str, u32> = vocabulary.iter().enumerate()
        .map(|(index, term)| Ok((term.as_str(), u32::try_from(index)?)))
        .collect::<Result<_>>()?;
    let statements = parsed.into_iter().map(|(graph, subject, predicate, object, document)| {
        let lookup = |term: &str| dictionary[term];
        Statement {
            quad: [
                graph.as_deref().map(lookup).unwrap_or(DEFAULT_GRAPH),
                lookup(&subject), lookup(&predicate), lookup(&object),
            ],
            asserted: true,
            sources: vec![document],
        }
    }).collect();
    let mut meta = Metadata::new("no-rules-v1", true);
    meta.documents = docs;
    // These identities describe this specific Pack v4 source/recipe contract.
    // They do not pretend to be the existing Full manifest's digest formula.
    let serialized = serde_json::to_vec(&meta.documents)?;
    meta.source_tree_digest = format!("{:x}", Sha256::digest(&serialized));
    meta.semantic_fingerprint = format!("{:x}", Sha256::digest(
        format!("pack-v4:no-rules-v1:{}", meta.source_tree_digest).as_bytes()
    ));
    PackV4::from_statements(vocabulary, statements, meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ontokiln_pack::v4::View;
    use std::io::Write;

    fn file(dir: &std::path::Path, name: &str, body: &str, format: &str) -> Source {
        let path = dir.join(name);
        let mut stream = File::create(&path).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
        Source {
            path, document_id: name.into(), format: format.into(),
            expected_sha256: format!("{:x}", Sha256::digest(body.as_bytes())),
            base_iri: None,
        }
    }

    #[test]
    fn v4_compiles_turtle_trig_and_nquads_with_scoped_blank_nodes() {
        let dir = tempfile::tempdir().unwrap();
        let sources = vec![
            file(dir.path(), "a.ttl", "_:shared <urn:p> <urn:o> .", "turtle"),
            file(dir.path(), "b.trig", "<urn:g> { _:shared <urn:p> <urn:o> . }", "trig"),
            file(dir.path(), "c.nq", "<urn:s> <urn:p> <urn:o> <urn:g> .", "nquads"),
        ];
        let pack = compile_sources(&sources).unwrap();
        assert_eq!(pack.quad_count(), 3);
        assert!(pack.metadata().fixpoint_reached);
        assert_eq!(pack.metadata().profile_id, "no-rules-v1");
        assert_eq!(pack.matching(None, None, None, Some(DEFAULT_GRAPH), View::Closure).count(), 1);
        assert_eq!(pack.union_triples(None, None, None, View::Closure).count(), 3);
        assert_eq!(pack.metadata().documents.len(), 3);
        let target = dir.path().join("compiled.pack");
        pack.save(&target).unwrap();
        let loaded = PackV4::load(&target).unwrap();
        assert_eq!(loaded.quad_count(), pack.quad_count());
        assert_eq!(loaded.metadata(), pack.metadata());
    }

    #[test]
    fn v4_rejects_digest_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = file(dir.path(), "input.nt", "<urn:s> <urn:p> <urn:o> .", "ntriples");
        source.expected_sha256 = "0".repeat(64);
        assert!(compile_sources(&[source]).is_err());
    }
}
