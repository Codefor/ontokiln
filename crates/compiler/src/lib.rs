//! Compile a directory snapshot. This crate is not needed to open a pack.
pub mod rules;
use anyhow::{Context, Result, ensure};
use ontokiln_pack::{Id, Pack, Triple};
use oxrdf::{BlankNode, NamedOrBlankNode, Term};
use oxrdfio::{RdfFormat, RdfParser};
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
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

    let mut ids = HashMap::new();
    let mut triples = Vec::new();
    for (file_id, path) in paths.iter().enumerate() {
        parse_file(path, file_id, &mut ids, &mut triples)
            .with_context(|| format!("parse {}", path.display()))?;
    }
    let parsed = triples.len();
    let mut entries: Vec<_> = ids.into_iter().collect();
    entries.par_sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let mut remap = vec![0; entries.len()];
    let mut terms = Vec::with_capacity(entries.len());
    for (term, old) in entries {
        remap[old as usize] = terms.len() as Id;
        terms.push(term);
    }
    triples
        .par_iter_mut()
        .for_each(|t| *t = t.map(|id| remap[id as usize]));
    triples.par_sort_unstable();
    triples.dedup();
    Ok(Compilation {
        pack: Pack::from_sorted(terms, triples)?,
        files: paths.len(),
        parsed,
    })
}

fn parse_file(
    path: &Path,
    file_id: usize,
    ids: &mut HashMap<String, Id>,
    triples: &mut Vec<Triple>,
) -> Result<()> {
    let file = BufReader::new(File::open(path)?);
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
        ]);
    }
    Ok(())
}

fn intern(ids: &mut HashMap<String, Id>, term: String) -> Result<Id> {
    let next = Id::try_from(ids.len()).context("term ID space exhausted")?;
    Ok(*ids.entry(term).or_insert(next))
}
