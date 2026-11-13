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


FOCUSED_KERNEL = {"version": "test-build", "kernel_sha256": "f" * 64}


def focused_validation_result(name):
    # Independent fixtures: do not read expected_violations from the workload.
    focus = bench.NS + "order/000000008"
    shape = {"focus_core_pass": "OrderShape", "focus_core_fail": "MinCountProperty",
             "focus_nonlocal_fail": "FocusedScopeShape", "focus_path_fail": "FocusedPathShape",
             "focus_full_scan_fail": "FocusedScanShape"}[name]
    violations = []
    if name == "focus_core_fail":
        violations = [{"focus_node": focus, "path": bench.NS + "amount",
                       "shape_ref": bench.NS + "MinCountProperty", "message": "one value is below minCount 2"}]
    elif name == "focus_nonlocal_fail":
        violations = [{"focus_node": focus, "path": None,
                       "source_shape": bench.NS + "FocusedScopeShape",
                       "source_constraint_component": bench.SH + "SPARQLConstraintComponent",
                       "shape_ref": bench.NS + "FocusedScopeShape", "message": "background marker exists"}]
    elif name == "focus_path_fail":
        violations = [{"focus_node": focus, "path": f"(<{bench.NS}customer>/<{bench.NS}region>)",
                       "source_shape": bench.NS + "FocusedPathMissing",
                       "source_constraint_component": bench.SH + "HasValueConstraintComponent",
                       "shape_ref": bench.NS + "FocusedPathMissing", "message": "region 1 is not reachable"}]
    elif name == "focus_full_scan_fail":
        violations = [{"focus_node": focus, "path": None,
                       "source_shape": bench.NS + "FocusedScanShape",
                       "source_constraint_component": bench.SH + "SPARQLConstraintComponent",
                       "shape_ref": bench.NS + "FocusedScanShape", "message": "complete union aggregates matched"}]
    conforms = not violations
    return {"conforms": conforms, "checked_shapes": 1, "violations": len(violations),
            "report": {"conforms": conforms, "violations": violations, "target_count": 1,
                       "coverage": [{"shape": bench.NS + shape, "target_count": 1,
                                     "focus_nodes": ["<" + focus + ">"]}],
                       "engine": {"name": "ontokiln-rust-shacl", "version": "test-build"},
                       "kernel": dict(FOCUSED_KERNEL)}}


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


class FocusedWorkloadTests(unittest.TestCase):
    def test_focused_shapes_do_not_change_kernel_inputs(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            default = bench.generate(root / "default", 100)
            explicit = bench.generate(root / "explicit", 100, "kernel")
            focused = bench.generate(root / "focused", 100, "focused-shacl")
            self.assertEqual(default, explicit)
            self.assertEqual(default["files"][:2], focused["files"][:2])
            self.assertEqual(default["files"][2]["sha256"],
                             "e567dae83f7aebdafed38bc5843ae47a1bc6a888c0aceafb83ffa2da3a08cb7a")
            self.assertNotEqual(default["files"][2]["sha256"], focused["files"][2]["sha256"])
            self.assertEqual((root / "focused/shapes.ttl").read_text(),
                             bench.shapes_text() + bench.focused_shapes_text(100))
            self.assertNotIn("FocusedScopeShape", (root / "default/shapes.ttl").read_text())

    def test_fixed_focus_and_nonlocal_facts_exist_from_smoke_to_large_backgrounds(self):
        for facts in [100, 10000, 100000, 1000000]:
            self.assertEqual({case["focus_node"] for case in bench.focused_validation_cases(facts)},
                             {"<" + bench.NS + "order/000000008>"})
            n = bench.records_for(facts)
            local = bench.record_triples(8, n)
            self.assertIn((bench.order(8), bench.iri("customer"), bench.customer(8)), local)
            self.assertIn((bench.customer(8), bench.iri("region"), bench.iri("region/0")), local)
            self.assertIn((bench.order(8), bench.iri("amount"), f'"008"^^<{bench.XSD}integer>'), local)
            self.assertIn((bench.order(0), bench.iri("label"), '"Order 0"@en'), bench.record_triples(0, n))
        self.assertIn('sh:path (b:customer b:region)', bench.focused_shapes_text(100))
        self.assertIn('?other ' + bench.iri("label") + ' "Order 0"@en', bench.focused_shapes_text(100))

    def test_full_scan_expectation_matches_an_independent_union_of_generated_rows(self):
        with tempfile.TemporaryDirectory() as tmp:
            for facts in [100, 200, 1000]:
                with self.subTest(facts=facts):
                    directory = Path(tmp) / str(facts)
                    bench.generate(directory, facts, "focused-shacl")
                    lines = (directory / "input/data.nt").read_text().splitlines()
                    union = set(lines + lines[:5])
                    # All generated subjects are IRIs: SPARQL STR removes the
                    # angle brackets, and the union must not count duplicates.
                    subjects = [row.split(" ", 1)[0][1:-1] for row in union]
                    expected = {"fact_count": len(union), "subject_characters": sum(map(len, subjects))}
                    self.assertEqual(bench.full_scan_expected(facts), expected)
                    case = next(c for c in bench.focused_validation_cases(facts)
                                if c["name"] == "focus_full_scan_fail")
                    self.assertEqual(case["expected_aggregate"], expected)
                    self.assertTrue(case["requires_full_scan"])
                    shapes = (directory / "shapes.ttl").read_text()
                    self.assertIn('(COUNT(*) AS ?factCount) (SUM(STRLEN(STR(?s))) AS ?subjectChars)', shapes)
                    self.assertIn('WHERE { ?s ?p ?o }', shapes)
                    self.assertIn(f'FILTER (?factCount = {expected["fact_count"]} && '
                                  f'?subjectChars = {expected["subject_characters"]})', shapes)

    def test_selective_path_has_positive_and_negative_constraints_without_sparql(self):
        shapes = bench.focused_shapes_text(100)
        path_shapes = shapes.split('b:FocusedPathShape a sh:NodeShape;', 1)[1].split(
            'b:FocusedScanShape a sh:NodeShape;', 1)[0]
        self.assertNotIn('sh:sparql', path_shapes)
        self.assertIn('sh:property b:FocusedPathPresent, b:FocusedPathMissing', path_shapes)
        self.assertIn('b:FocusedPathPresent sh:path (b:customer b:region); sh:hasValue '
                      + bench.iri('region/0'), path_shapes)
        self.assertIn('b:FocusedPathMissing sh:path (b:customer b:region); sh:hasValue '
                      + bench.iri('region/1'), path_shapes)

    def test_focused_oracles_accept_complete_independent_reports(self):
        for case in bench.focused_validation_cases(100):
            with self.subTest(case=case["name"]):
                checked = bench.check_focused_validation(focused_validation_result(case["name"]), case, FOCUSED_KERNEL)
                self.assertTrue(checked["checked"])
                self.assertEqual(checked["target_count"], 1)
                self.assertEqual(checked["conforms"], case["name"] == "focus_core_pass")
                self.assertEqual(len(checked["semantic_report_sha256"]), 64)

    def test_focused_oracles_reject_zero_wrong_or_extra_focus_and_wrong_report_identity(self):
        mutations = [lambda r: r.update(checked_shapes=0),
                     lambda r: r["report"].update(target_count=0, coverage=[]),
                     lambda r: r["report"]["coverage"][0].update(focus_nodes=[bench.order(0)]),
                     lambda r: r["report"]["coverage"][0]["focus_nodes"].append(bench.order(9)),
                     lambda r: r["report"]["coverage"][0].update(shape="urn:wrong-shape"),
                     lambda r: r["report"]["kernel"].update(kernel_sha256="0" * 64),
                     lambda r: r["report"].pop("engine"),
                     lambda r: r["report"].update(unexpected_semantics=True)]
        for case in bench.focused_validation_cases(100):
            for index, mutate in enumerate(mutations):
                with self.subTest(case=case["name"], mutation=index):
                    result = focused_validation_result(case["name"])
                    mutate(result)
                    with self.assertRaises(ValueError):
                        bench.check_focused_validation(result, case, FOCUSED_KERNEL)

    def test_focused_negative_controls_reject_missing_wrong_or_duplicate_violations(self):
        mutations = [lambda r: r.update(conforms=True, violations=0),
                     lambda r: r["report"].update(violations=[]),
                     lambda r: r["report"]["violations"][0].update(focus_node=bench.NS + "order/000000000"),
                     lambda r: r["report"]["violations"][0].update(shape_ref="urn:wrong-shape"),
                     lambda r: r["report"]["violations"][0].update(path="urn:wrong-path"),
                     lambda r: r["report"]["violations"][0].update(source_shape="urn:wrong-shape"),
                     lambda r: r["report"]["violations"][0].update(source_constraint_component="urn:wrong-component"),
                     lambda r: r["report"]["violations"].append(copy.deepcopy(r["report"]["violations"][0]))]
        for case in bench.focused_validation_cases(100)[1:]:
            for index, mutate in enumerate(mutations):
                with self.subTest(case=case["name"], mutation=index):
                    result = focused_validation_result(case["name"])
                    mutate(result)
                    with self.assertRaises(ValueError):
                        bench.check_focused_validation(result, case, FOCUSED_KERNEL)
        case = bench.focused_validation_cases(100)[2]
        result = focused_validation_result(case["name"])
        # A cropped graph can lose the SPARQL failure and add a HasValue failure;
        # one violation alone must not make this incorrect result acceptable.
        result["report"]["violations"][0]["source_constraint_component"] = bench.SH + "HasValueConstraintComponent"
        with self.assertRaises(ValueError):
            bench.check_focused_validation(result, case, FOCUSED_KERNEL)

    def test_selective_path_oracle_rejects_a_single_failure_on_the_positive_property(self):
        case = next(c for c in bench.focused_validation_cases(100) if c["name"] == "focus_path_fail")
        result = focused_validation_result(case["name"])
        # A partial path reader can lose the present value while retaining just
        # one wrong violation. Its exact source shape must still be rejected.
        result["report"]["violations"][0].update(source_shape=bench.NS + "FocusedPathPresent",
                                                shape_ref=bench.NS + "FocusedPathPresent")
        with self.assertRaises(ValueError):
            bench.check_focused_validation(result, case, FOCUSED_KERNEL)

    def test_diagnostic_prose_does_not_change_the_semantic_report_digest(self):
        for case in bench.focused_validation_cases(100)[1:]:
            result = focused_validation_result(case["name"])
            before = bench.check_focused_validation(result, case, FOCUSED_KERNEL)
            result["report"]["violations"][0]["message"] = "different diagnostic wording"
            self.assertEqual(before, bench.check_focused_validation(result, case, FOCUSED_KERNEL))

    def test_binary_revision_does_not_change_the_semantic_report_digest(self):
        for case in bench.focused_validation_cases(100):
            result = focused_validation_result(case["name"])
            before = bench.check_focused_validation(result, case, FOCUSED_KERNEL)
            kernel = {**FOCUSED_KERNEL, "source_revision": "a" * 40, "kernel_sha256": "b" * 64}
            result["report"]["kernel"] = kernel
            self.assertEqual(before, bench.check_focused_validation(result, case, kernel))

    def test_suite_defaults_to_kernel_and_selects_focused_explicitly(self):
        common = ["--binary", sys.executable, "--output", "report.json"]
        self.assertEqual(bench.parse_args(common).suite, "kernel")
        self.assertEqual(bench.parse_args(common + ["--suite", "focused-shacl"]).suite, "focused-shacl")

    def test_focused_suite_only_compiles_mounts_and_runs_its_five_focus_cases(self):
        for transport in ["file", "session"]:
            with self.subTest(transport=transport), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                args = bench.parse_args(["--binary", sys.executable, "--output", str(root / "report.json"),
                                         "--suite", "focused-shacl", "--sizes", "100", "--build-samples", "1",
                                         "--validation-samples", "1", "--read-transport", transport])
                report = {"environment": {"kernel_identity": FOCUSED_KERNEL}, "scales": []}
                runner = bench.Baseline(args, root, report)
                operations, mounted = [], []

                def api(operation, oracle):
                    operations.append(operation)
                    if operation["kind"] == "compile":
                        output = Path(operation["output"])
                        output.mkdir()
                        (output / "facts.sqlite").write_bytes(b"artifact fixture")
                        artifact = {"stats": {"indexed_facts": 100, "parsed_facts": 100,
                                               "distinct_terms": bench.expected_terms(9) + 2}}
                        (output / "manifest.json").write_bytes(bench.canonical(artifact))
                        result = {"stats": {"explicit_facts": 100, "inferred_facts": 0}, "fixpoint_reached": False,
                                  "artifact": {"content_digest": "a" * 64,
                                               "manifest_sha256": bench.digest_file(output / "manifest.json")}}
                    else:
                        self.assertEqual(operation["kind"], "validate")
                        self.assertEqual(operation["focus_node"], bench.order(8))
                        name = {bench.iri("OrderShape"): "focus_core_pass", bench.iri("MinCountProperty"): "focus_core_fail",
                                bench.iri("FocusedScopeShape"): "focus_nonlocal_fail",
                                bench.iri("FocusedPathShape"): "focus_path_fail",
                                bench.iri("FocusedScanShape"): "focus_full_scan_fail"}[operation["selected_shapes"][0]]
                        result = focused_validation_result(name)
                    return {"status": "COMPLETE", "end_to_end_seconds": .01, "oracle": oracle(result)}

                def mount(scale, backend, dataset, guard):
                    mounted.append(backend)
                    runner.series(scale, backend + "/session_mount", 1,
                                  lambda _: {"status": "COMPLETE", "end_to_end_seconds": .01, "oracle": {"checked": True}},
                                  metadata={"one_time_mount": True})

                runner.api, runner.mount = api, mount
                with patch.object(bench, "timed_command", side_effect=AssertionError("no engine in a harness test")), \
                        contextlib.redirect_stdout(io.StringIO()):
                    runner.scale(100)
                names = [stage["name"] for stage in report["scales"][0]["stages"]]
                self.assertEqual(names, ["full_compile"] + (["compiled/session_mount"] if transport == "session" else [])
                                 + ["shacl/focus_core_pass", "shacl/focus_core_fail", "shacl/focus_nonlocal_fail",
                                    "shacl/focus_path_fail", "shacl/focus_full_scan_fail"])
                self.assertEqual(mounted, ["compiled"] if transport == "session" else [])
                self.assertEqual([operation["kind"] for operation in operations], ["compile"] + ["validate"] * 10)
                self.assertTrue(all(sample["status"] == "COMPLETE" for stage in report["scales"][0]["stages"]
                                    for sample in stage["samples"]))


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
