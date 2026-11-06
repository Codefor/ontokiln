use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::get,
};
use clap::{Args, Parser, Subcommand};
use ontokiln_pack::Pack;
use ontokiln_reasoner::{Answer, Reasoner};
use oxrdf::NamedNodeRef;
use serde::{Deserialize, Serialize};
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
    /// Compile every .ttl file under a directory into one offline pack.
    Compile {
        #[arg(long)]
        ttl: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Load one immutable snapshot and start the HTTP service.
    Serve {
        #[command(flatten)]
        source: Source,
        #[arg(long, default_value = "127.0.0.1:7878")]
        listen: SocketAddr,
    },
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
}

fn compile(dir: &Path) -> Result<Pack> {
    let compiled = ontokiln_compiler::compile_directory(dir)?;
    eprintln!(
        "compiled {} TTL files, {} parsed triples, {} distinct",
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
        Command::Serve { source, listen } => {
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
            };
            let app = Router::new()
                .route("/health", get(health))
                .route("/v1/query", get(query))
                .with_state(state);
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

#[derive(Serialize)]
struct Health {
    status: &'static str,
    source: &'static str,
    profile: &'static str,
    triples: usize,
    terms: usize,
}

async fn health(State(app): State<App>) -> Json<Health> {
    Json(Health {
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
    error: &'static str,
}
type ApiError = (StatusCode, Json<ErrorBody>);

fn error(status: StatusCode, message: &'static str) -> ApiError {
    (status, Json(ErrorBody { error: message }))
}

async fn query(
    State(app): State<App>,
    Query(request): Query<Request>,
) -> Result<Json<Response>, ApiError> {
    let iri = |value: &str| {
        NamedNodeRef::new(value)
            .map(|n| n.to_string())
            .map_err(|_| {
                error(
                    StatusCode::BAD_REQUEST,
                    "subject and predicate must be absolute IRIs",
                )
            })
    };
    let subject = iri(&request.subject)?;
    let predicate = iri(&request.predicate)?;
    let slot = app
        .slots
        .try_acquire_owned()
        .map_err(|_| error(StatusCode::TOO_MANY_REQUESTS, "query workers are busy"))?;
    let results = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        app.engine.query(&subject, &predicate, request.explain)
    })
    .await
    .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "query worker failed"))?
    .map_err(|msg| error(StatusCode::UNPROCESSABLE_ENTITY, msg))?;
    Ok(Json(Response { results }))
}
