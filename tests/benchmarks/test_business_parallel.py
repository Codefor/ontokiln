"""Prove bounded read-case concurrency with real, short-lived processes.

The fake records monotonic intervals under a file lock. It executes real child
processes, preserves the API fixture's digest checks, and never evaluates RDF
or builds Rust. Interval overlap is evidence of orchestration, not a speedup
benchmark or an independent business-answer oracle.
"""
import ctypes
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
import zipfile


ROOT = Path(__file__).resolve().parents[2]
DRIVER = ROOT / "benchmarks/business_baseline.py"
SPEC = importlib.util.spec_from_file_location("parallel_api_fixture", Path(__file__).with_name("test_business_baseline.py"))
fixtures = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fixtures)
MIB = 1024**2
LIMITS = {"memory_bytes": 32 * MIB, "scratch_bytes": 32 * MIB, "timeout_ms": 10000,
          "max_rows": 19, "max_bytes": 4096, "max_iterations": 7,
          "max_derived_facts": 23, "max_intermediate_rows": 101,
          "max_validation_depth": 4, "max_term_bytes": 4096}


PARALLEL_FAKE = r'''
import fcntl
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

root = Path(__file__).resolve().parent
if sys.argv[1] != "api":
    raise SystemExit(subprocess.call([sys.executable, str(root / "fixture_api.py"), *sys.argv[1:]]))
request_path = Path(sys.argv[sys.argv.index("--request") + 1])
result_path = Path(sys.argv[sys.argv.index("--result") + 1])
request = json.loads(request_path.read_text())
operation = request["operation"]
kind = operation["kind"]
case = kind
if kind == "query":
    case = re.search(r"# parallel-case:([a-z]+)", operation["query"]).group(1)
elif kind == "validate":
    case = "validation"
config = json.loads((root / "parallel.json").read_text())
lock_path = root / "events.lock"
state_path = root / "state.json"
events_path = root / "events.jsonl"

def event_locked(value):
    data = (json.dumps(value, separators=(",", ":")) + "\n").encode()
    descriptor = os.open(events_path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        if os.write(descriptor, data) != len(data):
            raise RuntimeError("short fixture event write")
    finally:
        os.close(descriptor)

def event(value):
    with lock_path.open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        event_locked(value)

with lock_path.open("a") as lock:
    fcntl.flock(lock, fcntl.LOCK_EX)
    state = json.loads(state_path.read_text()) if state_path.exists() else {}
    ordinal = state.get(case, 0) + 1
    state[case] = ordinal
    state_path.write_text(json.dumps(state))
    base = {"case": case, "kind": kind, "ordinal": ordinal, "pid": os.getpid(), "pgid": os.getpgrp(),
            "request_id": request["request_id"], "request_file": str(request_path),
            "response_file": str(result_path), "tmpdir": os.environ.get("TMPDIR"), "limits": request["limits"]}
    event_locked(dict(base, event="start", timestamp=time.monotonic_ns()))
if config.get("hang_reads") and kind in ("query", "validate"):
    child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
    event(dict(base, event="child_ready", timestamp=time.monotonic_ns(),
               child_pid=child.pid, child_pgid=os.getpgid(child.pid)))
    time.sleep(60)
    raise SystemExit("test should have cancelled this process group")
time.sleep(config.get("read_delay", 0.15) if kind in ("query", "validate") else 0.015)
# Only fixture response generation is serialized: the observed delay above is
# real overlapping external-process work. This protects the older fixture's
# append-only trace/ordinal implementation without masking scheduler behavior.
with (root / "fixture.lock").open("a") as lock:
    fcntl.flock(lock, fcntl.LOCK_EX)
    code = subprocess.call([sys.executable, str(root / "fixture_api.py"), *sys.argv[1:]])
if code == 0 and case == config.get("failed_case") and ordinal == config.get("failed_ordinal", 1):
    reply = json.loads(result_path.read_text())
    reply.update(status="BUDGET_EXHAUSTED", result=None,
                 error={"code": "RESOURCE_EXHAUSTED", "reason": "BUDGET_EXHAUSTED", "message": "deliberate case failure"})
    result_path.write_text(json.dumps(reply))
event(dict(base, event="end", timestamp=time.monotonic_ns(), returncode=code))
raise SystemExit(code)
'''


def peak_intervals(intervals):
    edges = [(start, 1) for start, _ in intervals] + [(end, -1) for _, end in intervals]
    current = peak = 0
    for _, change in sorted(edges, key=lambda edge: (edge[0], edge[1])):
        current += change
        peak = max(peak, current)
    return peak


class BusinessParallelTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.fake = self.directory / "fake"
        self.fake.mkdir()
        self.binary = self.fake / "ontokiln"
        self.binary.write_text("#!" + sys.executable + "\n" + PARALLEL_FAKE)
        self.binary.chmod(0o755)
        (self.fake / "fixture_api.py").write_text(fixtures.FAKE_API)
        fixtures.write_json(self.fake / "kernel.json", fixtures.KERNEL)
        fixtures.write_json(self.fake / "config.json", {"checked_shapes": 1})
        self.config = {}
        self.source = self.directory / "data.nt"
        self.source.write_text('<urn:parallel:s> <urn:parallel:p> "synthetic" .\n')
        self.compile_path = self.directory / "compile.json"
        fixtures.write_json(self.compile_path, {"contract": "ontokiln-api/v1", "request_id": "compile-template",
            "limits": dict(LIMITS), "operation": {"kind": "compile", "profile": "datalog-v1", "output": "unused",
                "sources": [{"path": str(self.source), "document_id": "synthetic", "format": "ntriples",
                             "expected_sha256": fixtures.sha256(self.source)}]}})
        self.reads = self.directory / "read-cases"
        self.reads.mkdir()
        for case in ("a", "b", "c"):
            fixtures.write_json(self.reads / (case + ".json"), {"contract": "ontokiln-api/v1", "request_id": "template-" + case,
                "limits": dict(LIMITS), "operation": {"kind": "query", "dataset": "unused", "view": "closure",
                    "query": "SELECT ?s WHERE { ?s <urn:parallel:p> ?o } # parallel-case:" + case}})
        fixtures.write_json(self.reads / "validation.json", {"contract": "ontokiln-api/v1", "request_id": "validation-template",
            "limits": dict(LIMITS), "operation": {"kind": "validate", "dataset": "unused", "shapes": [],
                                                 "selected_shapes": [], "focus_node": None}})
        self.work = self.directory / "work"

    def require_two_slots(self):
        spec = importlib.util.spec_from_file_location("parallel_capacity_for_integration", ROOT / "benchmarks/business_parallel.py")
        planner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(planner)
        cases = [(path.name, {"request": json.loads(path.read_text())}) for path in sorted(self.reads.glob("*.json"))]
        args = SimpleNamespace(jobs=2, threads=1, total_memory_mib=64, total_scratch_mib=64)
        plan = planner.plan_parallelism(args, cases, self.directory)
        if plan["effective_jobs"] < 2:
            self.skipTest("real-process overlap needs two admitted slots: " + ", ".join(plan["limiting_factors"]))

    def fresh_run(self, name):
        self.work = self.directory / ("work-" + name)
        for file in ("state.json", "events.jsonl", "trace.jsonl"):
            (self.fake / file).unlink(missing_ok=True)

    def command(self, *extra, jobs="2", build_samples=1, read_samples=2, warmups=1,
                total_memory=64, total_scratch=64):
        fixtures.write_json(self.fake / "parallel.json", self.config)
        command = [sys.executable, "-B", str(DRIVER), "--binary", str(self.binary),
                   "--compile-request", str(self.compile_path), "--read-requests", str(self.reads),
                   "--work-dir", str(self.work), "--build-samples", str(build_samples),
                   "--read-samples", str(read_samples), "--warmups", str(warmups),
                   "--total-memory-mib", str(total_memory), "--total-scratch-mib", str(total_scratch)]
        if jobs is not None:
            command.extend(["--jobs", str(jobs)])
        return command + list(extra)

    def events(self):
        path = self.fake / "events.jsonl"
        with (self.fake / "events.lock").open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_SH)
            return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def start(self, **options):
        return subprocess.Popen(self.command(**options), cwd=self.directory, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, text=True, start_new_session=True)

    def clean_processes(self, process):
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate(timeout=5)
        events = self.events()
        pids = {event["pid"] for event in events}
        pids.update(event["child_pid"] for event in events if "child_pid" in event)
        ended = {event["request_id"] for event in events if event["event"] == "end"}
        for group in {event["pgid"] for event in events if event["request_id"] not in ended}:
            try:
                os.killpg(group, signal.SIGKILL)
            except ProcessLookupError:
                pass
        for pid in pids:
            deadline = time.monotonic() + 2
            while time.monotonic() < deadline:
                try:
                    waited, _ = os.waitpid(pid, os.WNOHANG)
                except ChildProcessError:
                    break
                if waited:
                    break
                time.sleep(0.005)

    def run_driver(self, observe=False, **options):
        process = self.start(**options)
        previous_complete, observed = set(), 0
        deadline = time.monotonic() + 30
        try:
            while process.poll() is None and time.monotonic() < deadline:
                if observe:
                    paths = list(self.work.glob("run-*/summary.json"))
                    if paths:
                        report = json.loads(paths[0].read_bytes())
                        samples = [sample for stage in report.get("stages", []) for sample in stage["samples"]]
                        complete = {sample["request_id"] for sample in samples if sample["status"] == "COMPLETE"}
                        self.assertTrue(previous_complete <= complete, "a newer checkpoint lost a completed result")
                        previous_complete = complete
                        ids = [sample["request_id"] for sample in samples if "request_id" in sample]
                        self.assertEqual(len(ids), len(set(ids)))
                        observed += 1
                time.sleep(0.005)
            stdout, stderr = process.communicate(timeout=2)
            self.last_result = subprocess.CompletedProcess(process.args, process.returncode, stdout, stderr)
            self.live_checkpoints = observed
            return self.last_result
        finally:
            self.clean_processes(process)

    def report(self):
        paths = list(self.work.glob("run-*/summary.json"))
        self.assertEqual(len(paths), 1, self.last_result.stdout + self.last_result.stderr)
        return json.loads(paths[0].read_text()), paths[0].parent

    def read_stages(self, report):
        return {Path(stage["template"]["path"]).stem: stage for stage in report["stages"]
                if stage["kind"] in ("query", "validate")}

    def intervals(self, read_only=False):
        starts, ends = {}, {}
        for event in self.events():
            if read_only and event["kind"] not in ("query", "validate"):
                continue
            if event["event"] == "start":
                starts[event["request_id"]] = event
            elif event["event"] == "end":
                ends[event["request_id"]] = event
        return [(event["timestamp"], ends[key]["timestamp"]) for key, event in starts.items() if key in ends]

    def test_external_reads_overlap_within_slots_and_each_case_keeps_warmup_order(self):
        self.require_two_slots()
        result = self.run_driver(observe=True, build_samples=2)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        report, directory = self.report()
        self.assertEqual(report["parallelism"]["effective_jobs"], 2)
        self.assertEqual(report["read_execution"]["mode"], "concurrent_workload")
        self.assertEqual(report["read_execution"]["peak_active_requests"], 2)
        self.assertEqual(peak_intervals(self.intervals(read_only=True)), 2)
        self.assertGreater(self.live_checkpoints, 2)
        events = self.events()
        builds = [event for event in events if event["kind"] in ("compile", "materialize")]
        self.assertEqual([event["kind"] for event in builds if event["event"] == "start"],
                         ["compile", "compile", "materialize", "materialize"])
        self.assertEqual(peak_intervals([(event["timestamp"], next(end["timestamp"] for end in builds
            if end["event"] == "end" and end["request_id"] == event["request_id"]))
            for event in builds if event["event"] == "start"]), 1)
        read_starts = [event for event in events if event["event"] == "start" and event["kind"] in ("query", "validate")]
        self.assertGreater(min(event["timestamp"] for event in read_starts),
                           max(event["timestamp"] for event in builds if event["event"] == "end"))
        for case, stage in self.read_stages(report).items():
            starts = sorted((event for event in read_starts if event["case"] == case), key=lambda event: event["ordinal"])
            self.assertEqual([event["ordinal"] for event in starts], [1, 2, 3])
            self.assertEqual([sample["warmup"] for sample in stage["samples"]], [True, False, False])
            self.assertEqual([event["request_id"] for event in starts], [sample["request_id"] for sample in stage["samples"]])
            for left, right in zip(starts, starts[1:]):
                end = next(event for event in events if event["event"] == "end" and event["request_id"] == left["request_id"])
                self.assertGreater(right["timestamp"], end["timestamp"])
        samples = [sample for stage in report["stages"] for sample in stage["samples"]]
        for field in ("request_sequence", "request_id", "request_file", "response_file"):
            self.assertEqual(len({sample[field] for sample in samples}), len(samples), field)
        all_starts = [event for event in events if event["event"] == "start"]
        self.assertEqual(len({event["tmpdir"] for event in all_starts}), len(all_starts))
        for sample in samples:
            self.assertEqual(sample["status"], "COMPLETE")
            self.assertTrue(sample["started_at_utc"] and sample["finished_at_utc"])
            request = (directory / sample["request_file"]).read_bytes()
            response = json.loads((directory / sample["response_file"]).read_bytes())
            self.assertEqual(hashlib.sha256(request).hexdigest(), response["request_sha256"])
            self.assertEqual(response["request_id"], sample["request_id"])
        with zipfile.ZipFile(directory / "results.zip") as archive:
            shared = json.loads(archive.read("summary.json"))
        self.assertEqual(shared["parallelism"], report["parallelism"])
        self.assertEqual(shared["read_execution"], report["read_execution"])

    def test_auto_and_large_explicit_jobs_obey_resource_caps_without_changing_api_limits(self):
        self.require_two_slots()
        for jobs in (None, "8"):
            with self.subTest(jobs=jobs):
                self.fresh_run("auto" if jobs is None else jobs)
                result = self.run_driver(jobs=jobs, warmups=0, read_samples=1, total_memory=64, total_scratch=128)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                report, _ = self.report()
                self.assertEqual(report["parallelism"]["effective_jobs"], 2)
                self.assertEqual(report["parallelism"]["resource_limits"]["memory"]["slot_limit"], 2)
                self.assertEqual(peak_intervals(self.intervals(read_only=True)), 2)
                self.assertTrue(all(event["limits"] == LIMITS for event in self.events() if event["event"] == "start"))

    def test_case_failure_stops_only_that_case_remaining_samples(self):
        self.require_two_slots()
        self.config.update(failed_case="b", failed_ordinal=2)
        result = self.run_driver()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        report, _ = self.report()
        self.assertEqual(report["status"], "COMPLETE_WITH_FAILURES")
        stages = self.read_stages(report)
        self.assertEqual([sample["status"] for sample in stages["b"]["samples"]], ["COMPLETE", "API_FAILED", "SKIPPED"])
        self.assertEqual(len([event for event in self.events() if event["event"] == "start" and event["case"] == "b"]), 2)
        for case in ("a", "c", "validation"):
            self.assertEqual([sample["status"] for sample in stages[case]["samples"]], ["COMPLETE"] * 3)
        self.assertLessEqual(peak_intervals(self.intervals(read_only=True)), 2)

    def test_jobs_one_remains_serial_and_returns_the_same_read_results(self):
        self.require_two_slots()
        answers = []
        for jobs in ("1", "2"):
            self.fresh_run(jobs)
            result = self.run_driver(jobs=jobs, warmups=0, read_samples=1)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            report, directory = self.report()
            self.assertEqual(peak_intervals(self.intervals(read_only=True)), int(jobs))
            self.assertEqual(report["read_execution"]["mode"], "serial_latency" if jobs == "1" else "concurrent_workload")
            answers.append({case: [json.loads((directory / sample["response_file"]).read_text())["result"]
                                    for sample in stage["samples"]] for case, stage in self.read_stages(report).items()})
        self.assertEqual(answers[0], answers[1])

    def test_insufficient_resources_skip_reads_without_reducing_their_limits(self):
        result = self.run_driver(total_memory=16)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        report, directory = self.report()
        self.assertEqual(report["status"], "READ_ADMISSION_FAILED")
        self.assertEqual(report["parallelism"]["status"], "INSUFFICIENT_RESOURCES")
        self.assertEqual(report["parallelism"]["effective_jobs"], 0)
        self.assertFalse(any(event["kind"] in ("query", "validate") for event in self.events()))
        for stage in self.read_stages(report).values():
            self.assertEqual(stage["template"]["effective_limits"], LIMITS)
            self.assertTrue(all(sample["status"] == "SKIPPED" for sample in stage["samples"]))
        self.assertTrue((directory / "results.zip").is_file())

    def test_invalid_jobs_and_total_caps_fail_before_any_api_process(self):
        for args in (("--jobs", "0"), ("--jobs", "-1"), ("--jobs", "many"),
                     ("--total-memory-mib", "0"), ("--total-scratch-mib", "-1")):
            with self.subTest(args=args):
                result = subprocess.run(self.command(*args), cwd=self.directory, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(self.events(), [])

    @unittest.skipUnless(sys.platform.startswith("linux"), "Linux subreaping cleans up killed test grandchildren")
    def test_interrupts_kill_all_active_process_groups_and_finalize_zip(self):
        self.require_two_slots()
        libc = ctypes.CDLL(None, use_errno=True)
        previous = ctypes.c_int()
        if libc.prctl(37, ctypes.byref(previous), 0, 0, 0) or libc.prctl(36, 1, 0, 0, 0):
            self.skipTest("could not enable child subreaping for test cleanup")
        try:
            for signum in (signal.SIGINT, signal.SIGTERM):
                with self.subTest(signal=signal.Signals(signum).name):
                    self.fresh_run(signal.Signals(signum).name)
                    self.config.update(hang_reads=True)
                    process = self.start()
                    try:
                        deadline = time.monotonic() + 10
                        ready = []
                        while process.poll() is None and time.monotonic() < deadline:
                            ready = [event for event in self.events() if event["event"] == "child_ready"]
                            if len(ready) == 2:
                                break
                            time.sleep(0.005)
                        self.assertEqual(len(ready), 2, "two live API groups were not admitted")
                        self.assertEqual(len({event["pgid"] for event in ready}), 2)
                        self.assertTrue(all(event["pgid"] == event["child_pgid"] for event in ready))
                        process.send_signal(signum)
                        stdout, stderr = process.communicate(timeout=10)
                        self.last_result = subprocess.CompletedProcess(process.args, process.returncode, stdout, stderr)
                        self.assertEqual(process.returncode, 128 + signum, stdout + stderr)
                        for event in ready:
                            with self.assertRaises(ProcessLookupError, msg="runner did not reap an API child"):
                                os.kill(event["pid"], 0)
                            status = None
                            deadline = time.monotonic() + 3
                            while time.monotonic() < deadline:
                                pid, observed = os.waitpid(event["child_pid"], os.WNOHANG)
                                if pid:
                                    status = observed
                                    break
                                time.sleep(0.005)
                            self.assertIsNotNone(status, "API grandchild survived cancellation")
                            self.assertTrue(os.WIFSIGNALED(status))
                            self.assertEqual(os.WTERMSIG(status), signal.SIGKILL)
                        report, directory = self.report()
                        self.assertEqual(report["status"], "INTERRUPTED")
                        samples = [sample for stage in self.read_stages(report).values() for sample in stage["samples"]]
                        self.assertEqual(sum(sample["status"] == "INTERRUPTED" for sample in samples), 2)
                        self.assertTrue(all(sample["status"] in ("INTERRUPTED", "SKIPPED") for sample in samples))
                        with zipfile.ZipFile(directory / "results.zip") as archive:
                            self.assertEqual(json.loads(archive.read("summary.json"))["status"], "INTERRUPTED")
                    finally:
                        self.clean_processes(process)
        finally:
            libc.prctl(36, previous.value, 0, 0, 0)


if __name__ == "__main__":
    unittest.main()
