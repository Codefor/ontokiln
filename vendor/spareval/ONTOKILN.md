# Local changes to spareval 0.2.9

Base: the published spareval 0.2.9 crate from [Oxigraph](https://docs.rs/crate/spareval/0.2.9/source/), under the included [MIT license](LICENSE-MIT). Cargo selects this copy through the workspace patch.

* In `src/eval.rs`, probe a right-hand quad pattern with each left tuple when the optimizer has found nonempty join keys. Other joins retain the upstream algorithm. Preserve bag multiplicity and variable scopes when changing this path.
* `src/expression.rs` preserves RDF datatype/lexical identity in term-sensitive functions and evaluates unbounded XML Schema integer comparisons and basic arithmetic using `num-bigint`. The dataset and evaluator contexts expose original RDF terms for this purpose.
* `src/metrics.rs` and plan serialization support optional inclusive candidate/allocation counters and bounded intermediate binding samples. Without full profiling, no per-iterator timers or samples are collected.

When upgrading, reapply only these differences. Run the graph/scope, real HTTP, and 100M aggregate regressions described in the repository's AGENTS.md.

Upgrade: rebased the existing join-probe and metrics changes onto the published 0.2.9 crate with a three-way merge against published 0.2.6. No application-specific evaluator patches are included.
