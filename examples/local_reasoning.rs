//! A resident Library consumer, with optional content polling/full rebuild.
//! No HTTP server, container, remote dependency or per-request CLI process.
use anyhow::{Result, bail, ensure};
use clap::Parser;
use ontokiln::local::{input_digest, rebuild, source_file};
use ontokiln_pack::v4::View;
use std::fs;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    sources: PathBuf,
    #[arg(long)]
    rules: Option<PathBuf>,
    #[arg(long, default_value = "rdfs-supported-v1")]
    profile: String,
    /// Fresh immutable generations are saved here; old Packs remain available.
    #[arg(long)]
    pack_dir: PathBuf,
    #[arg(long)]
    watch: bool,
    #[arg(long, default_value_t = 1000)]
    poll_ms: u64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(args.poll_ms > 0, "poll-ms must be positive");
    fs::create_dir_all(&args.pack_dir)?;
    let session = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let mut generation = 0_u64;
    let path = |n| args.pack_dir.join(format!("{session}-{n}.pack"));
    let fingerprint = || input_digest(&args.sources, args.rules.as_deref());
    let mut observed = format!("{:?}", fingerprint());
    let (mut engine, _) = rebuild(
        &args.sources,
        &args.profile,
        args.rules.as_deref(),
        &path(generation),
    )?;
    eprintln!(
        "ready: {}; commands: query FILE, validate [FILE], rebuild, quit",
        path(generation).display()
    );
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    loop {
        let line = match receiver.recv_timeout(Duration::from_millis(args.poll_ms)) {
            Ok(line) => Some(line?),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let command = line.as_deref().unwrap_or("").trim();
        if command == "quit" {
            break;
        }
        let current = if args.watch || command == "rebuild" {
            format!("{:?}", fingerprint())
        } else {
            observed.clone()
        };
        if command == "rebuild" || current != observed {
            observed = current;
            generation += 1;
            match rebuild(
                &args.sources,
                &args.profile,
                args.rules.as_deref(),
                &path(generation),
            ) {
                Ok((next, stats)) => {
                    engine = next;
                    eprintln!(
                        "replaced engine: {} ({} inferred facts)",
                        path(generation).display(),
                        stats.inferred_facts
                    );
                }
                Err(error) => eprintln!("rebuild failed; retained current engine: {error:#}"),
            }
        }
        let result: Result<()> = (|| {
            if let Some(file) = command.strip_prefix("query ") {
                let query = fs::read_to_string(file.trim())?;
                println!(
                    "{}",
                    String::from_utf8(engine.query(&query, View::Closure)?)?
                );
            } else if command == "validate" {
                println!("{}", engine.validate(None, None, None, 64)?);
            } else if let Some(file) = command.strip_prefix("validate ") {
                let shape = source_file(PathBuf::from(file.trim()), "external-shapes".into())?;
                println!(
                    "{}",
                    engine.validate_shape_sources(&[shape], None, None, 64)?
                );
            } else if !command.is_empty() && command != "rebuild" {
                bail!("expected query FILE, validate [FILE], rebuild or quit");
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("request failed: {error:#}");
        }
    }
    Ok(())
}
