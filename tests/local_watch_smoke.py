#!/usr/bin/env python3
"""Real resident process: failed watch rebuild retains the previous generation."""
import argparse
import json
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
import time


def main(binary):
    with tempfile.TemporaryDirectory(prefix="ontokiln-local-watch-") as directory:
        root = Path(directory); source = root / "rdf"; source.mkdir()
        data = source / "data.ttl"; data.write_text('<urn:a> <urn:p> <urn:old> .\n')
        query = root / "query.rq"; query.write_text('SELECT ?value WHERE { <urn:a> <urn:p> ?value }')
        process = subprocess.Popen([str(binary.resolve()), "--sources", str(source),
            "--pack-dir", str(root / "packs"), "--profile", "no-rules-v1",
            "--watch", "--poll-ms", "50"], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        streams = {"out": queue.Queue(), "err": queue.Queue()}
        def read(stream, target):
            for line in stream: target.put(line)
        for name, stream in [("out", process.stdout), ("err", process.stderr)]:
            threading.Thread(target=read, args=(stream, streams[name]), daemon=True).start()
        def wait_for(name, text=None):
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                line = streams[name].get(timeout=max(0.01, deadline - time.monotonic()))
                if text is None or text in line: return line
            raise AssertionError(f"missing {text}")
        def ask(expected):
            process.stdin.write(f"query {query}\n"); process.stdin.flush()
            reply = json.loads(wait_for("out"))
            assert reply["results"]["bindings"] == [{"value": {"type": "uri", "value": expected}}], reply
        try:
            wait_for("err", "ready:"); ask("urn:old")
            original = next((root / "packs").glob("*.pack")).read_bytes()
            data.write_text('broken {\n'); wait_for("err", "rebuild failed; retained current engine")
            ask("urn:old"); assert len(list((root / "packs").glob("*.pack"))) == 1
            data.write_text('<urn:a> <urn:p> <urn:new> .\n')
            wait_for("err", "replaced engine:"); ask("urn:new")
            packs = list((root / "packs").glob("*.pack")); assert len(packs) == 2
            assert original in [pack.read_bytes() for pack in packs]
            process.stdin.write("quit\n"); process.stdin.flush()
            assert process.wait(timeout=15) == 0
            print("PASS: resident queries, failed rebuild retention, successful full rebuild and Pack generations")
        finally:
            if process.poll() is None: process.kill(); process.wait(timeout=15)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(); parser.add_argument("--binary", type=Path, required=True)
    main(parser.parse_args().binary)
