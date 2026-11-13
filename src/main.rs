mod allocation;
mod execution;
mod queries;
mod worker;
use anyhow::{Context, Result, ensure};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use futures_util::StreamExt;
use ontokiln::{full, full_execution, identity};
use ontokiln_compiler::rules::{self, Engine};
use ontokiln_pack::Pack;
use ontokiln_reasoner::sparql;
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
    /// Execute the versioned consumer contract, independent of storage adapters.
    Api {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        result: PathBuf,
    },
    /// Mount one Full snapshot once and execute newline-delimited API v1 requests.
    ApiSession {
        #[arg(long)]
        dataset: PathBuf,
        /// Maximum bytes reserved for the private mounted database.
        #[arg(long, default_value_t = 4 * 1024 * 1024 * 1024)]
        snapshot_bytes: u64,
    },
    /// Report the generic engine and artifact contracts.
    Identity,
    /// Validate a pack and report its content and source identities.
    Inspect {
        #[arg(long)]
        pack: PathBuf,
    },
    /// Require an exact engine and pack identity before consumption.
    Check {
        #[arg(long)]
        pack: PathBuf,
        #[arg(long)]
        expected: PathBuf,
    },
    /// Report the disk compiler and execution contracts.
    FullInfo,
    /// Execute a budgeted disk compile, inference, query or SHACL request.
    Full {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        result: PathBuf,
    },
    #[command(hide = true)]
    FullWorker {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        result: PathBuf,
    },
    #[command(hide = true)]
    QueryWorker { snapshot: i32, heap: usize },
    /// Compile every .ttl and .nt file under a directory into one offline pack.
    Compile {
        #[arg(long)]
        ttl: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Export a queryable N-Triples proof graph, optionally for one subject.
    Proofs {
        #[arg(long)]
        pack: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        subject: Option<String>,
        #[arg(long)]
        revision: Option<String>,
    },
    /// Run a read-only .rq query over the selected graph view.
    Sparql {
        #[arg(long)]
        pack: PathBuf,
        #[arg(long)]
        query: PathBuf,
        /// JSON object of typed RDF template parameters.
        #[arg(long, default_value = "{}")]
        parameters: String,
        /// JSON query options (view, revision, profile and output/scan limits).
        #[arg(long, default_value = "{}")]
        options: String,
        #[arg(long)]
        jsonl: bool,
        #[arg(long, default_value_t = 5000)]
        deadline_ms: u64,
        #[arg(long)]
        memory_bytes: Option<usize>,
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
        /// Write the proof graph for this offline rule closure.
        #[arg(long)]
        proofs: Option<PathBuf>,
        #[arg(long)]
        revision: Option<String>,
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
        #[arg(long)]
        query_slots: Option<usize>,
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
    identity: Arc<serde_json::Value>,
    jobs: Arc<execution::Registry>,
    worker_binary: PathBuf,
    slots: Arc<Semaphore>,
    source: &'static str,
    templates: Arc<queries::Templates>,
}

impl App {
    fn new(
        pack: Pack,
        source: &'static str,
        templates: queries::Templates,
        slots: usize,
    ) -> Result<Self> {
        let pack = pack.into_snapshot()?;
        let identity = Arc::new(identity::describe(&pack)?);
        Ok(Self {
            engine: Arc::new(Reasoner::new(pack)),
            identity,
            source,
            templates: Arc::new(templates),
            slots: Arc::new(Semaphore::new(slots)),
            jobs: Default::default(),
            worker_binary: std::env::current_exe()?,
        })
    }
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

fn main() -> Result<()> {
    let cmd = Cli::parse().command;
    if let Command::QueryWorker { snapshot, heap } = cmd {
        return worker::execute(snapshot, heap);
    }
    match cmd {
        Command::Api { request, result } => ontokiln::api::run_files(&request, &result),
        Command::ApiSession {
            dataset,
            snapshot_bytes,
        } => ontokiln::session::run(&dataset, snapshot_bytes),
        Command::Full { request, result } => full_execution::run_files(&request, &result),
        Command::FullWorker { request, result } => full_execution::run_worker(&request, &result),
        _ => application(cmd),
    }
}
#[tokio::main]
async fn application(cmd: Command) -> Result<()> {
    match cmd {
        Command::Api { .. }
        | Command::ApiSession { .. }
        | Command::QueryWorker { .. }
        | Command::Full { .. }
        | Command::FullWorker { .. } => {
            unreachable!()
        }
        Command::Identity => println!("{}", identity::kernel()),
        Command::Inspect { pack } => println!("{}", identity::inspect(&pack)?),
        Command::Check { pack, expected } => println!("{}", identity::check(&pack, &expected)?),
        Command::FullInfo => println!("{}", full::runtime_info()),
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
        Command::Proofs {
            pack,
            out,
            subject,
            revision,
        } => {
            ensure!(
                out.extension().is_some_and(|ext| ext == "nt"),
                "--out must end in .nt"
            );
            let engine = Reasoner::new(Pack::load(&pack)?);
            let mut file = tempfile::NamedTempFile::new_in(
                out.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )?;
            let s = subject
                .as_deref()
                .map(NamedNodeRef::new)
                .transpose()?
                .map(|s| s.to_string());
            engine.export_proofs(
                s.as_deref(),
                revision.as_deref(),
                std::io::BufWriter::new(file.as_file_mut()),
            )?;
            file.as_file().sync_all()?;
            file.persist(out)?;
        }
        Command::Sparql {
            pack,
            query,
            parameters,
            options,
            jsonl,
            deadline_ms,
            memory_bytes,
        } => {
            let query = queries::read_query(&query)?;
            let options: sparql::Options = serde_json::from_str(&options)?;
            options.validate(jsonl)?;
            let media = sparql::media(&query, jsonl);
            let state = App::new(Pack::load(&pack)?, "pack", Default::default(), 1)?;
            let message = |(_, err): ApiError| anyhow::anyhow!(err.0.error);
            let deadline = execution::timeout(deadline_ms).map_err(message)?;
            let heap = execution::heap(memory_bytes).map_err(message)?;
            let job = worker::Job {
                operation: worker::Operation::Sparql {
                    query: query.to_string(),
                    bindings: Default::default(),
                    parameters: serde_json::from_str(&parameters)?,
                },
                options,
                jsonl,
            };
            let response = execution::run(state, job, None, heap, deadline, media)
                .await
                .map_err(message)?;
            let mut body = response.into_body().into_data_stream();
            while let Some(bytes) = body.next().await {
                let bytes = bytes?;
                std::io::stdout().lock().write_all(&bytes)?;
                if jsonl && bytes.starts_with(b"{\"error\":") {
                    anyhow::bail!("stream ended with a query error")
                }
            }
        }
        Command::Rules {
            pack,
            program,
            engine,
            max_terms,
            proofs,
            revision,
        } => {
            let graph = Pack::load(&pack)?;
            let rules =
                serde_json::from_reader::<_, Vec<rules::Rule>>(std::fs::File::open(&program)?)?;
            let engine = match engine {
                RuleEngine::Datafrog => Engine::Datafrog,
                #[cfg(feature = "ascent-engine")]
                RuleEngine::Ascent => Engine::Ascent,
            };
            let mut artifact = proofs
                .as_ref()
                .map(|p| {
                    ensure!(
                        p.extension().is_some_and(|e| e == "nt"),
                        "--proofs must end in .nt"
                    );
                    Ok::<_, anyhow::Error>(tempfile::NamedTempFile::new_in(
                        p.parent()
                            .filter(|p| !p.as_os_str().is_empty())
                            .unwrap_or(Path::new(".")),
                    )?)
                })
                .transpose()?;
            let mut writer = artifact
                .as_mut()
                .map(|f| std::io::BufWriter::new(f.as_file_mut()));
            let report = rules::materialize_scoped(
                &graph,
                &rules,
                engine,
                max_terms,
                revision.as_deref(),
                std::io::stdout().lock(),
                writer.as_mut().map(|w| w as &mut dyn Write),
            )?;
            drop(writer);
            if let Some((file, path)) = artifact.zip(proofs) {
                file.as_file().sync_all()?;
                file.persist(path)?;
            }
            eprintln!(
                "{} derived triples; rule stages: {:?}",
                report.added, report.stages
            );
        }
        Command::Serve {
            source,
            listen,
            query_slots,
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
            let slots = query_slots.unwrap_or(slots);
            ensure!((1..=8).contains(&slots), "query-slots must be 1..8");
            let state = App::new(pack, mode, queries::templates(queries.as_deref())?, slots)?;
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
        .route("/v1/identity", get(runtime_identity))
        .route("/v1/query", get(query))
        .route("/v1/query/batch", post(queries::batch))
        .route("/v1/entities", post(queries::entities))
        .route("/v1/sources", get(queries::sources))
        .route("/v1/sparql/{id}", delete(queries::cancel))
        .route("/v1/queries/{id}", delete(queries::cancel))
        .route("/v1/sparql", post(queries::sparql))
        .with_state(state)
}

async fn runtime_identity(State(app): State<App>) -> Json<serde_json::Value> {
    Json((*app.identity).clone())
}

#[derive(Serialize)]
struct Health {
    available_query_slots: usize,
    pack_version: u32,
    status: &'static str,
    source: &'static str,
    profile: &'static str,
    triples: usize,
    terms: usize,
}

async fn health(State(app): State<App>) -> Json<Health> {
    Json(Health {
        available_query_slots: app.slots.available_permits(),
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
    request_id: Option<String>,
    memory_bytes: Option<usize>,
    deadline_ms: Option<u64>,
    max_rows: Option<usize>,
    max_bytes: Option<usize>,
    max_visits: Option<usize>,
    #[serde(default)]
    view: ontokiln_reasoner::View,
    revision: Option<String>,
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
) -> Result<axum::response::Response, ApiError> {
    let subject = iri(&request.subject)?;
    let predicate = iri(&request.predicate)?;
    queries::native(
        app,
        worker::Operation::Subjects {
            subjects: vec![subject],
            predicate,
            explain: request.explain,
            view: request.view,
            batch: false,
        },
        request.revision,
        queries::Controls {
            request_id: request.request_id,
            memory_bytes: request.memory_bytes,
            deadline_ms: request.deadline_ms,
            max_rows: request.max_rows,
            max_bytes: request.max_bytes,
            max_visits: request.max_visits,
        },
    )
    .await
}
