#!/usr/bin/env python3
"""Replay real API v1 workload templates and collect a bounded local baseline.

Only the Python standard library is required. No engine build, upload, rule
generation, query rewriting, or implicit business-answer oracle is performed.
See benchmarks/business-baseline.md for the one-command workflow.
"""
import argparse
import copy
import datetime
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import shutil
import signal
import statistics
import sys
import tempfile
import traceback
import zipfile


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("business_baseline_core", ROOT / "benchmarks/kernel_baseline.py")
core = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(core)
CONTRACT = "ontokiln-api/v1"
SCHEMA = "ontokiln-business-baseline/v1"
DEFAULT_LIMITS = {
    "memory_bytes": 4 * 1024**3, "scratch_bytes": 8 * 1024**3,
    "timeout_ms": 300_000, "max_rows": 100_000, "max_bytes": 4 * 1024**2,
    "max_iterations": 100, "max_derived_facts": 10_000_000,
    "max_intermediate_rows": 100_000_000, "max_validation_depth": 64,
    "max_term_bytes": 1024**2,
}


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "duplicate JSON field: " + key)
        result[key] = value
    return result


def reject_constant(value):
    raise ValueError("nonfinite JSON value: " + value)


def decode_json(data):
    return json.loads(data, object_pairs_hook=unique_object, parse_constant=reject_constant)


def absolute_path(value, cwd):
    require(isinstance(value, str) and value, "input path must be a nonempty string")
    return Path(os.path.abspath(cwd / value))


def effective_limits(original, overrides):
    require(isinstance(original, dict), "limits must be an object")
    limits = {**DEFAULT_LIMITS, **original, **overrides}
    for key in ("memory_bytes", "scratch_bytes", "timeout_ms"):
        value = limits[key]
        require(type(value) in (int, float) and math.isfinite(value) and value > 0,
                f"{key} must be finite and positive for a bounded benchmark; "
                "use an explicit CLI budget override for an unbounded template")
    for key in ("memory_bytes", "scratch_bytes"):
        require(type(limits[key]) is int, key + " must be an integer")
    return limits


def template_info(template):
    request = template["request"]
    operation = request["operation"]
    info = {key: template[key] for key in ("path", "sha256", "inputs")}
    info.update(kind=operation["kind"], effective_limits=request["limits"])
    if operation["kind"] == "compile":
        info["profile"] = operation.get("profile")
    if operation["kind"] == "query":
        info["effective_view"] = operation.get("view", "asserted")
        info["derived_query_count"] = len(operation.get("derived_queries", []))
    return info


def load_template(path, allowed_kinds, input_cwd, overrides):
    path = path.resolve()
    raw = path.read_bytes()
    request = decode_json(raw)
    require(isinstance(request, dict) and request.get("contract") == CONTRACT,
            f"{path}: expected an API v1 request, not a response or legacy Full request")
    operation = request.get("operation")
    require(isinstance(operation, dict) and operation.get("kind") in allowed_kinds,
            f"{path}: expected operation kind in {sorted(allowed_kinds)}")
    if operation["kind"] == "compile":
        require(isinstance(operation.get("sources"), list) and operation["sources"],
                "compile template requires a nonempty, explicit sources list")
        require(isinstance(operation.get("profile"), str) and operation["profile"],
                "compile template requires its real profile")
    if operation["kind"] == "query":
        require(isinstance(operation.get("query"), str), "query must contain SPARQL text")
    request["limits"] = effective_limits(request.get("limits", {}), overrides)
    inputs = []
    for field in ("sources", "rules", "shapes"):
        entries = operation.get(field, [])
        require(isinstance(entries, list), f"{field} must be a file array")
        for entry in entries:
            require(isinstance(entry, dict), f"{field} entries must be objects")
            file = absolute_path(entry.get("path"), input_cwd)
            require(not file.is_symlink() and file.is_file(), f"input must be a regular non-symlink file: {file}")
            expected = entry.get("expected_sha256")
            require(isinstance(expected, str) and len(expected) == 64
                    and all(c in "0123456789abcdef" for c in expected),
                    f"invalid expected_sha256 for {file}; export the actual request, do not omit its hash")
            actual = core.digest_file(file)
            require(actual == expected, f"input SHA-256 mismatch: {file}; hashes are never refreshed automatically")
            entry["path"] = str(file)
            inputs.append({"role": field, "path": str(file), "bytes": file.stat().st_size,
                           "sha256": actual, **{k: entry[k] for k in ("document_id", "base_iri", "format") if k in entry}})
    return {"path": str(path), "sha256": hashlib.sha256(raw).hexdigest(),
            "original_bytes": raw, "request": request, "inputs": inputs}


def planned_stage(name, kind, template, count, warmups=0):
    samples = [{"sample_index": i, "warmup": True, "status": "PENDING"} for i in range(warmups)]
    samples += [{"sample_index": i, "warmup": False, "status": "PENDING"} for i in range(count)]
    return {"name": name, "kind": kind, "template": template_info(template),
            "planned_measured_samples": count, "planned_warmup_samples": warmups, "samples": samples}


def result_observation(kind, result, request):
    require(isinstance(result, dict), "COMPLETE response requires an object result")
    if kind in ("compile", "materialize"):
        stats = result.get("stats")
        require(isinstance(stats, dict) and all(type(stats.get(k)) is int and stats[k] >= 0
                for k in ("explicit_facts", "inferred_facts")), "invalid artifact fact counts")
        require(result.get("fixpoint_reached") is (kind == "materialize"), "invalid fixpoint completion flag")
        return {"stats": stats, "fixpoint_reached": result["fixpoint_reached"]}
    if kind == "query":
        require(result.get("complete") is True and result.get("truncated") is False,
                "query is incomplete or truncated")
        require(result.get("view") == request["operation"].get("view", "asserted"), "query view changed")
        require(result.get("query_type") in ("SELECT", "ASK") and isinstance(result.get("items"), list)
                and isinstance(result.get("variables"), list), "invalid query result")
        require(all(isinstance(v, str) for v in result["variables"])
                and all(isinstance(row, dict) for row in result["items"]), "invalid query rows or variables")
        if result["query_type"] == "ASK":
            require(type(result.get("boolean")) is bool and not result["items"] and not result["variables"],
                    "invalid ASK result")
        else:
            require(result.get("boolean") is None, "invalid non-ASK boolean")
        return {"query_type": result["query_type"], "returned_rows": len(result["items"]),
                "boolean": result.get("boolean"), "view": result["view"],
                "complete": True, "truncated": False}
    require(type(result.get("conforms")) is bool, "invalid validation conformance")
    require(all(type(result.get(k)) is int and result[k] >= 0 for k in ("checked_shapes", "violations")),
            "invalid validation counts")
    details = result.get("report")
    require(isinstance(details, dict) and isinstance(details.get("violations"), list)
            and len(details["violations"]) == result["violations"], "invalid validation report")
    require(details.get("conforms") is result["conforms"]
            and (not result["conforms"] or result["violations"] == 0), "contradictory validation conformance")
    require(details.get("target_count") is None or (type(details["target_count"]) is int and details["target_count"] >= 0),
            "invalid validation target count")
    return {"conforms": result["conforms"], "checked_shapes": result["checked_shapes"],
            "violations": result["violations"], "reported_target_count": details.get("target_count"),
            "empty_validation_scope": result["checked_shapes"] == 0 or details.get("target_count") == 0}


def artifact_observation(result, expected_output, kind, expected_profile, compiled=None):
    output = expected_output.absolute()
    require(result.get("dataset") == str(output), "artifact dataset differs from this run's requested output")
    artifact = result.get("artifact")
    require(isinstance(artifact, dict) and artifact.get("path") == str(output), "artifact path mismatch")
    require(output.is_dir() and not output.is_symlink(), "artifact is not this run's regular output directory")
    manifest_file = output / "manifest.json"
    require(manifest_file.is_file() and not manifest_file.is_symlink(), "missing regular artifact manifest")
    raw = manifest_file.read_bytes()
    require(hashlib.sha256(raw).hexdigest() == artifact.get("manifest_sha256"), "manifest digest mismatch")
    manifest = decode_json(raw)
    require(manifest.get("completion") == "COMPLETE" and manifest.get("kind") == kind,
            "artifact manifest is not a completed matching kind")
    require(manifest.get("content_digest") == artifact.get("content_digest"), "artifact content identity mismatch")
    require(manifest.get("profile_id") == artifact.get("profile_id") == expected_profile,
            "artifact profile differs from this run's compile request")
    stats = manifest.get("stats", {})
    for key in ("explicit_facts", "inferred_facts"):
        # A compiled manifest records only asserted counts. API v1 explicitly
        # supplies inferred_facts=0; a reasoned manifest must record its count.
        observed = stats.get(key, 0) if kind == "compiled" and key == "inferred_facts" else stats.get(key)
        require(observed == result["stats"][key], "manifest/response fact count mismatch")
    if kind == "reasoned":
        require(manifest.get("fixpoint_reached") is True, "reasoned manifest lacks completed fixpoint")
        require(manifest.get("compiled_content_digest") == compiled["artifact"]["content_digest"],
                "reasoned artifact does not bind this run's retained compiled artifact")
        require(result["stats"]["explicit_facts"] == compiled["stats"]["explicit_facts"],
                "materialization changed the explicit fact count")
    return {"artifact": artifact, "manifest_stats": stats, "files": core.artifact_files(output)}


def remove_owned_artifact(path, artifact_root):
    require(path.parent == artifact_root and path.name.startswith(("compiled-", "reasoned-")),
            "refuse cleanup outside the run's artifact directory")
    require(not path.is_symlink() and path.is_dir(), "refuse cleanup of a non-directory or symlink")
    shutil.rmtree(path)


class Runner:
    def __init__(self, args, directory, report, compile_template, cases):
        self.args, self.directory, self.report = args, directory, report
        self.compile_template, self.cases = compile_template, cases
        self.environment = os.environ.copy()
        scratch = directory / "scratch"
        scratch.mkdir()
        self.environment.update(TMPDIR=str(scratch), RAYON_NUM_THREADS=str(args.threads), TOKIO_WORKER_THREADS="1")
        self.artifact_root = directory / "artifacts"
        self.artifact_root.mkdir()
        self.artifacts = {}
        self.counter = 0
        report["stages"] = [planned_stage("compile", "compile", compile_template, args.build_samples),
                            planned_stage("materialize", "materialize", compile_template, args.build_samples)]
        report["stages"] += [planned_stage("read/" + name, template["request"]["operation"]["kind"],
                                          template, args.read_samples, args.warmups) for name, template in cases]
        report["process_environment_overrides"] = {"TMPDIR": "<run_directory>/scratch",
                                                  "RAYON_NUM_THREADS": str(args.threads), "TOKIO_WORKER_THREADS": "1"}
        report["input_cwd"] = str(args.input_cwd)
        report["read_dataset"] = args.read_dataset

    def checkpoint(self):
        self.report["updated_at_utc"] = now()
        for stage in self.report["stages"]:
            stage["summary"] = core.summarize(stage["samples"], stage["planned_measured_samples"],
                                               stage["planned_warmup_samples"])
            complete = [s for s in stage["samples"] if s["status"] == "COMPLETE" and not s["warmup"]]
            keys = sorted({k for s in complete for k, v in s.get("api_metrics", {}).items()
                           if type(v) in (int, float) and math.isfinite(v)})
            stage["metric_summary"] = {}
            for key in keys:
                values = [s["api_metrics"][key] for s in complete if type(s.get("api_metrics", {}).get(key)) in (int, float)
                          and math.isfinite(s["api_metrics"][key])]
                stage["metric_summary"][key] = {"samples": len(values), "median": statistics.median(values),
                                                 "min": min(values), "max": max(values)}
        core.publish(self.directory / "summary.json", self.report)

    def invoke(self, request, sample, output=None):
        self.counter += 1
        case_dir = self.directory / "requests" / f"{self.counter:04d}"
        case_dir.mkdir(parents=True)
        request = copy.deepcopy(request)
        request["request_id"] = f"business-baseline-{self.counter:04d}"
        request_bytes = core.canonical(request)
        request_file, response_file = case_dir / "request.json", case_dir / "response.json"
        request_file.write_bytes(request_bytes)
        sample.update(status="RUNNING", request_id=request["request_id"],
                      request_sha256=hashlib.sha256(request_bytes).hexdigest(),
                      request_file=str(request_file.relative_to(self.directory)),
                      response_file=str(response_file.relative_to(self.directory)),
                      protocol_verified=False, business_correctness="MANUAL_REVIEW_REQUIRED")
        self.checkpoint()
        try:
            measurements, payload = core.timed_command(
                [self.args.binary, "api", "--request", request_file, "--result", response_file],
                case_dir, request["limits"]["timeout_ms"] / 1000 + 5, self.environment, response_file)
        except KeyboardInterrupt:
            sample.update(status="INTERRUPTED", error={"message": "runner interrupted; process group terminated and supervisor waited; no worker-reaping receipt asserted"})
            self.checkpoint()
            raise
        sample.update(measurements)
        sample["status"] = "PROCESS_FAILED"
        if payload is not None:
            sample["response_sha256"] = hashlib.sha256(payload).hexdigest()
        if sample["watchdog_timeout"]:
            sample["status"] = "WATCHDOG_TIMEOUT"
            return None
        if payload is None:
            sample["error"] = {"message": "command did not write a response file"}
            return None
        try:
            reply = decode_json(payload)
            require(isinstance(reply, dict), "API response must be an object")
            execution = reply.get("execution")
            require(isinstance(execution, dict) and isinstance(execution.get("metrics", {}), dict),
                    "API execution and metrics must be objects")
            sample.update(api_status=reply.get("status"), error=reply.get("error"),
                          api_metrics=execution.get("metrics", {}))
            require(reply.get("contract") == CONTRACT and reply.get("request_id") == request["request_id"]
                    and reply.get("request_sha256") == sample["request_sha256"], "API envelope/request binding mismatch")
            require(reply.get("kernel") == self.report["environment"]["kernel_identity"], "engine identity changed")
            require(isinstance(reply.get("status"), str), "missing API terminal status")
            if reply["status"] != "COMPLETE":
                require(reply.get("result") is None and isinstance(reply.get("error"), dict), "invalid failed API result")
                sample["status"] = "API_FAILED"
                return None
            require(reply.get("error") is None and execution.get("worker_reaped") is True,
                    "COMPLETE response contains error or lacks worker reaping")
            require(sample["returncode"] == 0, "COMPLETE response with nonzero process exit")
            result = reply.get("result")
            kind = request["operation"]["kind"]
            observation = result_observation(kind, result, request)
            if output is not None:
                artifact_kind = "compiled" if kind == "compile" else "reasoned"
                before = self.artifacts.get("compiled")
                profile = self.compile_template["request"]["operation"]["profile"]
                observation.update(artifact_observation(result, output, artifact_kind, profile, before))
                if artifact_kind in self.artifacts:
                    require(result["stats"] == self.artifacts[artifact_kind]["stats"], "fact counts changed between repetitions")
                shutil.copyfile(output / "manifest.json", case_dir / "manifest.json")
            sample.update(status="COMPLETE", protocol_verified=True, observation=observation)
            return result
        except (ValueError, KeyError, TypeError, AttributeError, OSError) as error:
            sample.update(status="PROTOCOL_FAILED", protocol_error=str(error))
            return None

    def series(self, stage, template, dependency=None):
        failed = None
        for sample in stage["samples"]:
            if failed is not None or (dependency is not None and dependency not in self.artifacts):
                sample.update(status="SKIPPED", reason=failed or f"no successful retained {dependency} artifact")
                self.checkpoint()
                continue
            request = copy.deepcopy(template["request"])
            kind, output = stage["kind"], None
            if kind == "compile":
                output = self.artifact_root / f"compiled-{sample['sample_index'] + 1}"
                request["operation"]["output"] = str(output)
            elif kind == "materialize":
                output = self.artifact_root / f"reasoned-{sample['sample_index'] + 1}"
                request["operation"] = {"kind": "materialize", "dataset": self.artifacts["compiled"]["dataset"],
                                        "output": str(output)}
            else:
                retained = self.artifacts[self.args.read_dataset]
                request["operation"].update(dataset=retained["dataset"],
                    expected_content_digest=retained["artifact"]["content_digest"],
                    expected_manifest_sha256=retained["artifact"]["manifest_sha256"])
            if output is not None:
                require(not output.exists() and not output.is_symlink(), "run output already exists")
            result = self.invoke(request, sample, output)
            if result is None:
                failed = f"remaining repetitions stopped after {sample['status']}"
            elif output is not None:
                artifact_kind = "compiled" if kind == "compile" else "reasoned"
                first = artifact_kind not in self.artifacts
                if first:
                    self.artifacts[artifact_kind] = result
                retained = first or self.args.keep_all_artifacts
                sample["observation"]["artifact_retained"] = retained
                if not retained:
                    remove_owned_artifact(output, self.artifact_root)
            self.checkpoint()
            print(f"{stage['name']} {'warmup' if sample['warmup'] else sample['sample_index'] + 1}: {sample['status']}", flush=True)

    def run(self):
        self.checkpoint()
        self.series(self.report["stages"][0], self.compile_template)
        self.series(self.report["stages"][1], self.compile_template, "compiled")
        for stage, (_, template) in zip(self.report["stages"][2:], self.cases):
            self.series(stage, template, self.args.read_dataset)


def selected_fields(value, keys):
    return {key: copy.deepcopy(value[key]) for key in keys if key in value}


def numeric_fields(value):
    return {key: item for key, item in value.items()
            if type(item) is bool or (type(item) in (int, float) and math.isfinite(item))}


def sharing_summary(report):
    """Allow measured metadata, never copy free-form API/runner diagnostics.

    Paths, case names, hashes, hardware and build identity remain metadata, not
    anonymized identifiers. Original query results and diagnostic prose stay
    in the local run directory, even when an API error embeds private terms.
    """
    shared = selected_fields(report, (
        "schema_version", "status", "started_at_utc", "updated_at_utc", "finished_at_utc",
        "business_correctness", "run_directory", "configuration", "scope", "cache_policy",
        "timing_policy", "statistics", "concurrency", "method", "environment", "read_case_count",
        "read_dataset", "input_cwd", "process_environment_overrides", "budget_overrides",
        "binary_sha256_after", "interrupt_signal"))
    shared["summary_variant"] = "sharing-metadata/v1"
    shared["diagnostics"] = "Full diagnostic text is retained only in local summary.json and requests/. Paths and build metadata are not anonymized."
    if report.get("error"):
        shared["error"] = {"code": report["status"], "local_diagnostics": "summary.json"}
    codes = {"INVALID_REQUEST", "INVALID_DATASET", "INVALID_QUERY", "EXECUTION_FAILED",
             "UNSUPPORTED_CONTRACT", "UNSUPPORTED_CAPABILITY", "RESOURCE_EXHAUSTED", "CANCELLED"}
    reasons = codes | {"SOURCE_INVALID", "RULE_INVALID", "UNSAFE_RULE", "BUILD_ABORTED", "TTL_PARSE_ERROR",
                       "UNSUPPORTED_RULE_PROFILE", "FORMAT_VERSION_UNSUPPORTED", "MANIFEST_CORRUPT",
                       "QUERY_FAILED", "VALIDATION_FAILED", "RESOURCE_LIMIT_EXCEEDED", "BUDGET_EXHAUSTED",
                       "FIXPOINT_LIMIT_EXCEEDED"}
    shared["stages"] = []
    for stage in report.get("stages", []):
        item = selected_fields(stage, ("name", "kind", "planned_measured_samples", "planned_warmup_samples", "metric_summary"))
        template = stage["template"]
        item["template"] = selected_fields(template, ("path", "sha256", "kind", "profile", "effective_view", "derived_query_count"))
        item["template"]["effective_limits"] = selected_fields(template["effective_limits"], DEFAULT_LIMITS)
        item["template"]["inputs"] = [selected_fields(source, ("role", "path", "bytes", "sha256", "format"))
                                          for source in template["inputs"]]
        item["samples"] = []
        for sample in stage["samples"]:
            kept = selected_fields(sample, (
                "sample_index", "warmup", "status", "request_id", "request_sha256", "response_sha256",
                "request_file", "response_file", "protocol_verified", "business_correctness", "reason",
                "end_to_end_seconds", "process_wall_seconds", "process_user_seconds", "process_system_seconds",
                "process_wait4_peak_rss_bytes", "process_block_read_bytes", "process_block_write_bytes",
                "returncode", "watchdog_timeout", "response_bytes"))
            if "api_status" in sample:
                status = sample["api_status"]
                kept["api_status"] = status if isinstance(status, str) and status in {"COMPLETE", "FAILED", "UNSUPPORTED", "BUDGET_EXHAUSTED", "CANCELLED"} else "UNRECOGNIZED"
            if sample.get("error") or sample.get("protocol_error") or sample.get("stderr_tail"):
                error = sample.get("error")
                error = error if isinstance(error, dict) else {}
                kept["error"] = {"code": error.get("code") if isinstance(error.get("code"), str) and error["code"] in codes else "SEE_LOCAL_DIAGNOSTICS"}
                if isinstance(error.get("reason"), str) and error["reason"] in reasons:
                    kept["error"]["reason"] = error["reason"]
                kept["local_diagnostics"] = sample.get("response_file", "summary.json")
            kept["api_metrics"] = numeric_fields(sample.get("api_metrics", {}))
            observation = sample.get("observation", {})
            kept["observation"] = selected_fields(observation, (
                "fixpoint_reached", "query_type", "returned_rows", "boolean", "view", "complete", "truncated",
                "conforms", "checked_shapes", "violations", "reported_target_count", "empty_validation_scope", "artifact_retained"))
            for name in ("stats", "manifest_stats"):
                if name in observation:
                    kept["observation"][name] = numeric_fields(observation[name])
            if "artifact" in observation:
                kept["observation"]["artifact"] = selected_fields(observation["artifact"], (
                    "path", "manifest_sha256", "content_digest", "source_tree_digest", "semantic_fingerprint", "profile_id"))
            if "files" in observation:
                kept["observation"]["files"] = observation["files"]
            item["samples"].append(kept)
        item["summary"] = core.summarize(item["samples"], item["planned_measured_samples"], item["planned_warmup_samples"])
        shared["stages"].append(item)
    return shared


def render_report(report):
    def cell(value):
        return str(value).replace("|", "\\|").replace("\n", " ")

    lines = ["# 真实业务编译与推理基线", "", f"执行状态：**{report['status']}**。",
             "业务正确性：**需要人工核对（MANUAL_REVIEW_REQUIRED）**。协议检查和重复计数一致不构成业务答案证明。", "",
             "| 阶段 | 成功正式样本 | 中位数 s | 范围 s | RSS 最高 MiB | scratch 最高 MiB | 备注 |",
             "|---|---:|---:|---:|---:|---:|---|"]
    for stage in report.get("stages", []):
        summary = stage.get("summary", {})
        median = summary.get("median_seconds")
        spread = "—" if median is None else f"{summary['min_seconds']:.4f}–{summary['max_seconds']:.4f}"
        notes = summary.get("first_failure") or summary.get("skip_reason") or ""
        resources = [stage.get("metric_summary", {}).get(key, {}).get("max")
                     for key in ("peak_rss_bytes", "peak_scratch_bytes")]
        lines.append("| " + " | ".join([cell(stage["name"]), str(summary.get("successful_samples", 0)),
                      "—" if median is None else f"{median:.4f}", spread,
                      *("—" if value is None else f"{value / 1024**2:.2f}" for value in resources), cell(notes)]) + " |")
    lines += ["", "计时包含新进程启动至退出并读完响应字节；JSON 解析、摘要准备及结果检查在计时之外。",
              "预热独立记录；失败和跳过不进入成功中位数。没有 p95/p99 或跨机器加速比。",
              "输入、profile、参数和 Shape 选择来自原始请求；脚本仅管理新产物、快照摘要、请求 ID、预算及路径解析。",
              "`.rq` 准入不会自动物化 CONSTRUCT；derived_queries 仍只在其请求内生效。",
              "查询默认 asserted 不会自动改为 closure；SHACL 的 conforms=false 可为正常违规结果，空校验范围也需核对。", "",
              "`summary.json` 保留全部样本、阶段指标、事实计数、产物统计和失败原因。",
              "`requests/` 保留本地原始请求/响应/日志；`inputs/` 保留原始模板；`artifacts/` 默认保留首个 compiled/reasoned。",
              "资源列为成功正式样本中 API 上报的最高值；失败样本的已采集指标仍保存在 JSON 中。",
              "`results.zip` 打包分享版 summary.json / report.md 和环境、身份文件；分享版排除错误全文、stderr 和协议诊断文本。",
              "原始请求/响应、RDF 和 SQLite 不入包；路径、用例名、文件摘要及构建环境仍作为元数据保留。脚本不会上传文件。", ""]
    if report.get("error"):
        lines += ["错误：" + cell(report["error"]), ""]
    return "\n".join(lines)


def write_outputs(directory, report):
    (directory / "report.md").write_text(render_report(report), encoding="utf8")
    core.publish(directory / "summary.json", report)
    shared = sharing_summary(report)
    temporary = directory / "results.zip.tmp"
    with zipfile.ZipFile(temporary, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("summary.json", core.canonical(shared) + b"\n")
        archive.writestr("report.md", render_report(shared))
        for name in ("environment.json", "identity.json"):
            path = directory / name
            if path.exists():
                archive.write(path, name)
    temporary.replace(directory / "results.zip")


def positive_int(value):
    result = int(value)
    if result <= 0:
        raise argparse.ArgumentTypeError("must be positive")
    return result


def nonnegative_int(value):
    result = int(value)
    if result < 0:
        raise argparse.ArgumentTypeError("must not be negative")
    return result


def positive_float(value):
    result = float(value)
    if not math.isfinite(result) or result <= 0:
        raise argparse.ArgumentTypeError("must be finite and positive")
    return result


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/ontokiln")
    parser.add_argument("--compile-request", type=Path, required=True, help="existing real API v1 compile request JSON")
    parser.add_argument("--read-requests", type=Path, help="directory of query/validate API v1 JSON templates, recursively")
    parser.add_argument("--read-dataset", choices=("compiled", "reasoned"), default="reasoned")
    parser.add_argument("--input-cwd", type=Path, default=Path.cwd(), help="base for relative input paths (default: invocation cwd)")
    parser.add_argument("--work-dir", type=Path, default=Path("data/business-baseline"))
    parser.add_argument("--build-samples", type=positive_int, default=3)
    parser.add_argument("--read-samples", type=positive_int, default=3)
    parser.add_argument("--warmups", type=nonnegative_int, default=1)
    parser.add_argument("--threads", type=positive_int, default=1)
    parser.add_argument("--keep-all-artifacts", action="store_true")
    parser.add_argument("--memory-mib", type=positive_int, help="explicitly override each template's memory budget")
    parser.add_argument("--scratch-mib", type=positive_int, help="explicitly override each template's scratch budget")
    parser.add_argument("--timeout-seconds", type=positive_float, help="explicitly override each template's timeout")
    parser.add_argument("--max-iterations", type=nonnegative_int)
    parser.add_argument("--max-derived-facts", type=nonnegative_int)
    parser.add_argument("--max-intermediate-rows", type=nonnegative_int)
    parser.add_argument("--build-profile", default="unspecified", help="caller declaration, not inferred from binary filename")
    parser.add_argument("--build-features", default="unspecified", help="caller declaration, not inferred from binary filename")
    args = parser.parse_args(argv)
    if not (sys.platform.startswith("linux") or sys.platform == "darwin"):
        parser.error("this benchmark requires Linux or macOS (POSIX process groups and wait4)")
    args.binary = args.binary.resolve()
    args.input_cwd = args.input_cwd.resolve()
    args.work_dir = args.work_dir.resolve()
    return args


def main(argv=None):
    args = parse_args(argv)
    args.work_dir.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix="run-", dir=args.work_dir)).resolve()
    report = {"schema_version": SCHEMA, "status": "RUNNING", "started_at_utc": now(),
              "business_correctness": "MANUAL_REVIEW_REQUIRED", "run_directory": str(directory),
              "command": sys.argv if argv is None else [str(Path(__file__)), *argv],
              "configuration": {k: str(v) if isinstance(v, Path) else v for k, v in vars(args).items()},
              "scope": "Real API v1 file requests. Protocol/identity/completeness checks only; business answers require independent review.",
              "cache_policy": "No OS cache flush. Input hashing and independent compile processes may warm OS caches. No resident API session.",
              "timing_policy": "Process creation through exit and reading response bytes; JSON decoding, hashes and checks excluded. Watchdog = API timeout + 5 s.",
              "statistics": "Verified COMPLETE executions only; warm-ups, failures and skipped samples remain separate. No tail quantiles or speedup claims.",
              "concurrency": 1, "stages": [],
              "method": {"driver_sha256": core.digest_file(Path(__file__)),
                         "helper_sha256": core.digest_file(ROOT / "benchmarks/kernel_baseline.py")}}
    core.publish(directory / "summary.json", report)
    print("Run directory: " + str(directory), flush=True)
    runner, code = None, 0
    interrupted_signal = None

    def interrupt(signum, _frame):
        nonlocal interrupted_signal
        interrupted_signal = signum
        # Finish the existing timed_command process-group cleanup and final
        # checkpoint once, even if a terminal sends a second interrupt.
        for sig in (signal.SIGINT, signal.SIGTERM):
            signal.signal(sig, signal.SIG_IGN)
        raise KeyboardInterrupt

    previous_handlers = {sig: signal.signal(sig, interrupt) for sig in (signal.SIGINT, signal.SIGTERM)}
    try:
        require(args.input_cwd.is_dir(), "input-cwd is not a directory")
        overrides = {}
        for option, field, scale in [("memory_mib", "memory_bytes", 1024**2),
                                      ("scratch_mib", "scratch_bytes", 1024**2),
                                      ("timeout_seconds", "timeout_ms", 1000),
                                      ("max_iterations", "max_iterations", 1),
                                      ("max_derived_facts", "max_derived_facts", 1),
                                      ("max_intermediate_rows", "max_intermediate_rows", 1)]:
            if getattr(args, option) is not None:
                overrides[field] = getattr(args, option) * scale
        compiled = load_template(args.compile_request, {"compile"}, args.input_cwd, overrides)
        cases = []
        if args.read_requests is not None:
            require(args.read_requests.is_dir(), "read-requests must be a directory containing only query/validate JSON templates")
            paths = sorted(args.read_requests.rglob("*.json"))
            require(paths, "read-requests directory contains no JSON templates")
            for path in paths:
                cases.append((path.relative_to(args.read_requests).as_posix(),
                              load_template(path, {"query", "validate"}, args.input_cwd, overrides)))
        inputs = directory / "inputs"
        inputs.mkdir()
        (inputs / "compile.request.json").write_bytes(compiled["original_bytes"])
        for i, (_, template) in enumerate(cases, 1):
            (inputs / f"read-{i:04d}.request.json").write_bytes(template["original_bytes"])
        require(args.binary.is_file() and os.access(args.binary, os.X_OK), "binary is missing or not executable; build/install it before measurement")
        environment = core.environment_info(args.binary, directory, args)
        require(isinstance(environment.get("kernel_identity"), dict) and environment["kernel_identity"],
                "binary identity command did not return a valid kernel identity")
        report["environment"] = environment
        report["read_case_count"] = len(cases)
        report["budget_overrides"] = overrides
        core.publish(directory / "environment.json", environment)
        core.publish(directory / "identity.json", environment["kernel_identity"])
        runner = Runner(args, directory, report, compiled, cases)
        runner.run()
        report["binary_sha256_after"] = core.digest_file(args.binary)
        require(report["binary_sha256_after"] == environment["binary_sha256"], "binary changed during the benchmark")
        failed = any(s["status"] != "COMPLETE" for stage in report["stages"] for s in stage["samples"])
        report["status"] = "COMPLETE_WITH_FAILURES" if failed else "COMPLETE"
        code = 1 if failed else 0
    except KeyboardInterrupt:
        report.update(status="INTERRUPTED", interrupt_signal=signal.Signals(interrupted_signal or signal.SIGINT).name,
                      error="runner interrupted; active API command process group terminated and supervisor waited; worker_reaped is not asserted without an API receipt")
        code = 128 + (interrupted_signal or signal.SIGINT)
    except (ValueError, OSError, TypeError, KeyError) as error:
        report.update(status="SETUP_FAILED" if runner is None else "HARNESS_FAILED",
                      error={"type": type(error).__name__, "message": str(error)})
        code = 2 if runner is None else 1
    except Exception as error:
        report.update(status="HARNESS_FAILED", error={"type": type(error).__name__, "message": str(error)})
        (directory / "runner-error.log").write_text(traceback.format_exc(), encoding="utf8")
        code = 1
    finally:
        report["finished_at_utc"] = now()
        for stage in report["stages"]:
            for sample in stage["samples"]:
                if sample["status"] in ("PENDING", "RUNNING"):
                    sample.update(status="SKIPPED" if sample["status"] == "PENDING" else "INTERRUPTED",
                                  reason="run ended before this sample completed")
        try:
            if runner is not None:
                runner.checkpoint()
            write_outputs(directory, report)
        finally:
            for sig, handler in previous_handlers.items():
                signal.signal(sig, handler)
    print(f"{report['status']}: {directory / 'summary.json'}", flush=True)
    print("Return results: " + str(directory / "results.zip"), flush=True)
    return code


if __name__ == "__main__":
    sys.exit(main())
