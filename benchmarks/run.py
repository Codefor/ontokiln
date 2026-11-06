#!/usr/bin/env python3
"""Benchmark the release CLI, HTTP API and scenario lab on macOS or Linux."""
import argparse
from concurrent.futures import ProcessPoolExecutor
from contextlib import contextmanager
import http.client
import json
import math
import multiprocessing
import os
from pathlib import Path
import platform
import random
import re
import shutil
import subprocess
import sys
import tempfile
import time
from urllib.parse import urlencode

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / "target/release/ontokiln"
LAB = ROOT / "target/release/examples/scenario_lab"
FIXTURE = ROOT / "examples/scenarios/ontology"
NS = "urn:ontokiln:bench:"
TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
FANOUT = 1024
ROUNDS = 3
START_GATE = None


def generate(directory, subjects):
    shutil.copytree(FIXTURE, directory)
    with (directory / "panels.ttl").open("w", encoding="utf8") as f:
        f.write(f"@prefix b: <{NS}> .\n@prefix r: <http://www.w3.org/2000/01/rdf-schema#> .\n")
        f.write("b:Panel r:subClassOf b:View . b:View r:subClassOf b:Component .\n")
        f.write("b:readsField r:subPropertyOf b:dependsOn .\n")
        for i in range(FANOUT):
            f.write(f"b:hub b:readsField b:f{i} .\n")
        for i in range(subjects):
            f.write(f"b:v{i} a b:Panel ; b:readsField b:f{i % 4096} ; "
                    f"b:usesBuilder b:impl{i % 64} ; b:region b:r{i % 8} .\n")
    return 67 + 3 + FANOUT + 4 * subjects


def reap(proc):
    _, status, usage = os.wait4(proc.pid, 0)
    proc.returncode = os.waitstatus_to_exitcode(status)
    return usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024)


def compile_pack(directory, pack):
    start = time.perf_counter()
    proc = subprocess.Popen([BIN, "compile", "--ttl", directory, "--out", pack],
                            stdout=subprocess.DEVNULL)
    rss_bytes = reap(proc)
    elapsed = time.perf_counter() - start
    if proc.returncode:
        raise RuntimeError(f"compile failed: {proc.returncode}")
    return {"seconds": elapsed, "peak_rss_bytes": rss_bytes}


@contextmanager
def serve(source, path, triples, queries=None):
    start = time.perf_counter()
    metrics = {}
    args = [BIN, "serve", f"--{source}", path, "--listen", "127.0.0.1:0"]
    if queries is not None:
        args.extend(["--queries", queries])
    proc = subprocess.Popen(args,
                            stderr=subprocess.PIPE, text=True)
    try:
        last_line = ""
        for line in proc.stderr:
            last_line = line.strip()
            match = re.search(r"serving .* at http://127\.0\.0\.1:(\d+)", line)
            if match:
                break
        else:
            raise RuntimeError(f"server exited before bind ({proc.wait()}): {last_line}")
        port = int(match[1])
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
        conn.request("GET", "/health")
        res = conn.getresponse()
        data = json.loads(res.read())
        elapsed = time.perf_counter() - start
        conn.close()
        if (res.status, data["source"], data["triples"], data["pack_version"]) != (200, source, triples, 2):
            raise RuntimeError(f"unexpected health: {data}")
        rss = int(subprocess.check_output(["ps", "-o", "rss=", "-p", str(proc.pid)])) * 1024
        metrics.update(ready_seconds=elapsed, ready_rss_bytes=rss, terms=data["terms"])
        yield port, metrics
    finally:
        if proc.returncode is None:
            proc.terminate()
            metrics["peak_rss_bytes"] = reap(proc)
        proc.stderr.close()


def init_client(gate):
    global START_GATE
    START_GATE = gate


def load_worker(job):
    port, subjects, mode, worker, n = job
    rng = random.Random(20261106 + worker)
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
    wide = mode == "hub"
    proof = mode == "type_proof"
    predicate = NS + "dependsOn" if wide else TYPE
    expected = ({f"<{NS}f{i}>" for i in range(FANOUT)} if wide else
                {f"<{NS}{name}>" for name in ("Panel", "View", "Component")})

    def request():
        subject = NS + ("hub" if wide else f"v{rng.randrange(subjects)}")
        path = "/v1/query?" + urlencode({"subject": subject, "predicate": predicate,
                                         "explain": str(proof).lower()})
        start = time.perf_counter_ns()
        conn.request("GET", path)
        res = conn.getresponse()
        body = res.read()
        elapsed = time.perf_counter_ns() - start
        if res.status != 200:
            return elapsed, str(res.status)
        rows = json.loads(body)["results"]
        if len(rows) != len(expected) or {r["object"] for r in rows} != expected:
            raise RuntimeError("incorrect objects")
        for row in rows:
            direct = row["object"] == f"<{NS}Panel>"
            if row["inferred"] == direct:
                raise RuntimeError("incorrect inference status")
            if proof:
                steps = row["proof"]
                depth = {f"<{NS}Panel>": 1, f"<{NS}View>": 2, f"<{NS}Component>": 3}
                if (len(steps) != depth[row["object"]] or steps[0]["premise"][0] != f"<{subject}>"
                        or steps[-1]["premise"][2] != row["object"]):
                    raise RuntimeError("incorrect proof")
        return elapsed, None

    try:
        return measure_requests(n, request)
    finally:
        conn.close()


def measure_requests(n, request):
    samples, errors = [], {}
    try:
        for _ in range(32):
            _, error = request()
            if error:
                raise RuntimeError(f"warmup HTTP {error}")
        START_GATE.wait(timeout=60)
        START_GATE.wait(timeout=60)  # Parent releases the timed phase.
        for _ in range(n):
            elapsed, error = request()
            if error:
                errors[error] = errors.get(error, 0) + 1
            else:
                samples.append(elapsed)
        START_GATE.wait(timeout=180)
        return samples, errors
    except BaseException:
        START_GATE.abort()
        raise


def load_test(port, subjects, mode, clients, requests, worker_fn=load_worker):
    ctx = multiprocessing.get_context("spawn")
    gate = ctx.Barrier(clients + 1)
    jobs = [(port, subjects, mode, i, requests // clients) for i in range(clients)]
    with ProcessPoolExecutor(clients, mp_context=ctx, initializer=init_client, initargs=(gate,)) as pool:
        futures = [pool.submit(worker_fn, job) for job in jobs]
        gate.wait(timeout=60)  # Workers have warm persistent connections.
        start = time.perf_counter()
        gate.wait(timeout=60)
        gate.wait(timeout=180)
        seconds = time.perf_counter() - start
        runs = [f.result() for f in futures]
    samples = sorted(t for latencies, _ in runs for t in latencies)
    errors = {}
    for _, counts in runs:
        for status, count in counts.items():
            errors[status] = errors.get(status, 0) + count
    if len(samples) + sum(errors.values()) != requests or not samples:
        raise RuntimeError("incomplete workload")
    result = {"mode": mode, "clients": clients, "requests": requests, "warmup_per_client": 32,
              "errors": errors, "seconds": seconds, "ok_per_second": len(samples) / seconds}
    for p in (50, 95, 99):
        result[f"p{p}_ms"] = samples[math.ceil(p * len(samples) / 100) - 1] / 1e6
    return result


def benchmark(root, subjects, extended):
    directory, pack = root / "ttl", root / "bench.pack"
    triples = generate(directory, subjects)
    result = {"subjects": subjects, "triples": triples,
              "ttl_bytes": sum(p.stat().st_size for p in directory.glob("*.ttl")),
              "compile": [], "ttl_start": [], "pack_start": [], "http": []}
    for _ in range(ROUNDS):
        result["compile"].append(compile_pack(directory, pack))
        for source, path in [("ttl", directory), ("pack", pack)]:
            with serve(source, path, triples) as (_, startup):
                result[source + "_start"].append(startup)
    result["pack_bytes"] = pack.stat().st_size
    with serve("pack", pack, triples) as (port, _):
        workloads = [("type", 1, 32768), ("type", 8, 65536)]
        if extended:
            workloads += [("type_proof", 8, 65536), ("hub", 8, 8192)]
        for mode, clients, requests in workloads:
            result["http"].append(load_test(port, subjects, mode, clients, requests))
    result["scenario"] = json.loads(subprocess.check_output([LAB, "--pack", pack, "--bench-samples", "300"]))
    return result


def view_scaling():
    # V8 scans the registered views. The added views read Shop.name, so the
    # expected impact of Order.city is identical at all three sizes.
    rows = []
    for count in (3, 30, 300):
        with tempfile.TemporaryDirectory(prefix="ontokiln-views-") as tmp:
            directory = Path(tmp) / "ttl"
            shutil.copytree(FIXTURE, directory)
            with (directory / "extra.ttl").open("w", encoding="utf8") as f:
                f.write("@prefix x: <urn:ontokiln:lab:> .\n")
                for i in range(count - 3):
                    f.write(f"x:Views x:field x:Extra{i} . x:Extra{i} a x:ComponentProperty ; "
                            'x:arg0 x:Shop.name ; x:evidence "checkout.go#pickupTitle" .\n')
            for case in ("V4", "V8"):
                timing = json.loads(subprocess.check_output([
                    LAB, "--ttl", directory, "--bench-samples", "300", "--case", case]))
                rows.append({"registered_views": count, "triples": 67 + 4 * (count - 3), **timing})
    return rows


def environment():
    report = {"utc_date": time.strftime("%Y-%m-%d", time.gmtime()), "os": platform.platform(),
              "python": platform.python_version(), "logical_cpus": os.cpu_count(),
              "memory_bytes": os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE"),
              "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
              "engine_revision": subprocess.check_output(["git", "log", "-1", "--format=%H", "--", "src", "crates"],
                                                         cwd=ROOT, text=True).strip(),
              "profile": "release", "pack_version": 2}
    if sys.platform == "darwin":
        report["cpu"] = subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip()
    return report


def publish(path, report):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=path.parent) as staging:
        result = Path(staging) / "result.json"
        result.write_text(json.dumps(report, indent=2) + "\n")
        result.replace(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--subjects", type=int, nargs="+", default=[25_000, 250_000, 2_500_000])
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if min(args.subjects) < 1:
        parser.error("subject counts must be positive")
    report = dict(environment(), datasets=[])
    for n in args.subjects:
        with tempfile.TemporaryDirectory(prefix="ontokiln-bench-") as tmp:
            print(f"Benchmarking {n:,} subjects", file=sys.stderr, flush=True)
            report["datasets"].append(benchmark(Path(tmp), n, n == max(args.subjects)))
    report["view_scaling"] = view_scaling()
    publish(args.output, report)


if __name__ == "__main__":
    main()
