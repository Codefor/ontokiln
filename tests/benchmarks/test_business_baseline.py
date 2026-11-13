"""Exercise business benchmark orchestration with a small external fake API.

The fake verifies request digests and snapshot guards, but deliberately does
not implement RDF semantics. These tests prove harness behavior, not business
answers or OntoKiln performance; native engine smoke tests remain separate.
"""
import copy
import ctypes
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
import zipfile


ROOT = Path(__file__).resolve().parents[2]
DRIVER = ROOT / "benchmarks/business_baseline.py"
PRIVATE_MARKER = "private-business-content-not-for-metadata-archive"
KERNEL = {"version": "test-fake-api", "source_revision": "1" * 40,
          "kernel_sha256": "2" * 64, "contract_version": "ontokiln-core/v3",
          "pack_version": 3, "capabilities": ["ontokiln-api/v1"]}


FAKE_API = r'''
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

HOME_DIR = Path(__file__).resolve().parent
KERNEL = json.loads((HOME_DIR / "kernel.json").read_text())
CONFIG = json.loads((HOME_DIR / "config.json").read_text())

def digest(data):
    return hashlib.sha256(data).hexdigest()

def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()

def emit(path, value):
    path.write_text(json.dumps(value))

if sys.argv[1:] == ["identity"]:
    print(json.dumps(KERNEL))
    raise SystemExit(0)
if sys.argv[1:] == ["full-info"]:
    print(json.dumps({"engine": "fake-api-only", "kernel": KERNEL}))
    raise SystemExit(0)
if sys.argv[1] != "api":
    raise SystemExit("unexpected fake API command")
request_path = Path(sys.argv[sys.argv.index("--request") + 1])
result_path = Path(sys.argv[sys.argv.index("--result") + 1])
raw = request_path.read_bytes()
request = json.loads(raw)
operation = request["operation"]
kind = operation["kind"]
trace_path = HOME_DIR / "trace.jsonl"
previous = [json.loads(line) for line in trace_path.read_text().splitlines()] if trace_path.exists() else []
ordinal = 1 + sum(row["request"]["operation"]["kind"] == kind for row in previous)
with trace_path.open("a") as trace:
    trace.write(json.dumps({"request": request, "request_path": str(request_path),
                           "result_path": str(result_path), "cwd": os.getcwd(),
                           "environment": {key: os.environ.get(key) for key in
                                           ["RAYON_NUM_THREADS", "TOKIO_WORKER_THREADS", "TMPDIR"]}}) + "\n")
print("private-business-content-not-for-metadata-archive")
action = next((item for item in CONFIG.get("actions", [])
               if item["kind"] == kind and item.get("ordinal", 1) == ordinal), {})
mode = action.get("mode")
if mode == "hang_with_child":
    child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
    temporary = HOME_DIR / "processes.tmp"
    emit(temporary, {"api_pid": os.getpid(), "api_pgid": os.getpgrp(),
                     "child_pid": child.pid, "child_pgid": os.getpgid(child.pid)})
    temporary.replace(HOME_DIR / "processes.json")
    time.sleep(60)
    raise SystemExit("hanging fake should have been interrupted")
if mode in ("nonzero_exit", "private_process_failure"):
    if mode == "private_process_failure":
        print("private-business-content-not-for-metadata-archive", file=sys.stderr, flush=True)
    raise SystemExit(7)
if mode == "missing_result":
    raise SystemExit(0)
if mode == "malformed_json":
    result_path.write_text("{not-json")
    raise SystemExit(0)
reply = {"contract": "ontokiln-api/v1", "request_id": request["request_id"],
         "request_sha256": digest(raw), "status": "COMPLETE", "result": {}, "error": None,
         "kernel": KERNEL, "execution": {"worker_reaped": True, "worker_pid": 123,
             "supervisor_pid": os.getpid(), "integrity_checked": True,
             "metrics": {"execution_seconds": 0.001, "peak_rss_bytes": 4096,
                         "supervisor_peak_rss_bytes": 4096, "peak_scratch_bytes": 1024}}}
error = None
for source in operation.get("sources", []) + operation.get("rules", []) + operation.get("shapes", []):
    if digest(Path(source["path"]).read_bytes()) != source["expected_sha256"]:
        error = ("FAILED", "SOURCE_INVALID", "source digest mismatch")
if kind in ("query", "validate"):
    manifest_bytes = (Path(operation["dataset"]) / "manifest.json").read_bytes()
    manifest = json.loads(manifest_bytes)
    if (operation.get("expected_content_digest") != manifest["content_digest"] or
            operation.get("expected_manifest_sha256") != digest(manifest_bytes)):
        error = ("FAILED", "INVALID_DATASET", "read did not bind both published snapshot digests")
if mode == "budget_failure":
    error = ("BUDGET_EXHAUSTED", "RESOURCE_EXHAUSTED", "deliberate test budget failure")
    if action.get("leave_partial_artifact"):
        partial = Path(operation["output"])
        partial.mkdir(parents=True)
        (partial / "failure-evidence.txt").write_text("retain failed-run evidence")
if mode == "private_error":
    error = ("FAILED", "EXECUTION_FAILED", "private-business-content-not-for-metadata-archive")
if error:
    reply.update(status=error[0], result=None, error={"code": error[1], "message": error[2]})
else:
    if kind in ("compile", "materialize"):
        output = Path(operation["output"])
        output.mkdir(parents=True, exist_ok=False)
        if kind == "compile":
            profile = operation["profile"]
            source_digest = operation.get("expected_source_tree_digest") or "3" * 64
        else:
            source = json.loads((Path(operation["dataset"]) / "manifest.json").read_text())
            if source["kind"] != "compiled":
                raise SystemExit("materialize must consume a compiled artifact")
            profile, source_digest = source["profile_id"], source["source_tree_digest"]
        database = b"fake database private-business-content-not-for-metadata-archive"
        (output / "facts.sqlite").write_bytes(database)
        files = {"facts.sqlite": {"bytes": len(database), "sha256": digest(database)}}
        stats = {"explicit_facts": 1, "inferred_facts": int(kind == "materialize"),
                 "indexed_facts": 1 + int(kind == "materialize"), "parsed_facts": 1,
                 "distinct_terms": 3, "fixpoint_iterations": int(kind == "materialize")}
        manifest = {"schema_version": "kernel-manifest-v1", "format_version": "ontology-kernel-sqlite/v1",
                    "read_contract": "ontokiln-sqlite-read/v1", "engine_semantic_version": "positive-rdf-v5",
                    "completion": "COMPLETE", "kind": "compiled" if kind == "compile" else "reasoned",
                    "profile_id": profile, "source_tree_digest": source_digest,
                    "semantic_fingerprint": "4" * 64, "source_provenance_version": "explicit-doc-v1",
                    "stats": stats, "files": files, "content_digest": digest(canonical(files))}
        if kind == "materialize":
            manifest.update(fixpoint_reached=True, compiled_content_digest=source["content_digest"])
        else:
            # Real compiled manifests omit this counter; API v1 returns zero.
            manifest["stats"] = {key: value for key, value in stats.items() if key != "inferred_facts"}
        if mode == "wrong_profile":
            manifest["profile_id"] = profile = "no-rules-v1"
        if mode == "wrong_compiled_content_digest":
            manifest["compiled_content_digest"] = "0" * 64
        manifest_bytes = canonical(manifest)
        (output / "manifest.json").write_bytes(manifest_bytes)
        artifact = {"path": str(output), "manifest_sha256": digest(manifest_bytes),
                    "content_digest": manifest["content_digest"], "source_tree_digest": source_digest,
                    "semantic_fingerprint": manifest["semantic_fingerprint"], "profile_id": profile}
        reply["result"] = {"dataset": str(output), "artifact": artifact,
                           "stats": {key: stats[key] for key in ("explicit_facts", "inferred_facts")},
                           "fixpoint_reached": kind == "materialize",
                           "read_contract": "ontokiln-sqlite-read/v1", "proof_coverage": {"per_fact": "not_recorded"}}
    elif kind == "query":
        reply["result"] = {"query_type": "SELECT", "variables": ["s"],
                           "items": [{"s": {"type": "uri", "value": "urn:fake:unverified-answer"}}],
                           "boolean": None, "view": operation.get("view", "asserted"),
                           "complete": True, "truncated": False, "proof_coverage": {"per_fact": "not_recorded"}}
    elif kind == "validate":
        conforms = CONFIG.get("validation_conforms", True)
        count = CONFIG.get("checked_shapes", 0)
        violations = [] if conforms else [{"focus_node": "urn:fake:focus", "shape_ref": "urn:fake:shape",
                                          "path": "urn:fake:p", "message": "deliberate violation"}]
        reply["result"] = {"conforms": conforms, "checked_shapes": count, "violations": len(violations),
                           "report": {"conforms": conforms, "violations": violations,
                                      "target_count": count, "coverage": [], "kernel": KERNEL}}
    else:
        raise SystemExit("unexpected fake API operation")
if mode == "wrong_request_sha256":
    reply["request_sha256"] = "0" * 64
elif mode == "wrong_request_id":
    reply["request_id"] = "some-other-request"
elif mode == "wrong_contract":
    reply["contract"] = "ontokiln-api/v999"
elif mode == "wrong_kernel":
    reply["kernel"] = dict(KERNEL, kernel_sha256="0" * 64)
elif mode == "worker_not_reaped":
    reply["execution"]["worker_reaped"] = False
elif mode == "metrics_not_object":
    reply["execution"]["metrics"] = ["invalid metrics object"]
elif mode == "success_with_error":
    reply["error"] = {"code": "EXECUTION_FAILED", "message": "conflicting success"}
elif mode == "partial_query":
    reply["result"].update(complete=False, truncated=True)
elif mode == "wrong_artifact_path":
    reply["result"]["artifact"]["path"] = str(HOME_DIR)
elif mode == "wrong_manifest_sha256":
    reply["result"]["artifact"]["manifest_sha256"] = "0" * 64
elif mode == "wrong_content_digest":
    reply["result"]["artifact"]["content_digest"] = "0" * 64
elif mode == "inconsistent_stats":
    reply["result"]["stats"]["explicit_facts"] = 999
elif mode == "no_fixpoint":
    reply["result"]["fixpoint_reached"] = False
elif mode == "inconsistent_validation_conforms":
    reply["result"]["report"]["conforms"] = not reply["result"]["conforms"]
elif mode == "conforms_with_violations":
    reply["result"]["conforms"] = True
    reply["result"]["report"]["conforms"] = True
emit(result_path, reply)
'''


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class BusinessBaselineTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.input_cwd = self.directory / "input"
        self.input_cwd.mkdir()
        self.fake_directory = self.directory / "fake"
        self.fake_directory.mkdir()
        self.binary = self.fake_directory / "ontokiln"
        self.binary.write_text("#!" + sys.executable + "\n" + FAKE_API)
        self.binary.chmod(0o755)
        write_json(self.fake_directory / "kernel.json", KERNEL)
        self.config = {}
        self.work = self.directory / "work"
        self.original_output = self.input_cwd / "original-output"
        self.original_output.mkdir()
        (self.original_output / "do-not-delete.txt").write_text(PRIVATE_MARKER)
        for name, text in {"rdf/facts.nt": '<urn:business:s> <urn:business:p> "' + PRIVATE_MARKER + '" .\n',
                           "rules/rules.rq": "CONSTRUCT { ?s <urn:business:q> ?o } WHERE { ?s <urn:business:p> ?o }\n",
                           "rdf/shapes.ttl": "<urn:business:shape> a <http://www.w3.org/ns/shacl#NodeShape> .\n"}.items():
            path = self.input_cwd / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        self.compile = {"contract": "ontokiln-api/v1", "request_id": "consumer-compile",
                        "operation": {"kind": "compile", "output": "original-output", "profile": "datalog-v1",
                                      "sources": [self.source("rdf/facts.nt", "ntriples")],
                                      "rules": [{"path": "rules/rules.rq",
                                                 "expected_sha256": sha256(self.input_cwd / "rules/rules.rq")}],
                                      "import_lock": {}, "expected_source_tree_digest": "5" * 64}}
        self.compile_path = self.input_cwd / "templates/compile.json"
        self.read_directory = self.input_cwd / "templates/reads"
        self.read_directory.mkdir(parents=True)
        self.query = {"contract": "ontokiln-api/v1", "request_id": "consumer-query",
                      "operation": {"kind": "query", "dataset": "old-business-snapshot",
                                    "expected_content_digest": "a" * 64,
                                    "expected_manifest_sha256": "b" * 64,
                                    "query": "SELECT ?s WHERE { ?s <urn:business:p> <urn:ontokiln:param:thing> }",
                                    "parameters": {"thing": {"type": "literal", "value": PRIVATE_MARKER, "language": "en"}},
                                    "rdf_bindings": {"s": "<urn:business:s>"},
                                    "values": [{"variables": ["x"], "rows": [[{"type": "iri", "value": "urn:business:x"}], [None]]}],
                                    "derived_queries": ["CONSTRUCT { ?s <urn:business:q> ?o } WHERE { ?s <urn:business:p> ?o }"],
                                    "view": "closure", "require": ["select-ask/v1"], "revision": None}}
        self.validation = {"contract": "ontokiln-api/v1", "request_id": "consumer-validation",
                           "operation": {"kind": "validate", "dataset": "old-business-snapshot",
                                         "shapes": [self.source("rdf/shapes.ttl", "turtle")],
                                         "shapes_from_dataset": False, "selected_shapes": [],
                                         "focus_node": "<urn:business:focus>"}}

    def source(self, name, rdf_format):
        return {"path": name, "document_id": "urn:document:" + name,
                "format": rdf_format, "expected_sha256": sha256(self.input_cwd / name),
                "base_iri": "urn:business:base/"}

    def configure(self, **options):
        self.config.update(options)

    def command(self, *extra, reads=True, build_samples=1, read_samples=1, warmups=0):
        write_json(self.fake_directory / "config.json", self.config)
        write_json(self.compile_path, self.compile)
        if reads:
            write_json(self.read_directory / "nested/query.json", self.query)
            write_json(self.read_directory / "validate.json", self.validation)
        command = [sys.executable, "-B", str(DRIVER), "--binary", str(self.binary),
                   "--compile-request", str(self.compile_path), "--input-cwd", str(self.input_cwd),
                   "--work-dir", str(self.work), "--build-samples", str(build_samples),
                   "--read-samples", str(read_samples), "--warmups", str(warmups)]
        if reads:
            command.extend(["--read-requests", str(self.read_directory)])
        return command + list(extra)

    def run_driver(self, *extra, **options):
        result = subprocess.run(self.command(*extra, **options), cwd=self.directory, capture_output=True,
                                text=True, timeout=30)
        self.last_result = result
        return result

    def calls(self, kind=None):
        path = self.fake_directory / "trace.jsonl"
        rows = [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
        return rows if kind is None else [row for row in rows if row["request"]["operation"]["kind"] == kind]

    def summary(self):
        paths = list(self.work.rglob("summary.json"))
        self.assertEqual(len(paths), 1, self.last_result.stdout + self.last_result.stderr)
        return json.loads(paths[0].read_text()), paths[0].parent

    def assert_success(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def stage(self, report, kind):
        stages = [stage for stage in report["stages"] if stage["kind"] == kind]
        self.assertEqual(len(stages), 1, kind)
        return stages[0]

    def fresh_run(self, name):
        self.work = self.directory / ("work-" + name)
        (self.fake_directory / "trace.jsonl").unlink(missing_ok=True)
        self.config = {}

    def test_templates_are_preserved_and_reads_bind_first_retained_artifact(self):
        self.compile["limits"] = {"memory_bytes": 123456789, "scratch_bytes": 234567890,
                                  "timeout_ms": 4321, "max_rows": 7, "max_bytes": 987,
                                  "max_iterations": 5, "max_derived_facts": 6,
                                  "max_intermediate_rows": 9, "max_validation_depth": 3,
                                  "max_term_bytes": 4567}
        self.query["limits"] = {"max_rows": 0, "max_bytes": 0, "max_derived_facts": 0,
                                "max_intermediate_rows": 0, "max_iterations": 0}
        self.validation["limits"] = {"max_validation_depth": 7, "max_rows": 4}
        originals = [copy.deepcopy(value) for value in (self.compile, self.query, self.validation)]
        (self.read_directory / "ignored.txt").write_text("this is not a JSON request")
        result = self.run_driver(build_samples=3, read_samples=3, warmups=1)
        self.assert_success(result)
        calls = self.calls()
        self.assertEqual([row["request"]["operation"]["kind"] for row in calls[:6]],
                         ["compile"] * 3 + ["materialize"] * 3)
        compiled = [Path(row["request"]["operation"]["output"]) for row in self.calls("compile")]
        reasoned = [Path(row["request"]["operation"]["output"]) for row in self.calls("materialize")]
        self.assertEqual(len(set(compiled + reasoned)), 6)
        self.assertTrue(all(path.is_dir() for path in (compiled[0], reasoned[0])))
        self.assertTrue(all(not path.exists() for path in compiled[1:] + reasoned[1:]))
        self.assertTrue(all(row["request"]["operation"]["dataset"] == str(compiled[0])
                            for row in self.calls("materialize")))
        expected_compile = copy.deepcopy(originals[0]["operation"])
        for source in expected_compile["sources"] + expected_compile["rules"]:
            source["path"] = str(self.input_cwd / source["path"])
        for call in self.calls("compile"):
            operation = copy.deepcopy(call["request"]["operation"])
            operation["output"] = expected_compile["output"]
            self.assertEqual(operation, expected_compile)
            self.assertEqual(call["request"]["limits"], self.compile["limits"])
        manifest_bytes = (reasoned[0] / "manifest.json").read_bytes()
        manifest = json.loads(manifest_bytes)
        for kind, template in [("query", originals[1]), ("validate", originals[2])]:
            self.assertEqual(len(self.calls(kind)), 4)
            expected = copy.deepcopy(template["operation"])
            expected.update(dataset=str(reasoned[0]), expected_content_digest=manifest["content_digest"],
                            expected_manifest_sha256=hashlib.sha256(manifest_bytes).hexdigest())
            for source in expected.get("shapes", []):
                source["path"] = str(self.input_cwd / source["path"])
            for call in self.calls(kind):
                self.assertEqual(call["request"]["operation"], expected)
                for key, value in template["limits"].items():
                    self.assertEqual(call["request"]["limits"][key], value)
        for call in calls:
            self.assertEqual(call["environment"]["RAYON_NUM_THREADS"], "1")
            self.assertTrue(Path(call["environment"]["TMPDIR"]).is_relative_to(self.work))
        self.assertEqual(json.loads(self.compile_path.read_text()), originals[0])
        self.assertEqual(json.loads((self.read_directory / "nested/query.json").read_text()), originals[1])
        self.assertEqual(json.loads((self.read_directory / "validate.json").read_text()), originals[2])
        self.assertEqual((self.original_output / "do-not-delete.txt").read_text(), PRIVATE_MARKER)
        report, _ = self.summary()
        for kind in ("query", "validate"):
            samples = self.stage(report, kind)["samples"]
            self.assertEqual(sum(bool(sample["warmup"]) for sample in samples), 1)
            self.assertEqual(sum(not sample["warmup"] for sample in samples), 3)

    def test_missing_limits_use_documented_defaults_and_cli_overrides_win(self):
        self.compile["limits"] = {"max_rows": 19, "max_validation_depth": 11}
        self.assert_success(self.run_driver(reads=False))
        for call in self.calls():
            limits = call["request"]["limits"]
            for key, value in {"memory_bytes": 4 * 1024**3, "scratch_bytes": 8 * 1024**3,
                               "timeout_ms": 300000, "max_derived_facts": 10000000,
                               "max_intermediate_rows": 100000000, "max_iterations": 100,
                               "max_rows": 19, "max_validation_depth": 11}.items():
                self.assertEqual(limits[key], value, key)
        self.fresh_run("overrides")
        self.compile["limits"].update(memory_bytes=999, scratch_bytes=999, timeout_ms=999,
                                      max_derived_facts=999, max_intermediate_rows=999, max_iterations=999)
        self.assert_success(self.run_driver("--memory-mib", "8", "--scratch-mib", "16",
                                            "--timeout-seconds", "1.5", "--max-derived-facts", "0",
                                            "--max-intermediate-rows", "13", "--max-iterations", "0"))
        for call in self.calls():
            limits = call["request"]["limits"]
            for key, value in {"memory_bytes": 8 * 1024**2, "scratch_bytes": 16 * 1024**2,
                               "timeout_ms": 1500, "max_derived_facts": 0,
                               "max_intermediate_rows": 13, "max_iterations": 0}.items():
                self.assertEqual(limits[key], value, key)
        self.assertEqual(self.calls("compile")[0]["request"]["limits"]["max_rows"], 19)

    def test_unbounded_limits_require_explicit_bounded_override(self):
        for field, bad in [("memory_bytes", None), ("scratch_bytes", None), ("timeout_ms", None),
                           ("timeout_ms", 0), ("timeout_ms", float("inf"))]:
            with self.subTest(field=field, value=bad):
                self.fresh_run(field + "-" + str(bad))
                self.compile["limits"] = {field: bad}
                result = self.run_driver(reads=False)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(self.calls(), [], "invalid budgets must fail before API execution")
        self.fresh_run("bounded-null")
        self.compile["limits"] = {"memory_bytes": None, "scratch_bytes": None, "timeout_ms": None}
        self.assert_success(self.run_driver("--memory-mib", "8", "--scratch-mib", "16",
                                            "--timeout-seconds", "2", reads=False))
        self.assertEqual(self.calls("compile")[0]["request"]["limits"]["timeout_ms"], 2000)

    def test_input_hash_mismatch_is_rejected_without_rewriting_template(self):
        for file_name in ("rdf/facts.nt", "rules/rules.rq", "rdf/shapes.ttl"):
            with self.subTest(file=file_name):
                self.fresh_run(file_name.replace("/", "-"))
                path = self.input_cwd / file_name
                original = path.read_bytes()
                path.write_bytes(original + b"# changed after request preparation\n")
                try:
                    result = self.run_driver()
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(self.calls(), [], "bad input must be detected before measuring")
                    self.assertEqual(json.loads(self.compile_path.read_text()), self.compile)
                    self.assertEqual(json.loads((self.read_directory / "validate.json").read_text()), self.validation)
                finally:
                    path.write_bytes(original)

    def test_read_directory_rejects_non_v1_or_non_read_templates_before_execution(self):
        original = copy.deepcopy(self.query)
        for contract, kind in [("ontokiln-api/v2", "query"), ("ontokiln-api/v1", "compile"),
                               ("ontokiln-api/v1", "capabilities")]:
            with self.subTest(contract=contract, kind=kind):
                self.fresh_run(contract.rsplit("/", 1)[-1] + "-" + kind)
                self.query = copy.deepcopy(original)
                self.query["contract"] = contract
                self.query["operation"]["kind"] = kind
                result = self.run_driver()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(self.calls(), [])

    def test_omitted_asserted_view_and_dataset_shapes_selection_are_not_rewritten(self):
        for field in ("view", "expected_content_digest", "expected_manifest_sha256", "revision"):
            self.query["operation"].pop(field)
        self.validation["operation"].update(shapes=[], shapes_from_dataset=True, selected_shapes=None, focus_node=None)
        self.compile["operation"]["import_lock"] = None
        self.assert_success(self.run_driver("--read-dataset", "compiled"))
        query = self.calls("query")[0]["request"]["operation"]
        self.assertNotIn("view", query)
        self.assertNotIn("revision", query)
        validation = self.calls("validate")[0]["request"]["operation"]
        self.assertEqual(validation["shapes"], [])
        self.assertIs(validation["shapes_from_dataset"], True)
        self.assertIsNone(validation["selected_shapes"])
        self.assertIsNone(validation["focus_node"])
        self.assertIsNone(self.calls("compile")[0]["request"]["operation"]["import_lock"])
        report, _ = self.summary()
        self.assertEqual(self.stage(report, "query")["samples"][0]["observation"]["view"], "asserted")

    def test_first_compile_failure_skips_all_dependent_execution(self):
        self.configure(actions=[{"kind": "compile", "mode": "budget_failure"}])
        result = self.run_driver(build_samples=3, read_samples=3, warmups=1)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual([row["request"]["operation"]["kind"] for row in self.calls()], ["compile"])
        report, _ = self.summary()
        self.assertNotEqual(report["status"], "COMPLETE")
        samples = self.stage(report, "compile")["samples"]
        self.assertEqual([sample["status"] for sample in samples], ["API_FAILED", "SKIPPED", "SKIPPED"])
        self.assertEqual(samples[0]["api_status"], "BUDGET_EXHAUSTED")
        self.assertEqual(samples[0]["error"]["code"], "RESOURCE_EXHAUSTED")
        for kind in ("materialize", "query", "validate"):
            self.assertTrue(all(sample["status"] == "SKIPPED" for sample in self.stage(report, kind)["samples"]))

    def test_later_compile_failure_keeps_first_artifact_and_independent_stages_run(self):
        self.configure(actions=[{"kind": "compile", "ordinal": 2, "mode": "budget_failure",
                                 "leave_partial_artifact": True}])
        result = self.run_driver(build_samples=3)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(self.calls("compile")), 2)
        self.assertEqual(len(self.calls("materialize")), 3)
        self.assertEqual(len(self.calls("query")), 1)
        self.assertEqual(len(self.calls("validate")), 1)
        first = self.calls("compile")[0]["request"]["operation"]["output"]
        self.assertTrue(Path(first).is_dir())
        failed = Path(self.calls("compile")[1]["request"]["operation"]["output"])
        self.assertEqual((failed / "failure-evidence.txt").read_text(), "retain failed-run evidence")
        self.assertTrue(all(row["request"]["operation"]["dataset"] == first for row in self.calls("materialize")))
        report, _ = self.summary()
        samples = self.stage(report, "compile")["samples"]
        self.assertEqual([sample["status"] for sample in samples], ["COMPLETE", "API_FAILED", "SKIPPED"])
        self.assertEqual(samples[1]["api_status"], "BUDGET_EXHAUSTED")
        self.assertEqual(samples[1]["error"]["code"], "RESOURCE_EXHAUSTED")
        self.assertEqual(self.stage(report, "query")["samples"][0]["status"], "COMPLETE")

    def test_materialize_failure_blocks_reasoned_reads_but_not_compiled_reads(self):
        for dataset in ("reasoned", "compiled"):
            with self.subTest(dataset=dataset):
                self.fresh_run(dataset)
                self.configure(actions=[{"kind": "materialize", "mode": "budget_failure"}])
                result = self.run_driver("--read-dataset", dataset, build_samples=3, read_samples=2)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(len(self.calls("materialize")), 1)
                self.assertEqual(len(self.calls("query")), 0 if dataset == "reasoned" else 2)
                self.assertEqual(len(self.calls("validate")), 0 if dataset == "reasoned" else 2)
                if dataset == "compiled":
                    first = self.calls("compile")[0]["request"]["operation"]["output"]
                    self.assertTrue(all(row["request"]["operation"]["dataset"] == first
                                        for row in self.calls("query") + self.calls("validate")))
                    self.assertEqual(self.calls("query")[0]["request"]["operation"]["view"], "closure")

    def test_malformed_or_unbound_responses_never_count_as_success(self):
        for mode in ("nonzero_exit", "missing_result", "malformed_json", "wrong_request_sha256",
                     "wrong_request_id", "wrong_contract", "wrong_kernel", "worker_not_reaped",
                     "success_with_error", "partial_query", "metrics_not_object"):
            with self.subTest(mode=mode):
                self.fresh_run(mode)
                self.configure(actions=[{"kind": "query", "mode": mode}])
                result = self.run_driver(read_samples=3, warmups=1)
                self.assertNotEqual(result.returncode, 0, mode)
                self.assertEqual(len(self.calls("query")), 1)
                self.assertEqual(len(self.calls("validate")), 4, "a failed query must not cancel independent validation")
                report, _ = self.summary()
                samples = self.stage(report, "query")["samples"]
                self.assertNotEqual(samples[0]["status"], "COMPLETE")
                self.assertIsNot(samples[0].get("protocol_verified"), True)
                self.assertTrue(all(sample["status"] == "SKIPPED" for sample in samples[1:]))

    def test_unbound_or_inconsistent_build_artifact_cannot_enable_dependents(self):
        for kind, mode in [("compile", "wrong_artifact_path"), ("compile", "wrong_manifest_sha256"),
                           ("compile", "wrong_content_digest"), ("compile", "inconsistent_stats"),
                           ("compile", "wrong_profile"), ("materialize", "wrong_profile"),
                           ("materialize", "wrong_compiled_content_digest"), ("materialize", "no_fixpoint")]:
            with self.subTest(kind=kind, mode=mode):
                self.fresh_run(kind + "-" + mode)
                self.configure(actions=[{"kind": kind, "mode": mode}])
                result = self.run_driver()
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(len(self.calls("query")), 0)
                self.assertEqual(len(self.calls("validate")), 0)
                if kind == "compile":
                    self.assertEqual(len(self.calls("materialize")), 0)
                report, _ = self.summary()
                self.assertNotEqual(self.stage(report, kind)["samples"][0]["status"], "COMPLETE")

    def test_self_contradictory_shacl_report_is_a_protocol_failure(self):
        for mode in ("inconsistent_validation_conforms", "conforms_with_violations"):
            with self.subTest(mode=mode):
                self.fresh_run(mode)
                self.configure(validation_conforms=False, checked_shapes=1,
                               actions=[{"kind": "validate", "mode": mode}])
                result = self.run_driver(read_samples=2)
                self.assertNotEqual(result.returncode, 0)
                report, _ = self.summary()
                samples = self.stage(report, "validate")["samples"]
                self.assertEqual(samples[0]["status"], "PROTOCOL_FAILED")
                self.assertIs(samples[0]["protocol_verified"], False)
                self.assertEqual(samples[1]["status"], "SKIPPED")

    def test_zero_shape_validation_and_nonconformance_do_not_claim_business_pass(self):
        for conforms, checked in [(True, 0), (False, 1)]:
            with self.subTest(conforms=conforms, checked_shapes=checked):
                self.fresh_run("shapes-" + str(checked))
                self.configure(validation_conforms=conforms, checked_shapes=checked)
                self.assert_success(self.run_driver())
                report, _ = self.summary()
                self.assertEqual(report["schema_version"], "ontokiln-business-baseline/v1")
                self.assertEqual(report["business_correctness"], "MANUAL_REVIEW_REQUIRED")
                sample = self.stage(report, "validate")["samples"][0]
                self.assertEqual(sample["status"], "COMPLETE")
                self.assertIs(sample["protocol_verified"], True)
                self.assertEqual(sample["observation"]["conforms"], conforms)
                self.assertEqual(sample["observation"]["checked_shapes"], checked)
                self.assertNotIn('"business_correctness": "PASS"', json.dumps(report))

    def test_metadata_archive_excludes_private_templates_results_logs_and_artifacts(self):
        self.assert_success(self.run_driver())
        report, directory = self.summary()
        for name in ("summary.json", "report.md", "environment.json", "identity.json", "results.zip"):
            self.assertTrue((directory / name).is_file(), name)
        with zipfile.ZipFile(directory / "results.zip") as archive:
            names = archive.namelist()
            self.assertTrue({"summary.json", "report.md", "environment.json", "identity.json"}.issubset(names))
            for name in names:
                self.assertNotIn("..", Path(name).parts)
                self.assertFalse(Path(name).is_absolute())
                self.assertFalse(name.endswith((".sqlite", ".nt", ".ttl", ".rq", ".log")), name)
                self.assertNotIn("manifest.json", name)
                self.assertNotIn("request", name.lower())
                self.assertNotIn("response", name.lower())
                self.assertNotIn(PRIVATE_MARKER.encode(), archive.read(name), name)
        self.assertEqual(report["business_correctness"], "MANUAL_REVIEW_REQUIRED")
        for call in self.calls():
            self.assertTrue(Path(call["request_path"]).is_file())
            self.assertTrue(Path(call["result_path"]).is_file())
        self.assertTrue(any(PRIVATE_MARKER in path.read_text(errors="replace")
                            for path in directory.rglob("*.log")))

    def test_failure_diagnostics_remain_local_and_are_excluded_from_shareable_archive(self):
        self.configure(actions=[{"kind": "query", "mode": "private_error"},
                                {"kind": "validate", "mode": "private_process_failure"}])
        result = self.run_driver()
        self.assertNotEqual(result.returncode, 0)
        report, directory = self.summary()
        query_sample = self.stage(report, "query")["samples"][0]
        validate_sample = self.stage(report, "validate")["samples"][0]
        self.assertIn(PRIVATE_MARKER, query_sample["error"]["message"])
        self.assertIn(PRIVATE_MARKER, validate_sample["stderr_tail"])
        with zipfile.ZipFile(directory / "results.zip") as archive:
            for name in archive.namelist():
                self.assertNotIn(PRIVATE_MARKER.encode(), archive.read(name), name)
            shared = json.loads(archive.read("summary.json"))
            self.assertEqual(shared["summary_variant"], "sharing-metadata/v1")
            self.assertEqual(shared["status"], "COMPLETE_WITH_FAILURES")
            self.assertEqual(shared["business_correctness"], "MANUAL_REVIEW_REQUIRED")
            self.assertEqual(self.stage(shared, "query")["samples"][0]["api_status"], "FAILED")
            self.assertEqual(self.stage(shared, "validate")["samples"][0]["status"], "PROCESS_FAILED")

    @unittest.skipUnless(sys.platform.startswith("linux"), "uses Linux subreaping to clean the test grandchild")
    def test_sigterm_stops_actual_api_process_group_and_writes_interrupted_report(self):
        # A real grandchild proves group termination. Subreaping is only test
        # cleanup: the harness must itself reap its direct API command; this
        # test adopts and reaps the killed grandchild instead of leaving a
        # zombie behind in containers whose init does not reap orphans.
        libc = ctypes.CDLL(None, use_errno=True)
        previous = ctypes.c_int()
        if libc.prctl(37, ctypes.byref(previous), 0, 0, 0) or libc.prctl(36, 1, 0, 0, 0):
            self.skipTest("test process cannot enable Linux child subreaping")
        process, children = None, None
        self.configure(actions=[{"kind": "compile", "mode": "hang_with_child"}])
        command = self.command(reads=False, build_samples=3)
        try:
            process = subprocess.Popen(command, cwd=self.directory, stdout=subprocess.PIPE,
                                       stderr=subprocess.PIPE, text=True, start_new_session=True)
            child_file = self.fake_directory / "processes.json"
            deadline = time.monotonic() + 10
            while not child_file.exists() and process.poll() is None and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertTrue(child_file.exists(), "fake API never reached its live-child checkpoint")
            children = json.loads(child_file.read_text())
            self.assertEqual(children["api_pgid"], children["child_pgid"])
            process.terminate()
            stdout, stderr = process.communicate(timeout=10)
            self.last_result = subprocess.CompletedProcess(command, process.returncode, stdout, stderr)
            self.assertEqual(process.returncode, 143)
            with self.assertRaises(ProcessLookupError, msg="harness must reap its direct API command"):
                os.kill(children["api_pid"], 0)
            child_status = None
            deadline = time.monotonic() + 3
            while time.monotonic() < deadline:
                pid, status = os.waitpid(children["child_pid"], os.WNOHANG)
                if pid:
                    child_status = status
                    break
                time.sleep(0.01)
            self.assertIsNotNone(child_status, "grandchild survived the API process-group termination")
            self.assertTrue(os.WIFSIGNALED(child_status))
            self.assertEqual(os.WTERMSIG(child_status), signal.SIGKILL)
            report, directory = self.summary()
            self.assertEqual(report["status"], "INTERRUPTED")
            samples = self.stage(report, "compile")["samples"]
            self.assertEqual([sample["status"] for sample in samples], ["INTERRUPTED", "SKIPPED", "SKIPPED"])
            self.assertTrue(all(sample["status"] == "SKIPPED"
                                for sample in self.stage(report, "materialize")["samples"]))
            self.assertTrue((directory / "results.zip").is_file())
        finally:
            if process is not None and process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.communicate(timeout=5)
            if children is not None:
                try:
                    os.killpg(children["api_pgid"], signal.SIGKILL)
                except ProcessLookupError:
                    pass
                for pid in (children["api_pid"], children["child_pid"]):
                    deadline = time.monotonic() + 3
                    while time.monotonic() < deadline:
                        try:
                            waited, _ = os.waitpid(pid, os.WNOHANG)
                        except ChildProcessError:
                            break
                        if waited:
                            break
                        time.sleep(0.01)
            libc.prctl(36, previous.value, 0, 0, 0)

    def test_keep_all_artifacts_is_explicit_and_does_not_delete_caller_output(self):
        self.assert_success(self.run_driver("--keep-all-artifacts", reads=False, build_samples=3))
        outputs = [Path(row["request"]["operation"]["output"]) for row in self.calls()]
        self.assertEqual(len(outputs), 6)
        self.assertTrue(all(path.is_dir() for path in outputs))
        self.assertEqual((self.original_output / "do-not-delete.txt").read_text(), PRIVATE_MARKER)


if __name__ == "__main__":
    unittest.main()
