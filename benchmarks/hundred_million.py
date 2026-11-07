#!/usr/bin/env python3
"""One 100,000,000-triple v3 pack, the same scan budget, and checked controls."""
import argparse
from contextlib import closing
import hashlib
import http.client
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

import capabilities as cap
import run as bench

PANELS = 22_471_664
TRIPLES = 100_000_000


def offline_rules(pack):
    # Independent oracle for the argument slots in the checked-in synthetic Go model.
    edges = {
        "FormatAddress": ["Order.city", "Order.detail"], "AddressVM.text": ["FormatAddress"],
        "Summary.address": ["AddressVM.text"], "PickupCard.title": ["Shop.name"],
        "DeliveryPanel.text": ["BadgeHook"], "PrependBadge": ["Campaign.badge", "SelectBuilder"],
        "PartnerText": ["Order.alias"], "PostcodeText": ["Order.postcode"],
    }
    expected = set()

    def add(s, p, o):
        ns = "urn:ontokiln:lab:"
        expected.add(f"<{ns}{s}> <{ns}{p}> <{ns}{o}> .".encode())

    for src, inputs in edges.items():
        for target in inputs:
            add(src, "input", target)
        pending = list(inputs)
        while pending:
            target = pending.pop()
            add(src, "upstream", target)
            pending.extend(edges.get(target, []))
    rows = []
    for engine in ("datafrog", "ascent", "ascent", "datafrog"):
        start = time.perf_counter()
        out = subprocess.check_output([bench.BIN, "rules", "--pack", pack, "--program",
                                      bench.ROOT / "examples/rules/lineage.json", "--engine", engine],
                                     stderr=subprocess.PIPE, timeout=180)
        elapsed = time.perf_counter() - start
        lines = out.splitlines()
        if len(lines) != len(expected) or set(lines) != expected:
            raise RuntimeError("incorrect offline closure")
        rows.append({"engine": engine, "seconds": elapsed, "derived": len(lines),
                     "output_sha256": hashlib.sha256(out).hexdigest()})
    return rows


def measure(root, compile_stats):
    pack, templates = root / "panels.pack", root / "queries"
    expected = cap.expected_bitmap(PANELS)
    result = dict(bench.environment(), pack_limit_mib=int(os.environ["ONTOKILN_MAX_PACK_MIB"]),
                  subjects=PANELS, triples=TRIPLES, compile=compile_stats,
                  ttl_bytes=sum(p.stat().st_size for p in (root / "ttl").glob("*.ttl")),
                  pack_bytes=pack.stat().st_size, bitmap_total=expected["total"],
                  startups=[], bitmap_first_ms=[], http=[], sparql_scans=[])
    result["compiler_revision"] = result["engine_revision"]
    for trial in range(3):
        with bench.serve("pack", pack, TRIPLES, queries=templates) as (port, startup):
            with closing(http.client.HTTPConnection("127.0.0.1", port, timeout=30)) as conn:
                conn.request("GET", "/health")
                result["health"] = json.loads(conn.getresponse().read())
                ns, err = cap.bitmap_request(conn, expected)
                if err:
                    raise RuntimeError(f"first bitmap request failed: {err}")
                result["bitmap_first_ms"].append(ns / 1e6)
            if trial == 2:
                for mode, worker in [
                    ("segmented_type", bench.load_worker), ("segmented_proof", bench.load_worker),
                    ("hub", bench.load_worker), ("batch8", cap.worker),
                    ("sparql_bound", cap.worker), ("bitmap", cap.worker),
                ]:
                    print("HTTP", mode, flush=True)
                    for rep in range(3):
                        row = bench.load_test(port, PANELS, mode, 8, 4096, worker_fn=worker)
                        result["http"].append(dict(row, trial=rep + 1))
                for name, total in [("premium", len(range(0, PANELS, 4))),
                                    ("intersection", expected["total"]), ("exists", expected["total"])]:
                    result["sparql_scans"].append(cap.slow_sparql(port, name, total))
        result["startups"].append(startup)
    result["scenario"] = json.loads(subprocess.check_output([bench.LAB, "--pack", pack,
                                                             "--bench-samples", "300"]))
    result["offline_rules"] = offline_rules(pack)
    # Repeat both forms at the sizes used for the original 422 report.
    result["controls"] = [cap.scan_control(root / f"control-{n}", n) for n in (250_000, 2_500_000)]
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    os.environ.setdefault("ONTOKILN_MAX_PACK_MIB", "4096")
    with tempfile.TemporaryDirectory(prefix="ontokiln-100m-") as tmp:
        root = Path(tmp)
        directory, pack, triples, _ = cap.generate_inputs(root, PANELS)
        if triples != TRIPLES:
            raise RuntimeError("unexpected fixture size")
        report = measure(root, bench.compile_pack(directory, pack))
        bench.publish(args.output, report)


if __name__ == "__main__":
    main()
