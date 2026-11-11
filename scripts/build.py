#!/usr/bin/env python3
"""Build and verify a clean, pinned source tree on its native target."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    result = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024**2), b""):
            result.update(block)
    return result.hexdigest()


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def clean_revision():
    if git("status", "--porcelain"):
        raise ValueError("build requires committed source with no untracked files")
    revision = git("rev-parse", "HEAD")
    if not re.fullmatch("[0-9a-f]{40}", revision):
        raise ValueError("build requires a complete source revision")
    return revision


def native_target():
    arch = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64"}.get(platform.machine())
    system = {"Darwin": "apple-darwin", "Linux": "unknown-linux-musl"}.get(platform.system())
    if not arch or not system:
        raise ValueError("supported native hosts are macOS/Linux on arm64/x86_64")
    return f"{arch}-{system}"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--target", help="must match the native build host")
    args = parser.parse_args()
    revision = clean_revision()
    target = native_target()
    if args.target and args.target != target:
        parser.error(f"native validation requires {target}")
    output = args.output.resolve()
    if output.exists():
        parser.error("output already exists")
    version = re.search(r'^channel\s*=\s*"(\d+\.\d+\.\d+)"',
                        (ROOT / "rust-toolchain.toml").read_text(), re.M)
    if not version:
        raise ValueError("a fixed toolchain version is required")
    version = version[1]
    env = {**os.environ, "ONTOKILN_BUILD_REVISION": revision}
    if target.endswith("linux-musl"):
        if not shutil.which("musl-gcc"):
            raise ValueError("native Linux builds require musl-gcc for bundled SQLite")
        env["CC_" + target.replace("-", "_")] = "musl-gcc"
        env["CARGO_TARGET_" + target.replace("-", "_").upper() + "_LINKER"] = "cc"
    subprocess.run(["rustup", "toolchain", "install", version, "--profile", "minimal"], cwd=ROOT, env=env, check=True)
    subprocess.run(["rustup", "target", "add", "--toolchain", version, target], cwd=ROOT, env=env, check=True)
    target_dir = Path(env.get("CARGO_TARGET_DIR", ROOT / "target")).resolve()
    env["CARGO_TARGET_DIR"] = str(target_dir)
    binary = target_dir / target / "release/ontokiln"
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".ontokiln-build-", dir=output.parent) as folder:
        stage = Path(folder)
        checks = []
        for name, command in [
            ("workspace_tests", ["cargo", f"+{version}", "test", "--workspace", "--release", "--locked", "--target", target]),
            ("native_build", ["cargo", f"+{version}", "build", "--release", "--locked", "--target", target, "--bin", "ontokiln"]),
        ]:
            log = stage / f"{name}.log"
            with log.open("wb") as stream:
                result = subprocess.run(command, cwd=ROOT, env=env, stdout=stream, stderr=subprocess.STDOUT)
            if result.returncode:
                raise RuntimeError(f"{name} failed:\n" + "\n".join(log.read_text(errors="replace").splitlines()[-60:]))
            checks.append({"check": name, "status": "passed", "log_sha256": digest(log)})
        kernel = json.loads(subprocess.check_output([str(binary), "identity"], env=env))
        if kernel["source_revision"] != revision or kernel["contract_version"] != "ontokiln-core/v3":
            raise ValueError("built engine identity does not match the source")
        archive = stage / f"ontokiln-{target}.tar.gz"
        with archive.open("wb") as raw, gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=0) as zipped:
            with tarfile.open(fileobj=zipped, mode="w") as tar:
                member = tarfile.TarInfo("ontokiln")
                member.size, member.mode = binary.stat().st_size, 0o755
                with binary.open("rb") as stream:
                    tar.addfile(member, stream)
        receipt = {"schema_version": "ontokiln-build/v1", "source_revision": revision,
            "kernel": kernel, "target": target, "toolchain": version,
            "cargo_lock_sha256": digest(ROOT / "Cargo.lock"),
            "build_entry_sha256": digest(Path(__file__)),
            "rustc": subprocess.check_output(["rustc", f"+{version}", "-Vv"], env=env, text=True).strip(),
            "rustflags": env.get("RUSTFLAGS", ""), "encoded_rustflags": env.get("CARGO_ENCODED_RUSTFLAGS", ""),
            "asset": {"filename": archive.name, "sha256": digest(archive), "binary_sha256": digest(binary)},
            "native_validation": checks}
        (stage / "build.json").write_text(json.dumps(receipt, indent=2) + "\n")
        if clean_revision() != revision:
            raise ValueError("source changed during native validation")
        stage.rename(output)
    print(json.dumps({"status": "native_verified", "receipt": str(output / "build.json")}))


if __name__ == "__main__":
    main()
