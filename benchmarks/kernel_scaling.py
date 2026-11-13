#!/usr/bin/env python3
"""Run only Full compile/materialize using the frozen 46e0fb5 measurement code.

Pass --harness PATH from a checkout of 46e0fb5 and --build-receipt PATH, followed
by kernel_baseline.py options. This is a separate scaling study with explicit
budgets, not an automatically comparable replacement for the 512 MiB baseline.
"""
import argparse
import datetime
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile


FROZEN_SHA256 = "9b69d22115f1177a7c3d7c0fe39b7c9dbbd238c95983b0cce73b6be230f662c8"
STAGES = {"full_compile", "full_materialize"}


def load_harness(path):
    path = path.resolve()
    if hashlib.sha256(path.read_bytes()).hexdigest() != FROZEN_SHA256:
        raise ValueError("harness does not match frozen 46e0fb5 measurement code")
    spec = importlib.util.spec_from_file_location("frozen_kernel_baseline", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def verify_identity(environment, receipt):
    if environment["binary_sha256"] != receipt["binary_sha256"]:
        raise ValueError("binary SHA-256 does not match build receipt")
    if environment["kernel_identity"] != receipt["kernel"]:
        raise ValueError("kernel identity does not match build receipt")


def build_only_class(base):
    class FullBuildBaseline(base):
        def series(self, scale, name, count, invoke, **options):
            if name in STAGES:
                return super().series(scale, name, count, invoke, **options)
            # Do not invoke pack compile, queries or validation. Unselected
            # workloads are outside this study, not failed or skipped trials.
            return None
    return FullBuildBaseline


def main(argv=None):
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--harness", type=Path, required=True)
    parser.add_argument("--build-receipt", type=Path, required=True)
    selected, remaining = parser.parse_known_args(argv)
    module = load_harness(selected.harness)
    args = module.parse_args(remaining)
    receipt = json.loads(selected.build_receipt.read_text())
    if args.work_dir:
        args.work_dir.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix="ontokiln-kernel-scaling-", dir=args.work_dir)).resolve()
    environment = module.environment_info(args.binary, directory, args)
    try:
        verify_identity(environment, receipt)
    except (KeyError, ValueError):
        shutil.rmtree(directory)
        raise
    driver = Path(__file__).resolve()
    report = {
        "schema_version": "ontokiln-kernel-scaling/v1", "status": "RUNNING",
        "started_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "command": sys.argv,
        "configuration": {k: str(v) if isinstance(v, Path) else v for k, v in vars(args).items()},
        "phase_selection": sorted(STAGES),
        "method": {"harness_source_revision": "46e0fb5b6089e9f5bacb8494437c956e21e936a2",
                   "harness_path": str(selected.harness.resolve()), "harness_sha256": FROZEN_SHA256,
                   "driver_sha256": hashlib.sha256(driver.read_bytes()).hexdigest(),
                   "build_receipt_sha256": hashlib.sha256(selected.build_receipt.read_bytes()).hexdigest(),
                   "stage_filter": "Only full_compile/full_materialize invoke the frozen dispatcher; all other workloads are outside scope."},
        "build_receipt": receipt, "work_directory": str(directory),
        "metric_definitions": module.METRIC_NOTES,
        "cache_policy": "No OS cache flush. Generation and hashing precede independent compile/materialize processes; no persistent engine cache.",
        "concurrency": 1,
        "statistics": "Three planned independent builds per stage by default; medians/ranges use verified COMPLETE samples. Failures stop remaining repeats.",
        "scope": "Full compilation and bounded positive-rule materialization only; manifest counts are checked, full fact content requires a separate read-only audit. Not a query/SHACL or arbitrary recursive-closure capacity claim.",
        "environment": environment, "scales": [],
    }
    runner = build_only_class(module.Baseline)(args, directory, report)
    runner.checkpoint()
    code = 0
    try:
        for facts in args.sizes:
            runner.scale(facts)
        failed = any(s["status"] != "COMPLETE" for scale in report["scales"]
                     for stage in scale["stages"] for s in stage["samples"])
        report["status"] = "COMPLETE_WITH_FAILURES" if failed else "COMPLETE"
        code = 1 if failed else 0
    except KeyboardInterrupt:
        report["status"], code = "INTERRUPTED", 130
    except Exception as error:
        report.update(status="HARNESS_FAILED", fatal_error={"type": type(error).__name__, "message": str(error)})
        code = 1
    finally:
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
