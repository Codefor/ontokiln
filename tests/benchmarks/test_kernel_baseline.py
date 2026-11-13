"""Exercise the benchmark's independent oracles and measurement bookkeeping.

These tests do not build or invoke OntoKiln. A release smoke run is a separate
gate, because a correct benchmark harness cannot prove engine performance.
"""
import contextlib
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("kernel_baseline", ROOT / "benchmarks/kernel_baseline.py")
bench = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bench)


def query_result(case):
    return {"query_type": "SELECT", "variables": case["variables"], "items": copy.deepcopy(case["rows"]),
            "boolean": None, "view": case["view"], "complete": True, "truncated": False}


def validation_result(n):
    return {"conforms": True, "checked_shapes": 1, "violations": 0,
            "report": {"conforms": True, "violations": [], "target_count": n,
                       "coverage": [{"shape": bench.NS + "OrderShape", "target_count": n,
                                     "focus_nodes": [bench.order(i) for i in range(n)]}]}}


class WorkloadTests(unittest.TestCase):
    def test_generated_fact_and_term_counts_are_independently_countable(self):
        with tempfile.TemporaryDirectory() as tmp:
            for facts in [100, 200, 1000]:
                directory = Path(tmp) / str(facts)
                manifest = bench.generate(directory, facts)
                rows = (directory / "input/data.nt").read_text().splitlines()
                self.assertEqual(len(rows), facts)
                self.assertEqual(len(set(rows)), facts)
                # This small fixture is N-Triples with no embedded newlines.
                terms = {term for row in rows for term in row.removesuffix(" .").split(" ", 2)}
                self.assertEqual(len(terms), manifest["distinct_asserted_terms"])
                self.assertEqual(manifest["expected_full_compiled_terms"], len(terms) + 2)
                for entry in manifest["files"]:
                    self.assertEqual(bench.digest_file(directory / entry["file"]), entry["sha256"])

    def test_input_bytes_do_not_depend_on_temporary_paths(self):
        with tempfile.TemporaryDirectory() as tmp:
            left = bench.generate(Path(tmp) / "one", 100)
            right = bench.generate(Path(tmp) / "two", 100)
            self.assertEqual(left, right)

    def test_recursion_formula_matches_independent_graph_walk(self):
        for n in [9, 19, 99]:
            edges = {}
            for i in range(n):
                for subject, predicate, obj in bench.record_triples(i, n):
                    if predicate == bench.iri("next"):
                        edges[subject] = obj
            reach = set()
            for start in edges:
                visited = set()
                current = edges[start]
                while current not in visited:
                    visited.add(current)
                    reach.add((start, current))
                    current = edges[current]
            self.assertEqual(bench.inferred_counts(n)[bench.NS + "reach"], len(reach))
        self.assertEqual(sum(bench.inferred_counts(9).values()), 77)
        self.assertEqual(sum(bench.inferred_counts(99).values()), 849)
        self.assertEqual(list(bench.reachable_indices(7, 9)), [7])
        self.assertEqual(list(bench.reachable_indices(8, 9)), [8])

    def test_business_oracles_have_fixed_values_not_only_cardinalities(self):
        cases = {case["name"]: case for case in bench.query_cases(99, 1000)}
        point = cases["point"]["rows"][0]
        self.assertEqual(point["amount"], {"type": "literal", "value": "049", "datatype": bench.XSD + "integer"})
        self.assertEqual(point["label"], {"type": "literal", "value": "Order 49", "xml:lang": "en"})
        self.assertEqual([row["order"]["value"].rsplit("/", 1)[1] for row in cases["customer_region_join"]["rows"]],
                         ["000000011", "000000051", "000000091"])
        aggregate = cases["region_aggregate"]["rows"]
        self.assertEqual(aggregate[0]["total"]["value"], "624")
        self.assertEqual(sum(int(row["n"]["value"]) for row in aggregate), 99)
        self.assertEqual(sum(int(row["total"]["value"]) for row in aggregate), 4851)
        self.assertEqual(len(cases["bounded_property_path"]["rows"]), 9)

    def test_same_row_count_with_wrong_term_is_rejected(self):
        case = bench.query_cases(99, 1000)[0]
        result = query_result(case)
        self.assertTrue(bench.check_query(result, case)["checked"])
        result["items"][0]["customer"]["value"] = "urn:wrong-customer"
        with self.assertRaises(ValueError):
            bench.check_query(result, case)

    def test_lexical_datatype_order_and_duplicate_changes_are_rejected(self):
        case = bench.query_cases(99, 1000)[0]
        for change in [lambda r: r["items"][0]["amount"].update(value="49"),
                       lambda r: r["items"][0]["amount"].update(datatype=bench.XSD + "string"),
                       lambda r: r["items"].append(copy.deepcopy(r["items"][0]))]:
            result = query_result(case)
            change(result)
            with self.assertRaises(ValueError):
                bench.check_query(result, case)
        ordered = bench.query_cases(99, 1000)[1]
        result = query_result(ordered)
        result["items"].reverse()
        with self.assertRaises(ValueError):
            bench.check_query(result, ordered)

    def test_shacl_cannot_succeed_by_checking_no_targets(self):
        result = validation_result(9)
        self.assertEqual(bench.check_validation(result, "all_orders", 9)["target_count"], 9)
        result["report"]["coverage"][0]["focus_nodes"][-1] = "<urn:wrong-focus>"
        with self.assertRaises(ValueError):
            bench.check_validation(result, "all_orders", 9)
        result = validation_result(0)
        with self.assertRaises(ValueError):
            bench.check_validation(result, "all_orders", 9)

    def test_compiled_probe_requires_the_correct_named_violation(self):
        violation = {"focus_node": bench.NS + "order/000000000", "path": f"<{bench.RDF}type>",
                     "source_shape": bench.NS + "ProbeProperty", "shape_ref": bench.NS + "ProbeProperty",
                     "source_constraint_component": bench.SH + "HasValueConstraintComponent", "message": "any diagnostic"}
        result = {"conforms": False, "checked_shapes": 1, "violations": 1,
                  "report": {"conforms": False, "violations": [violation], "target_count": 1,
                             "coverage": [{"shape": bench.NS + "ProbeShape", "target_count": 1,
                                           "focus_nodes": [bench.order(0)]}]}}
        self.assertFalse(bench.check_validation(result, "compiled_probe", 9)["conforms"])
        result["report"]["violations"][0]["source_shape"] = "urn:wrong-shape"
        with self.assertRaises(ValueError):
            bench.check_validation(result, "compiled_probe", 9)

    def test_target_class_and_target_subjects_require_the_same_foci(self):
        result = validation_result(9)
        result["report"]["coverage"][0]["shape"] = bench.NS + "ClassOrderShape"
        self.assertEqual(bench.check_validation(result, "class_orders", 9)["target_count"], 9)
        result["report"]["coverage"][0]["focus_nodes"] = []
        with self.assertRaises(ValueError):
            bench.check_validation(result, "class_orders", 9)

    def test_native_core_negative_control_rejects_false_success(self):
        result = {"conforms": False, "checked_shapes": 1, "violations": 1,
                  "report": {"conforms": False, "target_count": 1,
                             "coverage": [{"shape": bench.NS + "MinCountProperty", "target_count": 1,
                                           "focus_nodes": [bench.order(0)]}],
                             "violations": [{"focus_node": bench.NS + "order/000000000",
                                             "path": bench.NS + "amount", "shape_ref": bench.NS + "MinCountProperty",
                                             "message": "any diagnostic"}]}}
        self.assertFalse(bench.check_validation(result, "core_min_count", 9)["conforms"])
        result["report"]["violations"][0]["focus_node"] = "urn:wrong-focus"
        with self.assertRaises(ValueError):
            bench.check_validation(result, "core_min_count", 9)
        result["conforms"], result["violations"] = True, 0
        with self.assertRaises(ValueError):
            bench.check_validation(result, "core_min_count", 9)

    def test_sizes_are_explicit_fact_counts(self):
        self.assertEqual([bench.records_for(n) for n in [10000, 100000, 1000000]], [999, 9999, 99999])
        for facts in [0, 90, 101]:
            with self.assertRaises(ValueError):
                bench.records_for(facts)


class MeasurementTests(unittest.TestCase):
    def test_content_guard_does_not_send_an_unsupported_manifest_field(self):
        artifact = {"content_digest": "a" * 64, "manifest_sha256": "b" * 64}
        self.assertEqual(bench.snapshot_guards(artifact, "content"),
                         {"expected_content_digest": "a" * 64})

    def test_manifest_guard_requires_both_digests_without_downgrading(self):
        artifact = {"content_digest": "a" * 64, "manifest_sha256": "b" * 64}
        self.assertEqual(bench.snapshot_guards(artifact, "manifest"),
                         {"expected_content_digest": "a" * 64, "expected_manifest_sha256": "b" * 64})
        with self.assertRaises(KeyError):
            bench.snapshot_guards({"content_digest": "a" * 64}, "manifest")
        with self.assertRaises(ValueError):
            bench.snapshot_guards(artifact, "unknown")

    def test_failures_and_warmups_do_not_make_latency_look_faster(self):
        samples = [{"status": "COMPLETE", "end_to_end_seconds": 100, "warmup": True},
                   {"status": "COMPLETE", "end_to_end_seconds": 1},
                   {"status": "COMPLETE", "end_to_end_seconds": 3},
                   {"status": "BUDGET_EXHAUSTED", "end_to_end_seconds": .001},
                   {"status": "SKIPPED"}]
        result = bench.summarize(samples)
        self.assertEqual(result["median_seconds"], 2)
        self.assertEqual(result["successful_samples"], 2)
        self.assertEqual(result["measured_status_counts"], {"COMPLETE": 2, "BUDGET_EXHAUSTED": 1, "SKIPPED": 1})
        self.assertIsNone(bench.summarize([{"status": "FAILED"}])["median_seconds"])

    def test_failed_repetition_skips_only_that_series(self):
        runner = bench.Baseline.__new__(bench.Baseline)
        checkpoints = []
        runner.checkpoint = lambda: checkpoints.append(True)
        scale = {"facts": 100, "stages": []}
        invoked = []

        def fail(index):
            invoked.append(index)
            return {"status": "BUDGET_EXHAUSTED", "end_to_end_seconds": .1}

        with contextlib.redirect_stdout(io.StringIO()):
            first = runner.series(scale, "bad", 3, fail, warmup=True)
            second = runner.series(scale, "independent", 1, lambda i: {"status": "COMPLETE", "end_to_end_seconds": .2})
        self.assertEqual(invoked, [0])
        self.assertEqual([s["status"] for s in first["samples"]], ["BUDGET_EXHAUSTED", "SKIPPED", "SKIPPED", "SKIPPED"])
        self.assertEqual(second["summary"]["successful_samples"], 1)
        self.assertEqual(len(checkpoints), 5)
        self.assertEqual(first["summary"]["planned_measured_samples"], 3)
        self.assertEqual(first["summary"]["attempted_measured_samples"], 0)
        self.assertEqual(first["summary"]["skipped_measured_samples"], 3)
        self.assertEqual(first["summary"]["warmup_status_counts"], {"BUDGET_EXHAUSTED": 1})
        self.assertEqual(first["summary"]["first_failure"]["status"], "BUDGET_EXHAUSTED")
        self.assertTrue(first["summary"]["first_failure"]["warmup"])

    def test_failed_dependency_never_invokes_operation(self):
        runner = bench.Baseline.__new__(bench.Baseline)
        runner.checkpoint = lambda: None
        with contextlib.redirect_stdout(io.StringIO()):
            result = runner.series({"facts": 100, "stages": []}, "dependent", 1,
                                   lambda i: self.fail("must not invoke"), dependency=False)
        self.assertEqual(result["samples"][0]["status"], "SKIPPED")

    def test_wait4_records_cpu_and_nonzero_exit(self):
        with tempfile.TemporaryDirectory() as tmp:
            measured, payload = bench.timed_command([sys.executable, "-c", "raise SystemExit(3)"],
                                                    Path(tmp), 5, os.environ.copy())
        self.assertEqual(measured["returncode"], 3)
        self.assertFalse(measured["watchdog_timeout"])
        self.assertGreater(measured["process_wait4_peak_rss_bytes"], 0)
        self.assertGreaterEqual(measured["process_user_seconds"], 0)
        self.assertGreater(measured["end_to_end_seconds"], 0)
        self.assertIsNone(payload)

    def test_watchdog_kills_and_reaps_command(self):
        with tempfile.TemporaryDirectory() as tmp:
            measured, _ = bench.timed_command([sys.executable, "-c", "import time; time.sleep(10)"],
                                              Path(tmp), .05, os.environ.copy())
        self.assertTrue(measured["watchdog_timeout"])
        self.assertNotEqual(measured["returncode"], 0)
        self.assertLess(measured["process_wall_seconds"], 5)

    def test_watchdog_cannot_be_overridden_by_a_concurrent_success_response(self):
        with tempfile.TemporaryDirectory() as tmp:
            runner = bench.Baseline.__new__(bench.Baseline)
            runner.directory, runner.counter, runner.limits = Path(tmp), 0, {}
            runner.args = SimpleNamespace(binary=Path(sys.executable), timeout_seconds=1)
            runner.environment = os.environ.copy()
            runner.report = {"environment": {"kernel_identity": {}}}

            def completed_at_deadline(command, directory, timeout, environment, response):
                raw = (directory / "request.json").read_bytes()
                request = json.loads(raw)
                reply = {"contract": bench.CONTRACT, "request_id": request["request_id"],
                         "request_sha256": bench.hashlib.sha256(raw).hexdigest(), "kernel": {},
                         "execution": {"worker_reaped": True}, "status": "COMPLETE", "result": {}, "error": None}
                return {"watchdog_timeout": True, "returncode": 0}, bench.canonical(reply)

            with patch.object(bench, "timed_command", completed_at_deadline):
                measured = runner.api({"kind": "capabilities"}, lambda _: self.fail("timed-out result cannot pass the oracle"))
        self.assertEqual(measured["status"], "WATCHDOG_TIMEOUT")

    def test_checkpoint_roundtrip(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "report.json"
            bench.publish(path, {"status": "RUNNING"})
            bench.publish(path, {"status": "COMPLETE_WITH_FAILURES"})
            self.assertEqual(json.loads(path.read_text()), {"status": "COMPLETE_WITH_FAILURES"})
            self.assertFalse(path.with_name("report.json.tmp").exists())


class SessionTransportTests(unittest.TestCase):
    def test_serial_exchange_preserves_raw_json_and_reaps_on_eof(self):
        script = "import sys; print('{\"status\":\"READY\"}', flush=True)\nfor line in sys.stdin: print(line.rstrip('\\n'), flush=True)"
        with tempfile.TemporaryDirectory() as tmp:
            session = bench.JsonlSession([sys.executable, "-u", "-c", script], Path(tmp), 5, os.environ.copy())
            try:
                self.assertEqual(session.mount["status"], "RECEIVED")
                self.assertEqual(json.loads(session.ready_bytes), {"status": "READY"})
                for request in [b'{ "first": 1 }', b'{"second":["x",2]}']:
                    measured, payload = session.request(request)
                    self.assertEqual(measured["status"], "RECEIVED")
                    self.assertEqual(payload, request)
                    self.assertGreater(measured["end_to_end_seconds"], 0)
                    self.assertNotIn("process_wait4_peak_rss_bytes", measured)
            finally:
                lifecycle = session.close()
            self.assertEqual(lifecycle["returncode"], 0)
            self.assertFalse(lifecycle["shutdown_watchdog_timeout"])
            self.assertIs(session.close(), lifecycle)

    def test_startup_timeout_is_not_a_ready_session(self):
        with tempfile.TemporaryDirectory() as tmp:
            session = bench.JsonlSession([sys.executable, "-c", "import time; time.sleep(10)"],
                                        Path(tmp), .05, os.environ.copy())
            self.assertEqual(session.mount["status"], "WATCHDOG_TIMEOUT")
            self.assertIsNone(session.ready_bytes)
            measured, payload = session.request(b"{}")
            self.assertEqual(measured["status"], "PROCESS_FAILED")
            self.assertIsNone(payload)
            self.assertNotEqual(session.close()["returncode"], 0)

    def test_request_timeout_closes_the_failed_transport_without_restart(self):
        script = "import sys, time; print('{}', flush=True); sys.stdin.readline(); time.sleep(10)"
        with tempfile.TemporaryDirectory() as tmp:
            session = bench.JsonlSession([sys.executable, "-u", "-c", script], Path(tmp), 5, os.environ.copy())
            session.timeout = .05
            measured, payload = session.request(b"{}")
            self.assertEqual(measured["status"], "WATCHDOG_TIMEOUT")
            self.assertIsNone(payload)
            self.assertEqual(session.request(b"{}")[0]["status"], "PROCESS_FAILED")
            self.assertNotEqual(session.close()["returncode"], 0)

    def test_missing_required_session_never_falls_back_to_file_api(self):
        with tempfile.TemporaryDirectory() as tmp:
            runner = bench.Baseline.__new__(bench.Baseline)
            runner.directory, runner.counter, runner.limits = Path(tmp), 0, {}
            runner.session_required, runner.sessions = {"dataset"}, {}
            with patch.object(bench, "timed_command", side_effect=AssertionError("no fallback")):
                measured = runner.api({"kind": "query", "dataset": "dataset"}, lambda _: {})
            self.assertEqual(measured["status"], "PROCESS_FAILED")
            self.assertEqual(measured["transport"], "session")


if __name__ == "__main__":
    unittest.main()
