> **历史测试档案：** 本目录中的 Full/API-v1、SQLite 与旧业务 harness 测试只保留审计价值，不能直接在现行 Pack v4 CLI 执行。现行功能/性能回归详见 [docs/v4-baseline.md](../../docs/v4-baseline.md) 和 [benchmarks/v4_resident_baseline.py](../../benchmarks/v4_resident_baseline.py)。不要恢复历史兼容 API 以运行这里的用例。

# Benchmark harnesses

## Directory workloads: the shared integration entry point

Integration performance regressions for real or synthetic ontology directories
use `benchmarks/business_baseline.py` with `--ontology-repo`; follow the
[one-command business baseline guide](../../benchmarks/business-baseline.md).
Generated templates and the discovery inventory remain available for review;
the explicit API request mode is also supported.
Its file-API measurements keep protocol completion separate from business
correctness, which remains `MANUAL_REVIEW_REQUIRED`.
Detailed diagnostics remain in the local run directory; the ZIP uses a sharing
summary while retaining paths, case names, hashes and environment metadata.

The default `--jobs auto` admits independent read cases according to available
CPU and memory/scratch budgets. Compilation and materialization remain serial;
each read case still runs its warm-up and measured repetitions sequentially.
Use `--jobs 1` for an isolated serial request baseline. `--threads` controls
Rayon only and does not turn one Full query into a multicore query.

`--total-memory-mib` and `--total-scratch-mib` bound scheduling reservations,
not OS-enforced aggregate use; each API request keeps its own limits. Compare
the read phase's total wall time and observed concurrency separately from
individual request latency. Keep jobs, aggregate admission settings, per-request
budgets and thread settings fixed when comparing engine revisions.

Admission is planned after the serial build phases, using the maximum read
request memory/scratch limits per slot and a snapshot of available resources.
Check `parallelism.effective_jobs`; two auto runs need not admit the same count.
Known capacity too small for one slot produces `READ_ADMISSION_FAILED` (exit 2)
and skipped reads. Missing input artifacts produce `SKIPPED_DEPENDENCY` reads.
The `read_execution.completed_requests_per_second` metric includes successful
warm-ups in its numerator and the entire read phase, including checks and
failures, in its denominator; it is not isolated query QPS.

Updated scripts and concurrency options apply to newly started runs only.
Existing runs are not upgraded in place; runs do not resume or merge samples.

Rust unit tests, API/Full contract regressions and frozen consumer fixtures keep
their correctness and compatibility responsibilities. The fixed-generator
kernel/scaling harnesses below retain independent oracles and reproducible
1M/10M scale studies. A business run marked COMPLETE or faster cannot replace
those checks or serve as its own answer oracle.

The Consumer contracts workflow also runs
[business_native_smoke.py](business_native_smoke.py) after the Rust suites, using
the built native binary and a [static directory fixture](../fixtures/business-baseline/README.md).
The smoke launches the shared business driver exactly once with jobs 2, one
build/materialization sample, and one warm-up plus one measured read per case.
Its fixed independent oracle checks nine explicit facts, two inferred facts,
closure COUNT eleven, exact ordered RDF terms, ASK true, and one Shape with
exactly two expected targets. The CONSTRUCT probe checks execution only; it is
not a constructed-graph answer oracle. All responses and an independent
verification record remain in the work directory. This is functional CI with
no performance threshold or speedup claim; the smoke has no separate engine
execution loop.

## Fixed generator harness

`benchmarks/kernel_baseline.py` measures one already built binary. It never
builds Rust or starts the historical 100 million fact suites. Keep compilation,
tests and other heavy processes out of the timed run.

The default `--suite kernel` keeps the existing pack/Full workload and input
bytes. `--suite focused-shacl` uses the same runner for fixed-focus and multiple
explicit-target validation, with growing background and fanout controls; its
smaller stage plan is described below. Select the same suite on both sides of a
comparison.

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
  --work-dir data/kernel-work \
  --build-profile release --build-features default
```

The default `--read-transport file` also runs against the pre-optimization
API v1 binary. To measure the opt-in mounted Full transport separately:

```sh
python3 -B benchmarks/kernel_baseline.py \
  --binary target/release/ontokiln --output data/kernel-session-smoke.json \
  --sizes 100 --build-samples 1 --query-samples 1 --validation-samples 1 \
  --read-transport session --build-profile release --build-features default
```

Snapshot preconditions are explicit. The default `--snapshot-guard content`
sends `expected_content_digest`, supported by both the older `0fa3a6d` API v1
and newer binaries. It binds the artifact file inventory and contents, not the
manifest's own kind/profile/fixpoint declarations. Use `--snapshot-guard manifest`
to send both `expected_content_digest` and `expected_manifest_sha256`, preserving
the stronger full-snapshot precondition available in newer binaries. An older
binary rejects that field; the harness reports failure and never retries with a
weaker guard. The chosen mode is recorded in `configuration.snapshot_guard` and
must match for an equivalent before/after comparison. The session READY check
always additionally binds its mount's manifest, regardless of which per-request
guard fields are selected.

Session mode starts `api-session` once for each retained compiled/materialized
Full artifact. Each READY record must bind the exact dataset path, file-content
digest, manifest digest and kernel identity before any result is accepted.
One-time mount/startup latency and copy bytes are separate `*/session_mount`
stages; they are single diagnostic observations, not latency distributions.
Every successful Full read must report `snapshot_reused=true` and
`snapshot_bytes_copied=0`. Timing covers writing a request through receiving its
complete response line; JSON parsing and semantic checks stay outside it.
Workers remain isolated per request. Pack reads, compilation and materialization
always keep the separate-process path. Failure to mount never silently falls
back to the file API or restarts the session as a supposedly hot request.

Both transports use a deadline watchdog. A stuck session startup, write or read
kills its process group; closing stdin ends a healthy session and `wait4` reaps
it. Session lifetime CPU/high-water RSS is recorded once on its mount sample,
separately from the per-worker API metrics. The mount snapshot allowance equals
`--scratch-mib`; it is recorded by the command and its READY measurements. The
two retained Full snapshots can coexist during a scale. Session startup is not
amortized into request latency and the default file-API measurements do not
claim to show the benefit of snapshot reuse.

Choose a work directory on the filesystem you intend to measure; `/tmp` may be
tmpfs. The harness puts child-process scratch on the same filesystem and records
the actual type. The published [46e0fb5 baseline](../../benchmarks/kernel-baseline/README.md)
includes the raw report, build receipt and subsequent read-only planner evidence.


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
Session startup uses the same outer allowance; session shutdown has a separate
five-second watchdog.

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
diagnostic prose. Every Full query/validation request carries the compiled or
materialized artifact's file content digest; `--snapshot-guard manifest` also
carries its manifest SHA-256. Both digests are always recorded as provenance.
The default content guard keeps the same workload compatible with the older
API v1 binary without claiming that this field alone binds manifest state.
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

## Focused SHACL suite

`--suite focused-shacl` generates the same facts and frozen rule inputs as the
kernel suite, independently compiles a Full artifact, optionally mounts one
session, and runs seven validation workloads. It does not schedule pack builds,
materialization or query stages; these absent stages are not skipped successes.
Only this suite appends its extra named Shapes to the Shapes document. The
default kernel suite's input hashes and workloads remain unchanged.

Six workloads explicitly set `focus_node` to order `000000008`. Its customer,
amount and region remain fixed across 10K / 100K / 1M facts; this focus also
exists in the 100-fact smoke. The multiple-target workload omits `focus_node`
and checks the deduplicated explicit target set: orders 7, 8 and the last order.
The first two workloads reuse the existing Shapes.

| Stage | Independent expected result |
|---|---|
| `shacl/focus_core_pass` | Select `OrderShape`; one checked Shape, exactly order 8 in coverage, conforms and no violations. Its cardinality, IRI, integer and numeric constraints all run. |
| `shacl/focus_core_fail` | Select `MinCountProperty`; its declared target is order 0, but the explicit focus must override it. Order 8 has one amount, so `minCount 2` yields exactly one violation with the expected focus, amount path and named property shape. |
| `shacl/focus_nonlocal_fail` | Select `FocusedScopeShape`; the two-hop `customer/region` path must reach region 0 and satisfy its `hasValue`. A SELECT constraint finds the unrelated `"Order 0"@en` label and yields exactly one SPARQL violation on order 8. |
| `shacl/focus_path_fail` | Select `FocusedPathShape`; two properties traverse the same `customer/region` path. Region 0 satisfies the positive `hasValue`; region 1 is absent, producing exactly one HasValue violation on `FocusedPathMissing`. This isolates complex-path execution without a SPARQL constraint. |
| `shacl/focus_full_scan_fail` | Select `FocusedScanShape`; a subquery computes `COUNT(*)` and `SUM(STRLEN(STR(?s)))` over `?s ?p ?o`. The aggregate must match the generator's independently calculated union count and subject-character total, yielding exactly one SPARQL violation. This scans every triple while returning one aggregate row. |
| `shacl/multi_target_path_fail` | Select `MultiTargetPathShape` without overriding its targets. Traverse `customer/region` for orders 7, 8 and the last order, checking each focus against the region 0 and region 1 `hasValue` properties. Coverage and each property's violations are independently calculated. The 100-fact smoke deduplicates the last order (8), giving two targets and three violations; the standard 10K/100K/1M scales have three targets and five violations. |
| `shacl/focus_fanout_join_fail` | Select `FocusedFanoutJoinShape`. Join all region 0 customers to their orders and amounts, then join `VALUES ?copy { 1 1 }`. An aggregate checks both the exact bag count and numeric amount sum; a match yields exactly one SPARQL violation on order 8. The growing left input exercises a cost that single-focus point probes do not reveal, while duplicate VALUES make accidental deduplication observable. |

The nonlocal control requires full data visibility even with an explicit focus.
Its fixed predicate and object can use an index; it does not require a full scan.
Cropping to the focus's outgoing facts could lose the SELECT violation and add
an incorrect two-hop-path violation; checking only a violation count would miss
that error. The oracle instead compares the complete coverage and structured
violation records, including component and shape. It also checks report identity
and engine metadata, while diagnostic prose is excluded from the semantic report
digest. Violation ordering is normalized after removing prose, retaining all
duplicates and exact semantic field sets. A zero-target success, wrong focus, extra target, missing violation or
different violation cannot pass. The separate path control also rejects a single
violation on the wrong property. The full-scan control keeps the final result
small while measuring work that must grow with input size; it must not be used
to claim arbitrary global queries have constant latency or bounded intermediate
state. Its oracle reads neither engine statistics nor the produced database.
The multiple-target control checks different outcomes on different subjects,
including one near the end of the index; keeping the first focus's bindings for
later targets cannot pass its oracle. For the fanout control, region 0 selects
indices 0, 8, 16, ...; the selected amount sequence repeats every 25 customers
with a sum of 1,200. The duplicate VALUES double both row count and sum. At
10K / 100K / 1M facts, the expected pairs are `(250, 12000)`,
`(2500, 120000)` and `(25000, 1200000)`. These answers are calculated from the
generator definition and checked independently against joined small-fixture
rows in the harness tests. A bind join can improve selective paths while making
this larger-left-input workload slower; preserve either outcome. Neither this
fanout workload nor the full-scan workload has fixed work as the graph grows.
These workloads retain full-graph loading costs in versions that only optimize
Core navigation. Record actual failed warm-ups and subsequent skipped samples;
never count a skipped sample as a completed validation or compute a speedup from
a failed before-run.

Run a smoke first, then use the same command and harness revision for each
separately built before/after binary:

```sh
python3 -B benchmarks/kernel_baseline.py \
  --binary target/release/ontokiln --output data/focused-shacl-smoke.json \
  --suite focused-shacl --sizes 100 --build-samples 1 --validation-samples 1 \
  --read-transport session --snapshot-guard manifest \
  --build-profile release --build-features default

python3 -B benchmarks/kernel_baseline.py \
  --binary target/release/ontokiln --output data/focused-shacl-session.json \
  --suite focused-shacl --sizes 10000 100000 1000000 \
  --build-samples 3 --validation-samples 3 \
  --read-transport session --snapshot-guard manifest \
  --memory-mib 512 --scratch-mib 4096 --timeout-seconds 60 \
  --build-profile release --build-features default --work-dir data/bench-work
```

The primary comparison is session versus session. File requests still verify
and copy the whole database each time, so their end-to-end scaling includes
that separate cost. Keep file results separate. Each read has one warm-up and
three measured samples by default; compilation uses `--build-samples`, and
`--query-samples` has no effect in this suite. Mounts remain single diagnostic
observations. Use per-request API RSS and latency, not session-lifetime `wait4`
RSS, to assess the fixed-focus workload as background size grows.

Keep the same memory and time limits for both binaries. At 1M facts, an older
implementation may exhaust the 512 MiB budget while loading the graph. Retain
that failure and the skipped repetitions; they are not fast samples and do not
support a speedup ratio. Completion under the same budget is a separate result.
If a larger-budget latency comparison is needed, run both binaries again under
the same larger budget in separate reports, preserving the first run. Near-flat
latency/RSS over this finite ladder is evidence about these workloads, not a
proof of constant complexity or general SHACL capacity. Changing the harness
requires remeasuring both binaries; do not compare the new suite to historical
kernel reports with different method hashes.

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
subtracting those fields to invent a kernel latency. With `--read-transport file`,
Full query/validation verifies and copies the SQLite snapshot on every call.
Session mode records the mount separately and checks its reuse on every read.
Neither mode has a persistent worker cache between calls. Generation, hashing
and compilation can warm the OS cache before the first query; no sample is
called a cold-disk measurement.

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

## Comparing two reports

```sh
python3 -B benchmarks/compare_kernel_baseline.py before.json after.json \
  --format markdown --output data/kernel-comparison.md
```

The comparator checks input/query hashes, generator, method, budgets, build
declarations, transport and recorded host/filesystem configuration. Only verified,
complete measured samples enter medians. Failed/skipped/missing work or warm-ups
suppress stage percentage changes. At least three successful measured samples
are required by default; use the individual values from smoke runs only for
functional validation. File and session reports intentionally have different
timing boundaries and are rejected as equivalent configurations. Compare file
before/after to isolate internal changes, and retain separate session numbers
with their one-time mount cost to assess snapshot reuse. Matching metadata still
does not prove an idle, identical physical host or statistical significance.

Add `--format json` for machine-readable output. Engine identities may change;
build-only `ONTOKILN_BUILD_REVISION` is provenance, while runtime thread counts,
pack limits and scratch policy still affect comparability. Exit 0 means comparable
recorded conditions, 1 means non-comparable, and 2 means invalid input.

The published [46e0fb5 baseline](../../benchmarks/kernel-baseline/README.md)
used the frozen `46e0fb5` script and both snapshot digest guards. The later script
adds transport/guard configuration and changes its method hash; its default is
content-only. It must not be compared directly with that frozen report as an
unchanged method. For a new paired study use the same script on both binaries
with `--read-transport file --snapshot-guard manifest`, or preserve the original
frozen harness for both sides. Keep session mount/request observations separate.


## Focused compilation/materialization scaling

`benchmarks/kernel_scaling.py` runs only Full compilation and materialization
through the frozen `46e0fb5` harness. It verifies the harness SHA-256 before
importing it and checks both the binary SHA-256 and kernel identity against a
separately produced build receipt. It records its own driver hash and the selected
phases in the distinct `ontokiln-kernel-scaling/v1` schema. Unselected workloads
are outside this study, not silently skipped failures.

The [1M/10M report and 100M scenarios](../../benchmarks/kernel-baseline/46e0fb5-scaling.md)
include the exact command, raw measurements, build receipt, independent post-timing
SQL audit and scope limits. The 4 GiB study reruns both sizes with the same budgets;
it must not be spliced with the earlier 512 MiB report. No large generated input
or SQLite database is committed.

The comparator accepts matching scaling schemas as well as matching full-baseline
schemas. It rejects cross-schema comparisons, mismatched phase sets, missing actual
stages, or inconsistent driver/frozen-harness identities. Build-receipt hashes and
local harness paths are provenance, not runtime settings. Runtime budgets, inputs,
timing boundaries and host metadata remain part of the comparison gate. Changing
the measured engine is allowed; changing the measurement method suppresses the
performance comparison.
