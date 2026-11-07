#!/usr/bin/env python3
"""Compare three SPARQL aggregates at three sizes on the v3 release server."""
import argparse
from contextlib import closing
import http.client
import os
from pathlib import Path
import tempfile

import capabilities as cap
import run as bench
from hundred_million import PANELS, TRIPLES


def measure(root, count, existing):
    if not existing:
        root.mkdir()
        pack, triples, templates = cap.prepare(root, count)
    else:
        pack, templates = root/"panels.pack", root/"queries"
        triples = 1095 + 4*count + len(range(0,count,4)) + len(range(0,count,5))
    if count == PANELS and triples != TRIPLES:
        raise ValueError("incorrect fixture size")
    results = []
    point = []
    point_profile = None
    with bench.serve("pack",pack,triples,queries=templates) as (port,_):
        with closing(http.client.HTTPConnection("127.0.0.1",port,timeout=15)) as conn:
            for i in range(17):
                panel = (i * 97) % count
                req = {"template":"bound","bindings":{"panel":"<"+bench.NS+"v"+str(panel)+">"}}
                ns, status, body = cap.post(conn, "/v1/sparql", req)
                rows = body.get("results",{}).get("bindings",[])
                if status != 200 or len(rows)!=1 or rows[0]["field"]["value"] != bench.NS+"f"+str(panel%4096) or rows[0]["region"]["value"] != bench.NS+"r"+str(panel%8):
                    raise ValueError(("point",panel,status,body))
                if i: point.append(ns/1e6)
            _,status,point_profile = cap.post(conn,"/v1/sparql",dict(req,options={"profile":True}))
            if status!=200: raise ValueError(point_profile)
            for name, step in [("premium",4),("intersection",20),("exists",20)]:
                expected = len(range(0,count,step))
                row = cap.slow_sparql(port,name,expected)
                ns,status,body = cap.post(conn,"/v1/sparql",{"template":name,"options":{"counters":True}})
                if status == 200:
                    answer = int(body["result"]["results"]["bindings"][0]["n"]["value"])
                    if answer != expected:
                        raise ValueError((name,answer,expected))
                row["counter_run"] = {"ms":ns/1e6,"status":status,"body":body}
                results.append(row)
                print(triples,name,[r["status"] for r in row["runs"]],row["counter_run"],flush=True)
    return {"subjects":count,"triples":triples,"pack_bytes":pack.stat().st_size,"queries":results,"point_ms":point,"point_profile":point_profile["profile"]}


def main():
    args = argparse.ArgumentParser(description=__doc__)
    args.add_argument("--output",type=Path,required=True)
    args.add_argument("--existing",type=Path,help="Reuse fixture directories named by panel count")
    args = args.parse_args()
    os.environ.setdefault("ONTOKILN_MAX_PACK_MIB","4096")
    report = dict(bench.environment(),features=["ascent-engine"],deadline_ms=5000,max_visits=200_000_000,datasets=[])
    with tempfile.TemporaryDirectory(prefix="ontokiln-v3-") as tmp:
        for count in (250_000,2_500_000,PANELS):
            root=(args.existing or Path(tmp))/str(count)
            report["datasets"].append(measure(root,count,args.existing is not None))
    bench.publish(args.output,report)


if __name__ == "__main__":
    main()
