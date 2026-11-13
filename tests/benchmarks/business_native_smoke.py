#!/usr/bin/env python3
"""Check a native directory run against independent, fixed RDF expectations.

All engine execution and measurement goes through one business_baseline.py
invocation. This verifier does not build Rust, issue API calls or use SQL.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "tests/fixtures/business-baseline"
NS = "urn:business-baseline:smoke:"
XSD = "http://www.w3.org/2001/XMLSchema#"
EXPECTED_SELECT = [
    {"s": {"type": "uri", "value": NS + "a"},
     "value": {"type": "literal", "value": "alpha", "xml:lang": "en"}},
    {"s": {"type": "uri", "value": NS + "b"},
     "value": {"type": "literal", "value": "007", "datatype": XSD + "integer"}},
]
EXPECTED_COVERAGE = [{"shape": NS + "Shape", "target_count": 2,
                      "focus_nodes": ["<" + NS + "a>", "<" + NS + "b>"]}]
QUERY_FILES = {"queries/01-select.rq", "queries/02-count.rq",
               "queries/03-ask.rq", "queries/04-construct.rq"}


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def equal(actual, expected, label):
    # JSON distinguishes booleans from integer counters, unlike Python ==.
    if json.dumps(actual, sort_keys=True) != json.dumps(expected, sort_keys=True):
        raise AssertionError(f"{label}: expected {expected!r}, got {actual!r}")


def read_json(path):
    return json.loads(path.read_text(encoding="utf8"))


def check_query(asset, operation, result):
    original = (FIXTURE / asset).read_text(encoding="utf8")
    equal(operation["view"], "closure", asset + " request view")
    equal(result.get("view"), "closure", asset + " result view")
    equal(result.get("complete"), True, asset + " completeness")
    equal(result.get("truncated"), False, asset + " truncation")
    if asset == "queries/04-construct.rq":
        equal(operation["query"], "ASK {}", "CONSTRUCT terminal probe")
        equal(operation.get("derived_queries"), [original], "original CONSTRUCT")
    else:
        equal(operation["query"], original, asset + " original query")
        equal(operation.get("derived_queries", []), [], asset + " derived inputs")
    if asset == "queries/01-select.rq":
        equal(result.get("query_type"), "SELECT", "SELECT type")
        equal(result.get("variables"), ["s", "value"], "SELECT variables")
        equal(result.get("items"), EXPECTED_SELECT, "ordered RDF bindings")
        equal(result.get("boolean"), None, "SELECT boolean")
    elif asset == "queries/02-count.rq":
        equal(result.get("query_type"), "SELECT", "COUNT type")
        equal(result.get("variables"), ["count"], "COUNT variables")
        equal(result.get("items"), [{"count": {"type": "literal", "value": "11",
                                                 "datatype": XSD + "integer"}}], "closure COUNT")
        equal(result.get("boolean"), None, "COUNT boolean")
    else:
        equal(result.get("query_type"), "ASK", asset + " type")
        equal(result.get("variables"), [], asset + " variables")
        equal(result.get("items"), [], asset + " items")
        equal(result.get("boolean"), True, asset + " boolean")


def verify_run(directory):
    report = read_json(directory / "summary.json")
    equal(report["status"], "COMPLETE", "driver status")
    equal(report["input_mode"], "ontology-repository", "input mode")
    equal(report["business_correctness"], "MANUAL_REVIEW_REQUIRED", "generic business verdict")
    equal(report["discovery"]["skipped_assets"], [], "unexecuted assets")
    equal(report["coverage"]["empty_validation_cases"], [], "empty SHACL coverage")
    equal(report["read_case_count"], 5, "read-case count")
    equal(report["parallelism"]["requested_jobs"], 2, "requested parallelism")
    admitted = report["parallelism"]["effective_jobs"]
    peak = report["read_execution"]["peak_active_requests"]
    require(type(admitted) is int and 1 <= admitted <= 2, "invalid admitted parallelism")
    require(type(peak) is int and 1 <= peak <= admitted, "observed parallelism exceeds admission")
    equal(report["read_execution"]["completed_requests"], 10, "completed reads including warmups")
    equal(report["read_execution"]["successful_measured_requests"], 5, "formal reads")
    equal(report["read_execution"]["failed_requests"], 0, "failed reads")
    equal(report["read_execution"]["skipped_requests"], 0, "skipped reads")
    cases = {Path(case["request_file"]): case for case in report["discovery"]["read_cases"]}
    seen, responses, execution_only = set(), 0, 0
    for stage in report["stages"]:
        kind = stage["kind"]
        if kind == "query":
            case = cases[Path(stage["template"]["path"])]
            label = Path(case["asset_path"]).relative_to(FIXTURE).as_posix()
            require(label in QUERY_FILES, "unexpected query asset: " + label)
        elif kind == "validate":
            label = "shacl"
        else:
            label = kind
        require(label not in seen, "duplicate stage: " + label)
        seen.add(label)
        equal([sample["warmup"] for sample in stage["samples"]],
              [False] if kind in ("compile", "materialize") else [True, False], label + " repetitions")
        for sample in stage["samples"]:
            equal(sample["status"], "COMPLETE", label + " sample status")
            equal(sample["protocol_verified"], True, label + " protocol verification")
            request_path = directory / sample["request_file"]
            request = read_json(request_path)
            reply = read_json(directory / sample["response_file"])
            equal(reply.get("contract"), "ontokiln-api/v1", label + " contract")
            equal(reply.get("request_id"), request["request_id"], label + " request binding")
            equal(reply.get("request_sha256"), hashlib.sha256(request_path.read_bytes()).hexdigest(),
                  label + " request digest")
            equal(reply.get("kernel"), report["environment"]["kernel_identity"], label + " engine identity")
            equal(reply.get("status"), "COMPLETE", label + " API status")
            equal(reply.get("error"), None, label + " API error")
            equal(reply["execution"].get("worker_reaped"), True, label + " worker reaping")
            result, operation = reply["result"], request["operation"]
            if kind in ("compile", "materialize"):
                equal(result.get("stats"), {"explicit_facts": 9, "inferred_facts": 2 if kind == "materialize" else 0},
                      label + " independent fact counts")
                equal(result.get("fixpoint_reached"), kind == "materialize", label + " fixpoint")
                if kind == "compile":
                    equal(operation["profile"], "datalog-v1", "compile profile")
            elif kind == "query":
                check_query(label, operation, result)
                execution_only += int(label == "queries/04-construct.rq")
            elif kind == "validate":
                equal(operation.get("shapes_from_dataset"), True, "dataset Shapes selection")
                equal(result.get("conforms"), True, "SHACL conformance")
                equal(result.get("checked_shapes"), 1, "checked Shapes")
                equal(result.get("violations"), 0, "violation count")
                details = result["report"]
                equal(details.get("conforms"), True, "SHACL report conformance")
                equal(details.get("violations"), [], "SHACL violation records")
                equal(details.get("target_count"), 2, "SHACL target count")
                equal(details.get("coverage"), EXPECTED_COVERAGE, "exact SHACL targets")
            else:
                raise AssertionError("unexpected operation: " + kind)
            responses += 1
    equal(sorted(seen), sorted({"compile", "materialize", "shacl"} | QUERY_FILES), "executed stages")
    equal(responses, 12, "complete responses")
    return {"status": "PASS", "complete_responses": responses,
            "independent_answer_checked_responses": responses - execution_only,
            "construct_probe_execution_only_responses": execution_only,
            "explicit_facts": 9, "inferred_facts": 2, "closure_facts": 11,
            "shacl_targets": 2, "effective_jobs": admitted, "peak_active_requests": peak,
            "purpose": "Functional CI, not a performance result"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--work-dir", type=Path, default=ROOT / "data/business-native-smoke")
    args = parser.parse_args()
    args.work_dir.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix="smoke-", dir=args.work_dir)).resolve()
    command = [sys.executable, "-B", str(ROOT / "benchmarks/business_baseline.py"),
               "--ontology-repo", str(FIXTURE), "--profile", "datalog-v1",
               "--binary", str(args.binary.resolve()), "--jobs", "2",
               "--memory-mib", "512", "--scratch-mib", "512",
               "--total-memory-mib", "1024", "--total-scratch-mib", "1024",
               "--build-samples", "1", "--read-samples", "1", "--warmups", "1",
               "--work-dir", str(directory)]
    verification = {"schema_version": "ontokiln-business-native-smoke/v1",
                    "status": "FAIL", "command": command, "work_directory": str(directory)}
    print("Native smoke evidence: " + str(directory), flush=True)
    try:
        # The only execution loop is the shared business driver.
        completed = subprocess.run(command, cwd=ROOT, check=False)
        verification["driver_returncode"] = completed.returncode
        runs = list(directory.glob("run-*"))
        require(len(runs) == 1, "expected one retained business run")
        verification["run_directory"] = str(runs[0])
        equal(completed.returncode, 0, "business driver exit")
        verification.update(verify_run(runs[0]))
    except (AssertionError, KeyError, TypeError, ValueError, OSError) as error:
        verification["error"] = str(error)
    output = directory / "native-smoke-verification.json"
    output.write_text(json.dumps(verification, ensure_ascii=False, indent=2) + "\n", encoding="utf8")
    print(json.dumps(verification, ensure_ascii=False, indent=2), flush=True)
    print("Independent verification: " + str(output), flush=True)
    return 0 if verification["status"] == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
