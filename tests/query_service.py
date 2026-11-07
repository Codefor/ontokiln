"""Exercise worker lifetime across real HTTP disconnects and process exits."""
import concurrent.futures
import http.client
import json
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time

binary = Path(sys.argv[1])
root = Path(__file__).resolve().parents[1]

def run(tmp):
    ontology = tmp / "ontology"
    shutil.copytree(root/"examples/assignment/ontology", ontology)
    pack = tmp/"only.pack"
    subprocess.run([binary,"compile","--ttl",ontology,"--out",pack],check=True)
    code = ontology/"v2/model.go"
    code.write_text(code.read_text()+"// changed\n")
    failed = subprocess.run([binary,"compile","--ttl",ontology,"--out",pack],capture_output=True)
    assert failed.returncode and b"wrong source version" in failed.stderr
    shutil.rmtree(ontology)
    queries = tmp/"queries"
    shutil.copytree(root/"examples/assignment/queries",queries)
    (queries/"ask.rq").write_text("ASK { ?s ?p ?o }")
    (queries/"object.rq").write_text("SELECT ?s ?p { ?s ?p <urn:ontokiln:param:object> }")
    (queries/"typed.rq").write_text('SELECT ?value ?v WHERE { VALUES ?value { <urn:ontokiln:param:v> } {SELECT ("private" AS ?v) WHERE {}} }')
    values=" ".join(str(n) for n in range(150))
    product=" ".join("VALUES ?"+v+" { "+values+" }" for v in "abcd")
    for name,q in {
        "heavy":"SELECT (COUNT(*) AS ?n) WHERE { "+product+" }",
        "sort":"SELECT DISTINCT ?a ?b ?c WHERE { "+product+" } ORDER BY DESC(?c)",
        "stream":"SELECT ?a ?b ?c ?d WHERE { "+product+" }",
        "scan":"SELECT ?s ?p ?o WHERE { ?s ?p ?o }",
    }.items():
        (queries/(name+".rq")).write_text(q)
    server=subprocess.Popen([binary,"serve","--pack",pack,"--queries",queries,"--listen","127.0.0.1:0","--query-slots","1"],stderr=subprocess.PIPE,text=True)
    try:
        line=server.stderr.readline()
        assert "serving" in line,line
        port=int(line.rsplit(":",1)[1])
        def request(name, **args):
            opts={"revision":"v2",**args.pop("options",{})}
            conn=http.client.HTTPConnection("127.0.0.1",port,timeout=8)
            try:
                conn.request("POST","/v1/sparql",json.dumps({"template":name,"options":opts,**args}),{"Content-Type":"application/json"})
                res=conn.getresponse();raw=res.read()
                if res.getheader("content-type")=="application/x-ndjson":
                    return res.status,[json.loads(line) for line in raw.splitlines()]
                return res.status,json.loads(raw)
            finally:conn.close()
        for rev,field in [("v1","legacyCode"),("v2","displayName")]:
            status,body=request("assignment",parameters={"component":{"type":"iri","value":"urn:assignment:Panel"}},options={"revision":rev,"profile":True})
            assert status==200,body
            rows=body["result"]["results"]["bindings"]
            assert [(r["order"]["value"],r["stage"]["value"].split(":")[-1],r["input"]["value"].split(":")[-1]) for r in rows]==[("1","read",field),("2","write","load")]
            assert all(r["file"]["value"].startswith(rev+"/") for r in rows)
            profile=body["profile"]
            assert profile["candidate_rows"]>0 and profile["peak_heap_bytes"]>0
            plan=profile["execution"]["plan"]
            assert len(plan["sample_bindings"])==2 and plan["sample_bindings"][0]
            assert plan["allocated_bytes"]>0 and plan["candidate_rows"]>0 and plan["number of results"]==2
        assert request("ask",options={"revision":None})[0]==400
        assert request("ask",options={"revision":"bogus"})[0]==400
        for literal in [{"value":'quote" } UNION { ?s ?p ?o } #',"language":"fr"},
                        {"value":"42","datatype":"http://www.w3.org/2001/XMLSchema#integer"}]:
            status,body=request("typed",parameters={"v":{"type":"literal",**literal}})
            assert status==200,body
            row=body["results"]["bindings"][0]
            assert row["value"]["value"]==literal["value"] and row["v"]["value"]=="private"
            if "language" in literal: assert row["value"]["xml:lang"]==literal["language"]
            else: assert row["value"]["datatype"]==literal["datatype"]
        assert request("typed")[0]==400
        assert request("typed",parameters={"v":{"type":"iri","value":"invalid"}})[0]==400
        status, body=request("object",parameters={"object":{"type":"literal","value":"model"}},options={"profile":True})
        assert status==200 and body["profile"]["index_rows"]["OSP"]==1,body
        status,body=request("scan",options={"max_visits":1})
        assert status==422 and "attempted candidate 2, limit 1" in body["error"],body
        assert request("scan",options={"max_bytes":12})[0]==422
        assert request("scan",options={"max_rows":1})[0]==422
        status,records=request("scan",format="jsonl",options={"max_rows":1})
        assert status==200 and len(records)==2 and "bindings" in records[0] and records[1]["status"]==422,records
        status,records=request("scan",format="jsonl",options={"profile":True})
        assert status==200 and records[-1]["end"] and records[-1]["profile"]["output_rows"]==len(records)-1
        status,body=request("sort",memory_bytes=512*1024)
        assert status==422 and "heap budget" in body["error"],(status,body)
        assert request("heavy",deadline_ms=35)[0]==408
        def wait_status(wanted):
            end=time.monotonic()+1.5
            while time.monotonic()<end:
                conn=http.client.HTTPConnection("127.0.0.1",port)
                conn.request("GET","/health")
                slots=json.loads(conn.getresponse().read())["available_query_slots"]
                conn.close()
                if slots==int(wanted==200) and request("ask")[0]==wanted:return
                time.sleep(.01)
            raise AssertionError(f"expected {wanted} from sole query slot")
        # Cancel eager CPU-only work before it produces any HTTP body.
        with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
            task=pool.submit(request,"heavy",request_id="cancel-me");wait_status(429)
            conn=http.client.HTTPConnection("127.0.0.1",port)
            conn.request("DELETE","/v1/sparql/cancel-me");res=conn.getresponse()
            assert res.status==202;res.read();conn.close()
            assert task.result()[0]==409
        for template,streaming in [("heavy",False),("stream",True)]:
            sock=socket.create_connection(("127.0.0.1",port));sock.settimeout(2)
            data=json.dumps({"template":template,"format":"jsonl" if streaming else "json","options":{"revision":"v2","max_rows":100000,"max_bytes":4194304}}).encode()
            sock.sendall(b"POST /v1/sparql HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: "+str(len(data)).encode()+b"\r\n\r\n"+data)
            if streaming:
                buf=b""
                while b'"bindings"' not in buf:buf+=sock.recv(16384)
            wait_status(429)
            sock.close()
            wait_status(200)
    finally:
        server.terminate()
        server.wait(timeout=8)

def legacy_routes():
    server = subprocess.Popen([binary,"serve","--ttl",root/"examples/scenarios/ontology","--queries",root/"examples/queries","--listen","127.0.0.1:0"],stderr=subprocess.PIPE,text=True)
    try:
        for line in server.stderr:
            if "serving" in line:break
        port=int(line.rsplit(":",1)[1])
        def fetch(path, data=None):
            conn=http.client.HTTPConnection("127.0.0.1",port,timeout=8)
            try:
                conn.request("GET" if data is None else "POST",path,None if data is None else json.dumps(data),{"content-type":"application/json"})
                res=conn.getresponse()
                return res.status,json.loads(res.read())
            finally:conn.close()
        ns="urn:ontokiln:lab:"
        status, rows=fetch("/v1/sparql",{"template":"inputs","bindings":{"step":"<"+ns+"FormatAddress>"}})
        assert status==200 and [r["field"]["value"] for r in rows["results"]["bindings"]]==[ns+"Order.city",ns+"Order.detail"]
        assert fetch("/v1/sparql",{"template":"../inputs"})[0]==404
        assert fetch("/v1/sparql",{"template":"inputs","bindings":{"step":"not RDF"}})[0]==400
        ent={"types":[ns+"SourceField",ns+"GoField"],"subclasses":True,"limit":1}
        status,result=fetch("/v1/entities",ent)
        assert status==200 and result["total"]==7 and len(result["entities"])==1
        assert fetch("/v1/entities",dict(ent,memory_bytes=1))[0]==422
        batch={"subjects":["urn:missing",ns+"FormatAddress"],"predicate":ns+"input","explain":True}
        status,result=fetch("/v1/query/batch",batch)
        assert status==200 and not result[0]["results"] and len(result[1]["results"])==2
        assert result[1]["results"][0]["proof"][1]["rule"]=="inherited_property"
        assert fetch("/v1/query/batch",dict(batch,max_rows=1))[0]==422
        assert fetch("/v1/query?subject=urn:ontokiln:lab:FormatAddress&predicate=urn:ontokiln:lab:input&max_rows=1")[0]==422
    finally:
        server.terminate()
        server.wait(timeout=8)

legacy_routes()
with tempfile.TemporaryDirectory() as tmp:
    run(Path(tmp))
