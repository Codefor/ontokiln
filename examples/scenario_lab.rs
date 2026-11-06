//! A fixture-specific evaluator for the synthetic lineage experiments.
//! All graph reads use the same Reasoner::query as the HTTP handler.
use anyhow::{Context, Result, anyhow, bail, ensure};
use clap::Parser;
use ontokiln_compiler::compile_directory;
use ontokiln_pack::Pack;
use ontokiln_reasoner::{RDF_TYPE, Reasoner};
use oxrdf::Term;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const NS: &str = "urn:ontokiln:lab:";
type Fields = BTreeSet<String>;
type World = BTreeMap<String, bool>;

#[derive(Parser)]
struct Args {
    #[arg(long, conflicts_with = "pack")]
    ttl: Option<PathBuf>,
    #[arg(long)]
    pack: Option<PathBuf>,
    #[arg(long)]
    case: Option<String>,
    #[arg(long)]
    trace: bool,
    /// Compare traversing every candidate with selecting branches (V1–V4).
    #[arg(long)]
    compare: bool,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    #[serde(flatten)]
    query: Query,
    context: World,
    certain: Fields,
    possible: Fields,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Query {
    Sources(String),
    Impact(String),
}

struct Lab(Reasoner);

fn term(name: &str) -> String {
    format!("<{NS}{name}>")
}

impl Lab {
    fn objects(&self, node: &str, predicate: &str) -> Result<Vec<String>> {
        let predicate = if predicate == "type" {
            RDF_TYPE.to_owned()
        } else {
            term(predicate)
        };
        self.0
            .query(&term(node), &predicate, false)
            .map_err(|e| anyhow!(e))?
            .into_iter()
            .map(|row| match row.object.parse::<Term>()? {
                Term::NamedNode(n) => Ok(n
                    .as_str()
                    .strip_prefix(NS)
                    .context("foreign lab IRI")?
                    .to_owned()),
                Term::Literal(l) if predicate == term("evidence") => Ok(l.value().to_owned()),
                other => bail!("unexpected lab term {other}"),
            })
            .collect()
    }

    fn one(&self, node: &str, predicate: &str) -> Result<String> {
        let objects = self.objects(node, predicate)?;
        ensure!(
            objects.len() == 1,
            "expected one {node}.{predicate}, got {objects:?}"
        );
        Ok(objects.into_iter().next().unwrap())
    }

    fn sources(
        &self,
        field: &str,
        world: Option<&World>,
        trace: &mut Vec<String>,
    ) -> Result<Fields> {
        let mut sources = Fields::new();
        self.visit(field, world, &mut Vec::new(), &mut sources, trace)?;
        Ok(sources)
    }

    fn visit(
        &self,
        node: &str,
        world: Option<&World>,
        path: &mut Vec<String>,
        sources: &mut Fields,
        trace: &mut Vec<String>,
    ) -> Result<()> {
        let kinds = self.objects(node, "type")?;
        if kinds.iter().any(|k| k == "SourceField") {
            sources.insert(node.to_owned());
            return Ok(());
        }
        ensure!(
            !path.iter().any(|n| n == node),
            "cycle along {path:?} -> {node}"
        );
        path.push(node.to_owned());
        let evidence = self.one(node, "evidence")?;
        let symbol = evidence
            .strip_prefix("checkout.go#")
            .context("unknown source file")?;
        ensure!(
            include_str!("scenarios/checkout.go").contains(&format!("func {symbol}(")),
            "missing evidence symbol {evidence}"
        );
        if kinds.iter().any(|k| k == "Choice") {
            let branches = if let Some(world) = world {
                let guard = self.one(node, "guard")?;
                let enabled = *world
                    .get(&guard)
                    .with_context(|| format!("undeclared guard {guard}"))?;
                let next = self.one(node, if enabled { "onTrue" } else { "onFalse" })?;
                trace.push(format!("{node}: {guard}={enabled} -> {next} [{evidence}]"));
                vec![next]
            } else {
                self.objects(node, "candidate")?
            };
            for next in branches {
                self.visit(&next, world, path, sources, trace)?;
            }
        } else {
            // The fixture profile has unary and binary transforms. Read slots
            // separately so repeated inputs and argument order survive RDFS set semantics.
            let first = self.one(node, "arg0")?;
            let mut inputs = vec![first];
            let second = self.objects(node, "arg1")?;
            ensure!(second.len() <= 1, "ambiguous second argument of {node}");
            inputs.extend(second);
            trace.push(format!("{node}({}) [{evidence}]", inputs.join(", ")));
            for input in inputs {
                self.visit(&input, world, path, sources, trace)?;
            }
        }
        path.pop();
        Ok(())
    }

    fn worlds(&self, context: &World) -> Result<Vec<World>> {
        let flags = self.objects("Checkout", "flag")?;
        ensure!(
            flags.len() <= 8,
            "fixture limit: at most eight boolean flags"
        );
        ensure!(
            context.keys().all(|key| flags.contains(key)),
            "unknown context key"
        );
        let mut worlds = vec![context.clone()];
        for key in flags.into_iter().filter(|key| !context.contains_key(key)) {
            let mut more = worlds.clone();
            for world in &mut worlds {
                world.insert(key.clone(), false);
            }
            for world in &mut more {
                world.insert(key.clone(), true);
            }
            worlds.extend(more);
        }
        Ok(worlds)
    }

    fn run(&self, case: &Case, verbose: bool) -> Result<(Fields, Fields)> {
        let mut union = Fields::new();
        let mut intersection: Option<Fields> = None;
        for world in self.worlds(&case.context)? {
            let mut trace = Vec::new();
            let hits = match &case.query {
                Query::Sources(field) => self.sources(field, Some(&world), &mut trace)?,
                Query::Impact(source) => {
                    let mut views = Fields::new();
                    for view in self.objects("Views", "field")? {
                        if self
                            .sources(&view, Some(&world), &mut trace)?
                            .contains(source)
                        {
                            views.insert(view);
                        }
                    }
                    views
                }
            };
            if verbose {
                println!("  {world:?} => {hits:?}");
                for line in trace {
                    println!("    {line}");
                }
            }
            union.extend(hits.iter().cloned());
            match &mut intersection {
                None => intersection = Some(hits),
                Some(common) => common.retain(|field| hits.contains(field)),
            }
        }
        let certain = intersection.context("fixture has no feasible world")?;
        let possible = union.difference(&certain).cloned().collect();
        Ok((certain, possible))
    }
}

#[derive(Debug, Default)]
struct Counts {
    correct: usize,
    extra: usize,
    missing: usize,
}

impl Counts {
    fn add(&mut self, result: &Fields, expected: &Fields) {
        self.correct += result.intersection(expected).count();
        self.extra += result.difference(expected).count();
        self.missing += expected.difference(result).count();
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let default = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/scenarios/ontology");
    let pack = match args.pack {
        Some(path) => Pack::load(&path)?,
        None => compile_directory(args.ttl.as_deref().unwrap_or(&default))?.pack,
    };
    let lab = Lab(Reasoner::new(pack));
    let cases: Vec<Case> = serde_json::from_str(include_str!("scenarios/cases.json"))?;
    let selected: Vec<_> = cases
        .iter()
        .filter(|c| args.case.as_ref().is_none_or(|id| *id == c.id))
        .collect();
    ensure!(!selected.is_empty(), "unknown case ID");
    let mut failed = 0;
    let mut compared = 0;
    let mut all_branches = Counts::default();
    let mut selected_branches = Counts::default();
    for case in &selected {
        let (certain, possible) = lab
            .run(case, args.trace)
            .with_context(|| format!("case {}", case.id))?;
        let passed = certain == case.certain && possible == case.possible;
        println!(
            "{} {} certain={certain:?} possible_only={possible:?}",
            if passed { "PASS" } else { "FAIL" },
            case.id
        );
        if args.compare && matches!(case.id.as_str(), "V1" | "V2" | "V3" | "V4") {
            let Query::Sources(field) = &case.query else {
                bail!("comparison requires a source query")
            };
            let unguarded = lab.sources(field, None, &mut Vec::new())?;
            all_branches.add(&unguarded, &case.certain);
            selected_branches.add(&certain, &case.certain);
            compared += 1;
        }
        if !passed {
            println!(
                "  expected certain={:?} possible_only={:?}",
                case.certain, case.possible
            );
            failed += 1;
        }
    }
    if compared > 0 {
        println!(
            "comparison ({compared} cases), all candidates: {all_branches:?}, selected: {selected_branches:?}"
        );
    }
    ensure!(failed == 0, "{failed}/{} cases failed", selected.len());
    println!("{}/{} cases passed", selected.len(), selected.len());
    Ok(())
}
