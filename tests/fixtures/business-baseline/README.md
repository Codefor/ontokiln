# Business baseline native smoke fixture

This small synthetic repository exercises the same directory entry point used
for business workloads. It has nine explicit RDF facts: two records with one
type and one `p` value each, three NodeShape declarations, and two property
constraint declarations. The `datalog-v1` rule `p → q` adds exactly two facts;
the closure therefore contains eleven facts.

The independent expectations are written in
[business_native_smoke.py](../../benchmarks/business_native_smoke.py):

- SELECT returns exactly two ordered bindings, preserving the English language
  tag and the integer lexical form `007`.
- COUNT returns the integer eleven, and the explicit ASK is true.
- SHACL checks exactly `ex:Shape` with targets `ex:a` and `ex:b`, and reports
  no violations.
- The optional CONSTRUCT asset exercises the request-local probe path. Its
  `ASK {}` terminal result checks execution only, not the constructed graph's
  business meaning.

Every compile, materialize, query and validation request is executed by one
invocation of `benchmarks/business_baseline.py`. The smoke only launches that
driver and checks its retained responses; it never executes a second engine
loop or learns expected answers from a prior run. This is a functional CI check,
not a performance result. The run directory and an independent verification
record are retained under the smoke's work directory.
