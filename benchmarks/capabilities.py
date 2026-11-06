#!/usr/bin/env python3
"""Additional pack-v2 inference workloads. Build release with --features ascent-engine."""
import argparse
import hashlib
import heapq
import http.client
import json
from pathlib import Path
import random
import subprocess
import sys
import tempfile
import time

import run as bench

NS = bench.NS
ROUNDS = 3


def prepare(root, count):
    directory, pack = root / "ttl", root / "panels.pack"
    triples = bench.generate(directory, count)
    with (directory / "segments.ttl").open("w") as f:
        f.write(f"@prefix b: <{NS}> .\n@prefix r: <http://www.w3.org/2000/01/rdf-schema#> .\n")
        f.write("b:Premium r:subClassOf b:Commercial .\n")
        triples += 1
        for step, label in [(4, "Premium"), (5, "Badge")]:
            for i in range(0, count, step):
                f.write(f"b:v{i} a b:{label} .\n")
                triples += 1
    templates = root / "queries"
    templates.mkdir()
    for name, query in {
        "bound": "SELECT ?panel ?field ?region WHERE { ?panel b:readsField ?field ; b:region ?region }",
        "premium": "SELECT (COUNT(?panel) AS ?n) WHERE { ?panel a b:Premium }",
        "intersection": "SELECT (COUNT(?panel) AS ?n) WHERE { ?panel a b:Premium, b:Badge }",
        "exists": "SELECT (COUNT(?panel) AS ?n) WHERE { ?panel a b:Premium . FILTER EXISTS { ?panel a b:Badge } }",
    }.items():
        (templates / (name + ".rq")).write_text(f"PREFIX b: <{NS}>\n" + query)
    bench.compile_pack(directory, pack)
    return pack, triples, templates


def post(conn, path, payload):
    body = json.dumps(payload).encode()
    start = time.perf_counter_ns()
    conn.request("POST", path, body, {"Content-Type": "application/json"})
    res = conn.getresponse()
    raw = res.read()
    elapsed = time.perf_counter_ns() - start
    return elapsed, res.status, json.loads(raw)


def bitmap_request(conn, expected):
    elapsed, status, answer = post(conn, "/v1/entities", {
        "types": [NS + "Commercial", NS + "Badge"], "subclasses": True, "limit": 10})
    if status == 200 and answer != expected:
        raise RuntimeError(f"incorrect bitmap intersection: {answer}")
    return elapsed, None if status == 200 else str(status)


def expected_bitmap(count):
    hits = range(0, count, 20)
    return {"total": len(hits), "entities": heapq.nsmallest(10, (f"<{NS}v{i}>" for i in hits))}


def worker(job):
    port, count, mode, wid, samples = job
    rng = random.Random(20261106 + wid)
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
    bitmap = expected_bitmap(count) if mode == "bitmap" else None

    def request():
        if mode == "bitmap":
            return bitmap_request(conn, bitmap)
        ids = [rng.randrange(count) for _ in range(8 if mode == "batch8" else 1)]
        subjects = [NS + f"v{i}" for i in ids]
        if mode == "sparql_bound":
            elapsed, status, answer = post(conn, "/v1/sparql", {
                "template": "bound", "bindings": {"panel": f"<{subjects[0]}>"}})
            i = ids[0]
            values = {"panel": subjects[0], "field": NS + f"f{i % 4096}", "region": NS + f"r{i % 8}"}
            expected = [{key: {"type": "uri", "value": val} for key, val in values.items()}]
            if status == 200 and answer["results"]["bindings"] != expected:
                raise RuntimeError("incorrect SPARQL join")
        else:
            elapsed, status, answer = post(conn, "/v1/query/batch", {
                "subjects": subjects, "predicate": NS + "dependsOn"})
            expected = [{"results": [{"object": f"<{NS}f{i % 4096}>", "inferred": True}]} for i in ids]
            if status == 200 and answer != expected:
                raise RuntimeError("incorrect or unordered batch results")
        return elapsed, None if status == 200 else str(status)

    try:
        return bench.measure_requests(samples, request)
    finally:
        conn.close()


def slow_sparql(port, template, expected):
    # Deliberately do not heat the server for half a minute with 32 full scans.
    # Report deadlines/budget errors explicitly and return no successful latency for those trials.
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=12)
    rows = []
    try:
        for _ in range(10):
            ns, status, response = post(conn, "/v1/sparql", {"template": template})
            row = {"ms": ns / 1e6, "status": status}
            if status == 200:
                bindings = response["results"]["bindings"]
                if len(bindings) != 1 or int(bindings[0]["n"]["value"]) != expected:
                    raise RuntimeError(f"incorrect SPARQL count: {bindings}")
            else:
                row["error"] = response
            rows.append(row)
    finally:
        conn.close()
    return {"template": template, "expected_count": expected, "warmup": rows[0], "runs": rows[1:]}


def http_suite(root, count):
    pack, triples, templates = prepare(root, count)
    expected = expected_bitmap(count)
    firsts = []
    for _ in range(ROUNDS):
        with bench.serve("pack", pack, triples, queries=templates) as (port, _):
            conn = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
            try:
                # Establish the connection first. No bitmap queries have touched this process.
                conn.request("GET", "/health")
                conn.getresponse().read()
                ns, error = bitmap_request(conn, expected)
                if error:
                    raise RuntimeError(f"first bitmap request failed: {error}")
                firsts.append(ns / 1e6)
            finally:
                conn.close()
    loads = []
    with bench.serve("pack", pack, triples, queries=templates) as (port, _):
        modes = ["batch1", "batch8", "sparql_bound", "bitmap"]
        for trial in range(ROUNDS):
            for mode in modes[trial:] + modes[:trial]:
                for clients in (1, 8):
                    row = bench.load_test(port, count, mode, clients, 8192, worker_fn=worker)
                    loads.append(dict(row, trial=trial + 1))
        scans = [slow_sparql(port, "premium", len(range(0, count, 4))),
                 slow_sparql(port, "intersection", expected["total"]),
                 slow_sparql(port, "exists", expected["total"])]
    return {"subjects": count, "triples": triples, "pack_bytes": pack.stat().st_size,
            "bitmap_total": expected["total"], "bitmap_first_ms": firsts, "http": loads,
            "sparql_scans": scans}


def scan_control(root, count):
    root.mkdir()
    pack, triples, templates = prepare(root, count)
    expected = len(range(0, count, 20))
    with bench.serve("pack", pack, triples, queries=templates) as (port, _):
        scans = [slow_sparql(port, name, expected) for name in ("intersection", "exists")]
    return {"subjects": count, "triples": triples, "sparql_scans": scans}


def rule_suite(root, nodes):
    directory = root / f"chain{nodes}"
    directory.mkdir()
    edges = directory / "chain.nt"
    edges.write_text("".join(f"<urn:n{i}> <urn:edge> <urn:n{i+1}> .\n" for i in range(nodes - 1)))
    pack = root / f"chain{nodes}.pack"
    bench.compile_pack(directory, pack)
    program = root / "reach.json"
    program.write_text(json.dumps([{"head": "urn:reach", "body": ["urn:edge"]},
                                   {"head": "urn:reach", "body": ["urn:reach", "urn:edge"]}]))
    expected = {f"<urn:n{i}> <urn:reach> <urn:n{j}> .".encode()
                for i in range(nodes) for j in range(i + 1, nodes)}
    samples = {"datafrog": [], "ascent": []}
    fingerprint = None
    for rep in range(24):
        for engine in (list(samples) if rep % 2 == 0 else reversed(samples)):
            start = time.perf_counter_ns()
            out = subprocess.check_output([bench.BIN, "rules", "--pack", pack, "--program", program,
                                           "--engine", engine], stderr=subprocess.PIPE, timeout=60)
            ms = (time.perf_counter_ns() - start) / 1e6
            rows = out.splitlines()
            if len(rows) != len(expected) or set(rows) != expected:
                raise RuntimeError(f"incorrect {engine} transitive closure")
            digest = hashlib.sha256(out).hexdigest()
            if fingerprint is not None and fingerprint != digest:
                raise RuntimeError("unstable or different output across rule runs")
            fingerprint = digest
            if rep >= 3:
                samples[engine].append(ms)
    return {"nodes": nodes, "input_edges": nodes - 1, "derived_triples": len(expected),
            "output_sha256": fingerprint, "warmups_per_engine": 3, "wall_ms": samples}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--subjects", type=int, default=2_500_000)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.subjects < 1:
        parser.error("subjects must be positive")
    report = dict(bench.environment(), features=["ascent-engine"])
    with tempfile.TemporaryDirectory(prefix="ontokiln-capabilities-") as tmp:
        root = Path(tmp)
        print("Measuring v2 HTTP capabilities", file=sys.stderr, flush=True)
        report["online"] = http_suite(root, args.subjects)
        report["scan_control"] = scan_control(root / "control", max(1, args.subjects // 10))
        print("Measuring offline rule engines", file=sys.stderr, flush=True)
        report["rules"] = [rule_suite(root, nodes) for nodes in (32, 128, 256)]
    bench.publish(args.output, report)


if __name__ == "__main__":
    main()
