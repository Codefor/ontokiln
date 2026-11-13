import copy
import importlib.util
from pathlib import Path
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("kernel_scaling", ROOT / "benchmarks/kernel_scaling.py")
scaling = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(scaling)


class ScalingTests(unittest.TestCase):
    def test_only_build_stages_are_dispatched_and_keep_dependency_options(self):
        seen = []

        class Base:
            def series(self, scale, name, count, invoke, **options):
                seen.append((name, count, options))
                return invoke() if options.get("dependency", True) else "dependency failed"

        runner = scaling.build_only_class(Base)()
        for name in ("pack_compile", "compiled/point", "shacl/all_orders"):
            runner.series({}, name, 3, lambda: self.fail("unselected work was invoked"))
        self.assertEqual(runner.series({}, "full_compile", 3, lambda: "compiled", metadata={"profile": "rdfs"}), "compiled")
        self.assertEqual(runner.series({}, "full_materialize", 3, lambda: self.fail("failed dependency invoked"), dependency=False), "dependency failed")
        self.assertEqual(seen, [("full_compile", 3, {"metadata": {"profile": "rdfs"}}),
                                ("full_materialize", 3, {"dependency": False})])

    def test_different_harness_is_rejected_before_import(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "harness.py"
            path.write_text("raise RuntimeError('must not execute')\n")
            with self.assertRaisesRegex(ValueError, "frozen"):
                scaling.load_harness(path)

    def test_binary_and_kernel_must_both_match_receipt(self):
        environment = {"binary_sha256": "a" * 64, "kernel_identity": {"source_revision": "old", "kernel_sha256": "b" * 64}}
        receipt = {"binary_sha256": environment["binary_sha256"], "kernel": copy.deepcopy(environment["kernel_identity"])}
        scaling.verify_identity(environment, receipt)
        for field in ("binary_sha256", "kernel"):
            other = copy.deepcopy(receipt)
            other[field] = "different"
            with self.assertRaises(ValueError):
                scaling.verify_identity(environment, other)


if __name__ == "__main__":
    unittest.main()
