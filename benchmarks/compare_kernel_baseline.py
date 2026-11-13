#!/usr/bin/env python3
"""Compare kernel baseline or scaling reports without treating failed work as faster.

Exit 0 means the recorded configurations are comparable, not that performance
improved. Exit 1 means non-comparable; exit 2 means invalid input or CLI usage.
No engine is invoked and no performance threshold is imposed.
"""
import argparse
from collections import Counter
import json
import math
from pathlib import Path
import statistics
import sys


SCHEMA = "ontokiln-kernel-baseline/v1"
SCALING_SCHEMA = "ontokiln-kernel-scaling/v1"
SUPPORTED_SCHEMAS = {SCHEMA, SCALING_SCHEMA}
SCALING_METHOD_FIELDS = {"driver_sha256": 64, "harness_sha256": 64, "harness_source_revision": 40}
SCRIPT = "benchmarks/kernel_baseline.py"
METRICS = {
    "end_to_end_seconds": "seconds",
    "api_metrics.execution_seconds": "seconds",
    "api_metrics.parse_dictionary_seconds": "seconds",
    "api_metrics.index_seconds": "seconds",
    "api_metrics.engine_seconds": "seconds",
    "api_metrics.reasoning_seconds": "seconds",
    "api_metrics.peak_rss_bytes": "bytes",
    "api_metrics.supervisor_peak_rss_bytes": "bytes",
    "api_metrics.peak_scratch_bytes": "bytes",
    "process_wait4_peak_rss_bytes": "bytes",
    "mount_metrics.elapsed_seconds": "seconds",
    "mount_metrics.snapshot_bytes": "bytes",
    "mount_metrics.copied_bytes": "bytes",
}
NOTES = [
    "Matching recorded configuration does not establish the same physical host, storage device, host load or cache state; virtual environments may share resources.",
    "Percentages are descriptive median changes, not statistical significance. Negative means lower latency or memory; no speed/regression threshold is applied.",
    "Only COMPLETE samples with oracle.checked=true enter medians. Warm-ups never enter medians; failed, skipped, unverified or missing trials suppress stage percentages.",
    "Build declarations and rustc_on_path are recorded provenance, not independent proof of how a binary was built. Different engine identities are allowed.",
    "API RSS, supervisor RSS and wait4 RSS are distinct measurements; do not add them. Engine compile time overlaps parse/index phases; query/SHACL core time is unavailable.",
    "File and mounted-session reads have different timing boundaries; their configurations are deliberately non-comparable. Session mounts are one-time diagnostic samples and are not hidden in per-request times.",
    "Baseline and scaling reports are separate study schemas. Scaling comparisons require matching selected phases, driver and harness provenance; build receipt digests and local paths are not execution configuration.",
]


def at(value, path):
    for key in path.split("."):
        if not isinstance(value, dict):
            return None
        value = value.get(key)
    return value


def finite_number(value):
    return type(value) in (int, float) and math.isfinite(value) and value >= 0


def require(condition, message):
    if not condition:
        raise ValueError(message)


def indexed(items, key, label):
    require(isinstance(items, list), label + " must be an array")
    result = {}
    for item in items:
        require(isinstance(item, dict), label + " entries must be objects")
        identity = item.get(key)
        require(type(identity) in (int, str), label + " requires " + key)
        require(identity not in result, label + " has duplicate " + str(identity))
        result[identity] = item
    return result


def runtime_environment(value):
    # This variable is consumed by build.rs and embedded in kernel identity;
    # all other recorded environment keys remain execution configuration.
    return ({key: item for key, item in value.items() if key != "ONTOKILN_BUILD_REVISION"}
            if isinstance(value, dict) else value)


def recorded_configuration(report):
    env = report.get("environment", {})
    require(isinstance(env, dict), "environment must be an object")
    config = report.get("configuration", {})
    require(isinstance(config, dict), "configuration must be an object")
    affinity = env.get("cpu_affinity")
    overrides = runtime_environment(report.get("process_environment_overrides"))
    relevant = runtime_environment(env.get("relevant_environment"))
    effective = {**relevant, **overrides} if isinstance(relevant, dict) and isinstance(overrides, dict) else None
    recorded = {
        "configuration": {key: value for key, value in config.items()
                          if key not in {"binary", "output", "work_dir", "keep_work_dir"}},
        **{key: report.get(key) for key in ["limits", "cache_policy", "scratch_policy", "concurrency",
                                           "scope", "metric_definitions"]},
        "process_environment_overrides": overrides,
        **{"environment." + key: env.get(key) for key in [
            "cpu_model", "logical_cpus", "platform", "machine", "physical_memory_bytes",
            "rustc_on_path", "python", "cgroup_limits_raw",
            "build_profile_caller_declaration", "build_features_caller_declaration"]},
        "environment.effective_relevant_environment": effective,
        "environment.cpu_affinity_count": len(affinity) if isinstance(affinity, list) and affinity else None,
        "environment.filesystem": at(env, "filesystem.filesystem"),
        "method.script_sha256": env.get("provenance_files", {}).get(SCRIPT),
    }
    if report.get("schema_version") == SCALING_SCHEMA:
        recorded["phase_selection"] = report.get("phase_selection")
        recorded.update({"method." + field: at(report, "method." + field) for field in SCALING_METHOD_FIELDS})
    return recorded


def outcome(stage):
    samples = stage.get("samples", [])
    require(isinstance(samples, list), "stage samples must be an array")
    counts = {"measured": Counter(), "warmup": Counter()}
    verified = []
    failures = []
    for sample in samples:
        require(isinstance(sample, dict) and isinstance(sample.get("status"), str),
                "sample requires a status string")
        require(type(sample.get("warmup", False)) is bool, "warmup must be boolean")
        phase = "warmup" if sample.get("warmup", False) else "measured"
        status = sample["status"]
        if status == "COMPLETE" and (at(sample, "oracle.checked") is not True
                                     or not finite_number(sample.get("end_to_end_seconds"))):
            status = "UNVERIFIED"
        counts[phase][status] += 1
        if status == "COMPLETE" and phase == "measured":
            verified.append(sample)
        elif status != "COMPLETE":
            failures.append({"phase": phase, "status": status, **{key: sample[key] for key in
                             ["sample_index", "error", "oracle_error", "reason"] if key in sample}})
    summary = stage.get("summary", {})
    planned = {}
    for phase in counts:
        value = summary.get("planned_" + phase + "_samples")
        require(type(value) is int and value >= 0, "stage summary requires planned " + phase + " samples")
        planned[phase] = value
    complete = all(counts[phase]["COMPLETE"] == planned[phase]
                   and sum(counts[phase].values()) == planned[phase] for phase in counts)
    return {"planned_measured_samples": planned["measured"],
            "planned_warmup_samples": planned["warmup"],
            "successful_samples": len(verified),
            "measured_status_counts": dict(counts["measured"]),
            "warmup_status_counts": dict(counts["warmup"]),
            "missing_measured_samples": max(0, planned["measured"] - sum(counts["measured"].values())),
            "missing_warmup_samples": max(0, planned["warmup"] - sum(counts["warmup"].values())),
            "all_planned_samples_verified": complete, "failures": failures}, verified


def distribution(samples, metric):
    values = [at(sample, metric) for sample in samples]
    values = [value for value in values if finite_number(value)]
    return {"samples": len(values), "median": statistics.median(values) if values else None,
            "min": min(values) if values else None, "max": max(values) if values else None}


def compare(before, after, min_samples=3):
    require(type(min_samples) is int and min_samples >= 2, "min_samples must be at least 2")
    require(isinstance(before, dict) and isinstance(after, dict), "reports must be JSON objects")
    issues = []

    def check(field, left, right, known=True):
        if known and (left is None or right is None or left == "unspecified" or right == "unspecified"):
            issues.append({"field": field, "reason": "required comparability metadata is unknown",
                           "before": left, "after": right})
        elif left != right:
            issues.append({"field": field, "reason": "recorded values differ", "before": left, "after": right})

    for label, report in [("before", before), ("after", after)]:
        schema = report.get("schema_version")
        if not isinstance(schema, str) or schema not in SUPPORTED_SCHEMAS:
            issues.append({"field": label + ".schema_version", "reason": "unsupported report schema",
                           "value": schema})
        if report.get("status") not in {"COMPLETE", "COMPLETE_WITH_FAILURES"}:
            issues.append({"field": label + ".status", "reason": "report is unfinished or aborted",
                           "value": report.get("status")})
        if schema == SCALING_SCHEMA:
            phases = report.get("phase_selection")
            if not (isinstance(phases, list) and phases and all(isinstance(p, str) and p for p in phases)
                    and len(set(phases)) == len(phases)):
                issues.append({"field": label + ".phase_selection",
                               "reason": "scaling reports require a nonempty list of distinct phase names", "value": phases})
            for field, length in SCALING_METHOD_FIELDS.items():
                value = at(report, "method." + field)
                if not (isinstance(value, str) and len(value) == length
                        and all(c in "0123456789abcdef" for c in value)):
                    issues.append({"field": label + ".method." + field,
                                   "reason": f"scaling provenance requires {length} lowercase hexadecimal characters", "value": value})
    check("schema_version", before.get("schema_version"), after.get("schema_version"))
    left_config, right_config = recorded_configuration(before), recorded_configuration(after)
    for field in sorted(left_config.keys() | right_config.keys()):
        check(field, left_config.get(field), right_config.get(field))
    for label, report, config in [("before", before, left_config), ("after", after, right_config)]:
        if report.get("schema_version") == SCALING_SCHEMA:
            check(label + ".method.harness_sha256_matches_script_provenance",
                  config.get("method.harness_sha256"), config.get("method.script_sha256"))
    left_scales = indexed(before.get("scales"), "facts", "scales")
    right_scales = indexed(after.get("scales"), "facts", "scales")
    require(left_scales and right_scales, "reports require at least one measured scale")
    check("scales.facts", sorted(left_scales), sorted(right_scales))
    pairs = []
    for facts in sorted(left_scales.keys() | right_scales.keys()):
        left, right = left_scales.get(facts), right_scales.get(facts)
        if left is None or right is None:
            continue
        require(isinstance(left.get("input"), dict) and isinstance(right.get("input"), dict),
                "scale input must be an object")
        check(f"scales[{facts}].generator", left["input"].get("generator"), right["input"].get("generator"))
        for side in [left, right]:
            require(isinstance(side["input"].get("files"), list) and side["input"]["files"],
                    "scale input requires file hashes")
            for file in side["input"]["files"]:
                require(isinstance(file, dict) and isinstance(file.get("sha256"), str)
                        and len(file["sha256"]) == 64 and all(c in "0123456789abcdef" for c in file["sha256"]),
                        "input file requires a SHA-256")
        check(f"scales[{facts}].input", left["input"], right["input"])
        ls = indexed(left.get("stages"), "name", "stages")
        rs = indexed(right.get("stages"), "name", "stages")
        for label, report, stages in [("before", before, ls), ("after", after, rs)]:
            phases = report.get("phase_selection")
            if report.get("schema_version") == SCALING_SCHEMA and isinstance(phases, list) and all(isinstance(p, str) for p in phases):
                check(f"{label}.scales[{facts}].selected_phases_present", sorted(phases), sorted(stages))
        check(f"scales[{facts}].stages", sorted(ls), sorted(rs))
        for name in sorted(ls.keys() | rs.keys()):
            l, r = ls.get(name), rs.get(name)
            if l is None or r is None:
                # Retain the available side's failures even when a stage is absent.
                empty = {"samples": [], "summary": {"planned_measured_samples": 0, "planned_warmup_samples": 0}}
                pairs.append((facts, name, l or empty, r or empty))
                continue
            check(f"scales[{facts}].stages[{name}].metadata", l.get("metadata"), r.get("metadata"))
            pairs.append((facts, name, l, r))
    comparable = not issues
    stages = []
    for facts, name, left, right in pairs:
        lout, lsamples = outcome(left)
        rout, rsamples = outcome(right)
        blocked = []
        if not comparable:
            blocked.append("non-comparable reports")
        if not lout["all_planned_samples_verified"] or not rout["all_planned_samples_verified"]:
            blocked.append("failed, skipped, unverified or missing planned samples (including warm-ups)")
        if min(len(lsamples), len(rsamples)) < min_samples:
            blocked.append(f"fewer than {min_samples} verified measured samples")
        metrics = {}
        for metric, unit in METRICS.items():
            ld, rd = distribution(lsamples, metric), distribution(rsamples, metric)
            if not ld["samples"] and not rd["samples"] and metric != "end_to_end_seconds":
                continue
            reason = "; ".join(blocked) or None
            if reason is None and (ld["samples"] != len(lsamples) or rd["samples"] != len(rsamples)):
                reason = "metric missing or invalid in verified samples"
            if reason is None and min(ld["samples"], rd["samples"]) < min_samples:
                reason = f"fewer than {min_samples} metric samples"
            if reason is None and ld["median"] == 0:
                reason = "zero baseline median has no percentage change"
            metrics[metric] = {"unit": unit, "before": ld, "after": rd,
                               "change_percent": (rd["median"] / ld["median"] - 1) * 100 if reason is None else None,
                               "comparison_unavailable_reason": reason}
        stages.append({"facts": facts, "stage": name, "before": lout, "after": rout,
                       "comparison_blockers": blocked, "metrics": metrics})
    identity_keys = ["binary_sha256", "repository_head", "repository_dirty", "kernel_identity"]
    method_fields = ["method.script_sha256", "metric_definitions", "phase_selection"] + [
        "method." + field for field in SCALING_METHOD_FIELDS]
    return {"schema_version": "ontokiln-kernel-baseline-comparison/v1", "comparable": comparable,
            "methodology_changed": (before.get("schema_version") != after.get("schema_version")
                                    or any(left_config.get(field) != right_config.get(field) for field in method_fields)),
            "minimum_successful_samples": min_samples, "issues": issues,
            "before": {"status": before.get("status"), "engine": {k: at(before, "environment." + k) for k in identity_keys}},
            "after": {"status": after.get("status"), "engine": {k: at(after, "environment." + k) for k in identity_keys}},
            "stages": stages, "notes": NOTES}


def markdown(report):
    def cell(value):
        return str(value).replace("|", "\\|").replace("\n", " ")

    def counts(side):
        measured = ", ".join(f"{k}:{v}" for k, v in side["measured_status_counts"].items()) or "none"
        warm = ", ".join(f"{k}:{v}" for k, v in side["warmup_status_counts"].items()) or "none"
        return f"measured {side['successful_samples']}/{side['planned_measured_samples']} verified ({measured}); warm-up {warm}/{side['planned_warmup_samples']}"

    def median(value, unit):
        if value["median"] is None:
            return f"— (n={value['samples']})"
        number = value["median"] / 1024**2 if unit == "bytes" else value["median"]
        return f"{number:.6g} {'MiB' if unit == 'bytes' else 's'} (n={value['samples']})"

    lines = ["# Kernel baseline comparison", "",
             "Recorded configurations: **" + ("COMPARABLE" if report["comparable"] else "NON-COMPARABLE") + "**.", "",
             f"Report status: {report['before']['status']} → {report['after']['status']}. Minimum verified measured samples: {report['minimum_successful_samples']}.", ""]
    if report["methodology_changed"]:
        lines += ["**Measurement methodology changed** (study schema, script/driver/harness provenance, selected phases or metric definitions); percentages are suppressed.", ""]
    if report["issues"]:
        lines += ["## Comparability issues", ""]
        lines += [f"- `{cell(issue['field'])}`: {cell(issue['reason'])}." for issue in report["issues"]]
        lines.append("")
    lines += ["## Samples and outcomes", "", "| Facts | Stage | Before | After | Percentage blockers |",
              "|---:|---|---|---|---|"]
    for stage in report["stages"]:
        lines.append(f"| {stage['facts']} | {cell(stage['stage'])} | {cell(counts(stage['before']))} | {cell(counts(stage['after']))} | {cell('; '.join(stage['comparison_blockers']) or 'none')} |")
    lines += ["", "## Verified measured-sample medians", "",
              "Change = (after / before − 1) × 100; failure durations and warm-ups are excluded.", "",
              "| Facts | Stage / metric | Before | After | Change |", "|---:|---|---:|---:|---|"]
    for stage in report["stages"]:
        for name, metric in stage["metrics"].items():
            percent = metric["change_percent"]
            change = f"{percent:+.2f}%" if percent is not None else "—"
            if percent is None and not stage["comparison_blockers"]:
                change += "; " + metric["comparison_unavailable_reason"]
            lines.append(f"| {stage['facts']} | {cell(stage['stage'] + ' / ' + name)} | {median(metric['before'], metric['unit'])} | {median(metric['after'], metric['unit'])} | {cell(change)} |")
    lines += ["", "## Interpretation", ""] + ["- " + note for note in report["notes"]]
    return "\n".join(lines) + "\n"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("--format", choices=["markdown", "json"], default="markdown")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--min-samples", type=int, default=3, help="minimum per side for descriptive percentages (at least 2; not a significance test)")
    args = parser.parse_args(argv)
    try:
        report = compare(json.loads(args.before.read_text()), json.loads(args.after.read_text()), args.min_samples)
        text = json.dumps(report, indent=2, sort_keys=True, allow_nan=False) + "\n" if args.format == "json" else markdown(report)
        if args.output:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(text, encoding="utf8")
        else:
            print(text, end="")
    except (OSError, ValueError, TypeError, KeyError) as error:
        print("Cannot compare baselines: " + str(error), file=sys.stderr)
        return 2
    return 0 if report["comparable"] else 1


if __name__ == "__main__":
    sys.exit(main())
