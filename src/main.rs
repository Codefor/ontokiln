//! OntoKiln: one Pack v4 compiler and immutable in-memory reasoner.
//!
//! The public CLI only builds, queries, validates and inspects Pack v4.
//! Applications embed OntologyEngine directly and own its lifecycle.
use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use ontokiln::engine::OntologyEngine;
use ontokiln_compiler::v4::Source;
use ontokiln_compiler::v4_materialize::Rule;
use ontokiln_pack::v4::{PackV4, View};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::{fs, io};

#[derive(Parser)]
#[command(
    version,
    about = "Compile and query an immutable in-memory RDF Pack v4"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build a complete materialized Pack v4 and persist it.
    Build {
        /// Directory containing .ttl/.nt/.trig/.nq, or an explicit JSON Source array.
        #[arg(long)]
        sources: PathBuf,
        #[arg(long)]
        pack: PathBuf,
        #[arg(long, default_value = "rdfs-supported-v1")]
        profile: String,
        /// Optional ontology-rule-ir/v1 JSON file or JSON Rule array.
        #[arg(long)]
        rules: Option<PathBuf>,
    },
    /// Execute local SELECT / ASK against an immutable Pack v4.
    Query {
        #[arg(long)]
        pack: PathBuf,
        #[arg(long)]
        query: PathBuf,
        #[arg(long, value_enum, default_value_t = ViewMode::Closure)]
        view: ViewMode,
    },
    /// Validate a Pack v4 using its own Shapes or one external Shapes document.
    Validate {
        #[arg(long)]
        pack: PathBuf,
        #[arg(long)]
        shapes: Option<PathBuf>,
    },
    /// Verify a Pack v4 and print its semantic metadata.
    Inspect {
        #[arg(long)]
        pack: PathBuf,
    },

}

#[derive(Clone, Copy, ValueEnum)]
enum ViewMode {
    Asserted,
    Inferred,
    Closure,
}
impl From<ViewMode> for View {
    fn from(view: ViewMode) -> Self {
        match view {
            ViewMode::Asserted => Self::Asserted,
            ViewMode::Inferred => Self::Inferred,
            ViewMode::Closure => Self::Closure,
        }
    }
}

fn format_for(path: &Path) -> Result<&'static str> {
    Ok(match path.extension().and_then(|v| v.to_str()) {
        Some("ttl") => "turtle",
        Some("nt") => "ntriples",
        Some("trig") => "trig",
        Some("nq") => "nquads",
        _ => bail!("unsupported RDF input extension: {}", path.display()),
    })
}

fn source(path: PathBuf, document_id: String) -> Result<Source> {
    let file_type = fs::symlink_metadata(&path)?.file_type();
    ensure!(file_type.is_file(), "RDF source must be a regular file");
    let bytes = fs::read(&path)?;
    Ok(Source {
        format: format_for(&path)?.to_owned(),
        expected_sha256: format!("{:x}", Sha256::digest(bytes)),
        document_id,
        path,
        base_iri: None,
    })
}

fn sources(input: &Path) -> Result<Vec<Source>> {
    if input.is_dir() {
        let mut files = Vec::new();
        for file in walkdir::WalkDir::new(input).follow_links(false) {
            let file = file?;
            if !file.file_type().is_file() || format_for(file.path()).is_err() {
                continue;
            }
            let path = file.path().to_path_buf();
            let relative = path.strip_prefix(input)?.to_string_lossy().into_owned();
            files.push(source(path, relative)?);
        }
        files.sort_by(|a, b| a.document_id.cmp(&b.document_id));
        ensure!(!files.is_empty(), "no RDF source documents");
        Ok(files)
    } else {
        let sources: Vec<Source> = serde_json::from_slice(
            &fs::read(input).with_context(|| format!("read {}", input.display()))?,
        )?;
        ensure!(!sources.is_empty(), "source manifest is empty");
        Ok(sources)
    }
}

fn rules(path: Option<&Path>) -> Result<Vec<Rule>> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let document: Value = serde_json::from_slice(&fs::read(path)?)?;
    let entries = if let Some(rows) = document.as_array() {
        rows.clone()
    } else {
        ensure!(
            document["schema_version"] == "ontology-rule-ir/v1",
            "unsupported rule IR schema"
        );
        document["rules"]
            .as_array()
            .context("ontology-rule-ir/v1 requires rules array")?
            .clone()
    };
    Ok(serde_json::from_value(Value::Array(entries))?)
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Build {
            sources: input,
            pack,
            profile,
            rules: rules_file,
        } => {
            ensure!(
                !pack.exists(),
                "Pack output already exists: {}",
                pack.display()
            );
            let sources = sources(&input)?;
            let rules = rules(rules_file.as_deref())?;
            let (engine, stats) = OntologyEngine::build(&sources, &profile, &rules)?;
            engine.save_pack(&pack)?;
            println!(
                "{}",
                json!({
                    "pack": pack, "format":"ontokiln-pack/v4",
                    "profile":profile, "fixpoint_reached":engine.pack().metadata().fixpoint_reached,
                    "explicit_facts":stats.explicit_facts,
                    "inferred_facts":stats.inferred_facts,
                    "quads":engine.pack().quad_count()
                })
            );
        }
        Command::Query { pack, query, view } => {
            let engine = OntologyEngine::load_pack(&pack)?;
            let text = fs::read_to_string(query)?;
            io::stdout().write_all(&engine.query(&text, view.into())?)?;
            io::stdout().write_all(b"\n")?;
        }
        Command::Validate { pack, shapes } => {
            let engine = OntologyEngine::load_pack(&pack)?;
            let report = if let Some(path) = shapes {
                let id = path
                    .file_name()
                    .context("Shapes source has no filename")?
                    .to_string_lossy()
                    .into_owned();
                let shape = source(path, id)?;
                engine.validate_shape_sources(&[shape], None, None, 64)?
            } else {
                engine.validate(None, None, None, 64)?
            };
            println!("{}", serde_json::to_string(&report)?);
        }
        Command::Inspect { pack } => {
            let pack = PackV4::load(&pack)?;
            println!(
                "{}",
                json!({
                    "format":"ontokiln-pack/v4", "version":4,
                    "quads":pack.quad_count(), "terms":pack.term_count(),
                    "metadata":pack.metadata()
                })
            );
        }

    }
    Ok(())
}
