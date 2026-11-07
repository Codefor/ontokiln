#!/usr/bin/env python3
"""Sample parent and child RSS around the fixed 100M COUNT. macOS/Linux only."""
import argparse
import concurrent.futures
from contextlib import closing
import http.client
import os
from pathlib import Path
import subprocess
import tempfile
import time

import capabilities as cap
import run as bench
from hundred_million import PANELS, TRIPLES


def snapshot(pid):
    rows = subprocess.check_output(["ps", "-axo", "pid=,ppid=,rss="], text=True)
    parent = None
    workers = {}
    for line in rows.splitlines():
        proc, ppid, kib = map(int, line.split())
        if proc == pid:
            parent = kib * 1024
        elif ppid == pid:
            workers[proc] = kib * 1024
    if parent is None:
        raise RuntimeError("parent process disappeared")
    return parent, workers


def measure(root):
    report = dict(bench.environment(), subjects=PANELS, triples=TRIPLES, max_visits=200_000_000,
                  deadline_ms=5000, features=["ascent-engine"], batches=[])
    with bench.serve("pack", root/"panels.pack", TRIPLES, queries=root/"queries", include_pid=True) as (port, startup):
        pid = startup.pop("pid")
        def request():
            with closing(http.client.HTTPConnection("127.0.0.1",port,timeout=10)) as conn:
                ns, code, body = cap.post(conn,"/v1/sparql",{"template":"intersection"})
                if code==200 and int(body["results"]["bindings"][0]["n"]["value"])!=1_123_584:
                    raise ValueError(body)
                return {"ms":ns/1e6,"status":code,"body":body}
        report["ready_parent_rss_bytes"]=snapshot(pid)[0]
        for concurrency in [1,2]:
            parent, child = snapshot(pid)
            if child: raise ValueError("workers active before test")
            peak_parent=parent
            peak_child={}
            samples=0
            with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as pool:
                futures=[pool.submit(request) for _ in range(concurrency)]
                while any(not f.done() for f in futures):
                    rss, workers=snapshot(pid)
                    peak_parent=max(rss,peak_parent)
                    for proc, value in workers.items():peak_child[proc]=max(peak_child.get(proc,0),value)
                    samples+=1
                    time.sleep(.05)
                answers=[f.result() for f in futures]
            after, remaining=snapshot(pid)
            if remaining:raise ValueError("worker not reaped")
            report["batches"].append(dict(concurrency=concurrency, samples=samples, results=answers,
                parent_before_bytes=parent,parent_peak_bytes=peak_parent,parent_after_bytes=after,
                peak_rss_per_worker_bytes=sorted(peak_child.values()),remaining_workers=len(remaining)))
    report["startup"]=startup
    return report


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--existing",type=Path,help="Directory containing panels.pack and queries/")
    parser.add_argument("--output",type=Path,required=True)
    args=parser.parse_args()
    os.environ.setdefault("ONTOKILN_MAX_PACK_MIB","4096")
    with tempfile.TemporaryDirectory(prefix="ontokiln-worker-memory-") as tmp:
        root=args.existing or Path(tmp)
        if not args.existing:cap.prepare(root,PANELS)
        bench.publish(args.output,measure(root))


if __name__=="__main__":
    main()
