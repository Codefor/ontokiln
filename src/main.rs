mod queries;
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use ontokiln_compiler::rules::{self, Engine};
use ontokiln_pack::Pack;
use ontokiln_reasoner::sparql::CancellationToken;
use ontokiln_reasoner::{Answer, Reasoner};
use oxrdf::NamedNodeRef;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{net::TcpListener, sync::Semaphore};

#[derive(Parser)]
#[command(
    version,
    about = "Compile Turtle ontologies and serve in-memory inference"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compile every .ttl and .nt file under a directory into one offline pack.
    Compile {
        #[arg(long)]
        ttl: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Run a read-only .rq query over the explicit RDF graph.
    Sparql {
        #[arg(long)]
        pack: PathBuf,
        #[arg(long)]
        query: PathBuf,
    },
    /// Evaluate a bounded positive rule program and write only newly derived N-Triples.
    Rules {
        #[arg(long)]
        pack: PathBuf,
        #[arg(long)]
        program: PathBuf,
        #[arg(long, value_enum, default_value = "datafrog")]
        engine: RuleEngine,
        /// Maximum distinct subject/object terms in the predicates used by the program.
        #[arg(long, default_value_t = 256)]
        max_terms: usize,
    },
    /// Load one immutable snapshot and start the HTTP service.
    Serve {
        #[command(flatten)]
        source: Source,
        /// Directory of trusted read-only .rq templates for HTTP queries.
        #[arg(long)]
        queries: Option<PathBuf>,
        #[arg(long, default_value = "127.0.0.1:7878")]
        listen: SocketAddr,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum RuleEngine {
    Datafrog,
    #[cfg(feature = "ascent-engine")]
    Ascent,
}

#[derive(Args)]
#[group(required = true, multiple = false)]
struct Source {
    /// Compile this directory once at startup, in memory.
    #[arg(long)]
    ttl: Option<PathBuf>,
    /// Load a previously compiled pack without reading source Turtle files.
    #[arg(long)]
    pack: Option<PathBuf>,
}

#[derive(Clone)]
struct App {
    engine: Arc<Reasoner>,
    slots: Arc<Semaphore>,
    source: &'static str,
    templates: Arc<queries::Templates>,
}

impl App {
    fn slot(&self) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
        self.slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| error(StatusCode::TOO_MANY_REQUESTS, "query workers are busy"))
    }
}

fn compile(dir: &Path) -> Result<Pack> {
    let compiled = ontokiln_compiler::compile_directory(dir)?;
    eprintln!(
        "compiled {} RDF files, {} parsed triples, {} distinct",
        compiled.files,
        compiled.parsed,
        compiled.pack.triple_count()
    );
    Ok(compiled.pack)
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Compile { ttl, out } => {
            ensure!(
                out.extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pack")),
                "--out must end in .pack"
            );
            compile(&ttl)?
                .save(&out)
                .with_context(|| format!("write {}", out.display()))?;
            println!("saved {}", out.display());
        }
        Command::Sparql { pack, query } => {
            let engine = Reasoner::new(Pack::load(&pack)?);
            let result = engine.sparql(
                &queries::read_query(&query)?,
                vec![],
                CancellationToken::new(),
            )?;
            std::io::stdout().lock().write_all(&result.bytes)?;
        }
        Command::Rules {
            pack,
            program,
            engine,
            max_terms,
        } => {
            let graph = Pack::load(&pack)?;
            let rules =
                serde_json::from_reader::<_, Vec<rules::Rule>>(std::fs::File::open(&program)?)?;
            let engine = match engine {
                RuleEngine::Datafrog => Engine::Datafrog,
                #[cfg(feature = "ascent-engine")]
                RuleEngine::Ascent => Engine::Ascent,
            };
            let report =
                rules::materialize(&graph, &rules, engine, max_terms, std::io::stdout().lock())?;
            eprintln!(
                "{} derived triples; rule stages: {:?}",
                report.added, report.stages
            );
        }
        Command::Serve {
            source,
            listen,
            queries,
        } => {
            let (pack, mode) = match (source.ttl, source.pack) {
                (Some(dir), None) => (compile(&dir)?, "ttl"),
                (None, Some(file)) => (
                    Pack::load(&file).with_context(|| format!("load {}", file.display()))?,
                    "pack",
                ),
                _ => anyhow::bail!("choose exactly one of --ttl and --pack"),
            };
            let slots = std::thread::available_parallelism()
                .map_or(1, |n| n.get())
                .min(8);
            let state = App {
                engine: Arc::new(Reasoner::new(pack)),
                slots: Arc::new(Semaphore::new(slots)),
                source: mode,
                templates: Arc::new(queries::templates(queries.as_deref())?),
            };
            let app = router(state);
            let listener = TcpListener::bind(listen)
                .await
                .with_context(|| format!("bind {listen}"))?;
            eprintln!(
                "serving {mode} snapshot at http://{}",
                listener.local_addr()?
            );
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await?;
        }
    }
    Ok(())
}

fn router(state: App) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/query", get(query))
        .route("/v1/query/batch", post(queries::batch))
        .route("/v1/entities", post(queries::entities))
        .route("/v1/sparql", post(queries::sparql))
        .with_state(state)
}

#[cfg(test)]
mod http_tests;

#[derive(Serialize)]
struct Health {
    pack_version: u32,
    status: &'static str,
    source: &'static str,
    profile: &'static str,
    triples: usize,
    terms: usize,
}

async fn health(State(app): State<App>) -> Json<Health> {
    Json(Health {
        pack_version: ontokiln_pack::PACK_VERSION,
        status: "ready",
        source: app.source,
        profile: "rdfs-hierarchy-v1",
        triples: app.engine.triple_count(),
        terms: app.engine.term_count(),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    subject: String,
    predicate: String,
    #[serde(default)]
    explain: bool,
}

#[derive(Serialize)]
struct Response {
    results: Vec<Answer>,
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}
type ApiError = (StatusCode, Json<ErrorBody>);

fn error(status: StatusCode, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(ErrorBody {
            error: message.into(),
        }),
    )
}

fn iri(value: &str) -> Result<String, ApiError> {
    NamedNodeRef::new(value)
        .map(|n| n.to_string())
        .map_err(|_| error(StatusCode::BAD_REQUEST, "expected an absolute IRI"))
}

async fn query(
    State(app): State<App>,
    Query(request): Query<Request>,
) -> Result<Json<Response>, ApiError> {
    let subject = iri(&request.subject)?;
    let predicate = iri(&request.predicate)?;
    let slot = app.slot()?;
    let results = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        app.engine.query(&subject, &predicate, request.explain)
    })
    .await
    .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "query worker failed"))?
    .map_err(|msg| error(StatusCode::UNPROCESSABLE_ENTITY, msg))?;
    Ok(Json(Response { results }))
}
