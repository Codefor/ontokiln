#!/usr/bin/env python3
"""Pack v4 semantic/perf regression on the frozen Full kernel-baseline corpus.

This does NOT run a legacy engine or preserve its API; only the fixture
generator and independent, versioned expected answers are reused.
Release build must be performed before invoking this script.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time

from kernel_baseline import generate, query_cases, records_for


def invoke(binary, args, timeout):
    start = time.perf_counter()
    completed = subprocess.run(
        [str(binary), *map(str, args)],
        text=True, capture_output=True, timeout=timeout, check=False,
        env={**os.environ, "RAYON_NUM_THREADS": os.environ.get("RAYON_NUM_THREADS", "1")},
    )
    elapsed = time.perf_counter() - start
    if completed.returncode:
        raise RuntimeError(
            f"ontokiln exit={completed.returncode}: {completed.stderr[-2500:]}"
        )
    return completed.stdout, elapsed


def verify_rows(actual, case):
    if actual.get("head", {}).get("vars") != case["variables"]:
        raise AssertionError(f"{case['name']}: wrong SELECT projection")
    found = actual.get("results", {}).get("bindings")
    if found != case["rows"]:
        raise AssertionError(
            f"{case['name']}: expected {len(case['rows'])} rows; "
            f"got {len(found or [])}, expected first={case['rows'][:2]}, "
            f"actual first={(found or [])[:2]}"
        )
    return len(found)


def execute(binary: Path, facts: int, with_shapes: bool, timeout: float):
    with tempfile.TemporaryDirectory(prefix="ontokiln-v4-regression-") as temp:
        root = Path(temp)
        corpus = generate(root, facts, "kernel")
        pack = root / "reasoned.pack"
        build_raw, build_seconds = invoke(
            binary,
            ["build", "--sources", root / "input", "--pack", pack,
             "--profile", "rdfs-supported-v1", "--rules", root / "rules.json"],
            timeout,
        )
        stats = json.loads(build_raw)
        expected = {
            "explicit_facts": facts,
            "inferred_facts": corpus["expected_inferred_facts"],
            "quads": facts + corpus["expected_inferred_facts"],
            "format": "ontokiln-pack/v4",
        }
        for key, value in expected.items():
            if stats.get(key) != value:
                raise AssertionError(f"{key}: expected {value}, got {stats.get(key)}")
        test_cases = (query_cases(records_for(facts), facts, materialized=False) +
                      query_cases(records_for(facts), facts, materialized=True))
        query_times = {}
        for case in test_cases:
            filename = root / f"{case['name']}.rq"
            filename.write_text(case["query"], encoding="utf8")
            raw, seconds = invoke(binary, [
                "query", "--pack", pack, "--view", case["view"],
                "--query", filename,
            ], timeout)
            result = json.loads(raw)
            verify_rows(result, case)
            query_times[case["name"]] = {"seconds": round(seconds, 6),
                                         "rows": len(case["rows"])}
        validation_seconds = None
        validation = None
        if with_shapes:
            raw, validation_seconds = invoke(binary, [
                "validate", "--pack", pack, "--shapes", root / "shapes.ttl",
            ], timeout)
            report = json.loads(raw)
            if report["checked_shapes"] < 4:
                raise AssertionError("missing SHACL shapes")
            if report["conforms"] is not False or report["violations"] < 1:
                raise AssertionError("SHACL minCount negative control not enforced")
            validation = {"checked_shapes": report["checked_shapes"],
                          "conforms": report["conforms"],
                          "violations": report["violations"],
                          "seconds": round(validation_seconds, 6)}
        return {
            "schema_version": "ontokiln-v4-frozen-regression/v1",
            "generator": corpus["generator"],
            "facts": facts,
            "profile": "rdfs-supported-v1",
            "custom_rules": 3,
            "mode": "release",
            "status": "PASS",
            "build_and_save_seconds": round(build_seconds, 6),
            "pack_bytes": pack.stat().st_size,
            "explicit_facts": stats["explicit_facts"],
            "inferred_facts": stats["inferred_facts"],
            "queries": query_times,
            "shacl": validation,
            "host_cpu_threads": os.cpu_count(),
            "rayon_threads": os.environ.get("RAYON_NUM_THREADS", "1"),
        }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--facts", type=int, default=100_000)
    parser.add_argument("--timeout", type=float, default=900)
    parser.add_argument("--shacl", action="store_true")
    args = parser.parse_args()
    assert args.facts >= 100 and args.facts % 10 == 0
    result = execute(args.binary.resolve(), args.facts, args.shacl, args.timeout)
    print(json.dumps(result, sort_keys=True, indent=2))


if __name__ == "__main__":
    main()
