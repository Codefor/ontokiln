//! Wire protocol for a single, disposable query process.
use crate::allocation;
use anyhow::{Result, ensure};
use ontokiln_pack::Pack;
use ontokiln_reasoner::{
    Reasoner, View,
    parameters::{self, Parameter},
    sparql::{self, CancellationToken, Options, SparqlParser},
};
use oxrdf::{Term, Variable};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{FromRawFd, RawFd},
        unix::fs::MetadataExt,
    },
};

pub const MAX_HEAP: usize = 3 * 1024 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
pub struct Job {
    pub operation: Operation,
    pub options: Options,
    pub jsonl: bool,
}

#[derive(Serialize, Deserialize)]
pub enum Operation {
    Sparql {
        query: String,
        bindings: BTreeMap<String, String>,
        parameters: BTreeMap<String, Parameter>,
    },
    Subjects {
        subjects: Vec<String>,
        predicate: String,
        explain: bool,
        view: View,
        batch: bool,
    },
    Entities {
        types: Vec<String>,
        subclasses: bool,
        limit: usize,
    },
}

pub const INVALID_EXIT: i32 = 76;
fn invalid<T>(r: Result<T>) -> T {
    r.unwrap_or_else(|err| {
        eprintln!("{err}");
        std::process::exit(INVALID_EXIT)
    })
}

struct WireLimit<W> {
    inner: W,
    left: usize,
}
impl<W: Write> Write for WireLimit<W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if b.len() > self.left {
            return Err(io::Error::other("query output byte budget exceeded"));
        }
        let written = self.inner.write(b)?;
        self.left -= written;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
pub fn execute(fd: RawFd, limit: usize) -> Result<()> {
    ensure!(limit > 0 && limit <= MAX_HEAP, "invalid heap limit");
    // The HTTP supervisor passes its private validated snapshot, never the
    // user-supplied --pack path. No request can choose this file.
    allocation::begin(limit);
    // The supervisor transfers a read-only descriptor for an unlinked file.
    ensure!(
        fd > 2 && unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_ACCMODE == libc::O_RDONLY,
        "invalid snapshot descriptor"
    );
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.nlink() == 0,
        "expected anonymous private snapshot"
    );
    let pack = unsafe { Pack::map_validated_snapshot(&file)? };
    let engine = Reasoner::new(pack);
    run(&engine)
}
fn run(engine: &Reasoner) -> Result<()> {
    let job: Job = serde_json::from_reader(io::stdin().take(4 * 1024 * 1024))?;
    let stdout = io::stdout();
    let mut sink = WireLimit {
        inner: stdout.lock(),
        left: job.options.max_bytes,
    };
    let revision = job.options.revision.as_deref();
    invalid(engine.check_revision(revision).map_err(anyhow::Error::msg));
    let (text, bindings, params) = match job.operation {
        Operation::Sparql {
            query,
            bindings,
            parameters,
        } => (query, bindings, parameters),
        Operation::Subjects {
            subjects,
            predicate,
            explain,
            view,
            batch,
        } => {
            let results = engine
                .batch_counted(
                    &subjects,
                    &predicate,
                    explain,
                    view,
                    revision,
                    job.options.max_visits,
                )
                .map_err(anyhow::Error::msg)?;
            ensure!(
                results.iter().map(Vec::len).sum::<usize>() <= job.options.max_rows,
                "output row budget exceeded"
            );
            let mut wrapped: Vec<_> = results
                .into_iter()
                .map(|results| crate::Response { results })
                .collect();
            return if batch {
                serde_json::to_writer(sink, &wrapped)
            } else {
                serde_json::to_writer(sink, &wrapped.pop().unwrap())
            }
            .map_err(Into::into);
        }
        Operation::Entities {
            types,
            subclasses,
            limit,
        } => {
            let matched = engine.entities_indexed(
                &types,
                subclasses,
                limit,
                revision,
                job.options.max_visits,
            )?;
            ensure!(
                matched.entities.len() <= job.options.max_rows,
                "output row budget exceeded"
            );
            return serde_json::to_writer(sink, &matched).map_err(Into::into);
        }
    };
    let query = invalid(parameters::bind(
        SparqlParser::new().parse_query(&text)?,
        &params,
    ));
    let bindings = invalid(
        bindings
            .into_iter()
            .map(|(v, t)| Ok((Variable::new(v)?, t.parse::<Term>()?)))
            .collect::<Result<Vec<_>>>(),
    );
    let mut buffer = Vec::new();
    let buffered = (job.options.profile || job.options.counters) && !job.jsonl;
    let output: &mut dyn Write = if buffered { &mut buffer } else { &mut sink };
    let mut profile = sparql::metered(allocation::meter, || {
        engine.write_sparql(
            &query,
            bindings,
            &CancellationToken::new(),
            &job.options,
            job.jsonl,
            output,
        )
    })?;
    if let Some(p) = &mut profile {
        p["peak_heap_bytes"] = json!(allocation::peak());
    }
    if job.jsonl {
        let mut end = json!({"end":true});
        if let Some(p) = profile {
            end["profile"] = p;
        }
        writeln!(sink, "{end}")?;
    } else if buffered {
        let media = sparql::media(&query, false);
        let result: Value = if media == "application/n-triples" {
            String::from_utf8(buffer)?.into()
        } else {
            serde_json::from_slice(&buffer)?
        };
        serde_json::to_writer(sink, &json!({"result":result,"profile":profile}))?;
    }
    Ok(())
}
