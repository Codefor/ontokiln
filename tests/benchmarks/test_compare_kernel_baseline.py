"""Small synthetic reports test comparison policy without invoking the engine."""
import contextlib
import copy
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("compare_kernel_baseline", ROOT / "benchmarks/compare_kernel_baseline.py")
compare = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(compare)


def sample(seconds=2, rss=100, **extra):
    return {"status": "COMPLETE", "warmup": False, "oracle": {"checked": True},
            "end_to_end_seconds": seconds,
            "api_metrics": {"parse_dictionary_seconds": seconds / 4, "index_seconds": seconds / 2,
                            "engine_seconds": seconds * .75, "peak_rss_bytes": rss}, **extra}


def report():
    return {"schema_version": compare.SCHEMA, "status": "COMPLETE",
            "configuration": {"sizes": [1000], "build_samples": 3, "build_profile": "release",
                              "build_features": "default", "binary": "/old/ontokiln", "output": "/old/report.json"},
            "limits": {"timeout_ms": 60000, "memory_bytes": 512 * 1024**2},
            "cache_policy": "no flush; one warm-up for reads", "concurrency": 1,
            "scratch_policy": "child TMPDIR is <work_directory>/scratch",
            "process_environment_overrides": {"RAYON_NUM_THREADS": "1", "TMPDIR": "<work_directory>/scratch"},
            "scope": "synthetic RDF default graph", "metric_definitions": {"end_to_end_seconds": "Popen through response bytes"},
            "environment": {"cpu_model": "test CPU", "logical_cpus": 4, "cpu_affinity": [0, 1],
                            "platform": "Linux-6.0-x86_64", "machine": "x86_64", "physical_memory_bytes": 8 * 1024**3,
                            "rustc_on_path": "rustc 1.95.0", "python": "3.11.0", "cgroup_limits_raw": {"cpu.max": "max 100000"},
                            "relevant_environment": {"TMPDIR": "/old/tmp"},
                            "filesystem": {"filesystem": "ext4", "mountpoint": "/old/mount"},
                            "build_profile_caller_declaration": "release", "build_features_caller_declaration": "default",
                            "provenance_files": {compare.SCRIPT: "a" * 64}, "binary_sha256": "b" * 64,
                            "repository_head": "old", "kernel_identity": {"kernel_sha256": "c" * 64}},
            "scales": [{"facts": 1000, "completed": True,
                        "input": {"generator": "orders/v1", "records": 99, "explicit_facts": 1000,
                                  "files": [{"file": "data.nt", "bytes": 12345, "sha256": "d" * 64}]},
                        "stages": [{"name": "full_compile", "metadata": {"profile": "rdfs-supported-v1", "phase_metrics": True},
                                    "samples": [sample(1), sample(2), sample(3)],
                                    "summary": {"planned_measured_samples": 3, "planned_warmup_samples": 0,
                                                "median_seconds": 999}}]}]}


def stage(value):
    return value["scales"][0]["stages"][0]


class ComparisonTests(unittest.TestCase):
    def test_same_configuration_different_engine_compares_raw_verified_samples(self):
        before, after = report(), report()
        after["environment"].update(binary_sha256="e" * 64, repository_head="new", kernel_identity={"kernel_sha256": "f" * 64})
        after["configuration"].update(binary="/new/ontokiln", output="/new/report.json")
        stage(after)["samples"] = [sample(.5, 75), sample(1, 75), sample(1.5, 75)]
        result = compare.compare(before, after)
        self.assertTrue(result["comparable"])
        metrics = result["stages"][0]["metrics"]
        self.assertEqual(metrics["end_to_end_seconds"]["before"]["median"], 2)
        self.assertEqual(metrics["end_to_end_seconds"]["after"]["median"], 1)
        self.assertEqual(metrics["end_to_end_seconds"]["change_percent"], -50)
        self.assertEqual(metrics["api_metrics.index_seconds"]["change_percent"], -50)
        self.assertEqual(metrics["api_metrics.peak_rss_bytes"]["change_percent"], -25)
        self.assertEqual(metrics["api_metrics.peak_rss_bytes"]["after"]["samples"], 3)

    def test_input_hash_generator_and_workload_changes_are_non_comparable(self):
        mutations = [lambda r: r["scales"][0]["input"]["files"][0].update(sha256="e" * 64),
                     lambda r: r["scales"][0]["input"].update(generator="orders/v2"),
                     lambda r: stage(r)["metadata"].update(query_sha256="f" * 64)]
        for mutate in mutations:
            after = report()
            mutate(after)
            result = compare.compare(report(), after)
            self.assertFalse(result["comparable"])
            self.assertTrue(result["issues"])
            self.assertIsNone(result["stages"][0]["metrics"]["end_to_end_seconds"]["change_percent"])

    def test_limits_method_and_environment_changes_are_checked(self):
        for field, value in [("cpu_model", "other CPU"), ("cpu_affinity", [0]),
                             ("platform", "Darwin"), ("machine", "aarch64"),
                             ("physical_memory_bytes", 16 * 1024**3), ("rustc_on_path", "rustc 1.96.0"),
                             ("filesystem", {"filesystem": "tmpfs"}),
                             ("build_profile_caller_declaration", "debug"),
                             ("build_features_caller_declaration", "ascent-engine")]:
            with self.subTest(field=field):
                after = report()
                after["environment"][field] = value
                self.assertFalse(compare.compare(report(), after)["comparable"])
        for field, value in [("limits", {"timeout_ms": 1}), ("cache_policy", "flush"), ("scratch_policy", "system TMPDIR"), ("concurrency", 2),
                             ("process_environment_overrides", {"RAYON_NUM_THREADS": "4"})]:
            with self.subTest(field=field):
                after = report()
                after[field] = value
                self.assertFalse(compare.compare(report(), after)["comparable"])

    def test_script_change_is_explicit_methodology_change(self):
        after = report()
        after["environment"]["provenance_files"][compare.SCRIPT] = "e" * 64
        result = compare.compare(report(), after)
        self.assertFalse(result["comparable"])
        self.assertTrue(result["methodology_changed"])
        self.assertIn("Measurement methodology changed", compare.markdown(result))

    def test_file_and_session_boundaries_are_not_treated_as_equal(self):
        before, after = report(), report()
        before["configuration"]["read_transport"] = "file"
        after["configuration"]["read_transport"] = "session"
        result = compare.compare(before, after)
        self.assertFalse(result["comparable"])
        self.assertIsNone(result["stages"][0]["metrics"]["end_to_end_seconds"]["change_percent"])

    def test_different_snapshot_binding_strength_is_non_comparable(self):
        before, after = report(), report()
        before["configuration"]["snapshot_guard"] = "content"
        after["configuration"]["snapshot_guard"] = "manifest"
        result = compare.compare(before, after)
        self.assertFalse(result["comparable"])
        self.assertIsNone(result["stages"][0]["metrics"]["end_to_end_seconds"]["change_percent"])

    def test_unknown_critical_metadata_is_not_proof_of_comparability(self):
        before, after = report(), report()
        before["environment"]["rustc_on_path"] = after["environment"]["rustc_on_path"] = None
        result = compare.compare(before, after)
        self.assertFalse(result["comparable"])
        self.assertEqual(result["issues"][0]["field"], "environment.rustc_on_path")

    def test_affinity_count_and_normalized_temp_policy_ignore_absolute_paths(self):
        after = report()
        after["environment"]["cpu_affinity"] = [12, 13]
        after["environment"]["filesystem"]["mountpoint"] = "/new/mount"
        after["environment"]["relevant_environment"]["TMPDIR"] = "/new/tmp"
        self.assertTrue(compare.compare(report(), after)["comparable"])

    def test_failed_warmup_suppresses_percent_even_if_later_samples_succeeded(self):
        before, after = report(), report()
        for value in [before, after]:
            stage(value)["summary"]["planned_warmup_samples"] = 1
        stage(before)["samples"].insert(0, sample(20, warmup=True))
        stage(after)["samples"].insert(0, {"status": "BUDGET_EXHAUSTED", "warmup": True,
                                          "end_to_end_seconds": .001, "error": {"code": "RESOURCE_EXHAUSTED"}})
        after["status"] = "COMPLETE_WITH_FAILURES"
        result = compare.compare(before, after)
        self.assertTrue(result["comparable"])
        item = result["stages"][0]
        self.assertEqual(item["after"]["warmup_status_counts"], {"BUDGET_EXHAUSTED": 1})
        self.assertEqual(item["after"]["failures"][0]["phase"], "warmup")
        self.assertEqual(item["metrics"]["end_to_end_seconds"]["after"]["median"], 2)
        self.assertIsNone(item["metrics"]["end_to_end_seconds"]["change_percent"])

    def test_fast_failures_skips_and_unverified_successes_never_enter_median(self):
        after = report()
        stage(after)["samples"] = [sample(3), {"status": "FAILED", "end_to_end_seconds": .001},
                                    {"status": "SKIPPED", "reason": "previous failure"},
                                    sample(.002, oracle={"checked": False})]
        stage(after)["summary"]["planned_measured_samples"] = 4
        result = compare.compare(report(), after)["stages"][0]
        self.assertEqual(result["after"]["successful_samples"], 1)
        self.assertEqual(result["after"]["measured_status_counts"], {"COMPLETE": 1, "FAILED": 1, "SKIPPED": 1, "UNVERIFIED": 1})
        self.assertEqual(result["metrics"]["end_to_end_seconds"]["after"]["median"], 3)
        self.assertIsNone(result["metrics"]["end_to_end_seconds"]["change_percent"])

    def test_few_successful_samples_keep_median_but_have_no_percentage(self):
        before, after = report(), report()
        for value in [before, after]:
            stage(value)["samples"].pop()
            stage(value)["summary"]["planned_measured_samples"] = 2
        result = compare.compare(before, after)
        metric = result["stages"][0]["metrics"]["end_to_end_seconds"]
        self.assertEqual(metric["before"]["samples"], 2)
        self.assertEqual(metric["before"]["median"], 1.5)
        self.assertIsNone(metric["change_percent"])
        self.assertIn("fewer than 3", metric["comparison_unavailable_reason"])

    def test_missing_phase_measurement_does_not_compare_a_selected_subset(self):
        after = report()
        del stage(after)["samples"][0]["api_metrics"]["index_seconds"]
        result = compare.compare(report(), after)["stages"][0]["metrics"]
        self.assertEqual(result["end_to_end_seconds"]["change_percent"], 0)
        self.assertEqual(result["api_metrics.index_seconds"]["after"]["samples"], 2)
        self.assertIsNone(result["api_metrics.index_seconds"]["change_percent"])

    def test_unfinished_report_and_missing_planned_trials_are_visible(self):
        after = report()
        after["status"] = "RUNNING"
        stage(after)["samples"].pop()
        result = compare.compare(report(), after)
        self.assertFalse(result["comparable"])
        self.assertEqual(result["stages"][0]["after"]["missing_measured_samples"], 1)

    def test_cli_writes_json_and_returns_nonzero_for_configuration_mismatch(self):
        with tempfile.TemporaryDirectory() as tmp:
            before, after, output = [Path(tmp) / name for name in ["before.json", "after.json", "comparison.json"]]
            before.write_text(json.dumps(report()))
            after.write_text(json.dumps(report()))
            args = [str(before), str(after), "--format", "json", "--output", str(output)]
            self.assertEqual(compare.main(args), 0)
            self.assertTrue(json.loads(output.read_text())["comparable"])
            changed = report()
            changed["limits"]["timeout_ms"] = 1
            after.write_text(json.dumps(changed))
            self.assertEqual(compare.main(args), 1)
            self.assertFalse(json.loads(output.read_text())["comparable"])
            after.write_text("not JSON")
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(compare.main(args), 2)

    def test_no_claim_of_same_physical_host_or_statistical_significance(self):
        rendered = compare.markdown(compare.compare(report(), report()))
        self.assertIn("does not establish the same physical host", rendered)
        self.assertIn("not statistical significance", rendered)
        self.assertIn("n=3", rendered)


if __name__ == "__main__":
    unittest.main()
