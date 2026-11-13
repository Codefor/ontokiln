# Kernel baseline harness

`benchmarks/kernel_baseline.py` measures one already built binary. It never
builds Rust or starts the historical 100 million fact suites. Keep compilation,
tests and other heavy processes out of the timed run.

Run the harness tests without the engine:

```sh
python3 -B -m unittest discover -s tests/benchmarks -v
```

After separately building the intended release binary, use a short functional
smoke before the default 10K / 100K / 1M fact ladder:

```sh
python3 -B benchmarks/kernel_baseline.py \
  --binary target/release/ontokiln --output data/kernel-smoke.json \
  --sizes 100 --build-samples 1 --query-samples 1 --validation-samples 1 \
  --build-profile release --build-features default

python3 -B benchmarks/kernel_baseline.py \
  --binary target/release/ontokiln --output data/kernel-baseline.json \
  --build-profile release --build-features default
```

The default is three independent pack compilations, three independent Full
compilations, and three independent materializations. Each query has one
warm-up and nine measured requests. Each SHACL workload has one warm-up and
three measured requests. Requests are serial. Rayon defaults to one thread and
the legacy CLI's Tokio runtime to one worker; `--threads` changes Rayon only.
Internal engine threads and request concurrency are different parameters.

`--memory-mib`, `--scratch-mib`, `--timeout-seconds`, `--max-iterations`,
`--max-derived-facts` and `--max-intermediate-rows` fix the API execution limits.
Defaults are 512 MiB memory, 4 GiB scratch, 60 seconds, 100 iterations, one million
derived facts and ten million intermediate candidate rows. Pack compilation
uses the legacy compile CLI, which does **not** enforce the Full memory/scratch
budgets; its resource use is measured and an external timeout is enforced.
`--pack-max-mib` explicitly sets its pack size allowance (default 256 MiB).
The API has its own deadline; an outer watchdog allows five additional seconds
for terminal publication before killing the command process group and reaping
the direct command. Successful API replies must also report `worker_reaped=true`.

On a failed or exhausted repetition, the remaining repetitions of that
workload are marked skipped. Other independent workloads continue; a failed
artifact prevents only its dependent workloads. All statuses remain in the
report. Any failure or skipped workload makes the final exit status nonzero.
A partial report is atomically checkpointed after each sample. `--keep-work-dir`
retains inputs, the first valid artifact of each kind, and individual request,
response and log files. `--work-dir` chooses their parent directory. Temporary
artifacts are otherwise removed; no large generated datasets are committed.
Every child process gets `TMPDIR=<work_directory>/scratch`, so API private
copies and compiler spill files use that same selected filesystem. Choose a
disk-backed `--work-dir` for a disk baseline; a default system temporary
directory may be tmpfs. The report stores this policy and a normalized TMPDIR
override separately from the actual working-directory path.

## Fixed workload and independent answers

The scalable generator is separate from the frozen API contract fixtures. It
writes ten schema triples and ten triples for each synthetic order/customer
pair. Thus 10,000 / 100,000 / 1,000,000 facts mean exactly 999 / 9,999 / 99,999
orders, plus the same number of customers. Every input triple is unique.
The two engines receive exactly the same N-Triples bytes and use their asserted
default graph for common queries.

| Workload | Independently checked result |
|---|---|
| Point lookup | Exact customer, zero-padded integer lexical form, language-tagged label and status |
| Customer/region join | Exact ordered first 16 order/customer/region rows where index is 11 modulo 40 |
| Region aggregate | All region IRIs, counts and numeric sums calculated from record indices |
| Property path | Exact reachable pairs from the first and last bounded-chain roots, including the tail self-loop behavior |
| Asserted fact count | Exactly the requested input fact count |
| Inferred predicate counts | Every derived predicate and its closed-form count |
| Closure business join | Exact inherited types × inferred region × bounded reachability targets |
| All-order SHACL, `targetSubjectsOf` | Conforms, zero violations and the exact expected focus-node set; no vacuous zero-target success |
| All-order SHACL, `targetClass` | Same constraints and same independently expected focus set, measuring the alternative target-discovery path |
| Native Core negative control | Named `minCount 2` property shape checks one order with exactly one amount: false, one violation and exact focus/path/shape |
| Compiled/reasoned SHACL probe | One named type `hasValue` violation before materialization; conformance after materialization |

The Full profile is `rdfs-supported-v1` plus three explicit positive rules:
`next → reach`, recursive `reach + next → reach`, and `customer + region →
inRegion`. The schema exercises two levels of class inheritance, two levels of
property inheritance and domain/range whose type assertions already exist.
RDFS is the implementation's published supported subset, not complete OWL.

Every group of at most eight orders has a forward chain and a self-loop on its
last node. For a group of `m` nodes, `reach` contains `m(m−1)/2 + 1` pairs. This
keeps the total inferred graph linear as input size increases. If there are
`n` orders, the remaining inferred facts are `5n + 2`: two type assertions and
three inferred relationships per order, plus the transitive class/property
schema assertions. No arbitrary global chain with quadratic closure is used.

Answers are written in Python from these definitions. They are never learned
from an engine run or updated from a golden engine output. Query validation
checks RDF term types, datatype/language, lexical values, multiplicity and
declared ordering. SHACL validates named violation fields without freezing
diagnostic prose. Every Full query/validation request carries both the compiled
or materialized artifact's manifest SHA-256 and its file content digest.
`OrderShape` targets subjects of `customer`; `ClassOrderShape` targets class
`Order`. They have identical property constraints and must check exactly the
same `n` orders. Both are retained as separate workloads so a faster target
selection mechanism cannot hide the cost of the other.
The native Core negative control prevents a skipped Core validator from being
mistaken for successful all-conforming validation. Its fixed shape has exactly
one constraint, `sh:minCount 2`. The current native report exposes focus, path,
shape reference and diagnostic message, but no constraint-component field;
the benchmark records the intended MinCount component in workload metadata
and checks the available semantic fields without inventing a response field.

## Measurement boundaries

The JSON retains every measured sample, including warm-ups and failures. Its
latency summary contains only successful, non-warm-up samples, with median,
minimum and maximum. Three or nine observations do not support meaningful tail
latency claims; the script does not report p95/p99.

| Field | Scope |
|---|---|
| `end_to_end_seconds` | Before process creation through exit and reading response bytes; Python JSON parsing/oracles excluded |
| `process_wall_seconds` | Before process creation through `wait4` completion |
| `process_user_seconds`, `process_system_seconds` | Command rusage from `wait4`; descendant accounting depends on the OS |
| `api_metrics.execution_seconds` | Full supervisor boundary, including verified copies and artifact publication; not pure core time |
| `parse_dictionary_seconds` | Compile setup, RDF parse/dictionary population and rule constants |
| `index_seconds` | Deduplication/index work and subsequent plan/metadata/SQLite maintenance |
| `engine_seconds` | Full compile phase, overlapping the preceding two fields; not an additional cost to sum |
| `reasoning_seconds` | Materialization setup, rule joins, fixpoint, cleanup and SQLite maintenance |
| `rule_join_candidate_rows` | Candidate work recorded by materialization before final deduplication |
| `peak_rss_bytes` | API budget's combined supervisor/worker RSS metric; not PSS |
| `supervisor_peak_rss_bytes` | Supervisor high-water RSS, already represented in the combined budget metric |
| `peak_scratch_bytes` | Sampled private logical scratch, including detected open-deleted files on Linux |
| `io_*_bytes` | Worker block-I/O rusage counters, affected by the OS page cache; not logical input/output bytes |

Query and validation expose no separate core-phase timer through the current
public API. Their reports therefore use external and supervisor time without
subtracting those fields to invent a kernel latency. Full query/validation
verifies and copies the SQLite snapshot on every call. There is no persistent
worker cache between calls. Generation, hashing and compilation can warm the OS
cache before the first query; no sample is called a cold-disk measurement.

The report separately records logical/allocated artifact bytes, input bytes,
fact and term counts, manifest statistics (including fixpoint iteration and
rule evaluation counts where available), source/rule/shape/query SHA-256,
binary SHA-256, binary kernel identity, actual repository HEAD/dirty status,
Cargo.lock and toolchain-file hashes, local `rustc`, caller-declared build
profile/features, CPU model/affinity, host memory, visible cgroup limits,
filesystem type, free space, relevant environment and all budgets. A missing
cgroup value remains null; it does not imply unlimited resources. The build
profile/features declarations are not inferred from the binary filename.

This is a synthetic baseline for this machine and this binary. It does not
establish production capacity, HTTP throughput, update performance, full OWL
reasoning, or physical compatibility with historic artifacts. Historical
benchmark JSON from another machine/toolchain is not an equivalent comparison.
