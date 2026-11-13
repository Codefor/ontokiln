"""Consumer-owned v1 expectations exercised through real supervised CLI processes."""
from collections import Counter
import hashlib
import importlib.util
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
ERROR_STATUS = {
    "INVALID_REQUEST": "FAILED", "INVALID_DATASET": "FAILED",
    "INVALID_QUERY": "FAILED", "EXECUTION_FAILED": "FAILED",
    "UNSUPPORTED_CONTRACT": "UNSUPPORTED", "UNSUPPORTED_CAPABILITY": "UNSUPPORTED",
    "RESOURCE_EXHAUSTED": "BUDGET_EXHAUSTED", "CANCELLED": "CANCELLED",
}


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
        assert reply["status"] == ERROR_STATUS[error] and reply["result"] is None, reply
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
        cls.compiled_content_digest = reply["result"]["artifact"]["content_digest"]
        cls.compiled_manifest_sha256 = reply["result"]["artifact"]["manifest_sha256"]
        inspected = json.loads(subprocess.check_output([BINARY, "inspect", "--pack", cls.pack]))
        assert inspected["triples"] == 10000, inspected
        cls.reasoned = cls.root / "reasoned"
        reply = cls.call({"kind": "materialize", "dataset": str(cls.compiled), "output": str(cls.reasoned)})
        assert reply["result"]["fixpoint_reached"] is True
        cls.reasoned_content_digest = reply["result"]["artifact"]["content_digest"]
        cls.reasoned_manifest_sha256 = reply["result"]["artifact"]["manifest_sha256"]
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

    @classmethod
    def call_document(cls, document, error=None):
        """Send exact public input, without injecting defaults or request fields."""
        cls.sequence += 1
        name = f"document-{cls.sequence}"
        raw = document if isinstance(document, bytes) else json.dumps(document, ensure_ascii=False).encode()
        request, result = cls.root / f"{name}.request.json", cls.root / f"{name}.result.json"
        request.write_bytes(raw)
        subprocess.run([BINARY, "api", "--request", request, "--result", result], check=True, timeout=40)
        return terminal(result, raw, error), raw

    def query(self, dataset, query, **options):
        return self.call({"kind": "query", "dataset": str(dataset), "query": PREFIX + query, **options})["result"]

    def test_frozen_v1_client_and_requests(self):
        spec = importlib.util.spec_from_file_location("ontokiln_legacy_client_v1", ROOT / "legacy_client.py")
        client = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(client)
        datasets = {"pack": self.pack, "full": self.reasoned}
        shapes = ROOT / "legacy_shapes.rdf"

        def substitute(value, dataset):
            if isinstance(value, dict):
                return {key: substitute(item, dataset) for key, item in value.items()}
            if isinstance(value, list):
                return [substitute(item, dataset) for item in value]
            replacements = {"$DATASET": str(dataset), "$SHAPES": str(shapes),
                            "$SHAPES_SHA256": hashlib.sha256(shapes.read_bytes()).hexdigest()}
            return replacements.get(value, value) if isinstance(value, str) else value

        for case in json.loads((ROOT / "legacy_requests.json").read_text()):
            for dataset in case.get("datasets", ["full"]):
                with self.subTest(case=case["id"], dataset=dataset):
                    request = substitute(case["request"], datasets[dataset])
                    reply, raw = self.call_document(request, case["expect"].get("error_code"))
                    parsed = client.parse_reply(json.dumps(reply, ensure_ascii=False).encode(), raw)
                    client.check_expected(parsed, case["expect"])

    def test_contract_is_negotiated_before_operation_schema(self):
        future = [
            {"operation": {"kind": "future_operation", "future_option": True}},
            {"operation": {"kind": "capabilities"}, "future_envelope": True},
            {"operation": "future representation", "limits": {"future_budget": 1}},
        ]
        for body in future:
            for contract, error in [("ontokiln-api/v2", "UNSUPPORTED_CONTRACT"),
                                    (CONTRACT, "INVALID_REQUEST")]:
                with self.subTest(contract=contract, body=body):
                    reply, _ = self.call_document({"contract": contract, "request_id": "negotiate", **body}, error)
                    self.assertEqual(reply["request_id"], "negotiate")
                    self.assertIsNone(reply["execution"]["worker_pid"])
        for contract in [None, 1]:
            self.call_document({"contract": contract, "request_id": "invalid-contract",
                                "operation": {"kind": "capabilities"}}, "INVALID_REQUEST")
        self.call_document({"request_id": "missing-contract", "operation": {"kind": "capabilities"}},
                           "INVALID_REQUEST")
        reply, _ = self.call_document(b'{"contract":', "INVALID_REQUEST")
        self.assertIsNone(reply["request_id"])
        for duplicate in [
            b'{"contract":"ontokiln-api/v1","contract":"ontokiln-api/v1","request_id":"duplicate","operation":{"kind":"capabilities"}}',
            b'{"contract":"ontokiln-api/v1","request_id":"first","request_id":"last","operation":{"kind":"capabilities"}}',
        ]:
            self.call_document(duplicate, "INVALID_REQUEST")

    def test_omitted_and_null_limits_keep_their_v1_meaning(self):
        request = {"contract": CONTRACT, "request_id": "limit-defaults",
                   "operation": {"kind": "query", "dataset": str(self.reasoned), "query": "ASK {}"}}
        default, _ = self.call_document(request)
        self.assertEqual(default["result"]["view"], "asserted")
        for limits in [{}, {"memory_bytes": None, "scratch_bytes": None, "timeout_ms": None}]:
            reply, _ = self.call_document({**request, "limits": limits})
            self.assertEqual(reply["result"], default["result"])
        for limits in [None, {"max_rows": None}, {"max_validation_depth": None},
                       {"memory_bytes": 0}, {"max_term_bytes": 0}]:
            with self.subTest(limits=limits):
                self.call_document({**request, "limits": limits}, "INVALID_REQUEST")
        for operation, limits in [
            ({"kind": "compile", "sources": self.sources, "profile": "no-rules-v1"},
             {"memory_bytes": None}),
            ({"kind": "materialize", "dataset": str(self.compiled)}, {"scratch_bytes": None}),
        ]:
            output = self.root / f"null-budget-{operation['kind']}"
            self.call({**operation, "output": str(output)}, "INVALID_REQUEST", limits=limits)
            self.assertFalse(output.exists())

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

    def test_query_derived_edges_are_scoped_bounded_and_typed(self):
        before = hashlib.sha256((self.reasoned / 'facts.sqlite').read_bytes()).hexdigest()
        for prefix in ('urn:synthetic:first:', 'urn:synthetic:other:'):
            op = {'kind': 'query', 'dataset': str(self.reasoned),
                  'query': 'SELECT ?s WHERE { ?s <'+prefix+'edge>+ ?s } ORDER BY ?s',
                  'values': [{'variables': ['left', 'right'], 'rows': [
                      [{'type': 'iri', 'value': prefix+'a'}, {'type': 'iri', 'value': prefix+'b'}],
                      [{'type': 'iri', 'value': prefix+'b'}, {'type': 'iri', 'value': prefix+'a'}]]}],
                  'derived_queries': ['SELECT (?left AS ?s) (<'+prefix+'edge> AS ?p) (?right AS ?o) WHERE { VALUES (?left ?right) {} }'],
                  'require': ['values-bindings/v1', 'derived-query-data/v1']}
            rows = self.call(op)['result']['items']
            self.assertEqual([r['s']['value'] for r in rows], [prefix+'a', prefix+'b'])
            self.assertFalse(self.call({**op, 'derived_queries': []})['result']['items'])
            constructed = {**op, 'derived_queries': ['CONSTRUCT { ?left <'+prefix+'edge> ?right } WHERE { VALUES (?left ?right) {} }']}
            self.assertEqual(self.call(constructed)['result']['items'], rows)
            self.assertTrue(self.call({'kind':'prepare_query','query':constructed['derived_queries'][0]})['result']['valid'])
            self.call({**op, 'query':constructed['derived_queries'][0]}, 'INVALID_QUERY')
            self.assertFalse(self.call({**constructed, 'query':'SELECT ?g WHERE { GRAPH ?g {?s <'+prefix+'edge> ?o} }'})['result']['items'])
            self.call({**constructed, 'derived_queries':[*constructed['derived_queries'], 'CONSTRUCT {?s ?p ?o} WHERE {SERVICE <urn:remote> {?s ?p ?o}}']}, 'INVALID_QUERY')
            self.call(constructed, 'RESOURCE_EXHAUSTED', limits={'timeout_ms':1})

            self.call(op, 'RESOURCE_EXHAUSTED', limits={'max_derived_facts': 1})
            self.call(op, 'RESOURCE_EXHAUSTED', limits={'max_intermediate_rows': 1})
            self.call({**op, 'derived_queries': ['ASK {}']}, 'INVALID_QUERY')
            self.call({**op, 'derived_queries': ['SELECT ("bad" AS ?s) (<urn:p> AS ?p) (<urn:o> AS ?o) WHERE {}']}, 'INVALID_QUERY')
            self.call({**op, 'dataset': str(self.pack)}, 'UNSUPPORTED_CAPABILITY')
            invalid = [{'variables': ['left', 'right'], 'rows': [[{'type': 'iri', 'value': prefix+'a'}]]}]
            self.call({**op, 'values': invalid}, 'INVALID_REQUEST')
        self.assertEqual(before, hashlib.sha256((self.reasoned / 'facts.sqlite').read_bytes()).hexdigest())

    def test_cancel_derived_query_reaps_the_real_worker(self):
        op = {'kind':'query','dataset':str(self.reasoned),'query':'ASK {}',
              'derived_queries':['CONSTRUCT {<urn:new:s> <urn:new:p> <urn:new:o>} WHERE {}',
                  'SELECT (?a AS ?s) (<urn:new:edge> AS ?p) (?d AS ?o) WHERE {?a ?b ?c .?d ?e ?f .?g ?h ?i}']}
        request, result, raw = request_files(self.root, 'cancel-derived', op)
        process = subprocess.Popen([BINARY,'api','--request',request,'--result',result])
        worker = None
        try:
            deadline = time.monotonic()+5
            while time.monotonic()<deadline:
                listing = subprocess.check_output(['ps','-eo','pid=,ppid=,args='],text=True)
                for line in listing.splitlines():
                    parts=line.split(None,2)
                    if (len(parts)==3 and int(parts[1])==process.pid
                            and parts[2].startswith(BINARY + ' full-worker ')):
                        worker=int(parts[0]);break
                if worker:break
                self.assertIsNone(process.poll())
                time.sleep(.005)
            self.assertIsNotNone(worker)
            process.send_signal(signal.SIGTERM)
            self.assertEqual(process.wait(timeout=5),0)
            reply=terminal(result,raw,'CANCELLED')
            self.assertEqual(reply['execution']['worker_pid'],worker)
        finally:
            if process.poll() is None:
                process.kill();process.wait()

    def test_ask_initial_bindings_preserve_boolean_semantics(self):
        for text, expected in [
            ("ASK { FILTER(BOUND(?given)) }", True),
            ("ASK { FILTER(?given = <urn:binding:x>) }", True),
            ("ASK { FILTER(?given = <urn:binding:y>) }", False),
            ("ASK { FILTER(BOUND(?given)) } LIMIT 0", False),
        ]:
            with self.subTest(query=text):
                result = self.call({"kind": "query", "dataset": str(self.reasoned),
                                    "query": text, "rdf_bindings": {"given": "<urn:binding:x>"}})["result"]
                self.assertEqual(result["query_type"], "ASK")
                self.assertEqual(result["variables"], [])
                self.assertEqual(result["items"], [])
                self.assertIs(result["boolean"], expected)

    def test_unused_values_still_validate_rdf_terms(self):
        op = {"kind": "query", "dataset": str(self.reasoned), "query": "ASK {}"}
        valid = [None, {"type": "iri", "value": "urn:valid"},
                 {"type": "literal", "value": "name", "language": "en"}]
        invalid = [
            {"type": "iri", "value": "not an IRI"},
            {"type": "literal", "value": "x", "datatype": "not an IRI"},
            {"type": "literal", "value": "x", "language": "not a language"},
            {"type": "literal", "value": "x", "language": "en", "datatype": XSD + "string"},
        ]
        for cell in valid + invalid:
            with self.subTest(cell=cell):
                reply = self.call({**op, "values": [{"variables": ["unused"], "rows": [[cell]]}]},
                                  "INVALID_REQUEST" if cell in invalid else None)
                if cell in valid:
                    self.assertTrue(reply["result"]["boolean"])

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

    def test_local_query_assets_compile_without_execution(self):
        for index, prefix in enumerate(("urn:query-assets:first:", "urn:query-assets:renamed:")):
            with self.subTest(prefix=prefix):
                data = self.root / f"query-assets-{index}.ttl"
                data.write_text(f'<{prefix}a> <{prefix}SERVICE> <{prefix}b> .\n'
                                f'<{prefix}c> <{prefix}SERVICE> <{prefix}d> .\n')
                prologue = f"PREFIX ex: <{prefix}> "
                queries = [
                    prologue + 'SELECT ?SERVICE ?FROM WHERE { ?SERVICE ex:SERVICE ?FROM . '
                    'BIND("SERVICE FROM" AS ?text) BIND(<urn:SERVICE> AS ?iri) } # SERVICE FROM\n',
                    prologue + 'ASK { ex:a ex:SERVICE ex:b }',
                    prologue + 'CONSTRUCT { ?SERVICE ex:FROM ?FROM } '
                    'WHERE { ?SERVICE ex:SERVICE ?FROM }',
                ]
                files = []
                for number, query in enumerate(queries):
                    path = self.root / f"query-assets-{index}-{number}.rq"
                    path.write_text(query)
                    files.append({"path": str(path), "expected_sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
                    self.assertTrue(self.call({"kind": "prepare_query", "query": query})["result"]["valid"])
                for profile in ("datalog-v1", "rdfs-supported-v1", "rdfs-property-chain-v1"):
                    empty = self.call({"kind": "rules", "profile": profile, "files": []})["result"]
                    self.assertEqual(self.call({"kind": "rules", "profile": profile, "files": files})["result"], empty)
                self.call({"kind": "rules", "profile": "no-rules-v1", "files": files}, "INVALID_REQUEST")
                self.call({"kind": "rules", "profile": "unknown-v1", "files": files}, "UNSUPPORTED_CAPABILITY")
                invalid_digest = [{**files[2], "expected_sha256": "0" * 64}]
                self.call({"kind": "rules", "profile": "datalog-v1", "files": invalid_digest}, "INVALID_REQUEST")

                baseline = self.root / f"query-assets-baseline-{index}"
                operation = {"kind": "compile", "sources": [source(data)], "profile": "datalog-v1"}
                plain = self.call({**operation, "output": str(baseline)})["result"]
                plain_manifest = json.loads((baseline / "manifest.json").read_bytes())
                output = self.root / f"query-assets-full-{index}"
                compiled = self.call({**operation, "output": str(output), "rules": files,
                                      "expected_source_tree_digest": plain_manifest["source_tree_digest"]})
                self.assertTrue(compiled["execution"]["integrity_checked"])
                self.assertEqual(compiled["result"]["stats"], {"explicit_facts": 2, "inferred_facts": 0})
                self.assertEqual(compiled["result"]["stats"], plain["stats"])
                manifest = json.loads((output / "manifest.json").read_bytes())
                frozen = json.loads((output / "input_manifest.json").read_bytes())
                self.assertEqual(manifest["profile_id"], "datalog-v1")
                self.assertEqual(manifest["source_tree_digest"], plain_manifest["source_tree_digest"])
                self.assertNotEqual(manifest["semantic_fingerprint"], plain_manifest["semantic_fingerprint"])
                self.assertEqual(len(frozen["rules"]), len(files))
                for asset, original in zip(frozen["rules"], files):
                    self.assertEqual(asset["sha256"], original["expected_sha256"])
                    self.assertEqual((output / asset["file"]).read_bytes(), Path(original["path"]).read_bytes())

                query = {"kind": "query", "dataset": str(output),
                         "expected_content_digest": compiled["result"]["artifact"]["content_digest"]}
                selected = self.call({**query, "query": queries[0]})["result"]
                self.assertEqual(selected["query_type"], "SELECT")
                self.assertEqual({(row["SERVICE"]["value"], row["FROM"]["value"]) for row in selected["items"]},
                                 {(prefix + "a", prefix + "b"), (prefix + "c", prefix + "d")})
                asked = self.call({**query, "query": queries[1]})["result"]
                self.assertEqual((asked["query_type"], asked["variables"], asked["items"], asked["boolean"]),
                                 ("ASK", [], [], True))
                derived = prologue + 'ASK { ex:a ex:FROM ex:b }'
                self.assertFalse(self.call({**query, "query": derived})["result"]["boolean"])
                self.assertTrue(self.call({**query, "query": derived,
                                          "derived_queries": [Path(files[2]["path"]).read_text()]})["result"]["boolean"])
                self.assertFalse(self.call({**query, "query": derived})["result"]["boolean"])

    def test_construct_asset_preserves_rdf_semantics_and_snapshot_isolation(self):
        prologue = "PREFIX ex: <urn:construct-asset:> "
        data = self.root / "construct-semantics.ttl"
        data.write_text('@prefix ex: <urn:construct-asset:> .\n'
                        'ex:a ex:input ex:b . ex:c ex:input ex:d . _:created ex:tag "original" .\n')
        asset = self.root / "construct-semantics.rq"
        asset.write_text(prologue + '''CONSTRUCT {
            ?s ex:output ?o . ?s ex:unbound ?missing .
            _:created ex:subject ?s ; ex:object ?o ; ex:self _:created .
        } WHERE {
            ?s ex:input ?o . VALUES ?duplicate { 1 1 }
            OPTIONAL { ?s ex:missing ?missing }
        }''')
        output = self.root / "construct-semantics-full"
        self.call({"kind": "compile", "sources": [source(data)], "output": str(output), "profile": "datalog-v1",
                   "rules": [{"path": str(asset), "expected_sha256": hashlib.sha256(asset.read_bytes()).hexdigest()}]})
        before = {str(path.relative_to(output)): hashlib.sha256(path.read_bytes()).hexdigest()
                  for path in output.rglob("*") if path.is_file()}
        query = {"kind": "query", "dataset": str(output), "query": prologue + "SELECT ?s ?p ?o WHERE { ?s ?p ?o }"}
        original = self.call(query)["result"]["items"]
        self.assertEqual(len(original), 3)
        original_blank = next(row["s"]["value"] for row in original if row["s"]["type"] == "bnode")
        derived = {**query, "derived_queries": [asset.read_text()]}
        # Two repeated solutions per source edge yield two unique ordinary
        # triples and four fresh template blank nodes with three triples each.
        rows = self.call(derived, limits={"max_derived_facts": 14, "max_intermediate_rows": 14})["result"]["items"]
        self.assertEqual(len(rows), 17)
        outputs = [row for row in rows if row["p"]["value"] == "urn:construct-asset:output"]
        self.assertEqual(Counter((row["s"]["value"], row["o"]["value"]) for row in outputs),
                         {("urn:construct-asset:a", "urn:construct-asset:b"): 1,
                          ("urn:construct-asset:c", "urn:construct-asset:d"): 1})
        self.assertFalse(any(row["p"]["value"] == "urn:construct-asset:unbound" for row in rows))
        nodes = {}
        for row in rows:
            if row["p"]["value"] in {"urn:construct-asset:subject", "urn:construct-asset:object", "urn:construct-asset:self"}:
                self.assertEqual(row["s"]["type"], "bnode")
                nodes.setdefault(row["s"]["value"], {})[row["p"]["value"].rsplit(":", 1)[1]] = row["o"]
        self.assertEqual(len(nodes), 4)
        self.assertNotIn(original_blank, nodes)
        for label, terms in nodes.items():
            self.assertEqual(set(terms), {"subject", "object", "self"})
            self.assertEqual(terms["self"], {"type": "bnode", "value": label})
        self.assertEqual(Counter((terms["subject"]["value"], terms["object"]["value"]) for terms in nodes.values()),
                         {("urn:construct-asset:a", "urn:construct-asset:b"): 2,
                          ("urn:construct-asset:c", "urn:construct-asset:d"): 2})
        self.assertFalse(self.call({**derived, "query": prologue +
                                   "SELECT ?g WHERE { GRAPH ?g { ?s ex:output ?o } }"})["result"]["items"])
        self.call({**query, "query": asset.read_text()}, "INVALID_QUERY")
        self.call(derived, "RESOURCE_EXHAUSTED", limits={"max_derived_facts": 13})
        self.assertCountEqual(self.call(query)["result"]["items"], original)
        self.assertEqual(before, {str(path.relative_to(output)): hashlib.sha256(path.read_bytes()).hexdigest()
                                  for path in output.rglob("*") if path.is_file()})

    def test_query_asset_rejections_match_local_query_validation(self):
        data = self.root / "rejected-query-assets.ttl"
        data.write_text("<urn:local:s> <urn:local:p> <urn:local:o> .")
        cases = [
            ("syntax", "CONSTRUCT { ?s ?p ?o } WHERE {", "INVALID_REQUEST", "RULE_INVALID"),
            ("update", "INSERT DATA { <urn:s> <urn:p> <urn:o> }", "INVALID_REQUEST", "RULE_INVALID"),
            ("describe", "DESCRIBE <urn:s>", "UNSUPPORTED_CAPABILITY", "UNSUPPORTED_RULE_PROFILE"),
            ("select-service", "SELECT * WHERE { SERVICE <urn:remote> { ?s ?p ?o } }", "UNSUPPORTED_CAPABILITY", "UNSUPPORTED_RULE_PROFILE"),
            ("ask-service", "ASK { sErViCe SILENT <urn:remote> { ?s ?p ?o } }", "UNSUPPORTED_CAPABILITY", "UNSUPPORTED_RULE_PROFILE"),
            ("construct-service", "CONSTRUCT { ?s ?p ?o } WHERE { FILTER EXISTS { SERVICE <urn:remote> { ?s ?p ?o } } }", "UNSUPPORTED_CAPABILITY", "UNSUPPORTED_RULE_PROFILE"),
            ("select-from", "SELECT * FROM <urn:remote> WHERE { ?s ?p ?o }", "UNSUPPORTED_CAPABILITY", "UNSUPPORTED_RULE_PROFILE"),
            ("ask-from", "ASK FROM <urn:remote> WHERE { ?s ?p ?o }", "UNSUPPORTED_CAPABILITY", "UNSUPPORTED_RULE_PROFILE"),
            ("construct-from", "CONSTRUCT { ?s ?p ?o } FROM NAMED <urn:remote> WHERE { GRAPH ?g { ?s ?p ?o } }", "UNSUPPORTED_CAPABILITY", "UNSUPPORTED_RULE_PROFILE"),
        ]
        for name, text, code, reason in cases:
            with self.subTest(case=name):
                path = self.root / f"rejected-{name}.rq"
                path.write_text(text)
                files = [{"path": str(path), "expected_sha256": hashlib.sha256(path.read_bytes()).hexdigest()}]
                self.call({"kind": "prepare_query", "query": text}, "INVALID_QUERY")
                rejected = self.call({"kind": "rules", "profile": "datalog-v1", "files": files}, code)
                self.assertEqual(rejected["error"]["reason"], reason)
                output = self.root / f"rejected-{name}"
                rejected = self.call({"kind": "compile", "sources": [source(data)], "output": str(output),
                                      "profile": "datalog-v1", "rules": files}, code)
                self.assertEqual(rejected["error"]["reason"], reason)
                self.assertFalse(output.exists())
                self.assertFalse(list(self.root.glob(f".{output.name}.staging-*")))

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

    def test_materialization_large_row_allowances_do_not_overflow(self):
        tiny = self.root / "large-allowance.ttl"
        tiny.write_text('''@prefix ex: <urn:allowance:> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
ex:item a ex:Leaf .
ex:Leaf rdfs:subClassOf ex:Root .''')
        compiled = self.root / "large-allowance-compiled"
        self.call({"kind": "compile", "sources": [source(tiny)],
                   "output": str(compiled), "profile": "rdfs-supported-v1"})
        for allowance in [0, 2**63 - 1, 2**64 - 1]:
            with self.subTest(max_intermediate_rows=allowance):
                output = self.root / f"large-allowance-{allowance}"
                reply = self.call({"kind": "materialize", "dataset": str(compiled),
                                   "output": str(output)},
                                  None if allowance else "RESOURCE_EXHAUSTED",
                                  limits={"max_intermediate_rows": allowance})
                if allowance:
                    self.assertEqual(reply["result"]["stats"],
                                     {"explicit_facts": 2, "inferred_facts": 1})
                    self.assertTrue(reply["result"]["fixpoint_reached"])
                    answer = self.query(output, "ASK { <urn:allowance:item> a <urn:allowance:Root> }",
                                        view="inferred")
                    self.assertTrue(answer["boolean"])
                else:
                    self.assertFalse(output.exists())

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

    def test_validation_depth_is_a_resource_failure_not_nonconformance(self):
        shapes = self.root / "depth-shapes.ttl"
        shapes.write_text('''@prefix sh: <http://www.w3.org/ns/shacl#> .
<urn:depth:shape> a sh:NodeShape; sh:targetNode <urn:contract:e0000>;
    sh:property <urn:depth:property> .
<urn:depth:property> sh:path <urn:contract:score>; sh:minCount 1 .''')
        op = {"kind": "validate", "dataset": str(self.compiled), "shapes": [source(shapes)]}
        before = hashlib.sha256((self.compiled / "facts.sqlite").read_bytes()).hexdigest()
        result = self.call(op, limits={"max_validation_depth": 2})["result"]
        self.assertEqual(set(result), {"conforms", "checked_shapes", "violations", "report"})
        self.assertTrue(result["conforms"])
        self.assertEqual(result["checked_shapes"], 1)
        self.call(op, "RESOURCE_EXHAUSTED", limits={"max_validation_depth": 1})
        self.call(op, "INVALID_REQUEST", limits={"max_validation_depth": 0})
        self.assertTrue(self.call(op)["result"]["conforms"])
        self.call({**op, "selected_shapes": ["<urn:depth:unknown>"]}, "EXECUTION_FAILED")
        self.assertEqual(before, hashlib.sha256((self.compiled / "facts.sqlite").read_bytes()).hexdigest())

    def test_validation_uses_supplied_snapshot_and_reports_zero_shapes(self):
        shapes = self.root / "materialized-shapes.ttl"
        shapes.write_text('''@prefix sh: <http://www.w3.org/ns/shacl#> .
<urn:materialized:shape> a sh:NodeShape; sh:targetNode <urn:contract:e0000>;
    sh:property <urn:materialized:property> .
<urn:materialized:property> sh:path <urn:contract:related>;
    sh:hasValue <urn:contract:bucket0> .''')
        for dataset, conforms in [(self.compiled, False), (self.reasoned, True)]:
            with self.subTest(dataset=dataset.name):
                op = {"kind": "validate", "dataset": str(dataset), "shapes": [source(shapes)]}
                result = self.call(op)["result"]
                self.assertEqual(set(result), {"conforms", "checked_shapes", "violations", "report"})
                self.assertEqual(result["conforms"], conforms)
                self.assertEqual(result["checked_shapes"], 1)
                self.assertEqual(result["violations"], 0 if conforms else 1)
                self.assertEqual(result["report"]["conforms"], conforms)
                for empty in [{"kind": "validate", "dataset": str(dataset)},
                              {**op, "shapes": []}, {**op, "selected_shapes": []}]:
                    zero = self.call(empty)["result"]
                    self.assertTrue(zero["conforms"])
                    self.assertEqual(zero["checked_shapes"], 0)
                    self.assertEqual(zero["violations"], 0)
                    self.assertEqual(zero["report"]["violations"], [])
        self.call({**op, "shapes_from_dataset": True}, "INVALID_REQUEST")

    def test_optional_content_digest_pins_query_and_validation_inputs(self):
        shapes = self.root / "guard-shapes.ttl"
        shapes.write_text('''@prefix sh: <http://www.w3.org/ns/shacl#> .
<urn:guard:shape> a sh:NodeShape; sh:targetNode <urn:contract:e0000>;
    sh:property <urn:guard:property> .
<urn:guard:property> sh:path <urn:contract:score>; sh:minCount 1 .''')
        operations = [
            ({"kind": "query", "dataset": str(self.reasoned), "query": "ASK {}"},
             self.reasoned_content_digest),
            ({"kind": "validate", "dataset": str(self.compiled), "shapes": [source(shapes)]},
             self.compiled_content_digest),
        ]
        for op, digest in operations:
            with self.subTest(operation=op["kind"]):
                result = self.call(op)["result"]
                self.assertEqual(self.call({**op, "expected_content_digest": None})["result"], result)
                self.assertEqual(self.call({**op, "expected_content_digest": digest})["result"], result)
                wrong = ("1" if digest[0] == "0" else "0") + digest[1:]
                self.call({**op, "expected_content_digest": wrong}, "INVALID_DATASET")
                for invalid in ["0" * 63, "A" * 64, "g" * 64, 7]:
                    self.call({**op, "expected_content_digest": invalid}, "INVALID_REQUEST")
                self.call({**op, "dataset": str(self.pack), "expected_content_digest": digest},
                          "UNSUPPORTED_CAPABILITY")
        capability = "dataset-content-digest/v1"
        for dataset, supported in [(self.compiled, True), (self.reasoned, True), (self.pack, False)]:
            capabilities = self.call({"kind": "capabilities", "dataset": str(dataset)})["result"]["dataset"]["capabilities"]
            self.assertEqual(capability in capabilities, supported)

    def test_optional_manifest_digest_pins_metadata_and_raw_bytes(self):
        for dataset, manifest_sha, content_digest in [
            (self.compiled, self.compiled_manifest_sha256, self.compiled_content_digest),
            (self.reasoned, self.reasoned_manifest_sha256, self.reasoned_content_digest),
        ]:
            self.assertEqual(hashlib.sha256((dataset / "manifest.json").read_bytes()).hexdigest(), manifest_sha)
            for op in [{"kind": "query", "dataset": str(dataset), "query": "ASK {}"},
                       {"kind": "validate", "dataset": str(dataset)}]:
                with self.subTest(dataset=dataset.name, operation=op["kind"]):
                    result = self.call(op)["result"]
                    self.assertEqual(self.call({**op, "expected_manifest_sha256": None})["result"], result)
                    self.assertEqual(self.call({**op, "expected_manifest_sha256": manifest_sha,
                                                "expected_content_digest": content_digest})["result"], result)
                    wrong = ("1" if manifest_sha[0] == "0" else "0") + manifest_sha[1:]
                    self.call({**op, "expected_manifest_sha256": wrong}, "INVALID_DATASET")
                    for invalid in ["0" * 63, "A" * 64, "g" * 64, 7]:
                        self.call({**op, "expected_manifest_sha256": invalid}, "INVALID_REQUEST")
                    self.call({**op, "dataset": str(self.pack), "expected_manifest_sha256": manifest_sha},
                              "UNSUPPORTED_CAPABILITY")

        # Same files and declarations, different manifest bytes: this guard is a
        # byte identity precondition, not a hash of re-serialized parsed JSON.
        copied = self.root / "reformatted-manifest"
        shutil.copytree(self.reasoned, copied)
        path = copied / "manifest.json"
        manifest = json.loads(path.read_bytes())
        path.write_text(json.dumps(manifest, indent=4) + "\n")
        updated = hashlib.sha256(path.read_bytes()).hexdigest()
        self.assertNotEqual(updated, self.reasoned_manifest_sha256)
        for op in [{"kind": "query", "dataset": str(copied), "query": "ASK {}"},
                   {"kind": "validate", "dataset": str(copied)}]:
            self.call({**op, "expected_content_digest": self.reasoned_content_digest})
            self.call({**op, "expected_manifest_sha256": self.reasoned_manifest_sha256}, "INVALID_DATASET")
            self.call({**op, "expected_manifest_sha256": updated})

        for dataset, supported in [(self.compiled, True), (self.reasoned, True), (self.pack, False)]:
            meta = self.call({"kind": "capabilities", "dataset": str(dataset)})["result"]["dataset"]
            self.assertEqual("dataset-manifest-digest/v1" in meta["capabilities"], supported)
            self.assertNotIn("manifest_sha256", meta)
            self.assertNotIn("manifest", meta)

    def test_manifest_metadata_matches_frozen_inputs(self):
        copied = self.root / "inconsistent-manifest"
        shutil.copytree(self.compiled, copied)
        path = copied / "manifest.json"
        original = json.loads(path.read_bytes())
        for field, value in [("profile_id", "no-rules-v1"), ("source_tree_digest", "0" * 64),
                             ("semantic_fingerprint", "0" * 64), ("kind", "unknown")]:
            with self.subTest(field=field):
                path.write_text(json.dumps({**original, field: value}))
                for op in [{"kind": "query", "dataset": str(copied), "query": "ASK {}"},
                           {"kind": "validate", "dataset": str(copied)},
                           {"kind": "capabilities", "dataset": str(copied)}]:
                    self.call(op, "INVALID_DATASET")

        # A files-only digest cannot bind a declaration of completed reasoning.
        # Pin the producer's complete manifest when such state matters.
        path.write_text(json.dumps({**original, "kind": "reasoned", "fixpoint_reached": True}))
        for op in [{"kind": "query", "dataset": str(copied), "query": "ASK {}", "view": "closure"},
                   {"kind": "validate", "dataset": str(copied)}]:
            self.call({**op, "expected_content_digest": self.compiled_content_digest,
                       "expected_manifest_sha256": self.compiled_manifest_sha256}, "INVALID_DATASET")

    def test_digest_guard_rejects_a_different_valid_artifact_at_the_same_path(self):
        current = self.root / "guard-current"
        shutil.copytree(self.reasoned, current)
        query = {"kind": "query", "dataset": str(current), "view": "closure",
                 "query": "ASK { <urn:contract:e0000> a <urn:contract:Root> }"}
        old = self.reasoned_content_digest
        self.assertTrue(self.call({**query, "expected_content_digest": old})["result"]["boolean"])
        data = self.root / "guard-replacement.ttl"
        data.write_text('<urn:replacement:s> <urn:replacement:p> <urn:replacement:o> .')
        replacement = self.root / "guard-replacement"
        compiled = self.call({"kind": "compile", "sources": [source(data)],
                              "output": str(replacement), "profile": "no-rules-v1"})["result"]
        new = compiled["artifact"]["content_digest"]
        self.assertNotEqual(new, old)
        shutil.rmtree(current)
        shutil.copytree(replacement, current)
        self.call({**query, "expected_content_digest": old}, "INVALID_DATASET")
        self.call({"kind": "validate", "dataset": str(current), "expected_content_digest": old},
                  "INVALID_DATASET")
        self.assertFalse(self.call(query)["result"]["boolean"])
        self.assertFalse(self.call({**query, "expected_content_digest": new})["result"]["boolean"])

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

    def test_rdfs_rule_graph_scope_is_distinct_from_query_union(self):
        trig = self.root / "rule-scope.trig"
        trig.write_text('''@prefix ex: <urn:scope:> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
ex:Leaf rdfs:subClassOf ex:Root .
ex:default a ex:Leaf .
ex:named { ex:item a ex:Leaf . }
ex:other { ex:Other rdfs:subClassOf ex:Parent . ex:item a ex:Other . }''')
        compiled, reasoned = self.root / "scope-compiled", self.root / "scope-reasoned"
        self.call({"kind": "compile", "sources": [{**source(trig), "format": "trig"}],
                   "output": str(compiled), "profile": "rdfs-supported-v1"})
        result = self.call({"kind": "materialize", "dataset": str(compiled),
                            "output": str(reasoned)})["result"]
        self.assertEqual(result["stats"], {"explicit_facts": 5, "inferred_facts": 1})
        for query, expected in [
            ("ASK { <urn:scope:default> a <urn:scope:Root> }", True),
            ("ASK { <urn:scope:item> a <urn:scope:Leaf> }", True),
            ("ASK { GRAPH <urn:scope:named> { <urn:scope:item> a <urn:scope:Leaf> } }", True),
            ("ASK { <urn:scope:item> a <urn:scope:Root> }", False),
            ("ASK { <urn:scope:item> a <urn:scope:Parent> }", False),
        ]:
            with self.subTest(query=query):
                self.assertIs(self.query(reasoned, query, view="closure")["boolean"], expected)

    def test_large_query_request_has_no_implicit_transport_limit(self):
        query = 'SELECT ?value WHERE { VALUES ?value { "' + 'x' * (5 * 1024 * 1024) + '" } }'
        result = self.call({"kind": "prepare_query", "query": query})["result"]
        self.assertTrue(result["valid"])

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
