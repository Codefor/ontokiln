#!/usr/bin/env python3
"""Frozen two-template pack regression; imports the repository's existing harness.

Set TMPDIR to a workspace directory before running. --fixtures must have fresh
per-size subdirectories: existing inputs are never overwritten or regenerated.
Both binaries consume the one pack compiled with the before binary. Compilation
has a sampled 7.5 GiB RSS ceiling; generation/compilation are outside query timing.
"""

import argparse
import hashlib
import http.client
import importlib.util
import json
import os
from pathlib import Path
import platform
import signal
import subprocess
import sys
import time
from types import SimpleNamespace


PANELS = 22_471_664
COMPILE_RSS_LIMIT = 15 * 1024**3 // 2
POLL_SECONDS = 0.02
TEMPLATES = ("intersection", "exists")


class Interrupted(BaseException):
    def __init__(self, signum):
        self.signum = signum
        super().__init__(f"interrupted by signal {signum}")


def interrupt(signum, _frame):
    # A second signal must not bypass the kill-and-reap cleanup for owned groups.
    for sig in (signal.SIGINT, signal.SIGTERM):
        signal.signal(sig, signal.SIG_IGN)
    raise Interrupted(signum)


def utc():
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def files_receipt(directory):
    files = [{"path": str(p.relative_to(directory)), "bytes": p.stat().st_size,
              "sha256": sha256(p)}
             for p in sorted(directory.rglob("*")) if p.is_file()]
    encoded = json.dumps(files, sort_keys=True, separators=(",", ":")).encode()
    return {"files": files, "tree_sha256": hashlib.sha256(encoded).hexdigest(),
            "bytes": sum(f["bytes"] for f in files)}


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def rss_bytes(pid):
    try:
        fields = {}
        for line in Path(f"/proc/{pid}/status").read_text().splitlines():
            if line.startswith(("VmRSS:", "VmHWM:")):
                key, value, _ = line.split()
                fields[key[:-1]] = int(value) * 1024
        return fields.get("VmRSS", 0), fields.get("VmHWM", 0)
    except FileNotFoundError:
        return 0, 0


def kill_group(proc, sig=signal.SIGKILL):
    try:
        os.killpg(proc.pid, sig)
    except ProcessLookupError:
        pass


class Supervisor:
    """Adapt only Popen/reap, retaining bench.compile_pack and bench.serve."""

    def __init__(self, checkpoint):
        self.checkpoint = checkpoint
        self.active = {}
        self.kind = None
        self.metrics = None
        self.compile_stderr = None

    def popen(self, *args, **kwargs):
        kwargs["start_new_session"] = True
        if self.kind == "compile":
            kwargs["stderr"] = self.compile_stderr
        proc = subprocess.Popen(*args, **kwargs)
        self.active[proc.pid] = (proc, self.kind, self.metrics)
        self.metrics.update(pid=proc.pid, process_reaped=False)
        return proc

    def reap(self, proc):
        _, kind, metrics = self.active[proc.pid]
        started, last_checkpoint, peak = time.monotonic(), 0, 0
        try:
            while True:
                current, high = rss_bytes(proc.pid)
                peak = max(peak, current, high)
                metrics.update(last_rss_bytes=current, sampled_peak_rss_bytes=peak)
                if kind == "compile" and peak > COMPILE_RSS_LIMIT:
                    metrics.update(budget_exceeded=True, termination="RSS_LIMIT")
                    kill_group(proc)
                elif kind == "serve" and time.monotonic() - started > 5:
                    metrics["termination"] = "SHUTDOWN_KILL_AFTER_5_SECONDS"
                    kill_group(proc)
                pid, status, usage = os.wait4(proc.pid, os.WNOHANG)
                if pid:
                    proc.returncode = os.waitstatus_to_exitcode(status)
                    maximum = max(peak, usage.ru_maxrss * 1024)
                    metrics.update(returncode=proc.returncode, process_reaped=True,
                                   wait4_peak_rss_bytes=usage.ru_maxrss * 1024)
                    if kind == "compile" and maximum > COMPILE_RSS_LIMIT:
                        metrics.update(budget_exceeded=True, termination="RSS_LIMIT")
                    return maximum
                if kind == "compile" and time.monotonic() - last_checkpoint >= 2:
                    self.checkpoint()
                    last_checkpoint = time.monotonic()
                time.sleep(POLL_SECONDS)
        except BaseException:
            kill_group(proc)
            try:
                _, status, usage = os.wait4(proc.pid, 0)
                proc.returncode = os.waitstatus_to_exitcode(status)
                metrics.update(returncode=proc.returncode,
                               wait4_peak_rss_bytes=usage.ru_maxrss * 1024)
            except ChildProcessError:
                pass
            metrics["process_reaped"] = True
            raise
        finally:
            if kind == "serve":
                kill_group(proc)  # Also stop any request worker after its parent exits.
            self.active.pop(proc.pid, None)

    def cleanup(self):
        # Do not let a repeated interrupt abandon an owned child during reaping.
        previous = {sig: signal.signal(sig, signal.SIG_IGN)
                    for sig in (signal.SIGINT, signal.SIGTERM)}
        try:
            for proc, _, metrics in list(self.active.values()):
                kill_group(proc)
                if proc.returncode is None:
                    try:
                        _, status, usage = os.wait4(proc.pid, 0)
                        proc.returncode = os.waitstatus_to_exitcode(status)
                        metrics["wait4_peak_rss_bytes"] = usage.ru_maxrss * 1024
                    except ChildProcessError:
                        pass
                metrics.update(returncode=proc.returncode, process_reaped=True)
            self.active.clear()
        finally:
            for sig, previous_handler in previous.items():
                signal.signal(sig, previous_handler)


class RecordingConnection:
    """Keep cap.post's timing while retaining non-JSON HTTP failure bodies."""

    def __init__(self, port):
        self.connection = http.client.HTTPConnection("127.0.0.1", port, timeout=15)
        self.status, self.raw = None, b""

    def request(self, *args, **kwargs):
        self.status, self.raw = None, b""
        return self.connection.request(*args, **kwargs)

    def getresponse(self):
        response = self.connection.getresponse()
        self.status = response.status
        try:
            self.raw = response.read()
        except http.client.IncompleteRead as exc:
            self.raw = exc.partial
            raise
        return SimpleNamespace(status=response.status, read=lambda: self.raw)

    def close(self):
        self.connection.close()


def request_record(cap, connection, record, count, candidates):
    payload = {"template": record["template"], "deadline_ms": 5000,
               "options": {"max_visits": 200_000_000}}
    if record["phase"] == "counters":
        payload["options"]["counters"] = True
    started = time.perf_counter_ns()
    try:
        elapsed, status, body = cap.post(connection, "/v1/sparql", payload)
        record.update(elapsed_ns=elapsed, http_status=status,
                      timing="capabilities.post_request_through_body_read")
        if status != 200:
            record.update(status="HTTP_ERROR", failure_body=body)
            return
        result = body["result"] if record["phase"] == "counters" else body
        rows = result["results"]["bindings"]
        if len(rows) != 1:
            raise ValueError(f"expected one count row, got {len(rows)}")
        term = rows[0]["n"]
        actual = int(term["value"])
        record["actual_count"] = actual
        if (actual != count or term.get("type") != "literal"
                or term.get("datatype") != "http://www.w3.org/2001/XMLSchema#integer"):
            raise ValueError("count oracle mismatch")
        if record["phase"] == "counters":
            record["counter_body"] = body
            record["candidate_rows"] = body["profile"]["candidate_rows"]
            if record["candidate_rows"] != candidates:
                raise ValueError("candidate-row oracle mismatch")
        record.update(status="COMPLETE", oracle_checked=True)
    except Exception as exc:
        record.setdefault("elapsed_ns", time.perf_counter_ns() - started)
        record.setdefault("timing", "outer_timer_for_transport_or_decode_failure")
        record.update(http_status=connection.status,
                      status="ORACLE_FAILED" if connection.status == 200 else "TRANSPORT_ERROR",
                      error=f"{type(exc).__name__}: {exc}",
                      failure_body=connection.raw.decode("utf-8", errors="replace"))
        connection.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("repo", "before-binary", "after-binary", "fixtures", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--sizes", nargs="+", type=int, default=[250_000, PANELS])
    args = parser.parse_args()
    if not args.sizes or min(args.sizes) < 1 or max(args.sizes) > PANELS:
        parser.error("sizes must be positive panel counts no larger than 22471664")
    if len(set(args.sizes)) != len(args.sizes):
        parser.error("sizes must be distinct")
    if sys.platform != "linux":
        parser.error("this RSS-limited wrapper requires Linux /proc and wait4")
    args.repo, args.fixtures, args.output = (p.resolve() for p in (args.repo, args.fixtures, args.output))
    if args.output.exists() or args.output.is_symlink():
        parser.error("output must be new; existing raw reports are never overwritten")
    protected = {args.before_binary.resolve(), args.after_binary.resolve(), Path(__file__).resolve(),
                 args.repo / "benchmarks/run.py", args.repo / "benchmarks/capabilities.py"}
    if args.output in protected:
        parser.error("output must not replace a binary or harness source")
    if any(args.output == args.fixtures / str(size)
           or args.fixtures / str(size) in args.output.parents for size in args.sizes):
        parser.error("output must be outside the per-size fixture directories")
    tmp = Path(os.environ.get("TMPDIR", "")).resolve()
    if not os.environ.get("TMPDIR") or Path("/workspace") not in tmp.parents or not tmp.is_dir():
        parser.error("set TMPDIR to an existing directory under /workspace")
    os.environ.update(RAYON_NUM_THREADS="1", TOKIO_WORKER_THREADS="1",
                      ONTOKILN_MAX_PACK_MIB="4096")
    bench = load_module("run", args.repo / "benchmarks/run.py")
    cap = load_module("capabilities", args.repo / "benchmarks/capabilities.py")
    report = {"schema_version": "ontokiln-indexed-regression/v1", "status": "RUNNING",
              "started_at": utc(), "method": {"panel_sizes": args.sizes,
              "templates": list(TEMPLATES), "warmups": 1, "measurements": 9, "counter_requests": 1,
              "deadline_ms": 5000, "max_visits": 200_000_000,
              "max_visits_source": "explicit options.max_visits; same as engine MAX_VISITS",
              "compile_rss_limit_bytes": COMPILE_RSS_LIMIT, "rss_poll_seconds": POLL_SECONDS,
              "compile_limit_scope": "compiler process RSS, threads included; sampled then kill and wait4",
              "threads": 1, "tmpdir": str(tmp), "fixture_root": str(args.fixtures),
              "compile_binary": "before", "pack_shared_between_binaries": True,
              "query_order": "before then after; intersection then exists",
              "os_cache_flushed": False, "build_features_caller_declaration": "default",
              "build_profile_caller_declaration": "release"},
              "environment": {"platform": platform.platform(), "python": platform.python_version(),
              "logical_cpus": os.cpu_count(), "physical_memory_bytes": os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE"),
              "rustc": None}, "harness": [], "binaries": {}, "datasets": []}

    def checkpoint():
        report["checkpoint_at"] = utc()
        bench.publish(args.output, report)

    supervisor = Supervisor(checkpoint)
    original_subprocess, original_reap = bench.subprocess, bench.reap
    bench.subprocess = SimpleNamespace(Popen=supervisor.popen,
                                      check_output=subprocess.check_output,
                                      DEVNULL=subprocess.DEVNULL, PIPE=subprocess.PIPE)
    bench.reap = supervisor.reap
    handlers = {sig: signal.signal(sig, interrupt)
                for sig in (signal.SIGINT, signal.SIGTERM)}
    exit_code = 1
    try:
        checkpoint()
        for path in (Path(__file__), args.repo / "benchmarks/run.py", args.repo / "benchmarks/capabilities.py",
                     args.repo / "Cargo.lock", args.repo / "rust-toolchain.toml",
                     args.repo / "crates/reasoner/src/sparql.rs"):
            report["harness"].append({"path": str(path), "bytes": path.stat().st_size, "sha256": sha256(path)})
        report["environment"]["rustc"] = subprocess.check_output(["rustc", "-vV"], text=True, timeout=30).strip()
        report["environment"]["repository_head"] = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=args.repo, text=True).strip()
        report["environment"]["repository_status"] = subprocess.check_output(["git", "status", "--porcelain"], cwd=args.repo, text=True).strip()
        for role, supplied in (("before", args.before_binary), ("after", args.after_binary)):
            binary = supplied.resolve()
            identity = subprocess.check_output([str(binary), "identity"], stderr=subprocess.PIPE, timeout=30)
            report["binaries"][role] = {"path": str(binary), "bytes": binary.stat().st_size,
                "sha256": sha256(binary), "identity": json.loads(identity),
                "features_caller_declaration": "default", "profile_caller_declaration": "release"}
        checkpoint()
        args.fixtures.mkdir(parents=True, exist_ok=True)
        for size in args.sizes:
            expected = len(range(0, size, 20))
            candidates = len(range(0, size, 4)) + expected
            triples = 1095 + 4 * size + len(range(0, size, 4)) + len(range(0, size, 5))
            if size in (250_000, PANELS):
                assert (expected, candidates) == {250_000: (12500, 75000), PANELS: (1123584, 6741500)}[size]
            if size == PANELS:
                assert triples == 100_000_000
            root = args.fixtures / str(size)
            dataset = {"panels": size, "expected_triples": triples, "expected_count": expected,
                       "expected_candidate_rows": candidates, "directory": str(root),
                       "status": "RUNNING", "generation": {"status": "PENDING"},
                       "compile": {"status": "PENDING"}, "engines": []}
            for role in ("before", "after"):
                records = [{"template": name, "phase": phase, "sample": index,
                            "status": "PENDING", "http_status": None, "actual_count": None,
                            "oracle_checked": False}
                           for name in TEMPLATES for phase, index in
                           [("warmup", 0)] + [("measured", i) for i in range(1, 10)] + [("counters", 0)]]
                dataset["engines"].append({"role": role, "status": "PENDING", "startup": {}, "records": records})
            report["datasets"].append(dataset)
            checkpoint()
            stage = dataset["generation"]
            started = time.perf_counter()
            try:
                stage.update(status="RUNNING", started_at=utc())
                checkpoint()
                root.mkdir()  # Refuse to overwrite complete or interrupted prior fixtures.
                directory, pack, actual_triples, templates = cap.generate_inputs(root, size)
                if actual_triples != triples:
                    raise ValueError("fixture triple-count formula mismatch")
                stage.update(status="COMPLETE", seconds=time.perf_counter() - started)
                checkpoint()
                dataset["fixture"] = files_receipt(directory)
                dataset["query_assets"] = files_receipt(templates)
                stage = dataset["compile"]
                stage.update(status="RUNNING", started_at=utc(), budget_exceeded=False)
                started = time.perf_counter()
                supervisor.kind, supervisor.metrics = "compile", stage
                bench.BIN = Path(report["binaries"]["before"]["path"])
                if sha256(bench.BIN) != report["binaries"]["before"]["sha256"]:
                    raise ValueError("compiler binary changed after identity capture")
                checkpoint()
                started = time.perf_counter()
                stderr_path = root / "compile.stderr"
                stage["stderr_path"] = str(stderr_path)
                try:
                    with stderr_path.open("wb") as stderr:
                        supervisor.compile_stderr = stderr
                        stage.update(bench.compile_pack(directory, pack))
                finally:
                    supervisor.compile_stderr = None
                    stage["seconds"] = time.perf_counter() - started
                    stage["stderr_sha256"] = sha256(stderr_path)
                    stage["stderr_bytes"] = stderr_path.stat().st_size
                    with stderr_path.open("rb") as stderr:
                        stderr.seek(max(0, stage["stderr_bytes"] - 4096))
                        stage["stderr_tail"] = stderr.read().decode("utf-8", errors="replace")
                if stage["budget_exceeded"]:
                    raise RuntimeError("compile exceeded sampled RSS budget")
                stage["status"] = "COMPLETE"
                dataset["pack"] = {"path": str(pack), "bytes": pack.stat().st_size, "sha256": sha256(pack)}
                checkpoint()
            except Exception as exc:
                stage.update(status="RESOURCE_EXHAUSTED" if stage.get("budget_exceeded") else "FAILED",
                             error=f"{type(exc).__name__}: {exc}", seconds=time.perf_counter() - started)
                dataset["status"] = stage["status"]
                checkpoint()
                continue
            for engine in dataset["engines"]:
                engine["status"] = "RUNNING"
                supervisor.kind, supervisor.metrics = "serve", engine["startup"]
                bench.BIN = Path(report["binaries"][engine["role"]]["path"])
                startup_started = time.perf_counter()
                checkpoint()
                try:
                    if sha256(bench.BIN) != report["binaries"][engine["role"]]["sha256"]:
                        raise ValueError("binary changed after identity capture")
                    with bench.serve("pack", pack, triples, queries=templates, include_pid=True) as (port, startup):
                        engine["startup"]["metrics"] = startup
                        engine["startup"]["status"] = "READY"
                        checkpoint()
                        connection = RecordingConnection(port)
                        try:
                            connection.request("GET", "/v1/identity")
                            response = connection.getresponse()
                            identity = json.loads(response.read())
                            engine["runtime_identity_http_status"] = response.status
                            engine["runtime_identity"] = identity
                            if (response.status != 200
                                    or identity.get("kernel") != report["binaries"][engine["role"]]["identity"]
                                    or identity.get("pack_sha256") != dataset["pack"]["sha256"]
                                    or identity.get("triples") != triples):
                                raise ValueError("runtime binary/pack identity mismatch")
                            checkpoint()
                            for record in engine["records"]:
                                record.update(status="RUNNING", started_at=utc())
                                checkpoint()
                                request_record(cap, connection, record, expected, candidates)
                                checkpoint()
                        finally:
                            connection.close()
                    engine["status"] = "COMPLETE" if all(r["status"] == "COMPLETE" for r in engine["records"]) else "FAILED"
                except Exception as exc:
                    engine.update(status="FAILED", error=f"{type(exc).__name__}: {exc}")
                    engine["startup"].setdefault("failed_after_seconds", time.perf_counter() - startup_started)
                checkpoint()
            dataset["status"] = "COMPLETE" if all(e["status"] == "COMPLETE" for e in dataset["engines"]) else "FAILED"
            checkpoint()
        report["status"] = "COMPLETE" if all(d["status"] == "COMPLETE" for d in report["datasets"]) else "FAILED"
        exit_code = 0 if report["status"] == "COMPLETE" else 1
    except Interrupted as exc:
        report.update(status="INTERRUPTED", error=str(exc), signal=exc.signum)
        exit_code = 128 + exc.signum
    except BaseException as exc:
        report.update(status="FAILED", error=f"{type(exc).__name__}: {exc}")
    finally:
        supervisor.cleanup()
        for dataset in report["datasets"]:
            items = [dataset, dataset["generation"], dataset["compile"]]
            for engine in dataset["engines"]:
                items.extend([engine, *engine["records"]])
            for item in items:
                if item.get("status") in ("PENDING", "RUNNING"):
                    item.update(status="INTERRUPTED" if item["status"] == "RUNNING" and report["status"] == "INTERRUPTED" else "SKIPPED",
                                skip_reason="parent stage failed or run interrupted; no successful latency inferred")
        report["finished_at"] = utc()
        checkpoint()
        bench.subprocess, bench.reap = original_subprocess, original_reap
        for sig, previous_handler in handlers.items():
            signal.signal(sig, previous_handler)
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
