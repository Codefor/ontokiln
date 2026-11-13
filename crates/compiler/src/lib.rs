//! Compile a directory snapshot. This crate is not needed to open a pack.
pub mod rules;
pub mod v4;
pub mod v4_materialize;
use anyhow::{Context, Result, ensure};
use ontokiln_pack::{Id, Pack, Source};
use oxrdf::{BlankNode, NamedOrBlankNode, Term};
use oxrdfio::{RdfFormat, RdfParser};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    module: String,
    revision: String,
    source_file: String,
    ttl_sha256: String,
    source_sha256: String,
}
struct HashReader<R>(R, Sha256);
impl<R: Read> Read for HashReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.0.read(buf)?;
        self.1.update(&buf[..n]);
        Ok(n)
    }
}
use std::path::{Path, PathBuf};
use url::Url;
use walkdir::WalkDir;

pub struct Compilation {
    pub pack: Pack,
    pub files: usize,
    pub parsed: usize,
}

pub fn compile_directory(root: &Path) -> Result<Compilation> {
    let root = root
        .canonicalize()
        .with_context(|| format!("open TTL directory {}", root.display()))?;
    ensure!(
        root.is_dir(),
        "expected a TTL directory: {}",
        root.display()
    );
    let mut paths = Vec::<PathBuf>::new();
    for entry in WalkDir::new(&root).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_file()
            && entry.path().extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("ttl") || ext.eq_ignore_ascii_case("nt")
            })
        {
            paths.push(entry.into_path());
        }
    }
    paths.sort();
    ensure!(
        !paths.is_empty(),
        "no .ttl or .nt files in {}",
        root.display()
    );

    let manifest = root.join("sources.json");
    let mut declarations: std::collections::BTreeMap<String, Declaration> = if manifest.exists() {
        serde_json::from_reader(File::open(manifest)?)?
    } else {
        Default::default()
    };
    let mut documents = Vec::new();
    let mut ids = HashMap::new();
    let mut triples = Vec::new();
    for (file_id, path) in paths.iter().enumerate() {
        let sha256 = parse_file(path, file_id, &mut ids, &mut triples)
            .with_context(|| format!("parse {}", path.display()))?;
        let relative = path.strip_prefix(&root)?;
        let file = relative
            .to_str()
            .context("source path is not UTF-8")?
            .replace(std::path::MAIN_SEPARATOR, "/");
        let mut doc = Source {
            module: relative
                .parent()
                .unwrap_or(Path::new(""))
                .to_string_lossy()
                .into_owned(),
            file: file.clone(),
            sha256,
            revision: None,
            source_file: None,
            source_sha256: None,
        };
        if let Some(d) = declarations.remove(&file) {
            ensure!(
                !d.revision.is_empty() && !d.module.is_empty() && !d.source_file.is_empty(),
                "empty declaration for {file}"
            );
            ensure!(
                d.ttl_sha256 == doc.sha256,
                "source digest mismatch for {file}"
            );
            {
                let expected = d.source_sha256;
                let path = root
                    .join(&d.source_file)
                    .canonicalize()
                    .context("open declared source file")?;
                ensure!(path.starts_with(&root), "source outside input directory");
                let mut reader = HashReader(File::open(path)?, Sha256::new());
                std::io::copy(&mut reader, &mut std::io::sink())?;
                ensure!(
                    format!("{:x}", reader.1.finalize()) == expected,
                    "wrong source version for {}",
                    d.source_file
                );
                doc.source_sha256 = Some(expected);
            }
            doc.module = d.module;
            doc.source_file = Some(d.source_file);
            doc.revision = Some(d.revision);
        }
        documents.push(doc);
    }
    ensure!(
        declarations.is_empty(),
        "sources.json contains unknown input files"
    );
    let parsed = triples.len();
    let mut entries: Vec<_> = ids.into_iter().collect();
    entries.par_sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let mut remap = vec![0; entries.len()];
    let mut terms = Vec::with_capacity(entries.len());
    for (term, old) in entries {
        remap[old as usize] = terms.len() as Id;
        terms.push(term);
    }
    triples.par_iter_mut().for_each(|t| {
        for id in &mut t[..3] {
            *id = remap[*id as usize];
        }
    });
    triples.par_sort_unstable();
    triples.dedup(); // includes the source file ID
    let mut facts = Vec::with_capacity(triples.len());
    let mut primary = Vec::with_capacity(triples.len());
    let mut additional = Vec::new();
    for [s, p, o, doc] in triples {
        let fact = [s, p, o];
        if facts.last() == Some(&fact) {
            additional.push([Id::try_from(facts.len() - 1)?, doc]);
        } else {
            facts.push(fact);
            primary.push(doc);
        }
    }
    Ok(Compilation {
        pack: Pack::with_sources(terms, facts, documents, primary, additional)?,
        files: paths.len(),
        parsed,
    })
}

fn parse_file(
    path: &Path,
    file_id: usize,
    ids: &mut HashMap<String, Id>,
    triples: &mut Vec<[Id; 4]>,
) -> Result<String> {
    let mut hash = HashReader(File::open(path)?, Sha256::new());
    let file = BufReader::new(&mut hash);
    let document = Id::try_from(file_id)?;
    let base = Url::from_file_path(path).map_err(|_| anyhow::anyhow!("invalid file URL"))?;
    let format = if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("nt"))
    {
        RdfFormat::NTriples
    } else {
        RdfFormat::Turtle
    };
    let parser = RdfParser::from_format(format)
        .with_base_iri(base.as_str())?
        .for_reader(file);
    let mut blanks = HashMap::<BlankNode, usize>::new();
    let mut scoped = |node| {
        let next = blanks.len();
        let local = blanks.entry(node).or_insert(next);
        format!("_:f{file_id}n{local}")
    };
    for triple in parser {
        let t = triple?;
        let s = match t.subject {
            NamedOrBlankNode::BlankNode(b) => scoped(b),
            node => node.to_string(),
        };
        let o = match t.object {
            Term::BlankNode(b) => scoped(b),
            term => term.to_string(),
        };
        triples.push([
            intern(ids, s)?,
            intern(ids, t.predicate.to_string())?,
            intern(ids, o)?,
            document,
        ]);
    }
    Ok(format!("{:x}", hash.1.finalize()))
}

fn intern(ids: &mut HashMap<String, Id>, term: String) -> Result<Id> {
    let next = Id::try_from(ids.len()).context("term ID space exhausted")?;
    Ok(*ids.entry(term).or_insert(next))
}
