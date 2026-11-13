# Mounted Full sessions

`ontokiln api-session` mounts one Full artifact for repeated reads. The original
`ontokiln api --request ... --result ...` file protocol is unchanged. Both
transports execute API v1 operations in supervised workers.

Both entry points use the same `FullSnapshot` lifecycle and SQLite FD reader.
The file command owns an ephemeral instance that ends with its request; the
session owns the instance until EOF or cancellation. This is one engine with two
instance lifetimes, not separate legacy and new query implementations. Production
services should keep the instance inside their adapter while exposing unchanged
API v1 requests to application code.

```sh
ontokiln api-session --dataset /absolute/path/to/reasoned \
  --snapshot-bytes 4294967296
```

The session is a serial, process-scoped adapter supporting `capabilities`,
`prepare_query`, `query`, and `validate`. Compilation and materialization still
use the file API and produce independent immutable artifacts. To switch data,
start another session, wait for READY, direct new requests to it, and close the
old session after its in-flight request completes. No global ref or automatic
switch is introduced.

## Mount and identity

Before READY, the process verifies the artifact inventory, original manifest
bytes, frozen inputs, source-tree digest, semantic fingerprint, engine identity,
and completion. It copies `facts.sqlite` once into a private anonymous snapshot
and checks the copied bytes against the verified digest. All writable descriptors
are closed before the snapshot becomes available.

Workers inherit the read-only file descriptor. SQLite reads it through a small
read-only VFS using `pread`; each connection owns a duplicate descriptor, so
connections do not share a seek position. Temporary files use SQLite's platform
VFS. The mounted database has no writable connection, WAL, journal, or mmap.

Deleting, modifying, or replacing the original artifact after READY does not
change this view. Hot requests do not reopen the source, hash the complete
artifact, or copy its database. Descriptors are internal process resources and
cannot be supplied through API v1 JSON.

The first stdout line is a control record, not an API response:

```json
{
  "contract": "ontokiln-session/v1",
  "status": "READY",
  "operations": ["capabilities", "prepare_query", "query", "validate"],
  "dataset": "/absolute/path/to/reasoned",
  "content_digest": "<verified inventory digest>",
  "manifest_sha256": "<SHA-256 of verified original manifest bytes>",
  "kernel": {},
  "mount": {
    "elapsed_seconds": 0.0,
    "snapshot_bytes": 0,
    "copied_bytes": 0
  }
}
```

This example uses placeholders; the real record contains complete kernel
identity and measured counts. Mount failure exits nonzero with diagnostic stderr
and no READY. Consumers must verify READY before sending work. Its
`manifest_sha256` has the same raw-byte definition as API v1's
`expected_manifest_sha256`, rather than a re-serialized JSON digest.

## Requests and responses

After READY, write one complete UTF-8 API v1 JSON object per stdin line:

```json
{"contract":"ontokiln-api/v1","request_id":"read-1","operation":{"kind":"query","dataset":"/absolute/path/to/reasoned","query":"SELECT ?s WHERE { ?s ?p ?o } LIMIT 10","view":"asserted"}}
```

Each line receives one API v1 envelope on stdout, retaining RDF terms, result
completeness, error categories, kernel identity, request digest, and worker
reaping. `request_sha256` hashes the original line bytes after removing the
terminating LF or CRLF; JSON is not rewritten before hashing. EOF after a final
unterminated request still executes that request.

All dataset-bearing requests must name the fixed path. Matching uses
`std::path::absolute` from the process's unchanged working directory, not a
filesystem lookup. Use the absolute path returned by READY; symlink aliases and
alternative `..` spellings are not guaranteed to match. A different path returns
`INVALID_DATASET`. Digest preconditions bind against the mounted metadata even
after the original source disappears.

Malformed requests and domain errors return normal v1 failure envelopes; later
requests can continue. Write operations return `UNSUPPORTED_CAPABILITY`. The
engine-wide `capabilities` result keeps its original meaning; intersect it with
READY's `operations` when selecting a session operation.

Successful worker responses add `snapshot_reused: true`, `snapshot_bytes`, and
`snapshot_bytes_copied: 0` under `execution.metrics`.
`execution.integrity_checked` is true for the verified mounted snapshot. The
result payload and original file transport defaults do not change.

## Budgets and lifecycle

`--snapshot-bytes` is a separate mount disk allowance, defaulting to 4 GiB. It
must be positive; copying checks it before and during the transfer. The private
database remains allocated once for the session lifetime. Multiple sessions
require the sum of their snapshot sizes. Mounting is a startup operation outside
later request limits; applications may impose a startup deadline on the process.

Each request retains API timeout, memory, output, intermediate-result,
validation-depth, and scratch limits. Workers are killed and waited before
terminal response publication. Memory includes the current supervisor's resident
footprint; session accounting does not reuse historical supervisor high-water
marks from earlier requests. Request scratch excludes the already reserved
snapshot, including duplicate descriptors referencing its inode. Temporary files
and external Shapes snapshots remain chargeable.

One-shot requests reserve their anonymous database size inside the request's
scratch allowance before copying. The supervisor reads that reservation on both
Linux and macOS; Linux deduplicates descriptors by device/inode so the same
snapshot is not counted again by its open-file sampler. The reported peak
includes this reservation plus sampled temporary files. An interrupted copy can
therefore report its reserved size even when fewer bytes were physically copied.
Copying cannot exceed the verified reserved size; source growth after verification
is rejected as an invalid dataset. Linux can also sample other open-deleted
temporary files through `/proc`; the reservation does not provide equivalent
visibility into every SQLite temporary file on macOS.

Closing stdin drains the current request and exits. SIGTERM/SIGINT during a
request returns `CANCELLED` after worker reaping, then ends the session. Linux
retains the parent-death signal for abrupt supervisor death. Clients should
consume stdout continuously; a broken pipe ends the session after the current
worker has been reaped.

## Performance boundaries

The benchmark runner's `--read-transport session` records startup/mount separately
and checks zero database-copy bytes on each successful Full read. File transport
before/after measurements remain separate from this different timing boundary.

This adapter reuses the database image. Worker processes, SQLite page caches,
query plans, and validation caches are still request-scoped. For snapshots whose
verified input manifest establishes strict RDF parsing, Full SHACL Core reads
matching SQLite neighborhoods through the same Rudof constraint validators.
Explicit class targets traverse inverse subclass/type indexes. A fixed focus
with ordinary Core constraints no longer requires an initial full graph load.
Term ID and decoded-term caches have entry and estimated-byte bounds. The
adapter also retains at most the most recent complete subject/predicate
neighborhood, with row and estimated-byte bounds, so the composed evaluator and
Rudof can reuse one read. Oversized or incomplete neighborhoods are not cached;
reads and validation are never truncated to fit a cache. All these caches are
discarded when the worker exits and do not become a session result cache.

The data view remains the distinct SPO union of every stored graph, including
asserted and inferred facts. Complex paths and executed SHACL-SPARQL constraints
or targets lazily build the complete union query dataset, which can still consume
memory proportional to the data. This query dataset has only a default graph,
matching the existing validation semantics. Selecting a focus does not restrict
what those queries can see. Dataset-sourced Shapes also require loading the
complete Shapes graph. Legacy artifacts with unknown or lenient parser policy
retain their existing full graph and term-validation behavior.

File API validation uses this same implementation after its per-request mount.
Session validation amortizes mount verification and copying across requests;
the first mount still depends on artifact size. Full-target validation still
scales with its target and constraint work, even when ordinary Core navigation
does not materialize the complete data graph.

Target selection keeps the existing [Full contract](full.md#shacl). A shape's
`rdfs:Class` annotation does not add an implicit target; consumers continue to
declare `sh:targetClass` or provide an explicit focus.

New Full compilations include a covering SPO index matching the query reader's
pagination order. Existing compatible snapshots remain readable, but upgrading
the binary does not add an index to their immutable database. Recompile the
sources to obtain this index; materializing an older compiled image preserves
its existing indexes. Indexes remain outside the public SQLite read contract,
and new artifacts require their own content digests.

This does not establish disk-native SHACL, full OWL/SHACL compatibility, or
100 million fact Full capacity. The [baseline methodology](../tests/benchmarks/README.md)
and committed measurements define the actual workload and scale.

Real-process regressions cover source deletion/replacement, repeated reads,
digest preconditions, derived queries, validation, rejected requests, scratch
limits, timeouts, cancellation, and reaping. Native VFS tests cover short reads,
offset errors, read-only enforcement, and disk-backed temporary sorting. Linux
is tested locally; the POSIX implementation must also be validated on macOS
before claiming measured platform parity.

## Cold starts and application rollout

A new Bundle still pays full verification and one private snapshot copy. A
long-lived application should mount the replacement, run representative reads
and validation, then mark that instance ready and route new work to it. Retain
the old instance until its current request finishes. Readiness and per-Pod ref
switching belong to the application; this engine does not implement a global ref.

Every request still starts an isolated worker, opens a SQLite connection and
prepares its own query or validation state. Warm-up can populate operating-system
file pages and detect errors; it does not retain worker-local query plans or
SHACL graphs for the next worker. Replacing the binary alone therefore updates
the implementation but cannot create cross-request state when the caller still
starts and exits a process for every request. That caller must change its internal
adapter to retain an instance; its outward API v1 contract need not change.
