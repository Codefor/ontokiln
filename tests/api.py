"""Consumer-owned v1 expectations exercised through real supervised CLI processes."""
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time
import unittest

BINARY = str(Path(sys.argv.pop(1)).resolve())
ROOT = Path(__file__).resolve().parent / "fixtures" / "contract-v1"
CONTRACT = "ontokiln-api/v1"
PREFIX = "PREFIX ex: <urn:contract:> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> "
XSD = "http://www.w3.org/2001/XMLSchema#"


def source(path, document=None):
    return {"path": str(path), "document_id": document or path.name, "format": "turtle",
            "expected_sha256": hashlib.sha256(path.read_bytes()).hexdigest()}


def request_files(root, name, operation, limits=None, **fields):
    value = {"contract": CONTRACT, "request_id": name, "operation": operation,
             "limits": {"timeout_ms": 30000, **(limits or {})}, **fields}
    raw = json.dumps(value, ensure_ascii=False).encode()
    path, result = root / f"{name}.request.json", root / f"{name}.result.json"
    path.write_bytes(raw)
    return path, result, raw


def terminal(path, raw, error=None):
    reply = json.loads(path.read_text())
    assert set(reply) == {"contract", "request_id", "request_sha256", "status", "result", "error", "execution", "kernel"}, reply
    assert reply["contract"] == CONTRACT
    assert reply["request_sha256"] == hashlib.sha256(raw).hexdigest()
    assert reply["execution"]["worker_reaped"] is True, reply
    pid = reply["execution"]["worker_pid"]
    if pid:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            pass
        else:
            raise AssertionError(f"worker {pid} survived terminal response")
    if error:
        assert reply["status"] != "COMPLETE" and reply["result"] is None, reply
        assert reply["error"]["code"] == error, reply
    else:
        assert reply["status"] == "COMPLETE" and reply["error"] is None, reply
    return reply


class Contract(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory(prefix="ontokiln-api-test-")
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.root = Path(cls.tmp.name)
        cls.sequence = 0
        subprocess.run([sys.executable, "-B", ROOT / "generate.py", "--check"], check=True)
        cls.sources = [source(ROOT / name) for name in ["schema.ttl", "data.ttl"]]
        cls.pack = cls.root / "fixture.pack"
        subprocess.run([BINARY, "compile", "--ttl", ROOT, "--out", cls.pack], check=True, capture_output=True)
        cls.compiled = cls.root / "compiled"
        reply = cls.call({"kind": "compile", "sources": cls.sources,
                          "output": str(cls.compiled), "profile": "rdfs-supported-v1"})
        assert reply["result"]["stats"]["explicit_facts"] == 10000, reply
        inspected = json.loads(subprocess.check_output([BINARY, "inspect", "--pack", cls.pack]))
        assert inspected["triples"] == 10000, inspected
        cls.reasoned = cls.root / "reasoned"
        reply = cls.call({"kind": "materialize", "dataset": str(cls.compiled), "output": str(cls.reasoned)})
        assert reply["result"]["fixpoint_reached"] is True
        cls.datasets = [cls.pack, cls.reasoned]

    @classmethod
    def call(cls, op, error=None, limits=None, **fields):
        cls.sequence += 1
        name = f"call-{cls.sequence}"
        request, result, raw = request_files(cls.root, name, op, limits, **fields)
        subprocess.run([BINARY, "api", "--request", request, "--result", result], check=True, timeout=40)
        reply = terminal(result, raw, error)
        assert reply["request_id"] == name
        return reply

    def query(self, dataset, query, **options):
        return self.call({"kind": "query", "dataset": str(dataset), "query": PREFIX + query, **options})["result"]

    def test_consumer_preparation_bindings_and_unlimited_allowance(self):
        info = self.call({"kind": "capabilities"}, limits={"timeout_ms": None, "memory_bytes": None,
                         "scratch_bytes": None})["result"]
        self.assertIn("prepare_query", info["operations"])
        self.assertIn("rules", info["operations"])
        self.assertIn("ontokiln-sqlite-read/v1", info["read_contracts"])
        self.assertTrue(self.call({"kind": "prepare_query", "query": "ASK {}"})["result"]["valid"])
        self.call({"kind": "prepare_query", "query": "SELECT * WHERE { SERVICE <urn:remote> {?s ?p ?o}}"}, "INVALID_QUERY")
        op = {"kind": "query", "dataset": str(self.reasoned),
              "query": "SELECT ?label WHERE { BIND(?given AS ?label) FILTER(BOUND(?given)) }",
              "rdf_bindings": {"given": '"003"^^<http://www.w3.org/2001/XMLSchema#integer>'}}
        result = self.call(op, limits={"max_rows": 200000, "max_bytes": 64*1024**2, "timeout_ms": None})["result"]
        self.assertEqual(result["variables"], ["label"])
        self.assertEqual(result["items"], [{"label": {"type": "literal", "value": "003", "datatype": XSD+"integer"}}])
        self.call({**op, "dataset": str(self.pack)}, "UNSUPPORTED_CAPABILITY")

    def test_read_artifact_contract_and_terminal_ownership(self):
        manifest = json.loads((self.compiled/"manifest.json").read_bytes())
        self.assertEqual(manifest["read_contract"], "ontokiln-sqlite-read/v1")
        with sqlite3.connect(self.compiled/"facts.sqlite") as db:
            for table, columns in {"terms": {"id", "n", "kind", "value", "datatype", "lang"},
                                   "facts": {"g", "s", "p", "o", "explicit"},
                                   "source_documents": {"id", "document_id"},
                                   "source_facts": {"g", "s", "p", "o", "document"}}.items():
                self.assertTrue(columns <= {r[1] for r in db.execute("PRAGMA table_info("+table+")")})
            self.assertEqual(db.execute("SELECT COUNT(*) FROM facts WHERE explicit=1").fetchone()[0], 10000)
            self.assertEqual(db.execute("SELECT COUNT(DISTINCT document_id) FROM source_documents").fetchone()[0], 2)
            self.assertEqual(db.execute("SELECT COUNT(*) FROM facts f LEFT JOIN terms t ON f.s=t.id WHERE t.id IS NULL").fetchone()[0], 0)
        op = {"kind": "compile", "sources": self.sources, "output": str(self.root/"owned"), "profile": "no-rules-v1"}
        reply = self.call(op)
        record = json.loads((self.root/"owned.native-execution.json").read_bytes())
        self.assertEqual(record["request_sha256"], reply["request_sha256"])
        self.assertEqual(record["supervisor_pid"], reply["execution"]["supervisor_pid"])
        self.assertTrue(reply["execution"]["integrity_checked"])
        self.assertEqual(reply["result"]["artifact"]["manifest_sha256"], hashlib.sha256((self.root/"owned/manifest.json").read_bytes()).hexdigest())
        self.assertGreater(reply["execution"]["metrics"]["peak_rss_bytes"], 0)

    def test_rule_admission_frozen_inputs_and_compile_options(self):
        path = self.root/"admission.json"
        path.write_text(json.dumps({"schema_version": "ontology-rule-ir/v1", "rules": []}))
        file = {"path": str(path), "expected_sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
        self.assertEqual(self.call({"kind": "rules", "profile": "no-rules-v1", "files": []})["result"]["rules"], [])
        self.call({"kind": "rules", "profile": "datalog-v1", "files": [file]})
        self.call({"kind": "rules", "profile": "datalog-v1", "files": [{**file, "expected_sha256": "0"*64}]}, "INVALID_REQUEST")
        op = {"kind": "compile", "sources": self.sources, "output": str(self.root/"options"),
              "profile": "no-rules-v1", "import_lock": {}, "expected_source_tree_digest": "0"*64}
        self.call(op, "INVALID_REQUEST", limits={"max_term_bytes": 16*1024**2})
        self.assertFalse((self.root/"options").exists())

    def test_shared_golden_queries(self):
        for case in json.loads((ROOT / "cases.json").read_text()):
            outputs = []
            for dataset in self.datasets:
                with self.subTest(case=case["id"], dataset=dataset.name):
                    options = {"view": case["view"]} if "view" in case else {}
                    result = self.query(dataset, case["query"], **options)
                    self.assertEqual(set(result), {"query_type", "variables", "items", "boolean", "view", "complete", "truncated", "proof_coverage"})
                    self.assertTrue(result["complete"])
                    self.assertFalse(result["truncated"])
                    self.assertEqual(result["view"], case.get("view", "asserted"))
                    self.assertEqual(result["proof_coverage"], {"per_fact": "not_recorded"})
                    if "count" in case:
                        self.assertEqual(result["variables"], ["n"])
                        self.assertEqual(result["items"], [{"n": {"type": "literal", "value": str(case["count"]), "datatype": XSD + "integer"}}])
                    else:
                        self.assertEqual(result["boolean"], case["boolean"])
                        self.assertEqual(result["items"], [])
                    outputs.append(result)
            self.assertEqual(outputs[0], outputs[1], case["id"])

    def test_duplicates_order_datatypes_and_parameters(self):
        for dataset in self.datasets:
            rows = self.query(dataset, "SELECT ?o WHERE { ?s ex:alias ?o }")["items"]
            self.assertEqual(Counter(row["o"]["value"] for row in rows), {f"urn:contract:bucket{i}": 499 for i in range(4)})
            rows = self.query(dataset, "SELECT ?s ?v ?label WHERE { ?s ex:score ?v; ex:label ?label } ORDER BY ?v LIMIT 3")["items"]
            self.assertEqual(rows, [{"s": {"type": "uri", "value": f"urn:contract:e{i:04d}"},
                "v": {"type": "literal", "value": f"{i:04d}", "datatype": XSD + "integer"},
                "label": {"type": "literal", "value": f"条目 {i:04d}", "xml:lang": "zh"}} for i in range(3)])
            result = self.query(dataset, "SELECT ?label WHERE { <urn:ontokiln:param:target> ex:label ?label }",
                                parameters={"target": {"type": "iri", "value": "urn:contract:marker"}})
            self.assertEqual(result["items"], [{"label": {"type": "literal", "value": "schema", "datatype": XSD + "string"}}])
            self.call({"kind": "query", "dataset": str(dataset), "query": "SELECT * WHERE { ?s ?p <urn:ontokiln:param:missing> }"}, "INVALID_REQUEST")

    def test_document_scoped_blank_nodes(self):
        for dataset in self.datasets:
            rows = self.query(dataset, 'SELECT ?s ?v WHERE { ?s ex:label ?v FILTER(isBlank(?s)) } ORDER BY ?v')["items"]
            self.assertEqual([row["v"]["value"] for row in rows], ["data blank", "schema blank"])
            self.assertEqual([row["s"]["type"] for row in rows], ["bnode", "bnode"])
            self.assertNotEqual(rows[0]["s"]["value"], rows[1]["s"]["value"])

    def test_query_payload_byte_boundary_is_shared(self):
        for dataset in self.datasets:
            op = {"kind": "query", "dataset": str(dataset), "query": "SELECT (1 AS ?n) WHERE {}"}
            result = self.call(op)["result"]
            payload = {key: result[key] for key in ["query_type", "variables", "items", "boolean"]}
            size = len(json.dumps(payload, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode())
            self.assertEqual(self.call(op, limits={"max_bytes": size})["result"], result)
            self.call(op, "RESOURCE_EXHAUSTED", limits={"max_bytes": size - 1})

    def test_capabilities_and_graph_semantics_are_explicit(self):
        for path, capability, unsupported in [(self.pack, "document-graphs/v1", "rdf-named-graphs/v1"),
                                             (self.reasoned, "rdf-named-graphs/v1", "revision/v1")]:
            result = self.call({"kind": "capabilities", "dataset": str(path)})["result"]
            self.assertIn(capability, result["dataset"]["capabilities"])
            self.assertEqual(result["dataset"]["default_graph"], "union")
            self.call({"kind": "query", "dataset": str(path), "query": "ASK {}", "require": [unsupported]}, "UNSUPPORTED_CAPABILITY")
            self.call({"kind": "query", "dataset": str(path), "query": "ASK {}", "require": ["per-fact-proofs/v1"]}, "UNSUPPORTED_CAPABILITY")
        self.assertEqual(len(self.query(self.pack, "SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s ?p ?o } }")["items"]), 2)
        self.assertEqual(self.query(self.reasoned, "SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s ?p ?o } }")["items"], [])
        self.call({"kind": "query", "dataset": str(self.reasoned), "query": "ASK {}", "revision": "v1"}, "UNSUPPORTED_CAPABILITY")
        self.call({"kind": "query", "dataset": str(self.compiled), "query": "ASK {}", "view": "closure"}, "UNSUPPORTED_CAPABILITY")

    def test_reject_schema_version_and_unsupported_inputs(self):
        self.call({"kind": "capabilities"}, "UNSUPPORTED_CONTRACT", contract="ontokiln-api/v2")
        self.call({"kind": "capabilities"}, "INVALID_REQUEST", unexpected=True)
        self.call({"kind": "capabilities", "typo": True}, "INVALID_REQUEST")
        self.call({"kind": "capabilities"}, "INVALID_REQUEST", limits={"typo": 1})
        self.call({"kind": "capabilities"}, "INVALID_REQUEST", limits={"timeout_ms": 0})
        for query in ["not SPARQL", "SELECT * FROM <https://example.invalid/> WHERE {?s ?p ?o}",
                      "SELECT * WHERE { SERVICE <https://example.invalid/> {?s ?p ?o} }"]:
            self.call({"kind": "query", "dataset": str(self.pack), "query": query}, "INVALID_QUERY")

    def test_limits_never_return_partial_success(self):
        for dataset in self.datasets:
            op = {"kind": "query", "dataset": str(dataset), "query": "SELECT ?s WHERE {?s ?p ?o}"}
            for limits in [{"max_rows": 0}, {"max_bytes": 1}, {"memory_bytes": 1}, {"scratch_bytes": 1}]:
                with self.subTest(dataset=dataset.name, limits=limits):
                    result = self.call(op, "RESOURCE_EXHAUSTED", limits=limits)
                    self.assertEqual(result["status"], "BUDGET_EXHAUSTED")
            heavy = "SELECT (COUNT(*) AS ?n) WHERE { ?a ?b ?c . ?d ?e ?f . ?g ?h ?i }"
            self.call({**op, "query": heavy}, "RESOURCE_EXHAUSTED", limits={"timeout_ms": 1})
            self.call({**op, "query": "ASK {}"}, "RESOURCE_EXHAUSTED", limits={"max_rows": 0})

    def test_cancel_reaps_real_worker_and_leaves_no_artifact(self):
        output = self.root / "cancelled"
        op = {"kind": "compile", "sources": self.sources, "output": str(output), "profile": "rdfs-supported-v1"}
        request, result, raw = request_files(self.root, "cancel", op)
        process = subprocess.Popen([BINARY, "api", "--request", request, "--result", result])
        worker = None
        try:
            receipt = self.root / "cancelled.native-execution.json"
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                try:
                    worker = json.loads(receipt.read_text()).get("worker_pid")
                except (OSError, ValueError):
                    pass
                if worker:
                    break
                self.assertIsNone(process.poll())
                time.sleep(.002)
            self.assertIsNotNone(worker)
            process.send_signal(signal.SIGTERM)
            self.assertEqual(process.wait(timeout=5), 0)
            self.assertEqual(terminal(result, raw, "CANCELLED")["status"], "CANCELLED")
            self.assertFalse(output.exists())
            self.assertFalse(receipt.exists())
            self.assertFalse(list(self.root.glob(".cancelled*")))
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            if worker:
                try:
                    os.kill(worker, signal.SIGKILL)
                except ProcessLookupError:
                    pass

    def test_corruption_input_digest_and_atomic_output(self):
        corrupt = self.root / "corrupt"
        shutil.copytree(self.compiled, corrupt)
        with (corrupt / "facts.sqlite").open("ab") as stream:
            stream.write(b"corrupt")
        self.call({"kind": "query", "dataset": str(corrupt), "query": "ASK {}"}, "INVALID_DATASET")
        corrupt_pack = self.root / "corrupt.pack"
        corrupt_pack.write_bytes(self.pack.read_bytes()[:-1])
        self.call({"kind": "query", "dataset": str(corrupt_pack), "query": "ASK {}"}, "INVALID_DATASET")
        output = self.root / "bad-input"
        op = {"kind": "compile", "sources": [{**self.sources[0], "expected_sha256": "0" * 64}],
              "output": str(output), "profile": "no-rules-v1"}
        self.call(op, "INVALID_REQUEST")
        self.assertFalse(output.exists())
        old = (self.compiled / "manifest.json").read_bytes()
        self.call({**op, "sources": self.sources, "output": str(self.compiled)}, "EXECUTION_FAILED")
        self.assertEqual((self.compiled / "manifest.json").read_bytes(), old)
        limited = self.root / "limited"
        self.call({"kind": "materialize", "dataset": str(self.compiled), "output": str(limited)},
                  "RESOURCE_EXHAUSTED", limits={"max_iterations": 0})
        self.assertFalse(limited.exists())

    def test_shacl_checks_real_fixture_and_rejects_pack(self):
        shapes = self.root / "shapes.ttl"
        shapes.write_text('''@prefix sh: <http://www.w3.org/ns/shacl#> .
<urn:shape> a sh:NodeShape; sh:targetNode <urn:contract:e0000>;
    sh:property [ sh:path <urn:contract:score>; sh:minCount 1 ] .
<urn:missing> a sh:NodeShape; sh:targetNode <urn:contract:e0000>;
    sh:property [ sh:path <urn:contract:missing>; sh:minCount 1 ] .''')
        op = {"kind": "validate", "dataset": str(self.compiled), "shapes": [source(shapes)]}
        result = self.call({**op, "selected_shapes": ["<urn:shape>"]})["result"]
        self.assertTrue(result["conforms"])
        self.assertEqual(result["checked_shapes"], 1)
        result = self.call({**op, "selected_shapes": ["<urn:missing>"]})["result"]
        self.assertFalse(result["conforms"])
        self.assertEqual(len(result["report"]["violations"]), 1)
        self.call({**op, "dataset": str(self.pack)}, "UNSUPPORTED_CAPABILITY")

    def test_response_io_failure_rolls_back_artifact(self):
        output = self.root / "response-failed"
        op = {"kind": "compile", "sources": self.sources,
              "output": str(output), "profile": "no-rules-v1"}
        request, _, _ = request_files(self.root, "response-failed", op)
        run = subprocess.run([BINARY, "api", "--request", request,
                              "--result", self.root / "missing" / "result.json"], capture_output=True, timeout=40)
        self.assertNotEqual(run.returncode, 0)
        self.assertFalse(output.exists())
        self.assertFalse(list(self.root.glob(".response-failed*")))
        self.assertFalse((self.root / "response-failed.native-execution.json").exists())

    def test_revision_selection_and_named_rdf_graphs(self):
        directory = self.root / "versioned"
        directory.mkdir()
        code = directory / "model.txt"
        code.write_text("synthetic source identity")
        manifest = {}
        for revision in ["v1", "v2"]:
            path = directory / f"{revision}.ttl"
            path.write_text(f'<urn:entity> <urn:value> "{revision}" .')
            manifest[path.name] = {"module": "fixture", "revision": revision,
                "source_file": code.name, "source_sha256": hashlib.sha256(code.read_bytes()).hexdigest(),
                "ttl_sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
        (directory / "sources.json").write_text(json.dumps(manifest))
        pack = self.root / "versioned.pack"
        subprocess.run([BINARY, "compile", "--ttl", directory, "--out", pack], check=True, capture_output=True)
        op = {"kind": "query", "dataset": str(pack), "query": "SELECT ?v WHERE { <urn:entity> <urn:value> ?v }"}
        self.call(op, "INVALID_QUERY")
        for revision in ["v1", "v2"]:
            rows = self.call({**op, "revision": revision})["result"]["items"]
            self.assertEqual(rows, [{"v": {"type": "literal", "value": revision, "datatype": XSD + "string"}}])
        trig = self.root / "named.trig"
        trig.write_text('<urn:graph> { <urn:entity> <urn:value> "named" . }')
        artifact = self.root / "named"
        self.call({"kind": "compile", "sources": [{**source(trig), "format": "trig"}],
                   "output": str(artifact), "profile": "no-rules-v1"})
        self.assertEqual(self.query(artifact, 'SELECT ?v WHERE { GRAPH <urn:graph> { <urn:entity> <urn:value> ?v } }')["items"],
                         [{"v": {"type": "literal", "value": "named", "datatype": XSD + "string"}}])
        # Preserve Full's declared union-default policy, while GRAPH keeps the original name.
        self.assertTrue(self.query(artifact, 'ASK { <urn:entity> <urn:value> ?v }')["boolean"])

    def test_nonstandard_vocabulary_is_caller_owned(self):
        renamed = self.root / "renamed"
        renamed.mkdir()
        for name in ["schema.ttl", "data.ttl"]:
            (renamed / name).write_text((ROOT / name).read_text().replace("urn:contract:", "urn:arbitrary:"))
        pack = self.root / "renamed.pack"
        subprocess.run([BINARY, "compile", "--ttl", renamed, "--out", pack], check=True, capture_output=True)
        compiled = self.root / "renamed-full"
        self.call({"kind": "compile", "sources": [source(renamed / name) for name in ["schema.ttl", "data.ttl"]],
                   "output": str(compiled), "profile": "no-rules-v1"})
        for dataset in [pack, compiled]:
            result = self.call({"kind": "query", "dataset": str(dataset),
                "query": "SELECT (COUNT(*) AS ?n) WHERE { ?s a <urn:arbitrary:Leaf> }"})["result"]
            self.assertEqual(result["items"][0]["n"]["value"], "1996")


if __name__ == "__main__":
    unittest.main()
