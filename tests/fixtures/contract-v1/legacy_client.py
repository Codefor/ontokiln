"""Frozen consumer for the default ontokiln-api/v1 file protocol.

Baseline: PR #1 at fb2275c17fd9311a09b19a068c04406ad5aa45ac. This module has no
engine imports. Keep its accepted response shape and independently written
expectations unchanged when replacing an engine implementation.
"""
import hashlib
import json


CONTRACT = "ontokiln-api/v1"
ENVELOPE_FIELDS = {
    "contract", "request_id", "request_sha256", "status",
    "result", "error", "execution", "kernel",
}
QUERY_FIELDS = {
    "query_type", "variables", "items", "boolean", "view",
    "complete", "truncated", "proof_coverage",
}
ERROR_STATUS = {
    "INVALID_REQUEST": "FAILED", "INVALID_DATASET": "FAILED",
    "INVALID_QUERY": "FAILED", "EXECUTION_FAILED": "FAILED",
    "UNSUPPORTED_CONTRACT": "UNSUPPORTED", "UNSUPPORTED_CAPABILITY": "UNSUPPORTED",
    "RESOURCE_EXHAUSTED": "BUDGET_EXHAUSTED", "CANCELLED": "CANCELLED",
}


def parse_reply(response_bytes, request_bytes):
    request = json.loads(request_bytes)
    reply = json.loads(response_bytes)
    assert set(reply) == ENVELOPE_FIELDS, reply
    assert reply["contract"] == CONTRACT, reply
    assert reply["request_id"] == request["request_id"], reply
    assert reply["request_sha256"] == hashlib.sha256(request_bytes).hexdigest(), reply
    assert reply["execution"]["worker_reaped"] is True, reply
    pid = reply["execution"]["worker_pid"]
    assert pid is None or (type(pid) is int and pid > 0), reply
    assert isinstance(reply["kernel"], dict), reply
    if reply["status"] != "COMPLETE":
        assert reply["result"] is None, reply
        error = reply["error"]
        assert isinstance(error, dict), reply
        assert reply["status"] == ERROR_STATUS[error["code"]], reply
        assert isinstance(error["message"], str), reply
        # Diagnostic reason strings are not a decision API.
        return reply
    assert reply["error"] is None, reply
    result = reply["result"]
    assert isinstance(result, dict), reply
    kind = request["operation"]["kind"]
    if kind == "query":
        assert set(result) == QUERY_FIELDS, result
        assert result["view"] == request["operation"].get("view", "asserted"), result
        assert result["complete"] is True and result["truncated"] is False, result
        assert result["proof_coverage"] == {"per_fact": "not_recorded"}, result
        if result["query_type"] == "ASK":
            assert type(result["boolean"]) is bool, result
            assert result["variables"] == [] and result["items"] == [], result
        else:
            assert result["query_type"] == "SELECT", result
            assert result["boolean"] is None, result
            assert all(isinstance(name, str) for name in result["variables"]), result
            for row in result["items"]:
                assert set(row) <= set(result["variables"]), row
                for term in row.values():
                    assert term["type"] in {"uri", "bnode", "literal"}, term
                    assert isinstance(term["value"], str), term
                    if term["type"] == "literal":
                        assert ("datatype" in term) != ("xml:lang" in term), term
    elif kind == "prepare_query":
        assert result == {"valid": True}, result
    elif kind == "validate":
        assert set(result) == {"conforms", "checked_shapes", "violations", "report"}, result
        assert type(result["conforms"]) is bool, result
        assert type(result["checked_shapes"]) is int and result["checked_shapes"] >= 0, result
        assert type(result["violations"]) is int and result["violations"] >= 0, result
        assert result["report"]["conforms"] == result["conforms"], result
        assert len(result["report"]["violations"]) == result["violations"], result
    return reply


def check_expected(reply, expected):
    """Check human-authored expectations; never learn them from engine output."""
    assert reply["status"] == expected["status"], reply
    if "error_code" in expected:
        assert reply["error"]["code"] == expected["error_code"], reply
    if "result" in expected:
        assert reply["result"] == expected["result"], reply
    if "result_fields" in expected:
        for key, value in expected["result_fields"].items():
            assert reply["result"][key] == value, reply
    if "required_operations" in expected:
        assert set(expected["required_operations"]) <= set(reply["result"]["operations"]), reply
    if "report_violations" in expected:
        assert reply["result"]["report"]["violations"] == expected["report_violations"], reply
    if "violation_fields" in expected:
        violations = reply["result"]["report"]["violations"]
        assert len(violations) == len(expected["violation_fields"]), reply
        for violation, fields in zip(violations, expected["violation_fields"]):
            for key, value in fields.items():
                assert violation[key] == value, violation
            assert isinstance(violation["message"], str), violation
