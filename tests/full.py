"""Exercise real Full supervisors, workers, immutable artifacts and generic RDF."""
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

BINARY = str(Path(sys.argv.pop(1)).resolve())


class Full(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="ontokiln-full-test-")
        self.root = Path(self.directory.name)
        self.sequence = 0
        self.kernel = json.loads(subprocess.check_output([BINARY, "identity"]))

    def tearDown(self):
        self.directory.cleanup()

    def request(self, value, error=None):
        self.sequence += 1
        request = self.root / f"request-{self.sequence}.json"
        result = self.root / f"result-{self.sequence}.json"
        value = {"memory_budget_bytes": 512 * 1024**2,
                 "disk_budget_bytes": 1024**3, "timeout_seconds": 15, **value}
        raw = json.dumps(value, sort_keys=True).encode()
        request.write_bytes(raw)
        subprocess.run([BINARY, "full", "--request", str(request), "--result", str(result)],
                       check=True, timeout=25)
        reply = json.loads(result.read_bytes())
        execution = reply["execution"]
        self.assertEqual(execution["request_sha256"], hashlib.sha256(raw).hexdigest())
        self.assertEqual(execution["kernel"], self.kernel)
        self.assertIs(execution["worker_reaped"], True)
        pid = execution["worker_pid"]
        if pid:
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)
        if error:
            self.assertEqual(reply["error"]["code"], error, reply)
            self.assertNotEqual(execution["status"], "COMPLETE")
        else:
            self.assertNotIn("error", reply)
            self.assertEqual(execution["status"], "COMPLETE")
        return reply

    def compile(self, text, name="compiled", profile="no-rules-v1", rules=None, error=None):
        source = self.root / f"{name}.ttl"
        source.write_text(text)
        rule_files = []
        if rules is not None:
            rule = self.root / f"{name}-rules.json"
            rule.write_text(json.dumps({"schema_version": "ontology-rule-ir/v1", "rules": rules}))
            rule_files.append({"path": str(rule), "expected_sha256": hashlib.sha256(rule.read_bytes()).hexdigest()})
        output = self.root / name
        result = self.request({"operation": "compile", "max_term_bytes": 1024**2,
            "sources": [{"path": str(source), "document_id": "doc", "format": "turtle",
                         "expected_sha256": hashlib.sha256(source.read_bytes()).hexdigest()}],
            "artifact": {"output": str(output), "profile_id": profile,
                         "index_profile": "core-v1", "rules_files": rule_files}}, error)
        if error and error != "BUILD_ABORTED":
            self.assertFalse(output.exists())
            self.assertFalse(list(self.root.glob(f".{name}.staging-*")))
        elif not error:
            self.assertIs(result["integrity_checked"], True)
            self.assertEqual(result["proof_coverage"]["per_fact_proof"], "not_recorded")
        return output

    def reason(self, compiled, name="reasoned", error=None, **limits):
        output = self.root / name
        self.request({"operation": "materialize", "artifact": {"compiled": str(compiled), "output": str(output)},
            "max_iterations": 100, "max_derived_facts": 1000, "max_intermediate_rows": 10000,
            **limits}, error)
        if error:
            self.assertFalse(output.exists())
        return output

    def query(self, dataset, query, **options):
        return self.request({"operation": "query", "dataset": str(dataset / "facts.sqlite"),
                             "query": query, **options})

    def test_recursive_rules_views_and_reproducibility(self):
        rules = [
            {"rule_id": "seed", "head": ["?x", "<urn:up>", "?y"], "body": [["?x", "<urn:edge>", "?y"]]},
            {"rule_id": "step", "head": ["?x", "<urn:up>", "?z"],
             "body": [["?x", "<urn:up>", "?y"], ["?y", "<urn:up>", "?z"]]},
        ]
        text = '<urn:a> <urn:edge> <urn:b> . <urn:b> <urn:edge> <urn:c> .'
        compiled = self.compile(text, profile="datalog-v1", rules=rules)
        duplicate = self.compile(text, "duplicate", "datalog-v1", rules)
        self.assertEqual((compiled / "manifest.json").read_bytes(), (duplicate / "manifest.json").read_bytes())
        reasoned = self.reason(compiled)
        query = "SELECT ?o WHERE { <urn:a> <urn:up> ?o } ORDER BY ?o"
        self.assertEqual([row["o"]["value"] for row in self.query(reasoned, query)["items"]], ["urn:b", "urn:c"])
        self.assertEqual(self.query(reasoned, query, view="asserted")["items"], [])
        self.assertEqual(len(self.query(reasoned, query, view="inferred")["items"]), 2)
        self.reason(compiled, "limited", "FIXPOINT_LIMIT_EXCEEDED", max_iterations=0)
        before = (compiled / "manifest.json").read_bytes()
        self.compile(text, error="BUILD_ABORTED")
        self.assertEqual(before, (compiled / "manifest.json").read_bytes())

    def test_selected_shapes_focus_and_depth_are_generic(self):
        compiled = self.compile('<urn:a> <urn:present> "yes" .')
        shape = self.root / "selected.ttl"
        shape.write_text("""@prefix sh: <http://www.w3.org/ns/shacl#> .
<urn:shape> a sh:NodeShape; sh:property [ sh:path <urn:required>; sh:minCount 1 ] .
<urn:other> a sh:NodeShape; sh:targetNode <urn:other-data>;
    sh:property [ sh:path <urn:absent>; sh:minCount 1 ] .""")
        request = {"operation": "validate", "dataset": str(compiled),
            "shapes": [{"file": str(shape), "format": "turtle"}],
            "selected_shapes": ["<urn:shape>"], "focus_node": "<urn:a>"}
        reply = self.request(request)
        self.assertEqual(reply["checked_shapes"], 1)
        self.assertFalse(reply["conforms"])
        violations = json.loads(reply["report"])["violations"]
        self.assertEqual(len(violations), 1)
        self.assertEqual(violations[0]["focus_node"], "urn:a")
        self.assertTrue(self.request({**request, "selected_shapes": []})["conforms"])
        self.request({**request, "focus_node": '\"literal\"'}, "SOURCE_INVALID")
        self.request({**request, "max_validation_depth": 0}, "SOURCE_INVALID")
        self.request({**request, "max_validation_depth": 1}, "VALIDATION_FAILED")
        self.request({**request, "selected_shapes": ["<urn:unknown>"]}, "VALIDATION_FAILED")

    def test_input_binding_outside_projection_keeps_filter_order_and_columns(self):
        compiled = self.compile('<urn:a> <urn:score> 1, 2, 3 .')
        query = 'SELECT ?score WHERE { <urn:a> <urn:score> ?score FILTER(?score > ?minimum) } ORDER BY IF(?descending, -?score, ?score)'
        result = self.query(compiled, query, rdf_bindings={'minimum': '"1"^^<http://www.w3.org/2001/XMLSchema#integer>', 'descending': '"true"^^<http://www.w3.org/2001/XMLSchema#boolean>'})
        self.assertEqual(result['variables'], ['score'])
        self.assertEqual([row['score']['value'] for row in result['items']], ['3', '2'])
        self.assertEqual(self.query(compiled, query, rdf_bindings={'minimum': '"3"^^<http://www.w3.org/2001/XMLSchema#integer>', 'descending': '"false"^^<http://www.w3.org/2001/XMLSchema#boolean>'})['items'], [])

    def test_dataset_shapes_keep_shared_blank_node_identity(self):
        compiled = self.compile('@prefix sh: <http://www.w3.org/ns/shacl#> .\n<urn:shape> a sh:NodeShape; sh:targetNode _:x; sh:property [sh:path <urn:p>; sh:hasValue _:y] .\n_:x <urn:p> _:y .')
        result = self.request({'operation': 'validate', 'dataset': str(compiled),
            'shapes': [], 'shapes_from_dataset': True, 'selected_shapes': ['<urn:shape>']})
        self.assertTrue(result['conforms'], result)
        self.assertEqual(json.loads(result['report'])['target_count'], 1)

    def test_domain_vocabulary_and_json_are_only_rdf(self):
        for index, namespace in enumerate(["urn:first:", "urn:second:"]):
            graph = self.compile(f'<{namespace}x> a <{namespace}FieldAssignment>; '
                f'<{namespace}ruleExpression> "{{\\"contract\\":\\"view-field-members/v1\\"}}" .', f"domain-{index}")
            self.assertEqual(len(self.query(graph, "SELECT ?s ?p ?o WHERE { ?s ?p ?o }")["items"]), 2)
        self.request({"operation": "tools"}, "NATIVE_ENGINE_FAILED")
        self.request({"operation": "rules", "profile": "identity-priority/v1", "files": []}, "UNSUPPORTED_RULE_PROFILE")
        self.compile('<urn:has space> <urn:p> <urn:o> .', "invalid", error="TTL_PARSE_ERROR")

    def test_shacl_core_local_select_and_network_rejection(self):
        compiled = self.compile('<urn:x> <urn:score> 1 .')
        shapes = self.root / "shapes.ttl"
        prefix = '@prefix sh: <http://www.w3.org/ns/shacl#> . '
        def validate(text, error=None):
            shapes.write_text(prefix + text)
            return self.request({"operation": "validate", "dataset": str(compiled),
                "shapes": [{"file": str(shapes), "format": "turtle"}]}, error)
        result = validate('<urn:shape> a sh:NodeShape; sh:targetNode <urn:x>; '
                          'sh:property [ sh:path <urn:score>; sh:minCount 1; sh:minInclusive 2 ] .')
        self.assertFalse(result["conforms"])
        self.assertGreater(result["violations"], 0)
        self.assertTrue(validate('<urn:shape> a sh:NodeShape; sh:targetNode <urn:x>; '
                          'sh:property [ sh:path <urn:score>; sh:minCount 1 ] .')["conforms"])
        self.assertFalse(validate('<urn:shape> a sh:NodeShape; sh:targetNode <urn:x>; '
            'sh:sparql [ sh:select "SELECT $this WHERE { $this <urn:score> ?score FILTER (?score < 2) }" ] .')["conforms"])
        validate('<urn:shape> a sh:NodeShape; sh:targetNode <urn:x>; '
            'sh:sparql [ sh:select "SELECT $this WHERE { SERVICE <https://example.invalid/> { ?s ?p ?o } }" ] .',
            "UNSUPPORTED_RULE_PROFILE")

    def test_shacl_reuses_plans_without_cross_shape_or_focus_results(self):
        compiled = self.compile("\n".join(f"<urn:n{i}> <urn:score> {i} ." for i in range(128)))
        shapes = self.root / "many-shapes.ttl"
        shapes.write_text('@prefix sh: <http://www.w3.org/ns/shacl#> .\n<urn:lower> a sh:PropertyShape; sh:targetSubjectsOf <urn:score>; sh:path <urn:score>; sh:minInclusive 64 .\n<urn:upper> a sh:PropertyShape; sh:targetSubjectsOf <urn:score>; sh:path <urn:score>; sh:maxInclusive 63 .\n<urn:select> a sh:NodeShape; sh:targetSubjectsOf <urn:score>;\n sh:sparql [sh:select "SELECT $this WHERE { $this <urn:score> ?v FILTER (?v = 0) }"] .\n')
        result = self.request({"operation": "validate", "dataset": str(compiled),
            "shapes": [{"file": str(shapes), "format": "turtle"}]})
        report = json.loads(result["report"])
        pairs = {(row["shape_ref"].strip("<>"), row["focus_node"].strip("<>")) for row in report["violations"]}
        expected = {("urn:lower", f"urn:n{i}") for i in range(64)}
        expected |= {("urn:upper", f"urn:n{i}") for i in range(64, 128)}
        expected.add(("urn:select", "urn:n0"))
        self.assertEqual(pairs, expected)
        self.assertEqual(len(report["violations"]), 129)

    def test_shacl_closed_without_property_constraints_is_enforced(self):
        compiled = self.compile('<urn:x> <urn:p> 1 .')
        shapes = self.root / "closed.ttl"
        shapes.write_text('@prefix sh: <http://www.w3.org/ns/shacl#> . <urn:shape> a sh:NodeShape; sh:targetNode <urn:x>; sh:closed true .')
        result = self.request({"operation": "validate", "dataset": str(compiled),
            "shapes": [{"file": str(shapes), "format": "turtle"}]})
        self.assertFalse(result["conforms"])
        self.assertEqual(result["violations"], 1)

    def test_unsafe_rule_and_corrupt_artifact_fail_closed(self):
        self.compile('<urn:a> <urn:p> <urn:b> .', "unsafe", "datalog-v1",
            [{"rule_id": "unsafe", "head": ["?s", "<urn:q>", "?unknown"],
              "body": [["?s", "<urn:p>", "?o"]]}], "UNSAFE_RULE")
        compiled = self.compile('<urn:a> <urn:p> <urn:b> .')
        (compiled / "facts.sqlite").write_bytes(b"corrupt")
        self.reason(compiled, error="MANIFEST_CORRUPT")

    def test_typed_parameters_preserve_scope_and_term_identity(self):
        compiled = self.compile('<urn:x> <urn:p> "01"^^<http://www.w3.org/2001/XMLSchema#integer>; '
                                '<urn:label> "中文"@ZH .')
        result = self.query(compiled,
            'SELECT ?value ?v WHERE { VALUES ?value { <urn:ontokiln:param:v> } '
            '{ SELECT ("private" AS ?v) WHERE {} } }',
            parameters={"v": {"type": "literal", "value": "01", "datatype": "http://www.w3.org/2001/XMLSchema#integer"}})
        self.assertEqual(result["items"][0]["value"]["value"], "01")
        self.assertEqual(result["items"][0]["v"]["value"], "private")
        self.request({"operation": "query", "dataset": str(compiled / "facts.sqlite"),
                      "query": "SELECT ?s WHERE { ?s ?p ?o }", "params": {"s": "urn:x"}}, "QUERY_FAILED")

    def test_shacl_parameterized_target_ask_and_nested_focus(self):
        prefix = '@prefix sh: <http://www.w3.org/ns/shacl#> . @prefix ex: <http://example/> . '
        shapes = self.root / "parameterized.ttl"
        shapes.write_text(prefix + '''
            ex:Minimum a sh:ConstraintComponent; sh:parameter [sh:path ex:minimum];
                sh:parameter [sh:path ex:maximum; sh:optional true];
                sh:validator [a sh:SPARQLAskValidator;
                    sh:ask "ASK { FILTER ($value >= $minimum && (!BOUND($maximum) || $value <= $maximum)) }"] .
            ex:OfKind a sh:SPARQLTargetType; sh:parameter [sh:path ex:kind];
                sh:select "SELECT ?this WHERE { ?this a $kind }" .
            ex:Shape a sh:NodeShape; sh:target [a ex:OfKind; ex:kind ex:Item];
                sh:property [sh:path ex:value; ex:minimum 5] .''')
        for value, conforms in [(7, True), (3, False)]:
            compiled = self.compile(f'<http://example/item> a <http://example/Item>; <http://example/value> {value} .', f"value-{value}")
            result = self.request({"operation": "validate", "dataset": str(compiled),
                "shapes": [{"file": str(shapes), "format": "turtle"}]})
            self.assertIs(result["conforms"], conforms)

        shapes.write_text(prefix.replace('http://example/', 'urn:') + '''ex:Shape a sh:NodeShape; sh:targetNode ex:root;
            sh:property [sh:path ex:child; sh:node [sh:or (
                [sh:sparql [sh:select "SELECT $this WHERE { $this <urn:bad> true }"]]
                [sh:hasValue ex:allowed])]] .''')
        for child, bad, conforms in [("allowed", "allowed", True), ("rejected", "rejected", False), ("rejected", "root", True)]:
            compiled = self.compile(f'<urn:root> <urn:child> <urn:{child}> . <urn:{bad}> <urn:bad> true .', f"nested-{child}-{bad}")
            result = self.request({"operation": "validate", "dataset": str(compiled),
                "shapes": [{"file": str(shapes), "format": "turtle"}]})
            self.assertIs(result["conforms"], conforms)


    def test_unchecked_turtle_is_explicit_and_recorded(self):
        source = self.root / "unchecked.ttl"
        source.write_text('<urn:unvalidated[1]> <urn:p> <urn:o> .')
        output = self.root / "unchecked"
        result = self.request({"operation": "compile", "max_term_bytes": 1024**2,
            "sources": [{"path": str(source), "document_id": "doc", "format": "turtle",
                         "unchecked_turtle_terms": True, "expected_sha256": hashlib.sha256(source.read_bytes()).hexdigest()}],
            "artifact": {"output": str(output), "profile_id": "no-rules-v1", "index_profile": "core-v1", "rules_files": []}})
        self.assertEqual(result["stats"]["explicit_facts"], 1)
        manifest = json.loads((output / "input_manifest.json").read_text())
        self.assertIs(manifest["parser_options"][0]["unchecked_turtle_terms"], True)

    def test_numeric_values_do_not_erase_rdf_datatypes_or_overflow(self):
        xsd = 'http://www.w3.org/2001/XMLSchema#'
        compiled = self.compile(f'<urn:a> <urn:p> "003"^^<{xsd}nonNegativeInteger> . '
                                f'<urn:large> <urn:p> "1267650600228229401496703205376"^^<{xsd}integer> .')
        row = self.query(compiled, 'SELECT (DATATYPE(?v) AS ?direct) (STR(?v) AS ?lexical) '
            '(DATATYPE(IF(true, ?v, 0)) AS ?conditional) (STR(COALESCE(?missing, ?v)) AS ?fallback) '
            'WHERE { <urn:a> <urn:p> ?v }')["items"][0]
        self.assertEqual(row["direct"]["value"], xsd + 'nonNegativeInteger')
        self.assertEqual(row["conditional"]["value"], xsd + 'nonNegativeInteger')
        self.assertEqual(row["lexical"]["value"], '003')
        self.assertEqual(row["fallback"]["value"], '003')
        row = self.query(compiled, 'SELECT (?v + 1 AS ?next) (-?v AS ?negative) (?v > 0 AS ?positive) '
            'WHERE { <urn:large> <urn:p> ?v }')["items"][0]
        self.assertEqual(row["next"]["value"], '1267650600228229401496703205377')
        self.assertEqual(row["negative"]["value"], '-1267650600228229401496703205376')
        self.assertEqual(row["positive"]["value"], 'true')
    def test_timeout_reaps_worker_and_does_not_publish(self):
        source = self.root / "large.ttl"
        source.write_text("".join(f'<urn:s{i}> <urn:p> <urn:o> .\n' for i in range(20000)))
        output = self.root / "limited"
        self.request({"operation": "compile", "max_term_bytes": 1024**2, "timeout_seconds": 0.000001,
            "sources": [{"path": str(source), "document_id": "doc", "format": "turtle",
                         "expected_sha256": hashlib.sha256(source.read_bytes()).hexdigest()}],
            "artifact": {"output": str(output), "profile_id": "no-rules-v1", "index_profile": "core-v1", "rules_files": []}},
            "RESOURCE_LIMIT_EXCEEDED")
        self.assertFalse(output.exists())
        self.assertFalse(list(self.root.glob(".limited*")))

    def test_cancellation_reaps_worker_before_terminal_record(self):
        source = self.root / "cancel.ttl"
        source.write_text("".join(f'<urn:s{i}> <urn:p> <urn:o> .\n' for i in range(100000)))
        output = self.root / "cancelled"
        request = self.root / "cancel.json"
        result = self.root / "cancel-result.json"
        request.write_text(json.dumps({"operation": "compile", "max_term_bytes": 1024**2,
            "memory_budget_bytes": 512 * 1024**2, "disk_budget_bytes": 1024**3, "timeout_seconds": 15,
            "sources": [{"path": str(source), "document_id": "doc", "format": "turtle",
                         "expected_sha256": hashlib.sha256(source.read_bytes()).hexdigest()}],
            "artifact": {"output": str(output), "profile_id": "no-rules-v1", "index_profile": "core-v1", "rules_files": []}}))
        process = subprocess.Popen([BINARY, "full", "--request", str(request), "--result", str(result)])
        worker = None
        try:
            deadline = time.monotonic() + 5
            receipt = self.root / "cancelled.native-execution.json"
            while time.monotonic() < deadline:
                try:
                    worker = json.loads(receipt.read_text()).get("worker_pid")
                except (OSError, ValueError):
                    pass
                if worker:
                    break
                self.assertIsNone(process.poll(), "supervisor exited before cancellation")
                time.sleep(.005)
            self.assertIsNotNone(worker)
            process.send_signal(signal.SIGTERM)
            self.assertEqual(process.wait(timeout=5), 0)
            reply = json.loads(result.read_text())
            self.assertEqual(reply["execution"]["status"], "CANCELLED")
            self.assertIs(reply["execution"]["worker_reaped"], True)
            with self.assertRaises(ProcessLookupError):
                os.kill(worker, 0)
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


if __name__ == "__main__":
    unittest.main()
