#!/usr/bin/env python3
"""Pack v4 frozen-corpus *resident* functional and performance regression.

The generator and all SPARQL/SHACL expected results are independent of the
new Rust executor. The Rust example keeps one read-only engine alive through
warmups and measured requests; no CLI pack-load cost is hidden in query latency.
"""
import argparse
import hashlib
import json
import os
import platform
import re
from pathlib import Path
import subprocess
import sys
import tempfile

from kernel_baseline import GENERATOR, NS, generate, query_cases, records_for


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False,
                      separators=(",", ":")).encode("utf-8")


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def suite_digest(corpus, rules, selected, shapes_bytes):
    return hashlib.sha256(canonical({
        "generator": corpus["generator"], "source": corpus["files"],
        "profile": "rdfs-supported-v1", "rules": rules,
        "queries": selected,
        "shapes_sha256": hashlib.sha256(shapes_bytes).hexdigest(),
    })).hexdigest()


def compare_reports(before, after):
    for key in ("schema_version", "generator", "facts", "profile", "rule_count",
                "source_digest", "machine", "build_mode", "rayon_threads",
                "measurement"):
        require(key in before and key in after and before[key] == after[key],
                f"not comparable: missing or differing {key}")
    before_build = before.get("benchmark", {}).get("build_identity", {})
    after_build = after.get("benchmark", {}).get("build_identity", {})
    for key in ("rustc", "target", "profile", "opt_level"):
        require(key in before_build and key in after_build
                and before_build[key] == after_build[key],
                f"not comparable: missing or differing build_identity.{key}")
    before_metrics = flatten_metrics(before)
    after_metrics = flatten_metrics(after)
    require(set(before_metrics) == set(after_metrics),
            "not comparable: different query or measurement suites")
    comparison = {}
    for metric, previous in sorted(before_metrics.items()):
        current = after_metrics[metric]
        comparison[metric] = {
            "before_ms": round(previous, 3),
            "after_ms": round(current, 3),
            "ratio": round(current / previous, 4) if previous > 0 else None,
        }
    return comparison


def annotate_comparison(output, before, max_regression_ratio):
    if max_regression_ratio is not None:
        require(max_regression_ratio >= 1.0,
                "max regression ratio must be >= 1.0")
    comparison = compare_reports(before, output)
    output["comparison"] = comparison
    failed = sorted(name for name, value in comparison.items()
                    if max_regression_ratio is not None
                    and value["ratio"] is not None
                    and value["ratio"] > max_regression_ratio)
    if failed:
        output["status"] = "FAIL"
        output["regression_failures"] = failed
    return failed


def flatten_metrics(report):
    metrics = {f"build.{name}": number for name, number
               in report["benchmark"]["build_ms"].items()}
    metrics.update({f"hot.query.{name}": data["median_ms"] for name, data
                    in report["benchmark"]["queries"].items()})
    metrics.update({f"first.query.{name}": data["cold_after_load_ms"] for name, data
                    in report["benchmark"]["queries"].items()})
    metrics["first.shacl"] = report["benchmark"]["shacl"]["cold_after_load_ms"]
    metrics["first.shacl_focused"] = report["benchmark"]["shacl_focused"]["first_ms"]
    metrics["hot.shacl"] = report["benchmark"]["shacl"]["median_ms"]
    metrics["hot.shacl_focused"] = report["benchmark"]["shacl_focused"]["median_ms"]
    return metrics


def execute(args):
    with tempfile.TemporaryDirectory(prefix="ontokiln-resident-v4-") as directory:
        root = Path(directory)
        corpus = generate(root, args.facts, "kernel")
        data = root / "input" / "data.nt"
        file = corpus["files"][0]
        require(file["file"] == "input/data.nt", "frozen source contract drift")
        source_manifest = root / "sources.json"
        sources = [{
            "path": str(data),
            "document_id": corpus["source_document_id"],
            "format": "ntriples",
            "expected_sha256": file["sha256"],
            "base_iri": None,
        }]
        source_manifest.write_text(json.dumps(sources), encoding="utf8")
        rules = json.loads((root / "rules.json").read_text(encoding="utf8"))
        require(rules.get("schema_version") == "ontology-rule-ir/v1",
                "frozen rule IR contract changed")
        rule_file = root / "rule-list.json"
        rule_file.write_text(json.dumps(rules["rules"]), encoding="utf8")
        cases = (query_cases(records_for(args.facts), args.facts, False)
                 + query_cases(records_for(args.facts), args.facts, True))
        selected = [{"name": case["name"], "query": case["query"],
                     "view": case["view"]} for case in cases]
        query_file = root / "queries.json"
        query_file.write_text(json.dumps(selected), encoding="utf8")
        pack = root / "ontology.pack"
        raw_report = root / "resident.json"
        command = [
            str(args.binary.resolve()), "--sources", str(source_manifest),
            "--rules", str(rule_file), "--queries", str(query_file),
            "--shapes", str(root / "shapes.ttl"), "--pack", str(pack),
            "--output", str(raw_report), "--profile", "rdfs-supported-v1",
            "--samples", str(args.samples), "--warmups", str(args.warmups),
        ]
        if platform.system() == "Darwin":
            command = ["/usr/bin/time", "-l", *command]
        completed = subprocess.run(
            command, capture_output=True, text=True, timeout=args.timeout,
            env={**os.environ, "RAYON_NUM_THREADS": args.threads}, check=False,
        )
        if completed.returncode:
            raise RuntimeError(f"resident benchmark failed ({completed.returncode}): "
                               f"{completed.stderr[-6000:]}")
        data = json.loads(raw_report.read_text(encoding="utf8"))
        require(data["schema_version"] == "ontokiln-v4-resident-benchmark/v1",
                "unexpected benchmark result contract")
        require(data["build_identity"]["mode"] == "release", "benchmark is not Release")
        # Persist no ephemeral local directory identifiers in the public baseline.
        data["input"]["sources"][0]["path"] = "input/data.nt"
        peak_rss = data["memory"]["after_validation"]
        peak_rss = peak_rss["peak_rss_kib"] if peak_rss else None
        if platform.system() == "Darwin":
            match = re.search(r"(\d+)\s+maximum resident set size", completed.stderr)
            require(match is not None, "macOS time did not report process peak RSS")
            peak_rss = int(match.group(1)) // 1024
        data["memory"]["process_peak_rss_kib"] = peak_rss
        count = corpus["expected_inferred_facts"]
        expected = {"asserted": args.facts, "inferred": count,
                    "quads": args.facts + count}
        require(data["facts"] == expected, f"wrong materialized closure: {data['facts']}")
        require(data["input"]["rule_count"] == 3, "frozen rule count drift")
        require(data["input"]["query_count"] == len(cases), "query suite drift")
        require(data["samples"] == args.samples
                and data["warmups"] == args.warmups, "sample count drift")
        for case in cases:
            actual = data["queries"][case["name"]]["result"]
            require(actual["head"]["vars"] == case["variables"],
                    f"wrong SELECT columns: {case['name']}")
            require(actual["results"]["bindings"] == case["rows"],
                    f"wrong typed SPARQL multiset/order: {case['name']}")
            timings = data["queries"][case["name"]]
            require(len(timings["samples_ms"]) == args.samples,
                    f"missing query samples: {case['name']}")
        shacl = data["shacl"]["result"]
        require(shacl["checked_shapes"] == 4, "SHACL did not evaluate four shapes")
        require(shacl["conforms"] is False and shacl["violations"] == 1,
                "SHACL negative/positive control failed")
        expected_targets = 2 * records_for(args.facts) + 2
        require(shacl["target_count"] == expected_targets,
                "SHACL did not cover all independently expected target nodes")
        covered = {item["shape"]: item["target_count"]
                   for item in shacl["coverage"]}
        expect_coverage = {
            NS + "OrderShape": records_for(args.facts),
            NS + "ClassOrderShape": records_for(args.facts),
            NS + "ProbeShape": 1,
            NS + "MinCountProperty": 1,
        }
        require(covered == expect_coverage,
                f"SHACL target set differs: expected {expect_coverage}, got {covered}")
        require(len(data["shacl"]["samples_ms"]) == args.samples,
                "missing SHACL samples")
        focus = data["shacl_focused"]["result"]
        require(
            focus["checked_shapes"] == 1
            and focus["conforms"] is True
            and focus["violations"] == 0
            and focus["target_count"] == 1,
            "focused ProbeShape no longer has the independently expected target/result",
        )
        require(len(data["shacl_focused"]["samples_ms"]) == args.samples,
                "missing focused SHACL samples")
        require(data["pack_bytes"] > 0, "empty pack")
        digest = suite_digest(corpus, rules, selected,
                              (root / "shapes.ttl").read_bytes())
        output = {
            "schema_version": "ontokiln-v4-resident-regression/v1",
            "status": "PASS",
            "generator": GENERATOR,
            "facts": args.facts,
            "profile": "rdfs-supported-v1",
            "rule_count": 3,
            "source_digest": digest,
            "build_mode": data["build_identity"]["mode"],
            "machine": {
                "id": hashlib.sha256(platform.node().encode()).hexdigest(),
                "system": platform.system(), "release": platform.release(),
                "architecture": platform.machine(), "cpu_count": os.cpu_count(),
            },
            "measurement": {"samples": args.samples, "warmups": args.warmups},
            "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
            "rayon_threads": args.threads,
            "functional": {
                "verified_queries": len(cases),
                "explicit_facts": args.facts,
                "inferred_facts": count,
                "shacl_violations": 1,
                "shacl_targets": expected_targets,
            },
            "benchmark": data,
        }
        failed = []
        if args.compare:
            failed = annotate_comparison(
                output, json.loads(args.compare.read_text(encoding="utf8")),
                args.max_regression_ratio)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(output, ensure_ascii=False,
                                          sort_keys=True, indent=2)+"\n", encoding="utf8")
        summary = {
            "status": output["status"],
            "facts": args.facts,
            "asserted": args.facts,
            "inferred": count,
            "pack_bytes": data["pack_bytes"],
            "build_ms": data["build_ms"],
            "resident_queries_median_ms": {name: case["median_ms"]
                for name, case in data["queries"].items()},
            "resident_shacl_median_ms": data["shacl"]["median_ms"],
            "resident_shacl_focused_median_ms": data["shacl_focused"]["median_ms"],
            "peak_rss_kib": peak_rss,
            "report": str(args.output),
        }
        print(json.dumps(summary, ensure_ascii=False, sort_keys=True, indent=2))
        if args.compare:
            print(json.dumps({"comparison": output["comparison"]},
                             indent=2, sort_keys=True))
        require(not failed,
                f"timing regression beyond specified ratio: {', '.join(failed)}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True,
                        help="Release-built examples/v4_resident_bench binary")
    parser.add_argument("--facts", type=int, default=100_000)
    parser.add_argument("--samples", type=int, default=3)
    parser.add_argument("--warmups", type=int, default=1)
    parser.add_argument("--threads", default="1")
    parser.add_argument("--timeout", type=int, default=900)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--compare", type=Path,
                        help="Previous report from the same machine/build mode")
    parser.add_argument("--max-regression-ratio", type=float,
                        help="Optional hard failure threshold, intended for stable machines")
    args = parser.parse_args()
    require(args.facts >= 100 and args.facts % 10 == 0,
            "facts must be >=100 and divisible by 10")
    require(args.samples > 0 and args.warmups >= 0, "invalid sample counts")
    require(args.max_regression_ratio is None or args.compare is not None,
            "--max-regression-ratio requires --compare")
    if args.compare:
        require(args.compare.resolve() != args.output.resolve(),
                "comparison input and output must be different files")
    execute(args)


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, RuntimeError) as error:
        print(f"v4 resident regression FAILED: {error}",file=sys.stderr)
        sys.exit(1)
