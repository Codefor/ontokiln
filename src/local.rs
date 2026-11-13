//! Filesystem adapters for the same Pack v4 engine used by embedded consumers.
use crate::engine::OntologyEngine;
use anyhow::{Context, Result, bail, ensure};
use ontokiln_compiler::v4::Source;
use ontokiln_compiler::v4_materialize::{Materialization, Rule};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

fn format_for(path: &Path) -> Result<&'static str> {
    Ok(match path.extension().and_then(|v| v.to_str()) {
        Some("ttl") => "turtle",
        Some("nt") => "ntriples",
        Some("trig") => "trig",
        Some("nq") => "nquads",
        _ => bail!("unsupported RDF input extension: {}", path.display()),
    })
}

fn digest(path: &Path) -> Result<String> {
    ensure!(
        fs::symlink_metadata(path)?.file_type().is_file(),
        "input must be a regular non-symlink file: {}",
        path.display()
    );
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        hash.update(&buffer[..size]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn source_file(path: PathBuf, document_id: String) -> Result<Source> {
    Ok(Source {
        format: format_for(&path)?.to_owned(),
        expected_sha256: digest(&path)?,
        document_id,
        path,
        base_iri: None,
    })
}

/// Directory documents are sorted by relative UTF-8 path. Manifest order and
/// declared digests are preserved; relative paths resolve against the manifest.
pub fn load_sources(input: &Path) -> Result<Vec<Source>> {
    let metadata = fs::symlink_metadata(input)?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "source input must not be a symlink"
    );
    let sources = if metadata.is_dir() {
        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(input).follow_links(false) {
            let entry = entry?;
            ensure!(
                !entry.file_type().is_symlink(),
                "source directory contains symlink: {}",
                entry.path().display()
            );
            if !entry.file_type().is_file() || format_for(entry.path()).is_err() {
                continue;
            }
            let id = entry
                .path()
                .strip_prefix(input)?
                .to_str()
                .context("RDF document path must be UTF-8")?
                .replace('\\', "/");
            files.push(source_file(entry.path().to_owned(), id)?);
        }
        files.sort_by(|a, b| a.document_id.cmp(&b.document_id));
        files
    } else {
        ensure!(metadata.is_file(), "source manifest must be a regular file");
        let mut files: Vec<Source> = serde_json::from_slice(&fs::read(input)?)
            .with_context(|| format!("parse source manifest {}", input.display()))?;
        for file in &mut files {
            if file.path.is_relative() {
                file.path = input.parent().unwrap_or(Path::new(".")).join(&file.path);
            }
        }
        files
    };
    ensure!(!sources.is_empty(), "no RDF source documents");
    Ok(sources)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleDocument {
    schema_version: String,
    rules: Vec<Rule>,
}

/// Decode only the existing Rule array or ontology-rule-ir/v1 contract.
pub fn load_rules(path: Option<&Path>) -> Result<Vec<Rule>> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    digest(path)?;
    let bytes = fs::read(path)?;
    let document: serde_json::Value = serde_json::from_slice(&bytes)?;
    if document.is_array() {
        return Ok(serde_json::from_value(document)?);
    }
    let document: RuleDocument = serde_json::from_value(document)?;
    ensure!(
        document.schema_version == "ontology-rule-ir/v1",
        "unsupported rule IR schema"
    );
    Ok(document.rules)
}

/// Full rebuild to a fresh, non-clobbering Pack path. The caller replaces its
/// current engine only after this succeeds, including Pack integrity/reload.
pub fn rebuild(
    sources: &Path,
    profile: &str,
    rules: Option<&Path>,
    pack: &Path,
) -> Result<(OntologyEngine, Materialization)> {
    ensure!(
        !pack.exists(),
        "Pack output already exists: {}",
        pack.display()
    );
    let (engine, stats) =
        OntologyEngine::build(&load_sources(sources)?, profile, &load_rules(rules)?)?;
    engine.save_pack(pack)?;
    drop(engine);
    Ok((OntologyEngine::load_pack(pack)?, stats))
}

/// Content fingerprint for the optional polling example. Actual file digests
/// are included even for manifests, so a stale declared digest triggers a failed
/// rebuild rather than silently serving changed source as a new generation.
pub fn input_digest(sources: &Path, rules: Option<&Path>) -> Result<String> {
    let documents = load_sources(sources)?;
    let mut hash = Sha256::new();
    hash.update(serde_json::to_vec(&documents)?);
    for document in documents {
        hash.update(digest(&document.path)?);
    }
    if let Some(path) = rules {
        hash.update(digest(path)?);
    }
    Ok(format!("{:x}", hash.finalize()))
}
