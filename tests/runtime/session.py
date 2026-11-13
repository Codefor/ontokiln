"""Real supervised session processes: snapshot isolation, budgets and reaping."""
import hashlib
import json
import os
from pathlib import Path
import selectors
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest

BINARY = str(Path(sys.argv.pop(1)).resolve())
CONTRACT = "ontokiln-api/v1"


def receive(process, timeout=30):
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ)
        if not selector.select(timeout):
            raise AssertionError("session response timed out")
    raw = process.stdout.readline()
    assert raw, f"session exited early: {process.poll()}"
    return json.loads(raw)


def reaped(reply):
    assert reply["execution"]["worker_reaped"] is True, reply
    pid = reply["execution"]["worker_pid"]
    if pid is not None:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return
        raise AssertionError(f"worker {pid} survived terminal response")


class Sessions(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory(prefix="ontokiln-session-test-")
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.root = Path(cls.temporary.name)
        cls.dataset = cls.root / "compiled"
        source = cls.root / "input.nt"
        source.write_text("".join(f"<urn:item:{i}> <urn:value> \"{i}\" .\n" for i in range(8000)))
        cls.call_file({"kind": "compile", "profile": "no-rules-v1", "output": str(cls.dataset),
                       "sources": [{"path": str(source), "document_id": "input", "format": "ntriples",
                                    "expected_sha256": hashlib.sha256(source.read_bytes()).hexdigest()}]})
        cls.manifest = json.loads((cls.dataset / "manifest.json").read_text())

    @classmethod
    def call_file(cls, operation, limits=None):
        request, result = cls.root / "file.request.json", cls.root / "file.result.json"
        request.write_text(json.dumps({"contract": CONTRACT, "request_id": "file", "operation": operation,
                                       "limits": limits or {}}))
        subprocess.run([BINARY, "api", "--request", request, "--result", result], check=True, timeout=30)
        reply = json.loads(result.read_text())
        assert reply["status"] == "COMPLETE", reply
        reaped(reply)
        return reply

    def session(self, dataset=None):
        process = subprocess.Popen([BINARY, "api-session", "--dataset", dataset or self.dataset],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

        def cleanup():
            if process.stdin and not process.stdin.closed:
                process.stdin.close()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            process.stdout.close()
            process.stderr.close()

        self.addCleanup(cleanup)
        ready = receive(process)
        self.assertEqual((ready["contract"], ready["status"]), ("ontokiln-session/v1", "READY"))
        self.assertEqual(ready["content_digest"], self.manifest["content_digest"])
        self.assertEqual(ready["manifest_sha256"], hashlib.sha256((Path(dataset or self.dataset) / "manifest.json").read_bytes()).hexdigest())
        self.assertGreater(ready["mount"]["copied_bytes"], 0)
        self.assertEqual(ready["mount"]["copied_bytes"], ready["mount"]["snapshot_bytes"])
        return process, ready

    def send(self, process, operation=None, limits=None, raw=None):
        if raw is None:
            raw = json.dumps({"contract": CONTRACT, "request_id": "session", "operation": operation,
                              "limits": {"timeout_ms": 10000, **(limits or {})}}).encode()
        process.stdin.write(raw + b"\n")
        process.stdin.flush()
        reply = receive(process)
        self.assertEqual(reply["contract"], CONTRACT)
        self.assertEqual(reply["request_sha256"], hashlib.sha256(raw).hexdigest())
        reaped(reply)
        return reply

    def query(self, dataset=None, **fields):
        return {"kind": "query", "dataset": str(dataset or self.dataset),
                "query": "SELECT ?value WHERE { <urn:item:7> <urn:value> ?value }", **fields}

    def complete(self, reply):
        self.assertEqual(reply["status"], "COMPLETE", reply)
        metrics = reply["execution"]["metrics"]
        self.assertTrue(metrics["snapshot_reused"])
        self.assertEqual(metrics["snapshot_bytes_copied"], 0)

    def test_repeat_requests_preserve_file_results_and_exclude_mounted_disk(self):
        expected = self.call_file(self.query())["result"]
        process, ready = self.session()
        self.assertGreater(ready["mount"]["snapshot_bytes"], 32768)
        for _ in range(3):
            reply = self.send(process, self.query(), {"scratch_bytes": 32768})
            self.complete(reply)
            self.assertEqual(reply["result"], expected)
            self.assertLessEqual(reply["execution"]["metrics"]["peak_scratch_bytes"], 32768)
        process.stdin.close()
        self.assertEqual(process.wait(timeout=5), 0)

    def test_one_shot_uses_the_same_snapshot_without_double_charging_its_descriptors(self):
        database_bytes = (self.dataset / "facts.sqlite").stat().st_size
        allowance = database_bytes + 32768
        reply = self.call_file(self.query(), {"scratch_bytes": allowance})
        self.assertEqual(reply["result"]["items"][0]["value"]["value"], "7")
        self.assertGreaterEqual(reply["execution"]["metrics"]["peak_scratch_bytes"], database_bytes)
        self.assertLessEqual(reply["execution"]["metrics"]["peak_scratch_bytes"], allowance)
        self.assertNotIn("snapshot_reused", reply["execution"]["metrics"])

    def test_mounted_image_survives_source_corruption_and_removal(self):
        directory = self.root / "replaceable"
        shutil.copytree(self.dataset, directory)
        self.addCleanup(lambda: shutil.rmtree(directory, ignore_errors=True))
        inputs = json.loads((directory / "input_manifest.json").read_text())
        self.assertEqual([source["document_id"] for source in inputs["sources"]], ["input"])
        self.assertEqual(inputs["parser_options"],
                         [{"document_id": "input", "unchecked_turtle_terms": False}])
        process, _ = self.session(directory)
        (directory / "facts.sqlite").write_bytes(b"changed by source owner")
        for operation in [self.query(directory), {"kind": "capabilities", "dataset": str(directory)}]:
            self.complete(self.send(process, operation))
        shutil.rmtree(directory)
        reply = self.send(process, self.query(directory))
        self.complete(reply)
        self.assertEqual(reply["result"]["items"][0]["value"]["value"], "7")
        shape = self.root / "removed-source-shape.ttl"
        shape.write_text("""@prefix sh: <http://www.w3.org/ns/shacl#> .
            <urn:AfterRemoval> a sh:NodeShape; sh:targetNode <urn:item:6>;
                sh:property [ sh:path <urn:value>; sh:minCount 1; sh:maxCount 1;
                    sh:datatype <http://www.w3.org/2001/XMLSchema#string>; sh:hasValue "7" ].""")
        reply = self.send(process, {
            "kind": "validate", "dataset": str(directory), "focus_node": "<urn:item:7>",
            "shapes": [{"path": str(shape), "document_id": "removed-source-shape", "format": "turtle",
                        "expected_sha256": hashlib.sha256(shape.read_bytes()).hexdigest()}]})
        self.complete(reply)
        result = reply["result"]
        self.assertTrue(result["conforms"])
        self.assertEqual((result["checked_shapes"], result["violations"]), (1, 0))
        self.assertEqual(result["report"]["target_count"], 1)
        self.assertEqual(result["report"]["coverage"], [{
            "shape": "urn:AfterRemoval", "target_count": 1, "focus_nodes": ["<urn:item:7>"]}])
        self.assertEqual(result["report"]["violations"], [])

    def test_validation_and_derived_queries_share_the_pinned_view(self):
        shape = self.root / "shape.ttl"
        shape.write_text("""@prefix sh: <http://www.w3.org/ns/shacl#> .
            <urn:Shape> a sh:NodeShape; sh:targetNode <urn:item:7>;
                sh:property [ sh:path <urn:value>; sh:minCount 1; sh:maxCount 1 ].""")
        operation = {"kind": "validate", "dataset": str(self.dataset), "shapes": [{
            "path": str(shape), "document_id": "shapes", "format": "turtle",
            "expected_sha256": hashlib.sha256(shape.read_bytes()).hexdigest()}]}
        expected = self.call_file(operation)["result"]
        process, _ = self.session()
        reply = self.send(process, operation)
        self.complete(reply)
        self.assertEqual(reply["result"], expected)
        derived = self.query(query="SELECT ?value WHERE { <urn:derived> <urn:value> ?value }",
                             derived_queries=['CONSTRUCT { <urn:derived> <urn:value> ?v } '
                                              'WHERE { <urn:item:7> <urn:value> ?v }'])
        reply = self.send(process, derived)
        self.complete(reply)
        self.assertEqual(reply["result"]["items"][0]["value"]["value"], "7")

    def test_rejected_requests_do_not_change_or_poison_the_session(self):
        process, _ = self.session()
        cases = [
            (self.query(dataset=self.root / "other"), "INVALID_DATASET"),
            (self.query(expected_content_digest="0" * 64), "INVALID_DATASET"),
            (self.query(expected_manifest_sha256="0" * 64), "INVALID_DATASET"),
            ({"kind": "materialize", "dataset": str(self.dataset), "output": str(self.root / "forbidden")},
             "UNSUPPORTED_CAPABILITY"),
        ]
        for operation, error in cases:
            self.assertEqual(self.send(process, operation)["error"]["code"], error)
        for raw, error in [(b"bad json", "INVALID_REQUEST"),
                           (b'{"contract":"future","operation":{}}', "UNSUPPORTED_CONTRACT"),
                           (b'{"contract":"ontokiln-api/v1","request_id":"x","request_id":"y",'
                            b'"operation":{"kind":"capabilities"}}', "INVALID_REQUEST")]:
            self.assertEqual(self.send(process, raw=raw)["error"]["code"], error)
        self.assertFalse((self.root / "forbidden").exists())
        self.complete(self.send(process, self.query(
            expected_content_digest=self.manifest["content_digest"],
            expected_manifest_sha256=hashlib.sha256((self.dataset / "manifest.json").read_bytes()).hexdigest())))

    def test_timeout_reaps_worker_and_next_request_succeeds(self):
        process, _ = self.session()
        expensive = self.query(query="SELECT (COUNT(*) AS ?n) WHERE { ?a <urn:value> ?x . "
                                     "?b <urn:value> ?y . ?c <urn:value> ?z }")
        reply = self.send(process, expensive, {"timeout_ms": 5})
        self.assertEqual(reply["error"]["code"], "RESOURCE_EXHAUSTED", reply)
        self.complete(self.send(process, self.query()))

    def test_cancellation_reaps_worker_and_ends_session(self):
        process, _ = self.session()
        raw = json.dumps({"contract": CONTRACT, "request_id": "cancel", "operation": self.query(
            query="SELECT (COUNT(*) AS ?n) WHERE { ?a <urn:value> ?x . ?b <urn:value> ?y . ?c <urn:value> ?z }")}).encode()
        process.stdin.write(raw + b"\n")
        process.stdin.flush()
        deadline = time.monotonic() + 10
        worker = None
        while worker is None:
            self.assertLess(time.monotonic(), deadline)
            self.assertIsNone(process.poll())
            for row in subprocess.check_output(["ps", "-eo", "pid=,ppid=,args="], text=True).splitlines():
                fields = row.split(None, 2)
                if (len(fields) == 3 and int(fields[1]) == process.pid
                        and fields[2].startswith(BINARY + " full-worker ")):
                    worker = int(fields[0])
                    break
            time.sleep(0.01)
        process.send_signal(signal.SIGTERM)
        reply = receive(process)
        self.assertEqual(reply["error"]["code"], "CANCELLED", reply)
        self.assertEqual(reply["execution"]["worker_pid"], worker)
        reaped(reply)
        self.assertEqual(process.wait(timeout=5), 0)

    def test_native_read_retains_compatible_original_producer_identity(self):
        directory = self.root / "previous-producer"
        shutil.copytree(self.dataset, directory)
        manifest_path = directory / "manifest.json"
        manifest = json.loads(manifest_path.read_bytes())
        kernel = self.call_file({"kind": "capabilities"})["kernel"]
        manifest.update(schema_version="kernel-manifest-v2", kernel={
            **kernel, "source_revision": "1" * 40, "kernel_sha256": "2" * 64},
            proof_coverage={"mode": "closure-only", "per_fact_proof": "not_recorded"})
        manifest_path.write_text(json.dumps(manifest))
        original = manifest_path.read_bytes()
        expected = self.call_file(self.query())["result"]
        self.assertEqual(self.call_file(self.query(directory))["result"], expected)
        process, ready = self.session(directory)
        self.assertEqual(ready["manifest_sha256"], hashlib.sha256(original).hexdigest())
        reply = self.send(process, self.query(directory))
        self.complete(reply)
        self.assertEqual(reply["result"], expected)
        self.assertEqual(manifest_path.read_bytes(), original)
        process.stdin.close()
        self.assertEqual(process.wait(timeout=5), 0)
        for field, value in [("contract_version", "ontokiln-core/v4"),
                             ("source_revision", "invalid"), ("kernel_sha256", "invalid")]:
            with self.subTest(field=field):
                changed = {**manifest, "kernel": {**manifest["kernel"], field: value}}
                manifest_path.write_text(json.dumps(changed))
                rejected = subprocess.run([BINARY, "api-session", "--dataset", directory],
                                          capture_output=True, timeout=5)
                self.assertNotEqual(rejected.returncode, 0)
                self.assertEqual(rejected.stdout, b"")
        manifest_path.write_bytes(original)
        with (directory / "facts.sqlite").open("ab") as stream:
            stream.write(b"corrupt")
        rejected = subprocess.run([BINARY, "api-session", "--dataset", directory],
                                  capture_output=True, timeout=5)
        self.assertNotEqual(rejected.returncode, 0)
        self.assertEqual(rejected.stdout, b"")

    def test_mount_rejects_insufficient_disk_before_ready(self):
        result = subprocess.run([BINARY, "api-session", "--dataset", self.dataset, "--snapshot-bytes", "1"],
                                input=b"", capture_output=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, b"")
        self.assertIn(b"BUDGET_EXHAUSTED", result.stderr)


if __name__ == "__main__":
    unittest.main()
