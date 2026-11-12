"""Deterministic synthetic Turtle corpus: exactly 10,000 document-scoped triples."""
import argparse
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
PREFIX = '''@prefix ex: <urn:contract:> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
'''
SCHEMA = '''ex:Leaf rdfs:subClassOf ex:Middle .
ex:Middle rdfs:subClassOf ex:Root .
ex:alias rdfs:subPropertyOf ex:link .
ex:link rdfs:subPropertyOf ex:related .
ex:CycleA rdfs:subClassOf ex:CycleB .
ex:CycleB rdfs:subClassOf ex:CycleA .
ex:marker ex:label "schema" .
_:same ex:label "schema blank" .
ex:literal ex:p "003"^^xsd:integer .
ex:literal ex:q "3"^^xsd:integer .
ex:lang ex:label "name"@en .
ex:lang ex:label "名称"@zh .
ex:a ex:edge ex:b .
ex:b ex:edge ex:c .
ex:c ex:edge ex:a .
ex:ontology rdf:type <http://www.w3.org/2002/07/owl#Ontology> .
ex:dup ex:tag "repeat" .
ex:private ex:json "{\\"type\\":\\"hidden\\"}" .
ex:empty ex:label "" .
'''


def files():
    rows = []
    for i in range(1996):
        rows.extend([
            f'ex:e{i:04d} rdf:type ex:Leaf .',
            f'ex:e{i:04d} ex:label "条目 {i:04d}"@zh .',
            f'ex:e{i:04d} ex:score "{i:04d}"^^xsd:integer .',
            f'ex:e{i:04d} ex:alias ex:bucket{i % 4} .',
            f'ex:e{i:04d} ex:json "{{\\"kind\\":\\"opaque\\",\\"n\\":{i}}}" .',
        ])
    rows.append('_:same ex:label "data blank" .')
    data = {"schema.ttl": (PREFIX + SCHEMA).encode(),
            "data.ttl": (PREFIX + '\n'.join(rows) + '\n').encode()}
    manifest = {"schema_version": "ontokiln-contract-fixture/v1", "triples": 10000,
                "entities": 1996, "files": {name: {"sha256": hashlib.sha256(content).hexdigest(),
                "triples": 19 if name == "schema.ttl" else 9981} for name, content in data.items()}}
    data["manifest.json"] = (json.dumps(manifest, indent=2, sort_keys=True) + '\n').encode()
    return data


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    for name, content in files().items():
        path = ROOT / name
        if args.check:
            assert path.read_bytes() == content, f"fixture drift: {name}"
        else:
            path.write_bytes(content)
