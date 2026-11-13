//! Reproducible resident-engine benchmark. Not a production service or API.
//! The Python harness owns the independent expected answers and frozen corpus.
use anyhow::{Context, Result, ensure};
use clap::Parser;
use ontokiln::engine::OntologyEngine;
use ontokiln::rdf_graph::Graph;
use ontokiln_compiler::v4::{Source, compile_sources};
use ontokiln_compiler::v4_materialize::{Rule, materialize};
use ontokiln_pack::v4::View;
use oxigraph::io::{RdfFormat, RdfParser};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser)]
#[command(about = "Measure Pack v4 build, load and resident query/SHACL independently")]
struct Args {
    #[arg(long)]
    sources: PathBuf,
    #[arg(long)]
    rules: PathBuf,
    #[arg(long)]
    queries: PathBuf,
    #[arg(long)]
    shapes: PathBuf,
    #[arg(long)]
    pack: PathBuf,
    #[arg(long)]
    output: PathBuf,
    #[arg(long, default_value = "rdfs-supported-v1")]
    profile: String,
    #[arg(long, default_value_t = 1)]
    warmups: usize,
    #[arg(long, default_value_t = 3)]
    samples: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryCase {
    name: String,
    query: String,
    view: String,
}

fn view(value: &str) -> Result<View> {
    Ok(match value {
        "asserted" => View::Asserted,
        "inferred" => View::Inferred,
        "closure" => View::Closure,
        _ => anyhow::bail!("invalid benchmark view {value}"),
    })
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_unstable_by(f64::total_cmp);
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

fn sample<F>(warmups: usize, samples: usize, mut run: F) -> Result<(Value, Value)>
where
    F: FnMut() -> Result<Value>,
{
    for _ in 0..warmups {
        run()?;
    }
    let mut times_ms = Vec::with_capacity(samples);
    let mut reference = None;
    for _ in 0..samples {
        let started = Instant::now();
        let result = run()?;
        let elapsed = started.elapsed().as_secs_f64() * 1_000.0;
        if let Some(previous) = &reference {
            ensure!(previous == &result, "nondeterministic benchmark response");
        } else {
            reference = Some(result);
        }
        times_ms.push(elapsed);
    }
    Ok((
        reference.context("sample result missing")?,
        json!({"samples_ms":times_ms, "median_ms":median(&times_ms)}),
    ))
}

fn shapes_graph(path: &Path) -> Result<Graph> {
    let bytes = fs::read(path)?;
    let mut graph = Graph::default();
    for quad in RdfParser::from_format(RdfFormat::Turtle)
        .rename_blank_nodes()
        .for_reader(bytes.as_slice())
    {
        let q = quad?;
        graph.triples.insert([
            q.subject.to_string(),
            q.predicate.to_string(),
            q.object.to_string(),
        ]);
    }
    Ok(graph)
}

fn memory_stats() -> Value {
    // Only Linux offers this kernel accounting; non-Linux callers get null.
    let Ok(info) = fs::read_to_string("/proc/self/status") else {
        return Value::Null;
    };
    let field = |name: &str| -> Option<u64> {
        info.lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|suffix| suffix.split_whitespace().next())
            .and_then(|n| n.parse::<u64>().ok())
    };
    json!({"rss_kib":field("VmRSS:"),"peak_rss_kib":field("VmHWM:")})
}

fn main() -> Result<()> {
    ensure!(
        env!("ONTOKILN_BUILD_PROFILE") == "release"
            && env!("ONTOKILN_OPT_LEVEL") != "0"
            && !cfg!(debug_assertions),
        "resident performance benchmark requires optimized Release build"
    );
    let args = Args::parse();
    ensure!(
        args.samples > 0 && args.samples <= 30 && args.warmups <= 30,
        "benchmark sample counts must be between 1 and 30"
    );
    let sources: Vec<Source> = serde_json::from_slice(&fs::read(&args.sources)?)?;
    let rules: Vec<Rule> = serde_json::from_slice(&fs::read(&args.rules)?)?;
    let cases: Vec<QueryCase> = serde_json::from_slice(&fs::read(&args.queries)?)?;
    ensure!(
        !sources.is_empty() && !cases.is_empty(),
        "empty source/query list"
    );
    let mut names = std::collections::BTreeSet::new();
    for item in &cases {
        ensure!(
            names.insert(item.name.as_str()),
            "duplicate benchmark query name"
        );
    }
    let shapes = shapes_graph(&args.shapes)?;
    let before = memory_stats();

    let now = Instant::now();
    let asserted = compile_sources(&sources)?;
    let compile_ms = now.elapsed().as_secs_f64() * 1e3;
    let now = Instant::now();
    let (materialized, stats) = materialize(asserted, &args.profile, &rules)?;
    let materialize_ms = now.elapsed().as_secs_f64() * 1e3;
    let now = Instant::now();
    let engine = OntologyEngine::from_pack(materialized)?;
    let reasoner_ms = now.elapsed().as_secs_f64() * 1e3;
    ensure!(engine.pack().metadata().fixpoint_reached, "incomplete Pack");
    let after_build = memory_stats();

    let now = Instant::now();
    engine.save_pack(&args.pack)?;
    let save_ms = now.elapsed().as_secs_f64() * 1e3;
    let pack_bytes = fs::metadata(&args.pack)?.len();
    let now = Instant::now();
    let loaded = OntologyEngine::load_pack(&args.pack)?;
    let load_ms = now.elapsed().as_secs_f64() * 1e3;
    ensure!(
        loaded.pack().quad_count() == engine.pack().quad_count(),
        "reloaded Pack quad mismatch"
    );
    let after_load = memory_stats();

    let mut measured_queries = BTreeMap::new();
    for case in cases {
        let view = view(&case.view)?;
        let original: Value = serde_json::from_slice(&engine.query(&case.query, view)?)?;
        let loaded_first_start = Instant::now();
        let first: Value = serde_json::from_slice(&loaded.query(&case.query, view)?)?;
        let first_ms = loaded_first_start.elapsed().as_secs_f64() * 1e3;
        ensure!(
            original == first,
            "Pack save/reload changed query {}",
            case.name
        );
        let (result, timings) = sample(args.warmups, args.samples, || {
            Ok(serde_json::from_slice(&loaded.query(&case.query, view)?)?)
        })?;
        ensure!(first == result, "cold/warm query result mismatch");
        measured_queries.insert(
            case.name,
            json!({
                "view":case.view,
                "cold_after_load_ms":first_ms,
                "median_ms":timings["median_ms"],
                "samples_ms":timings["samples_ms"],
                "result":result
            }),
        );
    }
    // Full validator produces a rich report; record its counts + coverage
    // rather than allocating a huge second serialized JSON answer per sample.
    let summary = |result: Value| {
        json!({
            "conforms":result["conforms"],
            "checked_shapes":result["checked_shapes"],
            "violations":result["violations"],
            "coverage":result["report"]["coverage"],
            "target_count":result["report"]["target_count"],
        })
    };
    let first_started = Instant::now();
    let first = summary(loaded.validate(Some(&shapes), None, None, 64)?);
    let cold_validation_ms = first_started.elapsed().as_secs_f64() * 1e3;
    let (validation, timing) = sample(args.warmups, args.samples, || {
        Ok(summary(loaded.validate(Some(&shapes), None, None, 64)?))
    })?;
    ensure!(validation == first, "cold/warm SHACL result changed");
    // Fixed-focus SHACL distinguishes local neighborhood validation from
    // intentionally full-target work (which scales with all order records).
    let focus = "<urn:ontokiln:kernel-bench:order/000000000>";
    let chosen = BTreeSet::from(["<urn:ontokiln:kernel-bench:ProbeShape>".to_owned()]);
    let focused_started = Instant::now();
    let focus_first = summary(loaded.validate(Some(&shapes), Some(&chosen), Some(focus), 64)?);
    let focused_cold_ms = focused_started.elapsed().as_secs_f64() * 1e3;
    let (focused, focused_timing) = sample(args.warmups, args.samples, || {
        Ok(summary(loaded.validate(
            Some(&shapes),
            Some(&chosen),
            Some(focus),
            64,
        )?))
    })?;
    ensure!(
        focused == focus_first,
        "focused SHACL changed between samples"
    );
    let after_validation = memory_stats();
    let report = json!({
        "schema_version":"ontokiln-v4-resident-benchmark/v1",
        "build_identity":{"mode":env!("ONTOKILN_BUILD_PROFILE"),
            "profile":env!("ONTOKILN_BUILD_PROFILE"),
            "opt_level":env!("ONTOKILN_OPT_LEVEL"),
            "rustc":env!("ONTOKILN_RUSTC_VERSION"),
            "target":env!("ONTOKILN_TARGET"), "kernel":ontokiln::identity::kernel()},
        "input":{"sources":sources,"profile":args.profile,
            "rule_count":rules.len(), "query_count":measured_queries.len()},
        "facts":{"asserted":stats.explicit_facts,
            "inferred":stats.inferred_facts,
            "quads":loaded.pack().quad_count()},
        "pack_bytes":pack_bytes,
        "build_ms":{"compile":compile_ms,"materialize":materialize_ms,
            "reasoner_init":reasoner_ms,"save":save_ms,"load":load_ms},
        "memory":{"before":before,"after_build":after_build,
            "after_load":after_load,"after_validation":after_validation},
        "samples":args.samples,"warmups":args.warmups,
        "queries":measured_queries,
        "shacl":{"cold_after_load_ms":cold_validation_ms,
            "median_ms":timing["median_ms"],
            "samples_ms":timing["samples_ms"],
            "result":validation},
        "shacl_focused":{"first_ms":focused_cold_ms,
            "median_ms":focused_timing["median_ms"],
            "samples_ms":focused_timing["samples_ms"],
            "result":focused}
    });
    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&args.output, serde_json::to_vec_pretty(&report)?)?;
    println!(
        "{}",
        json!({"status":"PASS","output":args.output,
        "facts":report["facts"],"build_ms":report["build_ms"],
        "resident_query_medians_ms":measured_queries.iter()
            .map(|(key,value)|(key.clone(),value["median_ms"].clone()))
            .collect::<BTreeMap<_,_>>(),
        "resident_shacl_median_ms":report["shacl"]["median_ms"],
        "resident_shacl_focused_median_ms":report["shacl_focused"]["median_ms"],
        "peak_rss_kib":report["memory"]["after_validation"]["peak_rss_kib"]})
    );
    Ok(())
}
