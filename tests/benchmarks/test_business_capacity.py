"""Resource admission boundaries without depending on the test host's limits."""
import copy
import importlib.util
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("business_capacity", ROOT / "benchmarks/business_parallel.py")
capacity = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(capacity)
MIB = 1024 ** 2
GIB = 1024 ** 3


def arguments(**overrides):
    values = {"jobs": "auto", "threads": 1, "total_memory_mib": None, "total_scratch_mib": None}
    values.update(overrides)
    return SimpleNamespace(**values)


def read_cases(count=8, memory_mib=256, scratch_mib=256):
    return [("case-" + str(index), {"request": {
        "operation": {"kind": "query" if index % 2 == 0 else "validate"},
        "limits": {"memory_bytes": memory_mib * MIB, "scratch_bytes": scratch_mib * MIB,
                   "timeout_ms": 1000, "max_rows": 17}}}) for index in range(count)]


def snapshot():
    return {"cpu": {"logical_cpus": 16, "affinity_count": 16, "quota_cores": None,
                    "cpuset_count": None},
            "memory": {"physical_bytes": 16 * GIB, "available_bytes": 8 * GIB,
                       "cgroup_headroom_bytes": None},
            "scratch": {"available_bytes": 32 * GIB, "path": "/work"}}


class ResourceProbeTests(unittest.TestCase):
    def linux_snapshot(self, files):
        with patch.object(capacity.sys, "platform", "linux"), \
                patch.object(capacity, "_read", side_effect=lambda path: files.get(str(path))), \
                patch.object(capacity.os, "cpu_count", return_value=32), \
                patch.object(capacity.os, "sched_getaffinity", return_value=set(range(2, 8)), create=True), \
                patch.object(capacity.shutil, "disk_usage", return_value=SimpleNamespace(free=32 * GIB)):
            return capacity.resource_snapshot(Path("/work"))

    def test_linux_v2_parent_quota_and_memory_headroom_limit_the_child(self):
        files = {
            "/proc/self/cgroup": "0::/tenant/workload\n",
            "/proc/self/mountinfo": "32 25 0:29 / /sys/fs/cgroup rw - cgroup2 cgroup rw\n",
            "/proc/meminfo": "MemTotal: 16777216 kB\nMemAvailable: 12582912 kB\n",
        }
        for directory, quota, cpus, maximum, current in (
                ("/sys/fs/cgroup", "max 100000", "0-31", "max", str(10 * GIB)),
                ("/sys/fs/cgroup/tenant", "250000 100000", "2-9", str(10 * GIB), str(9 * GIB)),
                ("/sys/fs/cgroup/tenant/workload", "600000 100000", "2-7", str(5 * GIB), str(GIB))):
            files.update({directory + "/cpu.max": quota, directory + "/cpuset.cpus.effective": cpus,
                          directory + "/memory.max": maximum, directory + "/memory.current": current})
        observed = self.linux_snapshot(files)
        self.assertEqual(observed["cpu"]["affinity_count"], 6)
        self.assertEqual(observed["cpu"]["quota_cores"], 2.5)
        self.assertEqual(observed["cpu"]["cpuset_count"], 6)
        self.assertEqual(observed["memory"]["available_bytes"], 12 * GIB)
        self.assertEqual(observed["memory"]["cgroup_headroom_bytes"], GIB)
        self.assertEqual({row["path"] for row in observed["cpu"]["quota_observations"]},
                         {"/sys/fs/cgroup/tenant", "/sys/fs/cgroup/tenant/workload"})
        self.assertEqual(len(observed["memory"]["cgroup_observations"]), 2)
        with patch.object(capacity, "resource_snapshot", return_value=observed):
            planned = capacity.plan_parallelism(arguments(), read_cases(), Path("/work"))
        self.assertEqual(planned["effective_jobs"], 2)
        self.assertEqual(planned["resource_limits"]["cpu"]["effective_cores"], 2.5)
        self.assertEqual(planned["resource_limits"]["memory"]["available_bytes"], GIB)

    def test_linux_v1_exhausted_parent_memory_is_known_zero_even_with_explicit_totals(self):
        files = {
            "/proc/self/cgroup": "2:cpu,cpuacct:/tenant/job\n3:memory:/tenant/job\n4:cpuset:/tenant/job\n",
            "/proc/self/mountinfo": (
                "31 25 0:29 / /sys/fs/cgroup/cpu rw - cgroup cgroup rw,cpu,cpuacct\n"
                "32 25 0:30 / /sys/fs/cgroup/memory rw - cgroup cgroup rw,memory\n"
                "33 25 0:31 / /sys/fs/cgroup/cpuset rw - cgroup cgroup rw,cpuset\n"),
            "/proc/meminfo": "MemTotal: 16777216 kB\nMemAvailable: 8388608 kB\n",
            "/sys/fs/cgroup/cpu/cpu.cfs_quota_us": "-1",
            "/sys/fs/cgroup/cpu/cpu.cfs_period_us": "100000",
            "/sys/fs/cgroup/cpu/tenant/cpu.cfs_quota_us": "300000",
            "/sys/fs/cgroup/cpu/tenant/cpu.cfs_period_us": "100000",
            "/sys/fs/cgroup/cpu/tenant/job/cpu.cfs_quota_us": "800000",
            "/sys/fs/cgroup/cpu/tenant/job/cpu.cfs_period_us": "100000",
            "/sys/fs/cgroup/cpuset/tenant/cpuset.cpus": "0-7",
            "/sys/fs/cgroup/cpuset/tenant/job/cpuset.effective_cpus": "0-3",
            "/sys/fs/cgroup/memory/tenant/memory.limit_in_bytes": str(GIB),
            "/sys/fs/cgroup/memory/tenant/memory.usage_in_bytes": str(GIB),
            "/sys/fs/cgroup/memory/tenant/job/memory.limit_in_bytes": str(2 * GIB),
            "/sys/fs/cgroup/memory/tenant/job/memory.usage_in_bytes": str(128 * MIB),
        }
        observed = self.linux_snapshot(files)
        self.assertEqual(observed["cpu"]["quota_cores"], 3)
        self.assertEqual(observed["cpu"]["cpuset_count"], 4)
        self.assertEqual(observed["memory"]["cgroup_headroom_bytes"], 0)
        with patch.object(capacity, "resource_snapshot", return_value=observed):
            planned = capacity.plan_parallelism(arguments(total_memory_mib=8192), read_cases(), Path("/work"))
        self.assertEqual(planned["status"], "INSUFFICIENT_RESOURCES")
        self.assertEqual(planned["effective_jobs"], 0)
        self.assertTrue(planned["resource_limits"]["memory"]["capacity_known"])
        self.assertEqual(planned["resource_limits"]["memory"]["admission_bytes"], 0)

    def test_macos_vm_stat_available_estimate_excludes_active_and_wired_pages(self):
        output = {
            ("sysctl", "-n", "hw.logicalcpu"): "10",
            ("sysctl", "-n", "hw.memsize"): str(24 * GIB),
            ("vm_stat",): (
                "Mach Virtual Memory Statistics: (page size of 4096 bytes)\n"
                "Pages free: 100000.\nPages active: 900000.\nPages inactive: 200000.\n"
                "Pages speculative: 50000.\nPages wired down: 800000.\nPages purgeable: 40000.\n"),
        }
        with patch.object(capacity.sys, "platform", "darwin"), \
                patch.object(capacity, "_command", side_effect=lambda command: output[tuple(command)]), \
                patch.object(capacity.os, "cpu_count", return_value=8), \
                patch.object(capacity.os, "sched_getaffinity", side_effect=OSError, create=True), \
                patch.object(capacity.shutil, "disk_usage", return_value=SimpleNamespace(free=32 * GIB)):
            observed = capacity.resource_snapshot(Path("/work"))
        self.assertEqual(observed["cpu"]["logical_cpus"], 10)
        self.assertIsNone(observed["cpu"]["affinity_count"])
        self.assertEqual(observed["memory"]["physical_bytes"], 24 * GIB)
        self.assertEqual(observed["memory"]["available_bytes"], 1_433_600_000)
        self.assertEqual(observed["memory"]["available_source"],
                         "MACOS_VM_STAT_FREE_INACTIVE_SPECULATIVE_ESTIMATE")


class AdmissionPlanTests(unittest.TestCase):
    def plan(self, observed, options=None, cases=None):
        cases = read_cases() if cases is None else cases
        before = copy.deepcopy(cases)
        with patch.object(capacity, "resource_snapshot", return_value=observed):
            planned = capacity.plan_parallelism(options or arguments(), cases, Path("/work"))
        self.assertEqual(cases, before, "admission must not lower or otherwise edit request limits")
        self.assertFalse(planned["policy"]["request_limits_modified"])
        self.assertFalse(planned["policy"]["hard_isolation"])
        return planned

    def test_affinity_and_threads_cap_jobs_even_when_total_capacity_is_large(self):
        observed = snapshot()
        observed["cpu"].update(logical_cpus=32, affinity_count=4, quota_cores=9.5, cpuset_count=8)
        planned = self.plan(observed, arguments(jobs="8", threads=2))
        self.assertEqual(planned["effective_jobs"], 2)
        self.assertEqual(planned["resource_limits"]["cpu"]["effective_cores"], 4)
        self.assertIn("CPU_CAPACITY", planned["limiting_factors"])

    def test_unknown_capacity_caps_explicit_jobs_and_totals_at_one(self):
        observed = snapshot()
        observed["cpu"] = {key: None for key in observed["cpu"]}
        observed["memory"] = {key: None for key in observed["memory"]}
        observed["scratch"]["available_bytes"] = None
        planned = self.plan(observed, arguments(jobs="8", total_memory_mib=8192, total_scratch_mib=8192))
        self.assertEqual(planned["effective_jobs"], 1)
        for resource in ("cpu", "memory", "scratch"):
            self.assertFalse(planned["resource_limits"][resource]["capacity_known"])
            self.assertIn("UNKNOWN_" + resource.upper() + "_CAPACITY", planned["limiting_factors"])

    def test_known_cgroup_zero_still_rejects_when_host_available_is_unknown(self):
        observed = snapshot()
        observed["memory"].update(available_bytes=None, cgroup_headroom_bytes=0)
        planned = self.plan(observed, arguments(jobs="1", total_memory_mib=8192))
        self.assertEqual(planned["status"], "INSUFFICIENT_RESOURCES")
        self.assertEqual(planned["effective_jobs"], 0)
        self.assertEqual(planned["resource_limits"]["memory"]["admission_bytes"], 0)

    def test_explicit_totals_limit_slots_using_maximum_request_budgets(self):
        cases = read_cases(memory_mib=64, scratch_mib=64)
        cases[0][1]["request"]["limits"]["memory_bytes"] = 256 * MIB
        cases[1][1]["request"]["limits"]["scratch_bytes"] = 128 * MIB
        planned = self.plan(snapshot(), arguments(jobs="8", total_memory_mib=512, total_scratch_mib=512), cases)
        self.assertEqual(planned["effective_jobs"], 2)
        self.assertEqual(planned["per_slot"]["memory_bytes"], 256 * MIB)
        self.assertEqual(planned["per_slot"]["scratch_bytes"], 128 * MIB)
        self.assertEqual(planned["resource_limits"]["memory"]["slot_limit"], 2)
        self.assertEqual(planned["resource_limits"]["scratch"]["slot_limit"], 4)

    def test_explicit_totals_cannot_bypass_observed_capacity_and_twenty_percent_reserve(self):
        observed = snapshot()
        observed["memory"]["available_bytes"] = 400 * MIB
        observed["scratch"]["available_bytes"] = 800 * MIB
        planned = self.plan(observed, arguments(jobs="8", total_memory_mib=8192, total_scratch_mib=8192))
        self.assertEqual(planned["effective_jobs"], 1)
        self.assertEqual(planned["resource_limits"]["memory"]["admission_bytes"], 320 * MIB)
        self.assertEqual(planned["resource_limits"]["scratch"]["admission_bytes"], 640 * MIB)

    def test_one_requested_job_is_rejected_if_its_budget_cannot_fit(self):
        observed = snapshot()
        observed["scratch"]["available_bytes"] = 300 * MIB
        planned = self.plan(observed, arguments(jobs="1"))
        self.assertEqual(planned["status"], "INSUFFICIENT_RESOURCES")
        self.assertEqual(planned["effective_jobs"], 0)
        self.assertEqual(planned["resource_limits"]["scratch"]["admission_bytes"], 240 * MIB)
        self.assertIn("SCRATCH_CAPACITY", planned["limiting_factors"])


if __name__ == "__main__":
    unittest.main()
