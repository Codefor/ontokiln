"""SHACL navigation and target regressions through the stable supervised API."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

BINARY = str(Path(sys.argv.pop(1)).resolve())
PREFIXES = """@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
"""


class Validation(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="ontokiln-shacl-index-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.sequence = 0

    def source(self, text, format="turtle"):
        self.sequence += 1
        path = self.root / f"source-{self.sequence}.rdf"
        path.write_text(text)
        return {"path": str(path), "document_id": path.name, "format": format,
                "expected_sha256": hashlib.sha256(path.read_bytes()).hexdigest()}

    def call(self, operation, error=None):
        self.sequence += 1
        request = self.root / f"request-{self.sequence}.json"
        output = self.root / f"result-{self.sequence}.json"
        raw = json.dumps({"contract": "ontokiln-api/v1",
                          "request_id": request.stem, "operation": operation,
                          "limits": {"timeout_ms": 30000}}).encode()
        request.write_bytes(raw)
        subprocess.run([BINARY, "api", "--request", request, "--result", output],
                       check=True, timeout=40)
        reply = json.loads(output.read_bytes())
        self.assertEqual(reply["request_sha256"], hashlib.sha256(raw).hexdigest())
        self.assertIs(reply["execution"]["worker_reaped"], True)
        if pid := reply["execution"]["worker_pid"]:
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)
        if error:
            self.assertEqual(reply["error"]["code"], error, reply)
            self.assertEqual(reply["status"],
                             "BUDGET_EXHAUSTED" if error == "RESOURCE_EXHAUSTED" else "FAILED", reply)
            self.assertIsNone(reply["result"])
            return reply
        self.assertEqual(reply["status"], "COMPLETE", reply)
        self.assertIsNone(reply["error"], reply)
        return reply["result"]

    def compile(self, text, format="turtle"):
        source = self.source(PREFIXES + text, format)
        dataset = self.root / f"compiled-{self.sequence}"
        self.call({"kind": "compile", "profile": "no-rules-v1",
                   "sources": [source], "output": str(dataset)})
        return str(dataset)

    def validate(self, dataset, text, error=None, **options):
        result = self.call({"kind": "validate", "dataset": dataset,
                            "shapes": [self.source(PREFIXES + text)], **options}, error)
        return result if error else result["report"]

    def test_explicit_and_implicit_targets_follow_transitive_subclasses_and_cycles(self):
        # Renaming all nonstandard IRIs leaves target discovery and constraints unchanged.
        for namespace in ("urn:example:", "https://other.example/model/"):
            with self.subTest(namespace=namespace):
                prefix = f"@prefix ex: <{namespace}> .\n"
                dataset = self.compile(prefix + """
ex:Student rdfs:subClassOf ex:Person, ex:Pupil .
ex:Pupil rdfs:subClassOf ex:Student .
ex:alice a ex:Person; ex:name "Alice" .
ex:bob a ex:Student .
ex:carol a ex:Pupil; ex:name "Carol" .
ex:unrelated ex:name "Unrelated" .
ex:other a ex:Elsewhere .
""")
                report = self.validate(dataset, prefix + """
ex:Person a sh:NodeShape, rdfs:Class;
    sh:property [sh:path ex:name; sh:minCount 1] .
ex:Explicit a sh:NodeShape; sh:targetClass ex:Person;
    sh:property [sh:path ex:name; sh:minCount 1] .
""")
                self.assertFalse(report["conforms"])
                self.assertEqual(report["target_count"], 6)
                expected = {f"<{namespace}{name}>" for name in ("alice", "bob", "carol")}
                self.assertEqual(len(report["coverage"]), 2)
                for shape in report["coverage"]:
                    self.assertEqual(shape["target_count"], 3)
                    self.assertEqual(set(shape["focus_nodes"]), expected)
                self.assertEqual(len(report["violations"]), 2)
                self.assertEqual({v["focus_node"] for v in report["violations"]},
                                 {namespace + "bob"})

    def test_implicit_class_declaration_comes_from_shapes_and_focus_overrides_targets(self):
        dataset = self.compile("""
<urn:Shape> a rdfs:Class .
<urn:member> a <urn:Shape> .
<urn:outsider> <urn:unrelated> true .
""")
        shape = """<urn:Shape> a sh:NodeShape;
    sh:property [sh:path <urn:required>; sh:minCount 1] ."""
        report = self.validate(dataset, shape)
        self.assertEqual(report["target_count"], 0)
        self.assertTrue(report["conforms"])
        implicit = shape + "\n<urn:Shape> a rdfs:Class ."
        report = self.validate(dataset, implicit)
        self.assertEqual(report["target_count"], 1)
        self.assertEqual(report["violations"][0]["focus_node"], "urn:member")
        report = self.validate(dataset, implicit, focus_node="<urn:outsider>")
        self.assertEqual(report["target_count"], 1)
        self.assertEqual(report["violations"][0]["focus_node"], "urn:outsider")
        report = self.validate(dataset, implicit, selected_shapes=[])
        self.assertEqual(report["target_count"], 0)
        self.assertTrue(report["conforms"])

    def test_named_graph_union_deduplicates_targets(self):
        dataset = self.compile("""
<urn:g1> { <urn:alice> a <urn:Student> . }
<urn:g2> { <urn:alice> a <urn:Student> .
           <urn:Student> rdfs:subClassOf <urn:Person> . }
""", "trig")
        report = self.validate(dataset, """
<urn:Person> a sh:NodeShape, rdfs:Class;
    sh:property [sh:path <urn:name>; sh:minCount 1] .
""")
        self.assertEqual(report["target_count"], 1)
        self.assertEqual(len(report["violations"]), 1)

    def test_blank_class_shapes_are_ill_formed_even_with_an_explicit_focus(self):
        dataset = self.compile('<urn:a> <urn:present> true .')
        for kind in ("NodeShape", "PropertyShape"):
            with self.subTest(kind=kind):
                path = "; sh:path <urn:present>" if kind == "PropertyShape" else ""
                shape = f"_:shape a sh:{kind}, rdfs:Class{path} ."
                for options in ({}, {"focus_node": "<urn:a>"}):
                    self.validate(dataset, shape, error="EXECUTION_FAILED", **options)
        # Ordinary anonymous shapes remain valid when they do not denote classes.
        report = self.validate(dataset, """
[] a sh:NodeShape; sh:targetNode <urn:a>;
    sh:property [sh:path <urn:present>; sh:minCount 1] .
""")
        self.assertTrue(report["conforms"])
        self.assertEqual(report["target_count"], 1)

    def test_predicate_targets_include_literal_objects_and_keep_schema_caches_separate(self):
        dataset = self.compile("""
<urn:a> <urn:edge> <urn:b>, "literal"; <urn:present> true .
<urn:b> <urn:other> "outside" .
""")
        report = self.validate(dataset, """
<urn:Subjects> a sh:NodeShape; sh:targetSubjectsOf <urn:edge>;
    sh:property [sh:path <urn:present>; sh:minCount 1] .
<urn:DifferentSchema> a sh:NodeShape; sh:targetSubjectsOf <urn:edge>;
    sh:property [sh:path <urn:missing>; sh:minCount 1] .
<urn:Objects> a sh:NodeShape; sh:targetObjectsOf <urn:edge>; sh:nodeKind sh:IRI .
""")
        self.assertEqual(report["target_count"], 4)
        self.assertEqual(len(report["violations"]), 2)
        self.assertEqual({v["focus_node"] for v in report["violations"]}, {"urn:a", '"literal"'})

    def test_lazy_complex_path_and_sparql_read_complete_graph(self):
        dataset = self.compile("""
<urn:a> <urn:edge> <urn:b> .
<urn:b> <urn:name> "Bob"; <urn:blocked> true .
""")
        report = self.validate(dataset, """
<urn:Shape> a sh:NodeShape; sh:targetNode <urn:a>;
    sh:property [sh:path (<urn:edge> <urn:name>); sh:hasValue "Bob"];
    sh:sparql [sh:select "SELECT $this WHERE { $this <urn:edge> ?b . ?b <urn:blocked> true }"] .
""")
        self.assertFalse(report["conforms"])
        self.assertEqual(len(report["violations"]), 1)
        self.assertTrue(report["violations"][0]["source_constraint_component"].endswith("SPARQLConstraintComponent"))

    def test_simple_path_keeps_select_row_budget(self):
        dataset = self.compile("\n".join(f"<urn:a> <urn:p> <urn:v:{i}> ." for i in range(4097)))
        self.validate(dataset, """
<urn:Shape> a sh:NodeShape; sh:targetNode <urn:a>;
    sh:property [sh:path <urn:p>; sh:minCount 1] .
""", error="RESOURCE_EXHAUSTED")

    def test_simple_path_keeps_select_byte_budget(self):
        literal = "x" * (512 * 1024)
        dataset = self.compile(f'<urn:a> <urn:p> "{literal}0", "{literal}1" .')
        self.validate(dataset, """
<urn:Shape> a sh:NodeShape; sh:targetNode <urn:a>;
    sh:property [sh:path <urn:p>; sh:minCount 1] .
""", error="RESOURCE_EXHAUSTED")


if __name__ == "__main__":
    unittest.main()
