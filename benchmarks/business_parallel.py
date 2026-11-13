"""Conservative admission planning for independent API read requests.

This is a snapshot of resource availability, not an OS reservation or hard
isolation mechanism. It never changes the individual API request budgets.
"""
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys


MIB = 1024 ** 2
RESERVE_FRACTION = 0.20


def _read(path):
    try:
        return Path(path).read_text().strip()
    except (OSError, UnicodeError):
        return None


def _integer(value):
    try:
        number = int(value)
        return number if number >= 0 else None
    except (TypeError, ValueError):
        return None


def _command(arguments):
    try:
        process = subprocess.run(arguments, capture_output=True, text=True, timeout=2)
        return process.stdout.strip() if process.returncode == 0 else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def _cpuset_count(value):
    if not value:
        return None
    intervals = []
    try:
        for part in value.split(","):
            ends = part.split("-")
            first, last = int(ends[0]), int(ends[-1])
            if len(ends) > 2 or first < 0 or last < first:
                return None
            intervals.append((first, last))
    except ValueError:
        return None
    total, last_end = 0, -1
    for first, last in sorted(intervals):
        total += max(0, last - max(first, last_end + 1) + 1)
        last_end = max(last_end, last)
    return total or None


def _mount_path(text):
    return Path(re.sub(r"\\([0-7]{3})", lambda match: chr(int(match[1], 8)), text))


def _cgroup_directories():
    """Find this process's v1/v2 cgroups and all visible limiting ancestors."""
    memberships = []
    for line in (_read("/proc/self/cgroup") or "").splitlines():
        parts = line.split(":", 2)
        if len(parts) == 3:
            memberships.append((set(filter(None, parts[1].split(","))), Path(parts[2])))
    found = {}
    for line in (_read("/proc/self/mountinfo") or "").splitlines():
        left, separator, right = line.partition(" - ")
        fields, filesystem = left.split(), right.split()
        if not separator or len(fields) < 5 or len(filesystem) < 3 or filesystem[0] not in {"cgroup", "cgroup2"}:
            continue
        kind = "v2" if filesystem[0] == "cgroup2" else "v1"
        root, mount = _mount_path(fields[3]), _mount_path(fields[4])
        controllers = set(filesystem[2].split(","))
        for assigned, membership in memberships:
            if (kind == "v2" and assigned) or (kind == "v1" and not controllers.intersection(assigned)):
                continue
            try:
                relative = membership.relative_to(root)
            except ValueError:
                # A cgroup namespace may expose its mount root as '/'.
                if membership != Path("/"):
                    continue
                relative = Path(".")
            current = mount / relative
            while True:
                found[(kind, str(current))] = {"kind": kind, "path": current,
                                               "controllers": assigned if kind == "v1" else {"cpu", "cpuset", "memory"}}
                if current == mount:
                    break
                if mount not in current.parents:
                    break
                current = current.parent
    if not found:
        root = Path("/sys/fs/cgroup")
        if (root / "cgroup.controllers").exists():
            found[("v2", str(root))] = {"kind": "v2", "path": root, "controllers": {"cpu", "cpuset", "memory"}}
        else:
            for controller in ("cpu", "cpuset", "memory"):
                path = root / controller
                if path.is_dir():
                    found[("v1", str(path))] = {"kind": "v1", "path": path, "controllers": {controller}}
    return list(found.values())


def _linux_cgroups():
    quotas, cpusets, memory = [], [], []
    for entry in _cgroup_directories():
        path, kind, controllers = entry["path"], entry["kind"], entry["controllers"]
        if "cpu" in controllers:
            if kind == "v2":
                words = (_read(path / "cpu.max") or "").split()
                quota = _integer(words[0]) if len(words) == 2 else None
                period = _integer(words[1]) if len(words) == 2 else None
            else:
                quota, period = _integer(_read(path / "cpu.cfs_quota_us")), _integer(_read(path / "cpu.cfs_period_us"))
            if quota is not None and period and quota > 0:
                quotas.append({"path": str(path), "kind": kind, "quota_us": quota,
                               "period_us": period, "cores": quota / period})
        if "cpuset" in controllers:
            file = "cpuset.cpus.effective" if kind == "v2" else "cpuset.effective_cpus"
            raw = _read(path / file) or _read(path / "cpuset.cpus")
            count = _cpuset_count(raw)
            if count:
                cpusets.append({"path": str(path), "kind": kind, "cpus": raw, "count": count})
        if "memory" in controllers:
            limit_file, current_file = (("memory.max", "memory.current") if kind == "v2"
                                        else ("memory.limit_in_bytes", "memory.usage_in_bytes"))
            limit, current = _integer(_read(path / limit_file)), _integer(_read(path / current_file))
            if limit is not None and current is not None:
                memory.append({"path": str(path), "kind": kind, "limit_bytes": limit,
                               "current_bytes": current, "headroom_bytes": max(0, limit - current)})
    return {"quota_cores": min((row["cores"] for row in quotas), default=None),
            "quota_observations": quotas,
            "cpuset_count": min((row["count"] for row in cpusets), default=None),
            "cpuset_observations": cpusets,
            "memory_headroom_bytes": min((row["headroom_bytes"] for row in memory), default=None),
            "memory_observations": memory}


def resource_snapshot(run_directory):
    """Read host/process limits without changing affinity, cgroups or budgets."""
    cpu = {"logical_cpus": os.cpu_count(), "affinity_count": None, "affinity_cpus": None,
           "quota_cores": None, "cpuset_count": None, "quota_observations": [], "cpuset_observations": []}
    if hasattr(os, "sched_getaffinity"):
        try:
            affinity = sorted(os.sched_getaffinity(0))
            cpu.update(affinity_count=len(affinity), affinity_cpus=affinity)
        except OSError:
            pass
    memory = {"physical_bytes": None, "available_bytes": None, "cgroup_headroom_bytes": None,
              "cgroup_observations": [], "available_source": "UNKNOWN"}
    if sys.platform.startswith("linux"):
        meminfo = {}
        for line in (_read("/proc/meminfo") or "").splitlines():
            parts = line.split()
            if len(parts) >= 2 and _integer(parts[1]) is not None:
                meminfo[parts[0].rstrip(":")] = int(parts[1]) * (1024 if len(parts) > 2 and parts[2] == "kB" else 1)
        memory.update(physical_bytes=meminfo.get("MemTotal"), available_bytes=meminfo.get("MemAvailable"),
                      available_source="LINUX_MEMAVAILABLE" if "MemAvailable" in meminfo else "UNKNOWN")
        cgroups = _linux_cgroups()
        cpu.update({key: cgroups[key] for key in ("quota_cores", "quota_observations", "cpuset_count", "cpuset_observations")})
        memory.update(cgroup_headroom_bytes=cgroups["memory_headroom_bytes"], cgroup_observations=cgroups["memory_observations"])
    elif sys.platform == "darwin":
        cpu["logical_cpus"] = _integer(_command(["sysctl", "-n", "hw.logicalcpu"])) or cpu["logical_cpus"]
        memory["physical_bytes"] = _integer(_command(["sysctl", "-n", "hw.memsize"]))
        vm = _command(["vm_stat"]) or ""
        page_size = re.search(r"page size of (\d+) bytes", vm)
        pages = {name: int(count) for name, count in re.findall(r"^Pages ([^:]+):\s*(\d+)", vm, re.MULTILINE)}
        if page_size and "free" in pages and "inactive" in pages:
            memory["available_bytes"] = int(page_size[1]) * sum(pages.get(name, 0) for name in ("free", "inactive", "speculative"))
            memory["available_source"] = "MACOS_VM_STAT_FREE_INACTIVE_SPECULATIVE_ESTIMATE"
    try:
        free = shutil.disk_usage(run_directory).free
    except OSError:
        free = None
    return {"platform": sys.platform, "cpu": cpu, "memory": memory,
            "scratch": {"available_bytes": free, "path": str(Path(run_directory).absolute())}}


def _positive_integer(value, field):
    if type(value) is not int or value <= 0:
        raise ValueError(field + " must be a positive integer")
    return value


def _byte_capacity(available, requested, per_slot, ceilings=()):
    known = type(available) is int and available >= 0
    ceilings = [value for value in ceilings if type(value) is int and value >= 0]
    ceiling = min(ceilings) if ceilings else None
    if known and ceiling is not None:
        available = min(available, ceiling)
    observed = available * 4 // 5 if known else None
    upper_bound = ceiling * 4 // 5 if ceiling is not None else None
    candidates = [value for value in (observed, requested, upper_bound) if value is not None]
    admission = min(candidates) if candidates else None
    slots = admission // per_slot if admission is not None and per_slot else 1
    if not known:
        slots = min(1, slots)
    return {"available_bytes": available if known else None, "observed_admission_bytes": observed,
            "capacity_ceiling_bytes": ceiling, "upper_bound_admission_bytes": upper_bound,
            "requested_total_bytes": requested, "admission_bytes": admission,
            "capacity_known": known, "slot_limit": slots}


def plan_parallelism(args, cases, run_directory):
    """Plan slots from max read-request budgets; never weaken those budgets.

    cases has the business runner's [(name, template)] shape. A known capacity
    too small for one slot returns zero; an unknown capacity caps concurrency at
    one and is explicitly reported. The caller handles zero as admission failure.
    """
    requested = getattr(args, "jobs", "auto")
    if requested != "auto":
        if isinstance(requested, str) and requested.isdecimal():
            requested = int(requested)
        requested = _positive_integer(requested, "jobs")
    threads = _positive_integer(getattr(args, "threads", 1), "threads")
    rows = []
    for name, template in cases:
        request = template["request"]
        if request["operation"]["kind"] not in {"query", "validate"}:
            raise ValueError("parallel admission only accepts query/validate cases")
        limits = request["limits"]
        rows.append({"name": name,
                     "memory_bytes": _positive_integer(limits.get("memory_bytes"), "read memory_bytes"),
                     "scratch_bytes": _positive_integer(limits.get("scratch_bytes"), "read scratch_bytes")})
    per_slot = {"memory_bytes": max((row["memory_bytes"] for row in rows), default=0),
                "scratch_bytes": max((row["scratch_bytes"] for row in rows), default=0),
                "threads": threads, "basis": "MAXIMUM_OF_EACH_READ_REQUEST_LIMIT", "request_limits": rows}
    totals = {}
    for resource in ("memory", "scratch"):
        value = getattr(args, "total_" + resource + "_mib", None)
        totals[resource] = None if value is None else _positive_integer(value, "total_" + resource + "_mib") * MIB
    observed = resource_snapshot(run_directory)
    cpu = dict(observed["cpu"])
    cpu_values = [cpu.get(key) for key in ("logical_cpus", "affinity_count", "quota_cores", "cpuset_count")]
    cpu_values = [value for value in cpu_values if type(value) in (int, float) and math.isfinite(value) and value > 0]
    effective_cores = min(cpu_values) if cpu_values else None
    cpu.update(effective_cores=effective_cores, threads_per_slot=threads,
               capacity_known=effective_cores is not None,
               slot_limit=max(1, math.floor(effective_cores / threads)) if effective_cores is not None else 1)
    memory_observation = observed["memory"]
    available = memory_observation.get("available_bytes")
    memory_caps = [memory_observation.get(key) for key in ("physical_bytes", "cgroup_headroom_bytes")]
    memory = _byte_capacity(available, totals["memory"], per_slot["memory_bytes"], memory_caps)
    memory["observations"] = memory_observation
    scratch = _byte_capacity(observed["scratch"].get("available_bytes"), totals["scratch"], per_slot["scratch_bytes"])
    scratch["observations"] = observed["scratch"]
    resource_limits = {"cpu": cpu, "memory": memory, "scratch": scratch, "workload": {"slot_limit": len(rows)}}
    requested_cap = len(rows) if requested == "auto" else requested
    effective = min([requested_cap, len(rows)] + [resource_limits[name]["slot_limit"] for name in ("cpu", "memory", "scratch")])
    limiting = []
    for name, limit in resource_limits.items():
        if limit["slot_limit"] == effective:
            limiting.append(name.upper() + "_CAPACITY")
        if name != "workload" and not limit["capacity_known"]:
            limiting.append("UNKNOWN_" + name.upper() + "_CAPACITY")
    if requested != "auto" and requested_cap == effective:
        limiting.append("REQUESTED_JOBS")
    status = "NO_READ_CASES" if not rows else "INSUFFICIENT_RESOURCES" if effective == 0 else "READY"
    return {"schema_version": "ontokiln-business-parallel-plan/v1", "status": status,
            "requested_jobs": requested, "effective_jobs": effective, "per_slot": per_slot,
            "resource_limits": resource_limits, "limiting_factors": limiting,
            "policy": {"read_requests_only": True, "compile_materialize_serial": True,
                       "memory_scratch_reserve_fraction": RESERVE_FRACTION,
                       "explicit_totals": "CAPPED_BY_OBSERVED_AVAILABLE_CAPACITY_AFTER_RESERVE",
                       "unknown_capacity_max_jobs": 1, "request_limits_modified": False,
                       "hard_isolation": False,
                       "capacity_scope": "Availability snapshot at planning time; not a reservation. Independent requests may compete for CPU, memory and I/O."}}
