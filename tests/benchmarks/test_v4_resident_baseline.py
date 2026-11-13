#!/usr/bin/env python3
"""Same-machine regression gates, using synthetic timings independent of Rust."""
import copy
from pathlib import Path
import sys
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "benchmarks"))
from v4_resident_baseline import compare_reports


def report():
    return {
        "generator": "fixture/v1", "facts": 100, "profile": "datalog-v1",
        "rule_count": 1, "source_digest": "source", "machine": {"id": "machine"},
        "build_mode": "release", "rayon_threads": "1",
        "measurement": {"samples": 3, "warmups": 1},
        "benchmark": {
            "build_identity": {"rustc": "rustc fixture", "target": "fixture", "kernel": "before"},
            "build_ms": {"compile": 10, "materialize": 20, "save": 30, "load": 40},
            "queries": {"lookup": {"cold_after_load_ms": 5, "median_ms": 1}},
            "shacl": {"cold_after_load_ms": 100, "median_ms": 10},
            "shacl_focused": {"first_ms": 2, "median_ms": 1},
        },
    }


class Compare(unittest.TestCase):
    def test_cold_and_hot_measurements_are_compared_and_code_can_change(self):
        before = report(); after = copy.deepcopy(before)
        after["benchmark"]["build_identity"]["kernel"] = "after"
        after["benchmark"]["queries"]["lookup"]["cold_after_load_ms"] = 10
        actual = compare_reports(before, after)
        self.assertEqual(actual["first.query.lookup"]["ratio"], 2)
        self.assertIn("first.shacl", actual)
        self.assertIn("first.shacl_focused", actual)
        self.assertIn("hot.query.lookup", actual)

    def test_changed_or_missing_environment_is_rejected(self):
        for key in ("machine", "build_mode", "rayon_threads", "measurement", "source_digest"):
            for missing in (True, False):
                with self.subTest(key=key, missing=missing):
                    after = report()
                    if missing: del after[key]
                    else: after[key] = "different"
                    with self.assertRaisesRegex(AssertionError, "not comparable"):
                        compare_reports(report(), after)

    def test_toolchain_target_and_query_suite_are_frozen(self):
        for key in ("rustc", "target"):
            after = report(); after["benchmark"]["build_identity"][key] = "different"
            with self.assertRaisesRegex(AssertionError, "not comparable"):
                compare_reports(report(), after)
        after = report(); after["benchmark"]["queries"] = {}
        with self.assertRaisesRegex(AssertionError, "measurement suites"):
            compare_reports(report(), after)


if __name__ == "__main__": unittest.main()
