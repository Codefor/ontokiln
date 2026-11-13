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

    def test_explicit_class_targets_follow_transitive_subclasses_and_cycles(self):
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
ex:Person a sh:NodeShape, rdfs:Class; sh:targetClass ex:Person;
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

    def test_class_annotations_keep_explicit_target_contract_and_focus_override(self):
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
        self.assertEqual(report["target_count"], 0)
        self.assertTrue(report["conforms"])
        explicit = implicit + "\n<urn:Shape> sh:targetClass <urn:Shape> ."
        report = self.validate(dataset, explicit)
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
<urn:Person> a sh:NodeShape, rdfs:Class; sh:targetClass <urn:Person>;
    sh:property [sh:path <urn:name>; sh:minCount 1] .
""")
        self.assertEqual(report["target_count"], 1)
        self.assertEqual(len(report["violations"]), 1)

    def test_class_annotations_do_not_change_blank_shape_acceptance(self):
        dataset = self.compile('<urn:a> <urn:present> true .')
        for kind in ("NodeShape", "PropertyShape"):
            with self.subTest(kind=kind):
                path = "; sh:path <urn:present>" if kind == "PropertyShape" else ""
                shape = f"_:shape a sh:{kind}, rdfs:Class{path} ."
                for options in ({}, {"focus_node": "<urn:a>"}):
                    report = self.validate(dataset, shape, **options)
                    self.assertTrue(report["conforms"])
                    self.assertEqual(report["target_count"], 1 if options else 0)
        # Ordinary anonymous shapes keep the same explicit targeting behavior.
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

    def test_complex_path_and_sparql_read_complete_graph(self):
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

    def test_parameterized_target_and_validators_keep_value_and_term_equality(self):
        dataset = self.compile('''@prefix ex: <https://parameter.example/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
ex:first a ex:Item; ex:value "01"^^xsd:integer .
ex:second a ex:Item; ex:value "1"^^xsd:integer .''')
        report = self.validate(dataset, '''@prefix ex: <https://parameter.example/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
ex:Bounds a sh:ConstraintComponent; sh:parameter [sh:path ex:minimum];
    sh:parameter [sh:path ex:maximum; sh:optional true];
    sh:validator [a sh:SPARQLAskValidator;
        sh:ask "ASK { FILTER ($value >= $minimum && (!BOUND($maximum) || $value <= $maximum)) }"] .
ex:Lexical a sh:ConstraintComponent; sh:parameter [sh:path ex:exact];
    sh:validator [a sh:SPARQLSelectValidator;
        sh:select "SELECT $this WHERE { FILTER (!sameTerm($value, $exact)) }"] .
ex:OfKind a sh:SPARQLTargetType; sh:parameter [sh:path ex:kind];
    sh:select "SELECT ?this WHERE { ?this a $kind }" .
ex:Shape a sh:NodeShape; sh:target [a ex:OfKind; ex:kind ex:Item];
    sh:property ex:Value .
ex:Value sh:path ex:value; ex:minimum 0; ex:exact "01"^^xsd:integer .
''', selected_shapes=["<https://parameter.example/Shape>"])
        self.assertFalse(report["conforms"])
        self.assertEqual(report["target_count"], 2)
        self.assertEqual(len(report["violations"]), 1)
        violation = report["violations"][0]
        self.assertEqual(violation["focus_node"], "https://parameter.example/second")
        self.assertEqual(violation["source_shape"], "https://parameter.example/Value")
        self.assertEqual(violation["source_constraint_component"], "https://parameter.example/Lexical")

    def test_sparql_validation_aggregates_all_stored_facts_without_named_graphs(self):
        data = self.source(PREFIXES + '''
<urn:Leaf> rdfs:subClassOf <urn:Root> .
<urn:a> a <urn:Leaf> .
<urn:g1> { <urn:a> a <urn:Leaf>; <urn:edge> <urn:b> . }
<urn:g2> { <urn:a> <urn:edge> <urn:b> . }
''', "trig")
        compiled, reasoned = self.root / "union-compiled", self.root / "union-reasoned"
        self.call({"kind": "compile", "profile": "rdfs-supported-v1",
                   "sources": [data], "output": str(compiled)})
        self.call({"kind": "materialize", "dataset": str(compiled), "output": str(reasoned)})
        for dataset, count in [(compiled, 3), (reasoned, 4)]:
            with self.subTest(dataset=dataset.name):
                report = self.validate(str(dataset), f'''
<urn:UnionShape> a sh:NodeShape; sh:targetNode <urn:a>;
    sh:sparql [sh:select """SELECT $this WHERE {{
        {{ SELECT (COUNT(*) AS ?n) WHERE {{ ?s ?p ?o }} }}
        FILTER (?n = {count})
        FILTER NOT EXISTS {{ GRAPH ?g {{ ?s ?p ?o }} }}
    }}"""] .
''')
                self.assertFalse(report["conforms"])
                self.assertEqual(report["target_count"], 1)
                self.assertEqual(len(report["violations"]), 1)
                self.assertEqual(report["violations"][0]["focus_node"], "urn:a")

    def test_simple_and_complex_paths_keep_select_row_budget(self):
        dataset = self.compile("\n".join(f"<urn:a> <urn:p> <urn:v:{i}> ." for i in range(4097)))
        for path in ["<urn:p>", "[sh:alternativePath (<urn:p> <urn:absent>)]"]:
            with self.subTest(path=path):
                self.validate(dataset, f"""
<urn:Shape> a sh:NodeShape; sh:targetNode <urn:a>;
    sh:property [sh:path {path}; sh:minCount 1] .
""", error="RESOURCE_EXHAUSTED")

    def test_simple_and_complex_paths_keep_select_byte_budget(self):
        literal = "x" * (512 * 1024)
        dataset = self.compile(f'<urn:a> <urn:p> "{literal}0", "{literal}1" .')
        for path in ["<urn:p>", "[sh:alternativePath (<urn:p> <urn:absent>)]"]:
            with self.subTest(path=path):
                self.validate(dataset, f"""
<urn:Shape> a sh:NodeShape; sh:targetNode <urn:a>;
    sh:property [sh:path {path}; sh:minCount 1] .
""", error="RESOURCE_EXHAUSTED")

    def test_lenient_artifact_keeps_validation_of_unvisited_terms(self):
        # Historic Full artifacts can contain lenient Turtle terms. A valid
        # focus must not hide an invalid unrelated term that Core used to parse.
        source = self.source('<urn:valid> <urn:p> true .\n'
                             '<urn:unvalidated[1]> <urn:other> <urn:o> .')
        dataset = self.root / "lenient"
        request = self.root / "legacy-compile.json"
        result = self.root / "legacy-result.json"
        request.write_text(json.dumps({
            "operation": "compile", "max_term_bytes": 1024**2,
            "sources": [{**source, "unchecked_turtle_terms": True}],
            "artifact": {"output": str(dataset), "profile_id": "no-rules-v1",
                         "index_profile": "core-v1", "rules_files": []},
            "memory_budget_bytes": 512 * 1024**2,
            "disk_budget_bytes": 1024**3, "timeout_seconds": 15,
        }))
        subprocess.run([BINARY, "full", "--request", request, "--result", result],
                       check=True, timeout=25)
        compiled = json.loads(result.read_bytes())
        self.assertNotIn("error", compiled)
        manifest = json.loads((dataset / "input_manifest.json").read_bytes())
        self.assertIs(manifest["parser_options"][0]["unchecked_turtle_terms"], True)
        reply = self.validate(str(dataset), """
<urn:Shape> a sh:NodeShape; sh:nodeKind sh:IRI .
""", error="EXECUTION_FAILED", focus_node="<urn:valid>")
        self.assertIn("SHACL input failed", reply["error"]["message"])


if __name__ == "__main__":
    unittest.main()
