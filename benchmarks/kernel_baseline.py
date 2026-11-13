#!/usr/bin/env python3
"""Reproducible, serial Full/pack baseline through the public API transports.

No build is performed. See tests/benchmarks/README.md for the workload, timing
boundaries and a small smoke command. Only the Python standard library is used.
"""
import argparse
from collections import Counter
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import threading
import time


ROOT = Path(__file__).resolve().parents[1]
CONTRACT = "ontokiln-api/v1"
GENERATOR = "kernel-baseline-orders/v1"
PROFILE = "rdfs-supported-v1"
NS = "urn:ontokiln:kernel-bench:"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
XSD = "http://www.w3.org/2001/XMLSchema#"
SH = "http://www.w3.org/ns/shacl#"
CHAIN = 8
FOCUS_INDEX = 8
METRIC_NOTES = {
    "end_to_end_seconds": "Before Popen through process exit and reading the result bytes; JSON parsing, oracle checks and artifact inspection excluded.",
    "process_wall_seconds": "Before Popen through wait4 completion. Serial command invocation, including startup and supervisor/worker lifecycle.",
    "process_cpu": "wait4 command rusage user/system time; reaped-descendant accounting is OS dependent. Do not add worker CPU to these fields.",
    "process_wait4_peak_rss_bytes": "OS wait4 high-water RSS, not aggregate physical memory; Linux can include pre-exec inherited high-water state.",
    "execution_seconds": "API supervisor boundary, including setup, verified snapshots and publication; excludes outer API parsing/final response writing. Not pure kernel time.",
    "parse_dictionary_seconds": "Full compile phase, including SQLite setup, RDF parse/dictionary insertion and rule constants.",
    "index_seconds": "Full post-parse phase, including deduplication, indexes, ANALYZE/VACUUM and plan/import metadata; not index CPU alone.",
    "engine_seconds": "Full compile worker phase before artifact publication; overlaps parse_dictionary_seconds and index_seconds.",
    "reasoning_seconds": "Full materialization phase including SQLite setup, semi-naive joins, fixpoint and cleanup/ANALYZE/VACUUM.",
    "peak_rss_bytes": "API budget measurement combining worker RSS/HWM and supervisor RSS; not PSS and not directly comparable to pack compile process RSS.",
    "supervisor_peak_rss_bytes": "Supervisor process high-water RSS, recorded separately; not additive to peak_rss_bytes.",
    "peak_scratch_bytes": "API sampled private scratch logical bytes (including detected deleted open files on Linux); not total bytes written.",
    "cpu_user_seconds": "API worker self rusage, excluding supervisor CPU.",
    "cpu_system_seconds": "API worker self rusage, excluding supervisor CPU.",
    "io_read_bytes": "Worker block-I/O accounting (ru_inblock * 512), affected by page cache; not logical input bytes.",
    "io_write_bytes": "Worker block-I/O accounting (ru_oublock * 512), not logical artifact size.",
    "query_validate_core_seconds": "Unavailable in the public API. Query/SHACL are measured end to end; no inferred core time is reported.",
    "session_request_seconds": "Before writing a complete JSONL request through receipt of its response line; excludes one-time session startup/mount, JSON parsing and oracle checks. This replaces process-per-request end_to_end_seconds only when read_transport=session.",
    "session_mount_seconds": "Before starting api-session through receipt of READY; includes process startup, artifact verification and its private snapshot copy. Recorded separately, never amortized into request times.",
    "session_lifecycle_rusage": "wait4 counters after closing a session; lifetime measurements, not individual query/validation CPU or RSS. Do not add to worker measurements.",
}


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def digest_file(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def publish(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n", encoding="utf8")
    temporary.replace(path)


def iri(local):
    return f"<{NS}{local}>"


def order(i):
    return iri(f"order/{i:09d}")


def customer(i):
    return iri(f"customer/{i:09d}")


def records_for(facts):
    if facts < 100 or facts % 10:
        raise ValueError("sizes must be multiples of 10 and at least 100 explicit facts")
    return facts // 10 - 1


def schema_triples():
    return [
        (iri("Order"), f"<{RDFS}subClassOf>", iri("CommerceRecord")),
        (iri("CommerceRecord"), f"<{RDFS}subClassOf>", iri("Resource")),
        (iri("region"), f"<{RDFS}subPropertyOf>", iri("locatedIn")),
        (iri("locatedIn"), f"<{RDFS}subPropertyOf>", iri("relatedTo")),
        (iri("customer"), f"<{RDFS}domain>", iri("Order")),
        (iri("customer"), f"<{RDFS}range>", iri("Customer")),
        (iri("benchmark"), f"<{RDFS}label>", '"kernel-baseline-v1"'),
        (iri("benchmark"), iri("version"), '"1"'),
        (iri("benchmark"), iri("source"), iri("synthetic")),
        (iri("benchmark"), f"<{RDF}type>", iri("Dataset")),
    ]


def record_triples(i, n):
    # A bounded chain ends in a self-loop. Every record has exactly ten facts;
    # recursive reachability stays linear in n instead of a global O(n^2) path.
    end = min((i // CHAIN + 1) * CHAIN, n) - 1
    return [
        (order(i), f"<{RDF}type>", iri("Order")),
        (order(i), iri("customer"), customer(i)),
        (order(i), iri("amount"), f'"{i % 100:03d}"^^<{XSD}integer>'),
        (order(i), iri("status"), iri("Active" if i % 4 == 0 else "Inactive")),
        (order(i), iri("next"), order(min(i + 1, end))),
        (order(i), iri("label"), f'"Order {i}"@en'),
        (customer(i), f"<{RDF}type>", iri("Customer")),
        (customer(i), iri("region"), iri(f"region/{i % 8}")),
        (customer(i), iri("segment"), iri(f"segment/{i % 5}")),
        (customer(i), iri("code"), f'"C{i:09d}"'),
    ]


def rules():
    return {"schema_version": "ontology-rule-ir/v1", "rules": [
        {"rule_id": "bench:reach-seed", "head": ["?x", iri("reach"), "?y"],
         "body": [["?x", iri("next"), "?y"]]},
        {"rule_id": "bench:reach-step", "head": ["?x", iri("reach"), "?z"],
         "body": [["?x", iri("reach"), "?y"], ["?y", iri("next"), "?z"]]},
        {"rule_id": "bench:order-region", "head": ["?order", iri("inRegion"), "?region"],
         "body": [["?order", iri("customer"), "?customer"], ["?customer", iri("region"), "?region"]]},
    ]}


def shapes_text():
    return f'''@prefix b: <{NS}> .
@prefix sh: <{SH}> .
@prefix xsd: <{XSD}> .
@prefix rdf: <{RDF}> .
b:OrderShape a sh:NodeShape; sh:targetSubjectsOf b:customer;
    sh:property b:CustomerProperty, b:AmountProperty .
b:ClassOrderShape a sh:NodeShape; sh:targetClass b:Order;
    sh:property b:CustomerProperty, b:AmountProperty .
b:CustomerProperty sh:path b:customer; sh:minCount 1; sh:maxCount 1; sh:nodeKind sh:IRI .
b:AmountProperty sh:path b:amount; sh:minCount 1; sh:maxCount 1;
    sh:datatype xsd:integer; sh:minInclusive 0; sh:maxExclusive 100 .
b:ProbeShape a sh:NodeShape; sh:targetNode {order(0)}; sh:property b:ProbeProperty .
b:ProbeProperty sh:path rdf:type; sh:hasValue b:Resource .
b:MinCountProperty a sh:PropertyShape; sh:targetNode {order(0)};
    sh:path b:amount; sh:minCount 2 .
'''


def full_scan_expected(facts):
    """Independent COUNT/SUM oracle for the generator's distinct SPO union."""
    n = records_for(facts)
    # Each record has six order-subject facts and four customer-subject facts.
    # Indexes are padded to nine digits, including when a larger input would
    # eventually require more digits. No engine statistics or produced DB is read.
    index_chars = 9 * n
    boundary = 10**9
    while boundary < n:
        index_chars += n - boundary
        boundary *= 10
    schema_chars = sum(len(NS + subject) for subject in [
        "Order", "CommerceRecord", "region", "locatedIn", "customer", "customer",
        "benchmark", "benchmark", "benchmark", "benchmark"])
    subject_chars = schema_chars + n * (6 * len(NS + "order/") + 4 * len(NS + "customer/")) + 10 * index_chars
    return {"fact_count": 10 * n + 10, "subject_characters": subject_chars}


def multi_target_indices(facts):
    # Include different regions and a distant subject. The 100-fact smoke input
    # ends at order 8, so explicit targets must be deduplicated there.
    return sorted({7, FOCUS_INDEX, records_for(facts) - 1})


def fanout_join_expected(facts):
    """Independent bag COUNT/SUM for region 0 customers and duplicate VALUES."""
    selected = (records_for(facts) + 7) // 8
    # Amounts are index % 100. The selected indices 0, 8, 16, ... repeat their
    # amounts every 25 customers; each complete period sums to 1,200.
    periods, tail = divmod(selected, 25)
    amount_sum = 1200 * periods + sum((8 * i) % 100 for i in range(tail))
    return {"distinct_customers": selected, "row_count": 2 * selected,
            "amount_sum": 2 * amount_sum}


def focused_shapes_text(facts):
    # Keep the kernel suite's existing Shapes bytes unchanged. These additional
    # constraints separate selective paths/visibility from full-union scanning.
    aggregate = full_scan_expected(facts)
    fanout = fanout_join_expected(facts)
    targets = ", ".join(order(i) for i in multi_target_indices(facts))
    return f'''b:FocusedScopeShape a sh:NodeShape; sh:targetNode {order(0)};
    sh:property b:FocusedRegionProperty; sh:sparql b:FocusedOutsideConstraint .
b:FocusedRegionProperty sh:path (b:customer b:region); sh:hasValue {iri("region/0")} .
b:FocusedOutsideConstraint sh:select """SELECT $this WHERE {{
    ?other {iri("label")} "Order 0"@en .
}}""" .
b:FocusedPathShape a sh:NodeShape; sh:targetNode {order(0)};
    sh:property b:FocusedPathPresent, b:FocusedPathMissing .
b:FocusedPathPresent sh:path (b:customer b:region); sh:hasValue {iri("region/0")} .
b:FocusedPathMissing sh:path (b:customer b:region); sh:hasValue {iri("region/1")} .
b:FocusedScanShape a sh:NodeShape; sh:targetNode {order(0)};
    sh:sparql b:FocusedScanConstraint .
b:FocusedScanConstraint sh:select """SELECT $this WHERE {{
    {{ SELECT (COUNT(*) AS ?factCount) (SUM(STRLEN(STR(?s))) AS ?subjectChars)
       WHERE {{ ?s ?p ?o }} }}
    FILTER (?factCount = {aggregate["fact_count"]} && ?subjectChars = {aggregate["subject_characters"]})
}}""" .
b:MultiTargetPathShape a sh:NodeShape; sh:targetNode {targets};
    sh:property b:FocusedPathPresent, b:FocusedPathMissing .
b:FocusedFanoutJoinShape a sh:NodeShape; sh:targetNode {order(0)};
    sh:sparql b:FocusedFanoutJoinConstraint .
b:FocusedFanoutJoinConstraint sh:select """SELECT $this WHERE {{
    {{ SELECT (COUNT(*) AS ?matchedRows) (SUM(?amount) AS ?amountSum) WHERE {{
        ?customer {iri("region")} {iri("region/0")} .
        ?order {iri("customer")} ?customer .
        ?order {iri("amount")} ?amount .
        VALUES ?copy {{ 1 1 }}
    }} }}
    FILTER (?matchedRows = {fanout["row_count"]} && ?amountSum = {fanout["amount_sum"]})
}}""" .
'''


def focused_validation_cases(facts):
    focus = order(FOCUS_INDEX)
    multi_violations = []
    for i in multi_target_indices(facts):
        for shape, required_region in [("FocusedPathPresent", 0), ("FocusedPathMissing", 1)]:
            if i % 8 != required_region:
                multi_violations.append({"focus_node": uri(order(i))["value"],
                                         "path": f"({iri('customer')}/{iri('region')})",
                                         "source_shape": NS + shape,
                                         "source_constraint_component": SH + "HasValueConstraintComponent",
                                         "shape_ref": NS + shape})
    return [
        {"name": "focus_core_pass", "selected_shapes": [iri("OrderShape")], "focus_node": focus,
         "expected_conforms": True, "expected_violations": [], "requires_nonlocal_graph": False},
        {"name": "focus_core_fail", "selected_shapes": [iri("MinCountProperty")], "focus_node": focus,
         "expected_conforms": False, "requires_nonlocal_graph": False,
         "expected_violations": [{"focus_node": uri(focus)["value"], "path": NS + "amount",
                                  "shape_ref": NS + "MinCountProperty"}]},
        {"name": "focus_nonlocal_fail", "selected_shapes": [iri("FocusedScopeShape")], "focus_node": focus,
         "expected_conforms": False, "requires_nonlocal_graph": True,
         "expected_violations": [{"focus_node": uri(focus)["value"], "path": None,
                                  "source_shape": NS + "FocusedScopeShape",
                                  "source_constraint_component": SH + "SPARQLConstraintComponent",
                                  "shape_ref": NS + "FocusedScopeShape"}]},
        {"name": "focus_path_fail", "selected_shapes": [iri("FocusedPathShape")], "focus_node": focus,
         "expected_conforms": False, "requires_nonlocal_graph": True, "requires_full_scan": False,
         "expected_violations": [{"focus_node": uri(focus)["value"],
                                  "path": f"({iri('customer')}/{iri('region')})",
                                  "source_shape": NS + "FocusedPathMissing",
                                  "source_constraint_component": SH + "HasValueConstraintComponent",
                                  "shape_ref": NS + "FocusedPathMissing"}]},
        {"name": "focus_full_scan_fail", "selected_shapes": [iri("FocusedScanShape")], "focus_node": focus,
         "expected_conforms": False, "requires_nonlocal_graph": True, "requires_full_scan": True,
         "expected_aggregate": full_scan_expected(facts),
         "expected_violations": [{"focus_node": uri(focus)["value"], "path": None,
                                  "source_shape": NS + "FocusedScanShape",
                                  "source_constraint_component": SH + "SPARQLConstraintComponent",
                                  "shape_ref": NS + "FocusedScanShape"}]},
        {"name": "multi_target_path_fail", "selected_shapes": [iri("MultiTargetPathShape")],
         "expected_focus_nodes": sorted(order(i) for i in multi_target_indices(facts)),
         "expected_conforms": False, "requires_nonlocal_graph": True, "requires_full_scan": False,
         "expected_violations": multi_violations},
        {"name": "focus_fanout_join_fail", "selected_shapes": [iri("FocusedFanoutJoinShape")], "focus_node": focus,
         "expected_conforms": False, "requires_nonlocal_graph": True, "requires_full_scan": False,
         "expected_aggregate": fanout_join_expected(facts),
         "expected_violations": [{"focus_node": uri(focus)["value"], "path": None,
                                  "source_shape": NS + "FocusedFanoutJoinShape",
                                  "source_constraint_component": SH + "SPARQLConstraintComponent",
                                  "shape_ref": NS + "FocusedFanoutJoinShape"}]},
    ]


def reachable_indices(i, n):
    end = min((i // CHAIN + 1) * CHAIN, n) - 1
    return range(min(i + 1, end), end + 1)


def inferred_counts(n):
    groups, tail = divmod(n, CHAIN)
    reach = groups * (CHAIN * (CHAIN - 1) // 2 + 1)
    if tail:
        reach += tail * (tail - 1) // 2 + 1
    return {RDF + "type": 2 * n, RDFS + "subClassOf": 1, RDFS + "subPropertyOf": 1,
            NS + "locatedIn": n, NS + "relatedTo": n, NS + "inRegion": n, NS + "reach": reach}


def expected_terms(n):
    # Four disjoint, unique terms per record. Remaining terms repeat in <=100
    # records. This calculation never reads a produced pack/SQLite database.
    constants = {t for triple in schema_triples() for t in triple}
    for i in range(min(n, 100)):
        for triple in record_triples(i, n):
            for term in triple:
                if not (term.startswith(f"<{NS}order/") or term.startswith(f"<{NS}customer/")
                        or term.startswith('"Order ') or term.startswith('"C')):
                    constants.add(term)
    return 4 * n + len(constants)


def generate(directory, facts, suite="kernel"):
    if suite not in {"kernel", "focused-shacl"}:
        raise ValueError("unknown benchmark suite")
    n = records_for(facts)
    inputs = directory / "input"
    inputs.mkdir(parents=True)
    data = inputs / "data.nt"
    with data.open("w", encoding="utf8", newline="\n") as stream:
        for triple in schema_triples():
            stream.write(" ".join(triple) + " .\n")
        for i in range(n):
            for triple in record_triples(i, n):
                stream.write(" ".join(triple) + " .\n")
    rule_path, shape_path = directory / "rules.json", directory / "shapes.ttl"
    rule_path.write_bytes(canonical(rules()) + b"\n")
    with shape_path.open("w", encoding="utf8", newline="\n") as stream:
        stream.write(shapes_text())
        if suite == "focused-shacl":
            stream.write(focused_shapes_text(facts))
    files = [{"file": str(path.relative_to(directory)), "bytes": path.stat().st_size,
              "sha256": digest_file(path)} for path in (data, rule_path, shape_path)]
    return {"generator": GENERATOR, "explicit_facts": facts, "records": n,
            "chain_max_nodes": CHAIN, "distinct_asserted_terms": expected_terms(n),
            "expected_full_compiled_terms": expected_terms(n) + 2,
            "expected_inferred_facts": sum(inferred_counts(n).values()),
            "expected_inferred_by_predicate": inferred_counts(n), "files": files,
            "input_bytes": data.stat().st_size, "source_document_id": GENERATOR + "/data"}


def uri(value):
    return {"type": "uri", "value": value[1:-1] if value.startswith("<") else value}


def integer(value):
    return {"type": "literal", "value": str(value), "datatype": XSD + "integer"}


def query_cases(n, facts, materialized=False):
    prefix = f"PREFIX b: <{NS}>\n"
    i = n // 2
    if materialized:
        counts = inferred_counts(n)
        rows = [{"p": uri(p), "n": integer(counts[p])} for p in sorted(counts)]
        business = [{"type": uri(iri(t)), "region": uri(iri(f"region/{i % 8}")),
                     "target": uri(order(j))} for t in sorted(["Order", "CommerceRecord", "Resource"])
                    for j in reachable_indices(i, n)]
        return [
            {"name": "inferred_predicate_counts", "view": "inferred", "variables": ["p", "n"],
             "query": "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY ?p", "rows": rows},
            {"name": "closure_business_join", "view": "closure", "variables": ["type", "region", "target"],
             "query": prefix + f"SELECT ?type ?region ?target WHERE {{ {order(i)} a ?type; b:inRegion ?region; b:reach ?target }} ORDER BY ?type ?target",
             "rows": business},
        ]
    aggregate = []
    for region in range(8):
        indices = range(region, n, 8)
        if indices:
            aggregate.append({"region": uri(iri(f"region/{region}")), "n": integer(len(indices)),
                              "total": integer(sum(j % 100 for j in indices))})
    roots = sorted({0, (n - 1) // CHAIN * CHAIN})
    return [
        {"name": "point", "view": "asserted", "variables": ["customer", "amount", "label", "status"],
         "query": prefix + f"SELECT ?customer ?amount ?label ?status WHERE {{ {order(i)} b:customer ?customer; b:amount ?amount; b:label ?label; b:status ?status }}",
         "rows": [{"customer": uri(customer(i)), "amount": integer(f"{i % 100:03d}"),
                   "label": {"type": "literal", "value": f"Order {i}", "xml:lang": "en"},
                   "status": uri(iri("Active" if i % 4 == 0 else "Inactive"))}]},
        {"name": "customer_region_join", "view": "asserted", "variables": ["order", "customer", "region"],
         "query": prefix + "SELECT ?order ?customer ?region WHERE { ?order b:customer ?customer . ?customer b:region ?region; b:segment " + iri("segment/1") + " . FILTER(?region = " + iri("region/3") + ") } ORDER BY ?order LIMIT 16",
         "rows": [{"order": uri(order(j)), "customer": uri(customer(j)), "region": uri(iri("region/3"))}
                  for j in list(range(11, n, 40))[:16]]},
        {"name": "region_aggregate", "view": "asserted", "variables": ["region", "n", "total"],
         "query": prefix + "SELECT ?region (COUNT(*) AS ?n) (SUM(?amount) AS ?total) WHERE { ?order b:customer ?customer; b:amount ?amount . ?customer b:region ?region } GROUP BY ?region ORDER BY ?region",
         "rows": aggregate},
        {"name": "bounded_property_path", "view": "asserted", "variables": ["root", "target"],
         "query": prefix + "SELECT ?root ?target WHERE { VALUES ?root { " + " ".join(order(j) for j in roots) + " } ?root b:next+ ?target } ORDER BY ?root ?target",
         "rows": [{"root": uri(order(root)), "target": uri(order(j))} for root in roots for j in reachable_indices(root, n)]},
        {"name": "asserted_fact_count", "view": "asserted", "variables": ["n"],
         "query": "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }", "rows": [{"n": integer(facts)}]},
    ]


def check_query(result, case):
    if (result.get("query_type") != "SELECT" or result.get("variables") != case["variables"]
            or result.get("view") != case["view"] or result.get("boolean") is not None
            or result.get("complete") is not True or result.get("truncated") is not False):
        raise ValueError("query result envelope does not match workload")
    # Preserve RDF term type, datatype/language, lexical form, solution multiplicity
    # and explicit ORDER BY ordering; never approve a result merely by row count.
    if result.get("items") != case["rows"]:
        raise ValueError(f"wrong RDF bindings for {case['name']}: expected {case['rows'][:2]}, got {result.get('items', [])[:2]}")
    return {"checked": True, "rows": len(case["rows"]), "semantic_sha256": hashlib.sha256(canonical(result["items"])).hexdigest()}


def check_validation(result, name, n):
    all_orders = name in ["all_orders", "class_orders"]
    violation = name in ["compiled_probe", "core_min_count"]
    if (result.get("conforms") is not (not violation) or result.get("checked_shapes") != 1
            or result.get("violations") != int(violation)):
        raise ValueError("incorrect SHACL status/count")
    report = result["report"]
    expected_foci = [order(i) for i in range(n)] if all_orders else [order(0)]
    shape = NS + {"all_orders": "OrderShape", "class_orders": "ClassOrderShape",
                  "core_min_count": "MinCountProperty"}.get(name, "ProbeShape")
    expected_coverage = [{"shape": shape, "target_count": len(expected_foci), "focus_nodes": expected_foci}]
    if (report.get("conforms") is not (not violation) or report.get("coverage") != expected_coverage
            or report.get("target_count") != len(expected_foci)
            or len(report.get("violations", [])) != int(violation)):
        raise ValueError("SHACL did not check the independently expected focus set")
    if violation:
        if name == "core_min_count":
            # Native Core's public mapper exposes these three fields, but no
            # constraint-component field. Its named shape has only minCount 2.
            # Pinned rudof Object/SHACLPath Display uses bare IRIs here.
            fields = {"focus_node": NS + "order/000000000", "path": NS + "amount",
                      "shape_ref": NS + "MinCountProperty"}
        else:
            fields = {"focus_node": NS + "order/000000000", "path": f"<{RDF}type>",
                      "source_shape": NS + "ProbeProperty", "shape_ref": NS + "ProbeProperty",
                      "source_constraint_component": SH + "HasValueConstraintComponent"}
        if any(report["violations"][0].get(key) != value for key, value in fields.items()):
            raise ValueError("incorrect named-shape violation")
    return {"checked": True, "conforms": not violation, "target_count": len(expected_foci), "violations": int(violation)}


def check_focused_validation(result, case, kernel):
    expected_violations = case["expected_violations"]
    expected_foci = (case["expected_focus_nodes"] if "expected_focus_nodes" in case
                     else [case["focus_node"]])
    if (result.get("conforms") is not case["expected_conforms"]
            or type(result.get("checked_shapes")) is not int or result["checked_shapes"] != 1
            or type(result.get("violations")) is not int or result["violations"] != len(expected_violations)):
        raise ValueError("incorrect focused SHACL status/count")
    report = result["report"]
    if (set(report) != {"conforms", "violations", "coverage", "target_count", "engine", "kernel"}
            or report["kernel"] != kernel
            or report["engine"] != {"name": "ontokiln-rust-shacl", "version": kernel["version"]}):
        raise ValueError("incorrect focused SHACL report structure or identity")
    violations = report["violations"]
    if not isinstance(violations, list) or any(
            not isinstance(row, dict) or not isinstance(row.get("message"), str) for row in violations):
        raise ValueError("invalid focused SHACL violation records")
    # Diagnostic prose and build identity are not semantic answers. Compare all
    # remaining report fields, including each violation's exact field set, so a
    # missing background constraint cannot hide behind a different violation.
    semantic = {key: report[key] for key in ["conforms", "coverage", "target_count"]}
    # Engine ordering can depend on diagnostic prose. Normalize that ordering,
    # retaining duplicates and every semantic field rather than comparing sets.
    semantic["violations"] = sorted(
        ({key: value for key, value in row.items() if key != "message"} for row in violations), key=canonical)
    expected = {"conforms": case["expected_conforms"], "target_count": len(expected_foci),
                "coverage": [{"shape": uri(case["selected_shapes"][0])["value"], "target_count": len(expected_foci),
                              "focus_nodes": expected_foci}],
                "violations": sorted(expected_violations, key=canonical)}
    if (report["conforms"] is not case["expected_conforms"]
            or type(report["target_count"]) is not int or semantic != expected):
        raise ValueError("focused SHACL report differs from the independently expected focus and violations")
    return {"checked": True, "conforms": case["expected_conforms"], "target_count": len(expected_foci),
            "violations": len(expected_violations), "semantic_report_sha256": hashlib.sha256(canonical(semantic)).hexdigest()}


def summarize(samples, planned_measured=None, planned_warmup=None):
    counts = Counter(s["status"] for s in samples if not s.get("warmup"))
    warmups = Counter(s["status"] for s in samples if s.get("warmup"))
    times = [s["end_to_end_seconds"] for s in samples if not s.get("warmup") and s["status"] == "COMPLETE"]
    failed = next((s for s in samples if s["status"] not in ["COMPLETE", "SKIPPED"]), None)
    first_failure = ({key: failed[key] for key in ["status", "sample_index", "warmup", "error", "oracle_error", "stderr_tail"]
                      if key in failed} if failed else None)
    return {"planned_measured_samples": sum(counts.values()) if planned_measured is None else planned_measured,
            "planned_warmup_samples": sum(warmups.values()) if planned_warmup is None else planned_warmup,
            "attempted_measured_samples": sum(counts.values()) - counts["SKIPPED"],
            "skipped_measured_samples": counts["SKIPPED"],
            "attempted_warmup_samples": sum(warmups.values()) - warmups["SKIPPED"],
            "warmup_status_counts": dict(warmups), "measured_status_counts": dict(counts),
            "first_failure": first_failure,
            "skip_reason": next((s.get("reason") for s in samples if s["status"] == "SKIPPED"), None),
            "successful_samples": len(times),
            "median_seconds": statistics.median(times) if times else None,
            "min_seconds": min(times) if times else None, "max_seconds": max(times) if times else None}


def timed_command(command, directory, timeout, environment, response=None, cancel_event=None):
    """Wait and reap one command; optional cancellation kills its process group.

    The original callers retain their result fields. Callers supplying an Event
    also receive command_cancelled, separate from watchdog_timeout. Only the
    direct child is waited here; worker reaping requires the API's own receipt.
    """
    directory.mkdir(parents=True, exist_ok=True)
    with (directory / "stdout.log").open("wb") as stdout, (directory / "stderr.log").open("wb") as stderr:
        if cancel_event is not None and cancel_event.is_set():
            return {"end_to_end_seconds": 0.0, "process_wall_seconds": 0.0,
                    "process_user_seconds": 0.0, "process_system_seconds": 0.0,
                    "process_wait4_peak_rss_bytes": 0, "process_block_read_bytes": 0,
                    "process_block_write_bytes": 0, "returncode": None,
                    "watchdog_timeout": False, "command_cancelled": True,
                    "response_bytes": None}, None
        started = time.perf_counter()
        proc, watchdog, cancellation = None, None, None
        timed_out, command_cancelled = False, False
        finished = threading.Event()
        termination_lock = threading.Lock()

        def terminate(cancelled=False):
            nonlocal timed_out, command_cancelled
            with termination_lock:
                if finished.is_set() or timed_out or command_cancelled:
                    return
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except ProcessLookupError:
                    return
                command_cancelled = cancelled
                timed_out = not cancelled

        def watch_cancellation():
            while not finished.is_set():
                if cancel_event.is_set():
                    terminate(cancelled=True)
                    return
                finished.wait(0.02)

        try:
            proc = subprocess.Popen([str(arg) for arg in command], stdin=subprocess.DEVNULL,
                                    stdout=stdout, stderr=stderr, env=environment, start_new_session=True)
            watchdog = threading.Timer(max(0, timeout - (time.perf_counter() - started)), terminate)
            watchdog.daemon = True
            watchdog.start()
            if cancel_event is not None:
                cancellation = threading.Thread(target=watch_cancellation, daemon=True)
                cancellation.start()
            _, status, usage = os.wait4(proc.pid, 0)
            process_wall = time.perf_counter() - started
            proc.returncode = os.waitstatus_to_exitcode(status)
            with termination_lock:
                finished.set()
            watchdog.cancel()
            payload = response.read_bytes() if response is not None and response.exists() else None
            elapsed = time.perf_counter() - started
        except BaseException:
            finished.set()
            if proc is not None:
                try:
                    os.killpg(proc.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                try:
                    _, status, _ = os.wait4(proc.pid, 0)
                    proc.returncode = os.waitstatus_to_exitcode(status)
                except ChildProcessError:
                    pass
            raise
        finally:
            finished.set()
            if watchdog is not None:
                watchdog.cancel()
                if watchdog.ident is not None:
                    watchdog.join()
            if cancellation is not None and cancellation.ident is not None:
                cancellation.join()
    metrics = {"end_to_end_seconds": elapsed, "process_wall_seconds": process_wall,
               "process_user_seconds": usage.ru_utime, "process_system_seconds": usage.ru_stime,
               "process_wait4_peak_rss_bytes": usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024),
               "process_block_read_bytes": usage.ru_inblock * 512,
               "process_block_write_bytes": usage.ru_oublock * 512,
               "returncode": proc.returncode, "watchdog_timeout": timed_out,
               "response_bytes": len(payload) if payload is not None else None}
    if cancel_event is not None:
        metrics["command_cancelled"] = command_cancelled
    if timed_out or proc.returncode:
        metrics["stderr_tail"] = (directory / "stderr.log").read_bytes()[-16384:].decode("utf8", "replace")
    return metrics, payload


class JsonlSession:
    """One serial process with deadline-bounded startup, requests and shutdown.

    The watchdog kills the complete process group on a hung write/read. A
    failed transport is never transparently restarted and timed as a hot read.
    """
    def __init__(self, command, directory, timeout, environment):
        directory.mkdir(parents=True, exist_ok=True)
        self.timeout, self.buffer, self.failed, self.closed = timeout, b"", False, False
        self.log = (directory / "stderr.log").open("wb")
        self.proc = None
        self.lifecycle = None
        started = time.perf_counter()
        try:
            self.proc = subprocess.Popen([str(arg) for arg in command], stdin=subprocess.PIPE,
                                         stdout=subprocess.PIPE, stderr=self.log, env=environment,
                                         start_new_session=True, bufsize=0)
            self.mount, self.ready_bytes = self._exchange(None, started)
        except BaseException:
            self.close()
            raise

    def _line(self):
        while b"\n" not in self.buffer:
            data = self.proc.stdout.read(65536)
            if not data:
                raise EOFError("session exited before its complete JSONL response")
            self.buffer += data
        line, self.buffer = self.buffer.split(b"\n", 1)
        return line

    def _exchange(self, payload, started):
        timed_out = False

        def expire():
            nonlocal timed_out
            try:
                os.killpg(self.proc.pid, signal.SIGKILL)
                timed_out = True
            except ProcessLookupError:
                pass

        watchdog = threading.Timer(max(0, self.timeout - (time.perf_counter() - started)), expire)
        watchdog.daemon = True
        watchdog.start()
        failure, response = None, None
        try:
            if payload is not None:
                pending = memoryview(payload + b"\n")
                while pending:
                    written = self.proc.stdin.write(pending)
                    if not written:
                        raise BrokenPipeError("session stopped accepting request bytes")
                    pending = pending[written:]
                self.proc.stdin.flush()
            response = self._line()
        except (OSError, EOFError) as error:
            failure = str(error)
        finally:
            elapsed = time.perf_counter() - started
            watchdog.cancel()
            watchdog.join()
        status = "WATCHDOG_TIMEOUT" if timed_out else "PROCESS_FAILED" if failure else "RECEIVED"
        self.failed = self.failed or status != "RECEIVED"
        measured = {"end_to_end_seconds": elapsed, "watchdog_timeout": timed_out,
                    "response_bytes": len(response) if response is not None else None,
                    "status": status, "transport": "session"}
        if failure:
            measured["transport_error"] = failure
        return measured, response

    def request(self, payload):
        if self.failed or self.closed:
            return {"status": "PROCESS_FAILED", "transport": "session", "watchdog_timeout": False,
                    "transport_error": "session is unavailable after an earlier transport failure"}, None
        return self._exchange(payload, time.perf_counter())

    def close(self):
        if self.closed:
            return self.lifecycle
        self.closed = True
        if self.proc is None:
            self.log.close()
            return None
        try:
            self.proc.stdin.close()
        except OSError:
            pass
        expired = False

        def expire():
            nonlocal expired
            try:
                os.killpg(self.proc.pid, signal.SIGKILL)
                expired = True
            except ProcessLookupError:
                pass

        watchdog = threading.Timer(5, expire)
        watchdog.daemon = True
        watchdog.start()
        try:
            _, status, usage = os.wait4(self.proc.pid, 0)
            self.proc.returncode = os.waitstatus_to_exitcode(status)
            self.lifecycle = {"returncode": self.proc.returncode, "shutdown_watchdog_timeout": expired,
                              "process_user_seconds": usage.ru_utime, "process_system_seconds": usage.ru_stime,
                              "process_wait4_peak_rss_bytes": usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024),
                              "process_block_read_bytes": usage.ru_inblock * 512,
                              "process_block_write_bytes": usage.ru_oublock * 512}
        finally:
            watchdog.cancel()
            watchdog.join()
            self.proc.stdout.close()
            self.log.close()
        return self.lifecycle


def command_text(args, cwd=None, env=None):
    try:
        result = subprocess.run(args, cwd=cwd, env=env, capture_output=True, text=True, timeout=10)
        return result.stdout.strip() if result.returncode == 0 else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def read_optional(path):
    try:
        return Path(path).read_text().strip()
    except OSError:
        return None


def environment_info(binary, directory, args):
    cpuinfo = read_optional("/proc/cpuinfo") or ""
    cpu = next((line.split(":", 1)[1].strip() for line in cpuinfo.splitlines() if line.startswith("model name")), None)
    if cpu is None and sys.platform == "darwin":
        cpu = command_text(["sysctl", "-n", "machdep.cpu.brand_string"])
    cgroups = {name: read_optional("/sys/fs/cgroup/" + name) for name in
               ["cpu.max", "memory.max", "memory.swap.max", "cpuset.cpus.effective",
                "memory/memory.limit_in_bytes", "cpu/cpu.cfs_quota_us", "cpu/cpu.cfs_period_us"]}
    mount = None
    if sys.platform.startswith("linux"):
        for line in (read_optional("/proc/self/mountinfo") or "").splitlines():
            left, _, right = line.partition(" - ")
            fields, fs = left.split(), right.split()
            if len(fields) > 4 and fs and (str(directory) == fields[4] or str(directory).startswith(fields[4].rstrip("/") + "/")):
                if mount is None or len(fields[4]) > len(mount["mountpoint"]):
                    mount = {"mountpoint": fields[4], "filesystem": fs[0]}
    status = command_text(["git", "status", "--porcelain"], ROOT)
    return {"platform": platform.platform(), "machine": platform.machine(), "python": platform.python_version(),
            "cpu_model": cpu, "logical_cpus": os.cpu_count(),
            "cpu_affinity": sorted(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else None,
            "physical_memory_bytes": os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE"),
            "meminfo": read_optional("/proc/meminfo"), "cgroup_limits_raw": cgroups,
            "load_average": list(os.getloadavg()), "filesystem": mount,
            "disk_free_bytes": shutil.disk_usage(directory).free,
            "rustc_on_path": command_text(["rustc", "--version", "--verbose"], ROOT),
            "repository_head": command_text(["git", "rev-parse", "HEAD"], ROOT),
            "git_status_porcelain": status, "repository_dirty": bool(status),
            "binary": str(binary), "binary_sha256": digest_file(binary),
            "kernel_identity": json.loads(command_text([str(binary), "identity"]) or "null"),
            "full_runtime_info": json.loads(command_text([str(binary), "full-info"]) or "null"),
            "build_features_caller_declaration": args.build_features,
            "build_profile_caller_declaration": args.build_profile,
            "provenance_files": {str(p.relative_to(ROOT)): digest_file(p) for p in
                                 [Path(__file__), ROOT / "Cargo.lock", ROOT / "rust-toolchain.toml"]},
            "relevant_environment": {key: value for key, value in os.environ.items()
                                     if key in ["ONTOKILN_MAX_PACK_MIB", "ONTOKILN_BUILD_REVISION",
                                                "RAYON_NUM_THREADS", "TOKIO_WORKER_THREADS", "TMPDIR"]}}


def artifact_files(path):
    files = [path] if path.is_file() else sorted(p for p in path.rglob("*") if p.is_file())
    rows = [{"file": p.name if path.is_file() else str(p.relative_to(path)), "bytes": p.stat().st_size,
             "allocated_bytes": p.stat().st_blocks * 512} for p in files]
    return {"logical_bytes": sum(p["bytes"] for p in rows),
            "allocated_bytes": sum(p["allocated_bytes"] for p in rows), "files": rows}


def snapshot_guards(artifact, mode):
    """Keep binding strength explicit; never retry with a weaker precondition."""
    guards = {"expected_content_digest": artifact["content_digest"]}
    if mode == "manifest":
        guards["expected_manifest_sha256"] = artifact["manifest_sha256"]
    elif mode != "content":
        raise ValueError("unknown snapshot guard mode")
    return guards


class Baseline:
    def __init__(self, args, directory, report):
        self.args, self.directory, self.report = args, directory, report
        self.counter = 0
        self.sessions = {}
        self.session_required = set()
        self.environment = os.environ.copy()
        scratch = directory / "scratch"
        scratch.mkdir()
        self.environment.update(ONTOKILN_MAX_PACK_MIB=str(args.pack_max_mib),
                                RAYON_NUM_THREADS=str(args.threads), TOKIO_WORKER_THREADS="1", TMPDIR=str(scratch))
        self.limits = {"memory_bytes": args.memory_mib * 1024**2, "scratch_bytes": args.scratch_mib * 1024**2,
                       "timeout_ms": args.timeout_seconds * 1000, "max_rows": 100_000, "max_bytes": 4 * 1024**2,
                       "max_iterations": args.max_iterations, "max_derived_facts": args.max_derived_facts,
                       "max_intermediate_rows": args.max_intermediate_rows,
                       "max_validation_depth": 64, "max_term_bytes": 1024**2}
        report["limits"] = self.limits
        report["process_environment_overrides"] = {key: self.environment[key] for key in
                                                    ["ONTOKILN_MAX_PACK_MIB", "RAYON_NUM_THREADS", "TOKIO_WORKER_THREADS"]}
        report["process_environment_overrides"]["TMPDIR"] = "<work_directory>/scratch"
        report["scratch_policy"] = "All child TMPDIR paths are inside the isolated work_directory/scratch, on the selected work filesystem."

    def checkpoint(self):
        self.report["updated_at_utc"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        publish(self.args.output, self.report)

    def api(self, operation, oracle):
        self.counter += 1
        directory = self.directory / "requests" / str(self.counter)
        directory.mkdir(parents=True)
        request = {"contract": CONTRACT, "request_id": f"baseline-{self.counter}",
                   "operation": operation, "limits": self.limits}
        request_path, response = directory / "request.json", directory / "response.json"
        request_bytes = canonical(request)
        request_path.write_bytes(request_bytes)
        session = getattr(self, "sessions", {}).get(operation.get("dataset"))
        if (operation.get("dataset") in getattr(self, "session_required", set())
                and session is None and operation["kind"] in {"query", "validate"}):
            return {"status": "PROCESS_FAILED", "transport": "session", "watchdog_timeout": False,
                    "transport_error": "required session failed to start; file API fallback is disabled"}
        if session is not None and operation["kind"] in {"query", "validate"}:
            sample, payload = session[0].request(request_bytes)
            if payload is not None:
                response.write_bytes(payload)
        else:
            sample, payload = timed_command([self.args.binary, "api", "--request", request_path, "--result", response],
                                            directory, self.args.timeout_seconds + 5, self.environment, response)
            sample.update(status="PROCESS_FAILED", transport="file")
        sample.update(request_sha256=hashlib.sha256(request_bytes).hexdigest())
        if sample["watchdog_timeout"]:
            sample["status"] = "WATCHDOG_TIMEOUT"
        if sample["watchdog_timeout"] or sample.get("returncode", 0) or payload is None:
            return sample
        try:
            reply = json.loads(payload)
            sample["response_sha256"] = hashlib.sha256(payload).hexdigest()
            sample["status"] = reply["status"]
            sample["api_metrics"] = reply.get("execution", {}).get("metrics", {})
            if (reply.get("contract") != CONTRACT or reply.get("request_id") != request["request_id"]
                    or reply.get("request_sha256") != sample["request_sha256"]
                    or reply.get("execution", {}).get("worker_reaped") is not True
                    or reply.get("kernel") != self.report["environment"]["kernel_identity"]):
                raise ValueError("invalid API identity/envelope or unreaped worker")
            if reply["status"] == "COMPLETE":
                if reply.get("error") is not None:
                    raise ValueError("successful API response contains an error")
                if session is not None and operation["kind"] in {"query", "validate"}:
                    if (sample["api_metrics"].get("snapshot_reused") is not True
                            or sample["api_metrics"].get("snapshot_bytes_copied") != 0):
                        raise ValueError("mounted read did not report reuse with zero snapshot copies")
                sample["oracle"] = oracle(reply["result"])
            else:
                sample["error"] = reply.get("error")
        except (ValueError, KeyError, TypeError) as error:
            sample["status"] = "ORACLE_FAILED"
            sample["oracle_error"] = str(error)
        return sample

    def mount(self, scale, backend, dataset, guard):
        directory = self.directory / "sessions" / f"{scale['facts']}-{backend}"
        self.session_required.add(str(dataset))
        expected_manifest = digest_file(dataset / "manifest.json")

        def invoke(_):
            session = JsonlSession([self.args.binary, "api-session", "--dataset", dataset,
                                    "--snapshot-bytes", self.limits["scratch_bytes"]],
                                   directory, self.args.timeout_seconds + 5, self.environment)
            measured = session.mount
            self.sessions[str(dataset)] = (session, measured)
            if measured["status"] != "RECEIVED":
                return measured
            try:
                ready = json.loads(session.ready_bytes)
                measured["ready"] = ready
                measured["status"] = "ORACLE_FAILED"
                if (ready.get("contract") != "ontokiln-session/v1" or ready.get("status") != "READY"
                        or ready.get("dataset") != str(dataset.resolve())
                        or ready.get("content_digest") != guard["expected_content_digest"]
                        or ready.get("manifest_sha256") != expected_manifest
                        or ready.get("kernel") != self.report["environment"]["kernel_identity"]):
                    raise ValueError("session READY does not bind the expected dataset, artifact and kernel")
                mount = ready["mount"]
                if any(not isinstance(mount.get(key), (int, float)) or isinstance(mount[key], bool)
                       or not math.isfinite(mount[key]) or mount[key] < 0
                       for key in ["elapsed_seconds", "snapshot_bytes", "copied_bytes"]):
                    raise ValueError("session READY lacks finite mount timing/byte measurements")
                measured.update(status="COMPLETE", oracle={"checked": True}, mount_metrics=mount)
            except (ValueError, KeyError, TypeError) as error:
                measured["status"], measured["oracle_error"] = "ORACLE_FAILED", str(error)
                session.failed = True
            return measured

        self.series(scale, backend + "/session_mount", 1, invoke,
                    metadata={"transport": "session", "one_time_mount": True,
                              "statistics": "single diagnostic observation; not a latency distribution"})

    def close_sessions(self):
        for session, measured in self.sessions.values():
            measured["session_lifecycle_rusage"] = session.close()
            lifecycle = measured["session_lifecycle_rusage"]
            if (lifecycle is not None and (lifecycle["returncode"] != 0 or lifecycle["shutdown_watchdog_timeout"])
                    and measured["status"] == "COMPLETE"):
                measured.update(status="SESSION_EXIT_FAILED", error="session did not shut down cleanly")
        self.sessions.clear()
        self.session_required.clear()

    def series(self, scale, name, count, invoke, warmup=False, dependency=True, metadata=None):
        stage = {"name": name, "metadata": metadata or {}, "samples": []}
        scale["stages"].append(stage)
        blocked = None if dependency else "required artifact did not pass its oracle"
        for index in range(count + int(warmup)):
            is_warmup = warmup and index == 0
            if blocked:
                sample = {"status": "SKIPPED", "reason": blocked}
            else:
                try:
                    sample = invoke(index)
                except Exception as error:
                    sample = {"status": "HARNESS_FAILED", "error": {"type": type(error).__name__, "message": str(error)}}
                if sample["status"] != "COMPLETE":
                    blocked = "earlier repetition ended with " + sample["status"]
            sample.update(sample_index=index, warmup=is_warmup)
            stage["samples"].append(sample)
            stage["summary"] = summarize(stage["samples"], count, int(warmup))
            self.checkpoint()
            print(f"{scale['facts']} {name} {'warmup' if is_warmup else index}: {sample['status']}", flush=True)
        return stage

    def scale(self, facts):
        directory = self.directory / f"facts-{facts}"
        kernel_suite = self.args.suite == "kernel"
        manifest = generate(directory, facts, self.args.suite)
        n = manifest["records"]
        scale = {"facts": facts, "input": manifest, "stages": []}
        self.report["scales"].append(scale)
        self.checkpoint()
        data, rule_path, shapes = directory / "input/data.nt", directory / "rules.json", directory / "shapes.ttl"
        source = {"path": str(data), "document_id": manifest["source_document_id"],
                  "format": "ntriples", "expected_sha256": manifest["files"][0]["sha256"]}
        selected, guards = {}, {}

        def pack_compile(index):
            output = directory / f"pack-{index}.pack"
            sample, _ = timed_command([self.args.binary, "compile", "--ttl", directory / "input", "--out", output],
                                      directory / f"pack-command-{index}", self.args.timeout_seconds, self.environment)
            sample["status"] = "WATCHDOG_TIMEOUT" if sample["watchdog_timeout"] else "PROCESS_FAILED"
            if sample["returncode"] == 0 and not sample["watchdog_timeout"]:
                identity = json.loads(command_text([str(self.args.binary), "inspect", "--pack", str(output)], env=self.environment) or "null")
                if identity is None or identity.get("triples") != facts or identity.get("terms") != manifest["distinct_asserted_terms"]:
                    sample.update(status="ORACLE_FAILED", oracle_error="incorrect pack fact/term count", observed_identity=identity)
                else:
                    sample.update(status="COMPLETE", oracle={"checked": True}, artifact=artifact_files(output), identity=identity)
                    selected.setdefault("pack", output)
            if selected.get("pack") != output and output.exists():
                output.unlink()
            return sample

        def full_build(index, materialize=False):
            kind = "materialize" if materialize else "compile"
            output = directory / f"{kind}-{index}"
            if materialize:
                operation = {"kind": kind, "dataset": str(selected["compiled"]), "output": str(output)}
            else:
                operation = {"kind": kind, "sources": [source], "output": str(output), "profile": PROFILE,
                             "rules": [{"path": str(rule_path), "expected_sha256": manifest["files"][1]["sha256"]}]}

            def check(result):
                expected = {"explicit_facts": facts, "inferred_facts": manifest["expected_inferred_facts"] if materialize else 0}
                if result.get("stats") != expected or result.get("fixpoint_reached") is not materialize:
                    raise ValueError("incorrect Full fact counts/fixpoint")
                artifact = json.loads((output / "manifest.json").read_bytes())
                if artifact["stats"]["indexed_facts"] != facts + expected["inferred_facts"]:
                    raise ValueError("incorrect indexed fact count")
                if not materialize and artifact["stats"]["parsed_facts"] != facts:
                    raise ValueError("incorrect parsed fact count")
                if not materialize and artifact["stats"]["distinct_terms"] != manifest["expected_full_compiled_terms"]:
                    raise ValueError("incorrect compiled term count")
                return {"checked": True, "expected_stats": expected, "artifact": result["artifact"],
                        "manifest_stats": artifact["stats"], "files": artifact_files(output)}

            sample = self.api(operation, check)
            if sample["status"] == "COMPLETE":
                key = "reasoned" if materialize else "compiled"
                if key not in selected:
                    selected[key] = output
                    artifact = sample["oracle"]["artifact"]
                    guards[key] = snapshot_guards(artifact, self.args.snapshot_guard)
            if output != selected.get("reasoned" if materialize else "compiled") and output.exists():
                shutil.rmtree(output)
            return sample

        if kernel_suite:
            self.series(scale, "pack_compile", self.args.build_samples, pack_compile,
                        metadata={"entrypoint": "legacy compile CLI", "memory_scratch_budget": "not enforced by this CLI; measured only", "timeout": "external process-group watchdog"})
        self.series(scale, "full_compile", self.args.build_samples, full_build,
                    metadata={"profile": PROFILE, "explicit_rule_count": len(rules()["rules"]), "phase_metrics": True})
        if kernel_suite:
            self.series(scale, "full_materialize", self.args.build_samples, lambda i: full_build(i, True),
                        dependency="compiled" in selected, metadata={"profile": PROFILE, "phase_metrics": True})
        if self.args.read_transport == "session":
            for backend in ["compiled", "reasoned"]:
                if backend in selected:
                    self.mount(scale, backend, selected[backend], guards[backend])
        if kernel_suite:
            for backend in ["pack", "compiled", "reasoned"]:
                cases = query_cases(n, facts, materialized=backend == "reasoned")
                for case in cases:
                    def query(index, case=case, backend=backend):
                        return self.api({"kind": "query", "dataset": str(selected[backend]),
                                         "query": case["query"], "view": case["view"],
                                         **guards.get(backend, {})}, lambda r: check_query(r, case))
                    self.series(scale, backend + "/" + case["name"], self.args.query_samples, query,
                                warmup=True, dependency=backend in selected,
                                metadata={**case, "query_sha256": hashlib.sha256(case["query"].encode()).hexdigest(),
                                          "core_time_available": False})
            for name, backend in [("all_orders", "compiled"), ("class_orders", "compiled"), ("core_min_count", "compiled"),
                                  ("compiled_probe", "compiled"), ("reasoned_probe", "reasoned")]:
                def validate(index, name=name, backend=backend):
                    shape = {"all_orders": "OrderShape", "class_orders": "ClassOrderShape",
                             "core_min_count": "MinCountProperty"}.get(name, "ProbeShape")
                    return self.api({"kind": "validate", "dataset": str(selected[backend]),
                                     **guards[backend],
                                     "shapes": [{"path": str(shapes), "document_id": GENERATOR + "/shapes", "format": "turtle",
                                                 "expected_sha256": manifest["files"][2]["sha256"]}],
                                     "selected_shapes": [iri(shape)]}, lambda r: check_validation(r, name, n))
                self.series(scale, "shacl/" + name, self.args.validation_samples, validate, warmup=True,
                            dependency=backend in selected, metadata={"core_time_available": False,
                                                                     "expected_conforms": name not in ["compiled_probe", "core_min_count"],
                                                                     "expected_constraint_component": SH + "MinCountConstraintComponent" if name == "core_min_count" else None})
        else:
            for case in focused_validation_cases(facts):
                def validate(index, case=case):
                    return self.api({"kind": "validate", "dataset": str(selected["compiled"]),
                                     **guards["compiled"],
                                     "shapes": [{"path": str(shapes), "document_id": GENERATOR + "/shapes", "format": "turtle",
                                                 "expected_sha256": manifest["files"][2]["sha256"]}],
                                     "selected_shapes": case["selected_shapes"],
                                     **({"focus_node": case["focus_node"]} if "focus_node" in case else {})},
                                    lambda r: check_focused_validation(r, case, self.report["environment"]["kernel_identity"]))
                self.series(scale, "shacl/" + case["name"], self.args.validation_samples, validate, warmup=True,
                            dependency="compiled" in selected, metadata={**case, "core_time_available": False})
        self.close_sessions()
        # Session exit is part of successful mounting/lifetime management.
        for stage in scale["stages"]:
            if stage["metadata"].get("one_time_mount"):
                stage["summary"] = summarize(stage["samples"], 1, 0)
        scale["completed"] = True
        self.checkpoint()


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/ontokiln")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--suite", choices=["kernel", "focused-shacl"], default="kernel",
                        help="kernel runs the existing pack/Full workload; focused-shacl only compiles Full and validates seven focused, multi-target and fanout cases")
    parser.add_argument("--sizes", type=int, nargs="+", default=[10_000, 100_000, 1_000_000])
    parser.add_argument("--build-samples", type=int, default=3)
    parser.add_argument("--query-samples", type=int, default=9)
    parser.add_argument("--validation-samples", type=int, default=3)
    parser.add_argument("--memory-mib", type=int, default=512)
    parser.add_argument("--scratch-mib", type=int, default=4096)
    parser.add_argument("--timeout-seconds", type=float, default=60)
    parser.add_argument("--max-derived-facts", type=int, default=1_000_000)
    parser.add_argument("--max-intermediate-rows", type=int, default=10_000_000)
    parser.add_argument("--max-iterations", type=int, default=100)
    parser.add_argument("--pack-max-mib", type=int, default=256)
    parser.add_argument("--threads", type=int, default=1, help="Rayon threads; requests are always serial")
    parser.add_argument("--build-features", default="unspecified", help="caller-supplied build feature declaration; binary identity is also recorded")
    parser.add_argument("--build-profile", default="unspecified", help="caller-supplied build profile declaration")
    parser.add_argument("--work-dir", type=Path, help="parent for a new, isolated working directory")
    parser.add_argument("--keep-work-dir", action="store_true")
    parser.add_argument("--read-transport", choices=["file", "session"], default="file",
                        help="Full query/validate transport; compilation, materialization and pack reads always use separate processes")
    parser.add_argument("--snapshot-guard", choices=["content", "manifest"], default="content",
                        help="content binds artifact files and supports older API v1; manifest adds exact manifest/state binding and requires that capability (no fallback)")
    args = parser.parse_args(argv)
    for name in ["build_samples", "query_samples", "validation_samples", "memory_mib", "scratch_mib", "threads", "pack_max_mib"]:
        if getattr(args, name) <= 0:
            parser.error(name.replace("_", "-") + " must be positive")
    if not math.isfinite(args.timeout_seconds) or args.timeout_seconds <= 0:
        parser.error("timeout-seconds must be positive and finite")
    if any(getattr(args, name) < 0 for name in ["max_derived_facts", "max_intermediate_rows", "max_iterations"]):
        parser.error("rule limits must be nonnegative")
    try:
        for facts in args.sizes:
            records_for(facts)
    except ValueError as error:
        parser.error(str(error))
    if len(set(args.sizes)) != len(args.sizes):
        parser.error("sizes must not contain duplicates")
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    if not args.binary.is_file():
        parser.error("binary does not exist; build it separately before benchmarking")
    return args


def main(argv=None):
    args = parse_args(argv)
    if args.work_dir:
        args.work_dir.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix="ontokiln-kernel-baseline-", dir=args.work_dir)).resolve()
    report = {"schema_version": "ontokiln-kernel-baseline/v1", "status": "RUNNING",
              "started_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "command": sys.argv, "configuration": {key: str(value) if isinstance(value, Path) else value for key, value in vars(args).items()},
              "work_directory": str(directory), "metric_definitions": METRIC_NOTES,
              "cache_policy": ("No OS cache flush. Generation, hashing and compilation precede queries. "
                               + ("Full reads share one mounted private snapshot per artifact; each read still has an isolated worker. One-time mounts are separate diagnostic stages. " if args.read_transport == "session" else "Each request starts fresh processes; subsequent requests are not a persistent hot engine. ")
                               + "Each read workload has one warm-up."
                               + (" Pack reads always start fresh processes." if args.suite == "kernel" else " Focused SHACL includes fixed-focus, multiple explicit targets and growing-join controls.")),
              "concurrency": 1, "statistics": "Raw samples, median and range of successful measured requests; warm-ups and failures excluded. No p95/p99 estimate from small sample counts.",
              "scope": ("Synthetic asserted default-graph workload, RDFS supported profile plus bounded positive recursion; not complete OWL, production data or a cold-disk/HTTP throughput benchmark."
                        if args.suite == "kernel" else
                        "Synthetic SHACL over compiled asserted data: fixed-focus Core/path/nonlocal work, multiple explicit targets, a growing fanout join with duplicate VALUES, and full-union aggregation. No materialization or pack/query stages. Not complete SHACL, production capacity or a cold-disk benchmark."),
              "environment": environment_info(args.binary, directory, args), "scales": []}
    runner = Baseline(args, directory, report)
    runner.checkpoint()
    code = 0
    try:
        for facts in args.sizes:
            runner.scale(facts)
        failed = any(sample["status"] != "COMPLETE" for scale in report["scales"] for stage in scale["stages"] for sample in stage["samples"])
        report["status"] = "COMPLETE_WITH_FAILURES" if failed else "COMPLETE"
        code = 1 if failed else 0
    except KeyboardInterrupt:
        report["status"], code = "INTERRUPTED", 130
    except Exception as error:
        report.update(status="HARNESS_FAILED", fatal_error={"type": type(error).__name__, "message": str(error)})
        code = 1
    finally:
        runner.close_sessions()
        report["finished_at_utc"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        report["load_average_after"] = list(os.getloadavg())
        report["work_directory_retained"] = args.keep_work_dir
        if not args.keep_work_dir:
            shutil.rmtree(directory)
        runner.checkpoint()
    print(f"{report['status']}: {args.output}", flush=True)
    return code


if __name__ == "__main__":
    sys.exit(main())
