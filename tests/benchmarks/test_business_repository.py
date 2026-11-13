"""Check automatic repository coverage without native builds or RDF oracles.

Synthetic repositories keep discovery expectations independent of the helper.
CLI tests reuse the API fixture, not its TestCase, so the original template
tests remain a separate compatibility gate and are not collected twice.
"""
import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile


ROOT = Path(__file__).resolve().parents[2]
DRIVER = ROOT / "benchmarks/business_baseline.py"


def module_at(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


fixtures = module_at("business_repository_api_fixture", Path(__file__).with_name("test_business_baseline.py"))
PRIVATE = "private-ontology-content-never-share-this-literal"
RULE_IR = {"schema_version": "ontology-rule-ir/v1", "rules": [
    {"rule_id": "explicit-business-rule", "head": ["?s", "<urn:business:q>", "?o"],
     "body": [["?s", "<urn:business:p>", "?o"]]}]}
SELECT = '# A SELECT asset with a prefix and an inert keyword.\nPREFIX b: <urn:business:>\nSELECT ?s WHERE { ?s b:p "ASK CONSTRUCT" }\n'
ASK = 'PREFIX b: <urn:business:>\nASK { ?s b:p ?o }\n'
CONSTRUCT = 'PREFIX b: <urn:business:>\nCONSTRUCT { ?s b:q ?o } WHERE { ?s b:p ?o }\n'
SHAPES = ('@prefix sh: <http://www.w3.org/ns/shacl#> .\n'
          '<urn:business:shape> a sh:NodeShape; sh:targetNode <urn:business:s> .\n')


WRAPPER = r'''
import json
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parent
command = subprocess.run([sys.executable, str(root / "fixture_api.py"), *sys.argv[1:]])
if command.returncode == 0 and len(sys.argv) > 1 and sys.argv[1] == "api":
    request_file = Path(sys.argv[sys.argv.index("--request") + 1])
    result_file = Path(sys.argv[sys.argv.index("--result") + 1])
    request = json.loads(request_file.read_text())
    if result_file.exists():
        try:
            reply = json.loads(result_file.read_text())
        except json.JSONDecodeError:
            reply = None
        query = request["operation"].get("query", "")
        if (reply and reply.get("status") == "COMPLETE" and request["operation"]["kind"] == "query"
                and (query == "ASK {}" or query in json.loads((root / "ask_queries.json").read_text()))):
            reply["result"].update(query_type="ASK", variables=[], items=[], boolean=True)
            result_file.write_text(json.dumps(reply))
raise SystemExit(command.returncode)
'''


class BusinessRepositoryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.repository = self.directory / "ontology"
        self.repository.mkdir()
        self.work = self.directory / "measurements"
        self.generated = self.directory / "generated"
        self.fake = self.directory / "fake"
        self.fake.mkdir()
        self.binary = self.fake / "ontokiln"
        self.binary.write_text("#!" + sys.executable + "\n" + WRAPPER)
        self.binary.chmod(0o755)
        (self.fake / "fixture_api.py").write_text(fixtures.FAKE_API)
        fixtures.write_json(self.fake / "kernel.json", fixtures.KERNEL)
        fixtures.write_json(self.fake / "ask_queries.json", [ASK])
        self.config = {"checked_shapes": 1}
        self.originals = {}
        self.put("schema.ttl", '<urn:business:s> <urn:business:p> "' + PRIVATE + '" .\n')

    def put(self, name, text):
        path = self.repository / name
        path.parent.mkdir(parents=True, exist_ok=True)
        data = text.encode() if isinstance(text, str) else text
        path.write_bytes(data)
        self.originals[name] = data
        return path

    def put_json(self, name, value):
        return self.put(name, json.dumps(value, sort_keys=True) + "\n")

    def prepare(self, **options):
        helper = module_at("business_repository_under_test", ROOT / "benchmarks/business_repository.py")
        core = module_at("business_repository_test_core", ROOT / "benchmarks/kernel_baseline.py")
        self.generated.mkdir(parents=True, exist_ok=True)
        result = helper.prepare_repository(
            repository=self.repository, generated=self.generated, work_dir=self.work,
            profile=options.pop("profile", "rdfs-property-chain-v1"),
            view=options.pop("view", "closure"), excludes=options.pop("excludes", []),
            shapes_root=options.pop("shapes_root", None), core=core, **options)
        compile_request = json.loads(Path(result["compile_request"]).read_text())
        cases = [json.loads(path.read_text()) for path in sorted(Path(result["read_requests"]).glob("*.json"))]
        return result["inventory"], compile_request, cases

    def run_driver(self, *extra, prepare_only=False, binary=True, repository=True, env=None):
        fixtures.write_json(self.fake / "config.json", self.config)
        command = [sys.executable, "-B", str(DRIVER), "--work-dir", str(self.work),
                   "--build-samples", "1", "--read-samples", "1", "--warmups", "0"]
        if repository:
            command.extend(["--ontology-repo", str(self.repository)])
        if binary:
            command.extend(["--binary", str(self.binary)])
        if prepare_only:
            command.append("--prepare-only")
        result = subprocess.run(command + list(extra), cwd=self.directory, capture_output=True,
                                text=True, timeout=30, env=env)
        self.last_result = result
        return result

    def report(self):
        paths = list(self.work.rglob("summary.json"))
        # The shareable summary is in a ZIP; only the local summary is a file.
        self.assertEqual(len(paths), 1, self.last_result.stdout + self.last_result.stderr)
        return json.loads(paths[0].read_text()), paths[0].parent

    def calls(self, kind=None):
        path = self.fake / "trace.jsonl"
        calls = [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
        return calls if kind is None else [c for c in calls if c["request"]["operation"]["kind"] == kind]

    def assert_repository_unchanged(self):
        files = {str(path.relative_to(self.repository)): path.read_bytes()
                 for path in self.repository.rglob("*") if path.is_file() and not path.is_symlink()}
        self.assertEqual(files, self.originals)

    def test_rdf_formats_document_identity_and_tests_examples_are_discovered(self):
        self.put("facts/a.nt", '<urn:a> <urn:p> "a" .\n')
        self.put("other/a.trig", '<urn:g> { <urn:a> <urn:p> "a" }\n')
        self.put("quads.nq", '<urn:a> <urn:p> "a" <urn:g> .\n')
        self.put("examples/example.ttl", '<urn:e> <urn:p> "example" .\n')
        self.put("tests/fixture.ttl", '<urn:t> <urn:p> "test" .\n')
        inventory, compile_request, cases = self.prepare()
        self.assertEqual(inventory["schema_version"], "ontokiln-business-repository/v1")
        operation = compile_request["operation"]
        self.assertEqual(operation["profile"], "rdfs-property-chain-v1")
        expected = {"schema.ttl": "turtle", "facts/a.nt": "ntriples", "other/a.trig": "trig",
                    "quads.nq": "nquads", "examples/example.ttl": "turtle", "tests/fixture.ttl": "turtle"}
        self.assertEqual([source["document_id"] for source in operation["sources"]], sorted(expected))
        for source in operation["sources"]:
            name = source["document_id"]
            self.assertEqual(source["path"], str(self.repository / name))
            self.assertEqual(source["format"], expected[name])
            self.assertEqual(source["expected_sha256"], hashlib.sha256(self.originals[name]).hexdigest())
            self.assertNotIn("base_iri", source)
        self.assertEqual(inventory["shacl"]["mode"], "DATASET_SHAPES")
        validations = [case["operation"] for case in cases if case["operation"]["kind"] == "validate"]
        self.assertEqual(len(validations), 1)
        self.assertIs(validations[0]["shapes_from_dataset"], True)
        self.assert_repository_unchanged()

    def test_native_rule_ir_is_distinct_from_queries_and_legacy_rules(self):
        rule = self.put_json("rules/native.json", RULE_IR)
        self.put_json("rules/legacy.json", [{"head": "urn:p", "body": ["urn:q"]}])
        self.put("queries/construct.rq", CONSTRUCT)
        inventory, compile_request, cases = self.prepare()
        self.assertEqual(compile_request["operation"]["rules"],
                         [{"path": str(rule), "expected_sha256": hashlib.sha256(rule.read_bytes()).hexdigest()}])
        self.assertTrue(any(asset["path"].endswith("rules/legacy.json") for asset in inventory["skipped_assets"]))
        probes = [case["operation"] for case in cases if case["operation"].get("derived_queries")]
        self.assertEqual(len(probes), 1)
        self.assertEqual(probes[0]["query"], "ASK {}")
        self.assertEqual(probes[0]["derived_queries"], [CONSTRUCT])
        self.assertTrue(any(case["mode"] == "INDEPENDENT_REQUEST_LOCAL_CONSTRUCT_PROBE"
                            for case in inventory["read_cases"]))
        self.assert_repository_unchanged()

    def test_select_ask_and_construct_keep_source_text_and_explicit_view(self):
        for name, text in [("select", SELECT), ("ask", ASK), ("construct", CONSTRUCT)]:
            self.put("queries/" + name + ".rq", text)
        _, compile_request, cases = self.prepare(profile="datalog-v1", view="asserted")
        self.assertEqual(compile_request["operation"]["profile"], "datalog-v1")
        queries = [case["operation"] for case in cases if case["operation"]["kind"] == "query"]
        self.assertEqual({query["query"] for query in queries}, {SELECT, ASK, "ASK {}"})
        self.assertTrue(all(query["view"] == "asserted" for query in queries))
        self.assertTrue(all(query.get("parameters", {}) == {} for query in queries))
        self.assertTrue(all(query.get("rdf_bindings", {}) == {} for query in queries))
        self.assertTrue(all(query.get("values", []) == [] for query in queries))
        self.assertEqual([query["derived_queries"] for query in queries if query.get("derived_queries")], [[CONSTRUCT]])
        self.assertFalse(any(source["path"].endswith(".rq") for source in compile_request["operation"]["rules"]))

    def test_common_parameter_prefix_is_not_a_missing_binding_until_used(self):
        prefix = "PREFIX param: <urn:ontokiln:param:>\n"
        unused = prefix + 'SELECT ?s WHERE { ?s <urn:business:p> "param:value" }\n'
        used = prefix + 'SELECT ?s WHERE { ?s <urn:business:p> param:value }\n'
        self.put("queries/common-prefix.rq", unused)
        self.put("queries/needs-binding.rq", used)
        inventory, _, cases = self.prepare()
        query_operations = [case["operation"] for case in cases if case["operation"]["kind"] == "query"]
        self.assertEqual([query["query"] for query in query_operations], [unused])
        skipped = {asset["relative_path"]: asset["reason_code"] for asset in inventory["skipped_assets"]}
        self.assertEqual(skipped, {"queries/needs-binding.rq": "QUERY_REQUIRES_PARAMETERS"})

    def test_technical_outputs_globs_and_symlinks_cannot_become_rdf_inputs(self):
        for name in (".git/private.ttl", "target/cache.ttl", "node_modules/library.ttl", "skip/explicit.ttl"):
            self.put(name, '<urn:excluded> <urn:p> "excluded" .\n')
        self.work = self.repository / "measurement-output"
        self.put("measurement-output/old-run/copied.nt", '<urn:old> <urn:p> "prior output" .\n')
        outside = self.directory / "outside.ttl"
        outside.write_text('<urn:outside> <urn:p> "outside" .\n')
        (self.repository / "linked.ttl").symlink_to(outside)
        (self.repository / "linked-directory").symlink_to(self.directory, target_is_directory=True)
        inventory, compile_request, _ = self.prepare(excludes=["skip/**"])
        self.assertEqual([source["document_id"] for source in compile_request["operation"]["sources"]], ["schema.ttl"])
        self.assertTrue(inventory["excluded_paths"])
        self.assertEqual(outside.read_text(), '<urn:outside> <urn:p> "outside" .\n')
        self.assert_repository_unchanged()

    def test_unbound_or_unsupported_assets_are_listed_instead_of_inventing_values(self):
        skipped = {
            "queries/parameter.rq": ('PREFIX p: <urn:ontokiln:param:> SELECT ?s WHERE { ?s <urn:p> p:value }',
                                     "QUERY_REQUIRES_PARAMETERS"),
            "queries/template.rq": ('SELECT ?s WHERE { ?s <urn:p> {{business_value}} }',
                                    "QUERY_REQUIRES_TEMPLATE_EXPANSION"),
            "queries/values.rq": ('SELECT ?s WHERE { VALUES ?s {} ?s ?p ?o }', "QUERY_REQUIRES_VALUES"),
            "queries/shape-local.rq": ('SELECT $this WHERE { $this ?p ?o }', "QUERY_REQUIRES_SHACL_BINDING"),
            "queries/describe.rq": ('DESCRIBE <urn:business:s>', "UNSUPPORTED_QUERY_FORM"),
            "queries/update.rq": ('DELETE WHERE { ?s ?p ?o }', "UNSUPPORTED_QUERY_FORM"),
            "queries/nonutf8.rq": (b"SELECT \xff", "NON_UTF8_QUERY"),
            "legacy.owl": ('<rdf:RDF/>', "UNSUPPORTED_RDF_FORMAT"),
            "legacy.rdf": ('<rdf:RDF/>', "UNSUPPORTED_RDF_FORMAT"),
            "legacy.jsonld": ('{"@id":"urn:x"}', "UNSUPPORTED_RDF_FORMAT"),
            "data.ttl.gz": (b"not a real compressed RDF fixture", "COMPRESSED_RDF_UNSUPPORTED"),
        }
        for name, (text, _) in skipped.items():
            self.put(name, text)
        self.put_json("rules/future.json", {"schema_version": "ontology-rule-ir/v2", "rules": []})
        inventory, compile_request, cases = self.prepare()
        actual = {asset["relative_path"]: asset["reason_code"] for asset in inventory["skipped_assets"]}
        for name, (_, reason) in skipped.items():
            self.assertEqual(actual.get(name), reason, name)
        self.assertEqual(actual.get("rules/future.json"), "UNSUPPORTED_RULE_IR_VERSION")
        self.assertEqual([source["document_id"] for source in compile_request["operation"]["sources"]], ["schema.ttl"])
        self.assertFalse(any(case["operation"]["kind"] == "query" for case in cases))

    def test_external_shapes_are_explicit_without_guessing_the_data_scope(self):
        inside = self.put("governance/shapes.ttl", SHAPES)
        inventory, compile_request, cases = self.prepare(shapes_root=inside.parent)
        self.assertEqual(inventory["shacl"]["mode"], "EXTERNAL_SHAPES")
        self.assertEqual(inventory["shacl"]["selection"], "DECLARED_TARGETS")
        self.assertIn(str(inside), [source["path"] for source in compile_request["operation"]["sources"]])
        validation = next(case["operation"] for case in cases if case["operation"]["kind"] == "validate")
        self.assertFalse(validation.get("shapes_from_dataset", False))
        self.assertEqual([source["path"] for source in validation["shapes"]], [str(inside)])
        self.assertIsNone(validation.get("focus_node"))
        self.assertIsNone(validation.get("selected_shapes"))
        outside = self.directory / "external-shapes"
        outside.mkdir()
        (outside / "policy.ttl").write_text(SHAPES)
        self.generated = self.directory / "generated-outside"
        _, compile_request, cases = self.prepare(shapes_root=outside)
        self.assertNotIn(str(outside / "policy.ttl"), [source["path"] for source in compile_request["operation"]["sources"]])
        validation = next(case["operation"] for case in cases if case["operation"]["kind"] == "validate")
        self.assertEqual([source["path"] for source in validation["shapes"]], [str(outside / "policy.ttl")])
        self.assert_repository_unchanged()

    def test_prepare_only_needs_no_binary_and_does_not_invoke_cargo(self):
        self.put("queries/select.rq", SELECT + "# " + PRIVATE + "\n")
        canary = self.directory / "tools"
        canary.mkdir()
        cargo = canary / "cargo"
        cargo.write_text("#!" + sys.executable + "\nfrom pathlib import Path\n"
                         "Path(__file__).with_name('cargo-was-invoked').write_text('unexpected build')\nraise SystemExit(97)\n")
        cargo.chmod(0o755)
        environment = dict(os.environ, PATH=str(canary))
        result = self.run_driver(prepare_only=True, binary=False, env=environment)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        report, directory = self.report()
        self.assertEqual(report["status"], "PREPARED")
        self.assertEqual(report["business_correctness"], "MANUAL_REVIEW_REQUIRED")
        self.assertFalse((canary / "cargo-was-invoked").exists())
        self.assertEqual(self.calls(), [])
        for name in ("generated/compile.request.json", "generated/inventory.json", "results.zip"):
            self.assertTrue((directory / name).is_file(), name)
        self.assertTrue(list((directory / "generated/read-cases").glob("*.json")))
        self.assertFalse(any(stage.get("samples") for stage in report.get("stages", [])))
        with zipfile.ZipFile(directory / "results.zip") as archive:
            for name in archive.namelist():
                self.assertNotIn(PRIVATE.encode(), archive.read(name), name)
                self.assertNotIn("request", name)
                self.assertNotIn("inventory", name)
        self.assert_repository_unchanged()

    def test_repository_cli_executes_independent_queries_and_labels_construct_probe(self):
        self.put("governance/shapes.ttl", SHAPES)
        self.put_json("rules/native.json", RULE_IR)
        for name, text in [("select", SELECT), ("ask", ASK), ("construct", CONSTRUCT)]:
            self.put("queries/" + name + ".rq", text)
        result = self.run_driver()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        report, directory = self.report()
        self.assertEqual(report["status"], "COMPLETE")
        self.assertEqual(report["input_mode"], "ontology-repository")
        self.assertEqual(report["business_correctness"], "MANUAL_REVIEW_REQUIRED")
        self.assertEqual(report["coverage"]["whole_business_workflow"], "NOT_VERIFIED")
        self.assertEqual(report["coverage"]["unexecuted_assets"], 0)
        self.assertEqual(len(self.calls("compile")), 1)
        self.assertEqual(len(self.calls("materialize")), 1)
        self.assertEqual(len(self.calls("query")), 3)
        self.assertEqual(len(self.calls("validate")), 1)
        compile_operation = self.calls("compile")[0]["request"]["operation"]
        self.assertEqual(compile_operation["profile"], "rdfs-property-chain-v1")
        self.assertEqual(len(compile_operation["rules"]), 1)
        queries = [call["request"]["operation"] for call in self.calls("query")]
        self.assertEqual({query["query"] for query in queries}, {SELECT, ASK, "ASK {}"})
        self.assertTrue(all(query["view"] == "closure" for query in queries))
        self.assertEqual([query["derived_queries"] for query in queries if query.get("derived_queries")], [[CONSTRUCT]])
        probe = next(case for case in report["discovery"]["read_cases"]
                     if case["mode"] == "INDEPENDENT_REQUEST_LOCAL_CONSTRUCT_PROBE")
        self.assertIn("construct-probe", probe["name"])
        self.assertEqual(json.loads((directory / "generated/inventory.json").read_text()), report["discovery"])
        self.assert_repository_unchanged()

    def test_skipped_assets_and_empty_validation_are_coverage_gaps_not_success(self):
        for reason in ("unbound-query", "zero-shapes"):
            with self.subTest(reason=reason):
                self.work = self.directory / ("measurements-" + reason)
                (self.fake / "trace.jsonl").unlink(missing_ok=True)
                if reason == "unbound-query":
                    self.put("query.rq", 'SELECT ?s WHERE { ?s <urn:p> <urn:ontokiln:param:unprovided> }')
                else:
                    (self.repository / "query.rq").unlink()
                    del self.originals["query.rq"]
                    self.config["checked_shapes"] = 0
                result = self.run_driver()
                self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
                report, _ = self.report()
                self.assertEqual(report["status"], "COMPLETE_WITH_GAPS")
                self.assertEqual(report["business_correctness"], "MANUAL_REVIEW_REQUIRED")
                self.assertTrue(all(sample["status"] == "COMPLETE"
                                    for stage in report["stages"] for sample in stage["samples"]))
                if reason == "unbound-query":
                    self.assertEqual(report["coverage"]["unexecuted_assets"], 1)
                    self.assertEqual(self.calls("query"), [])
                else:
                    self.assertEqual(report["coverage"]["unexecuted_assets"], 0)
                    self.assertTrue(report["coverage"]["empty_validation_cases"])

    def test_empty_repository_is_setup_failure_without_engine_execution(self):
        (self.repository / "schema.ttl").unlink()
        del self.originals["schema.ttl"]
        self.put("only-query.rq", SELECT)
        result = self.run_driver(prepare_only=True, binary=False)
        self.assertNotEqual(result.returncode, 0)
        report, directory = self.report()
        self.assertEqual(report["status"], "SETUP_FAILED")
        self.assertEqual(self.calls(), [])
        self.assertTrue((directory / "generated/inventory.json").is_file())
        self.assert_repository_unchanged()

    def test_cli_rejects_mixed_input_modes_and_repository_options_in_template_mode(self):
        template = self.directory / "compile.json"
        fixtures.write_json(template, {"contract": "ontokiln-api/v1", "request_id": "existing-template",
                                      "operation": {"kind": "compile", "profile": "no-rules-v1", "output": "unused",
                                                    "sources": [{"path": str(self.repository / "schema.ttl"),
                                                                 "document_id": "schema.ttl", "format": "turtle",
                                                                 "expected_sha256": fixtures.sha256(self.repository / "schema.ttl")}]}})
        for extra, repo in [(["--compile-request", str(template)], True),
                            (["--read-requests", str(self.repository)], True),
                            (["--compile-request", str(template), "--prepare-only"], False),
                            (["--compile-request", str(template), "--query-view", "closure"], False),
                            (["--compile-request", str(template), "--profile", "rdfs-property-chain-v1"], False)]:
            with self.subTest(arguments=extra):
                result = self.run_driver(*extra, repository=repo)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(self.calls(), [])

    def test_explicit_missing_binary_does_not_fall_back_or_discard_generated_requests(self):
        result = self.run_driver("--binary", str(self.directory / "missing-engine"))
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        report, directory = self.report()
        self.assertEqual(report["status"], "SETUP_FAILED")
        self.assertTrue((directory / "generated/compile.request.json").is_file())
        self.assertEqual(self.calls(), [])
        main = module_at("business_repository_binary_check", DRIVER)
        args = SimpleNamespace(binary=self.directory / "missing-engine", ontology_repo=self.repository)
        with patch.object(main.shutil, "which", side_effect=AssertionError("explicit engine must not trigger fallback")):
            with self.assertRaisesRegex(ValueError, "explicit binary"):
                main.ensure_binary(args, directory, {})

    def test_automatic_build_uses_only_fake_cargo_with_checkout_target_outside_measurements(self):
        main = module_at("business_repository_fake_build", DRIVER)
        checkout = self.directory / "fake-engine-checkout"
        checkout.mkdir()
        directory = self.directory / "build-preparation"
        directory.mkdir()
        cargo = self.fake / "cargo"
        trace = self.fake / "cargo-trace.json"
        cargo.write_text("#!" + sys.executable + "\nimport json, os, sys\nfrom pathlib import Path\n"
                         + "Path(" + repr(str(trace)) + ").write_text(json.dumps({'cwd': os.getcwd(), 'argv': sys.argv[1:]}))\n"
                         + "target = Path(sys.argv[sys.argv.index('--target-dir') + 1]) / 'release/ontokiln'\n"
                         + "target.parent.mkdir(parents=True)\n"
                         + "target.write_text('#!/bin/sh\\nexit 0\\n')\ntarget.chmod(0o755)\n")
        cargo.chmod(0o755)
        args = SimpleNamespace(binary=None, ontology_repo=self.repository,
                               build_profile="unspecified", build_features="unspecified")
        report = {"stages": []}
        cwd = Path.cwd()
        with contextlib.redirect_stdout(io.StringIO()), patch.object(main, "ROOT", checkout), patch.object(main.shutil, "which",
                side_effect=lambda name: str(cargo) if name == "cargo" else None):
            main.ensure_binary(args, directory, report)
        recorded = json.loads(trace.read_text())
        self.assertEqual(recorded["cwd"], str(checkout))
        self.assertEqual(recorded["argv"], ["build", "--release", "--locked", "--bin", "ontokiln",
                                            "--target-dir", str(checkout / "target")])
        self.assertEqual(args.binary, checkout / "target/release/ontokiln")
        self.assertEqual(args.build_profile, "release")
        self.assertEqual(args.build_features, "default")
        self.assertEqual(Path.cwd(), cwd)
        self.assertEqual(report["stages"], [])
        self.assertEqual(report["engine_preparation"]["method"], "cargo-build")
        self.assertEqual(report["engine_preparation"]["status"], "READY")
        self.assertEqual(report["engine_preparation"]["returncode"], 0)


if __name__ == "__main__":
    unittest.main()
