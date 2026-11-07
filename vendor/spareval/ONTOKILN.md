# Local changes to spareval 0.2.6

Base: the published spareval 0.2.6 crate from [Oxigraph](https://github.com/oxigraph/oxigraph/tree/45e892308368d8b2c271d06dc04de17dfb6de80f/lib/spareval), under the included [MIT license](LICENSE-MIT). Cargo selects this copy through the workspace patch.

* In `src/eval.rs`, probe a right-hand quad pattern with each left tuple when the optimizer has found nonempty join keys. Other joins retain the upstream algorithm. Preserve bag multiplicity and variable scopes when changing this path.
* `src/metrics.rs` and plan serialization support optional inclusive candidate/allocation counters and bounded intermediate binding samples. Without full profiling, no per-iterator timers or samples are collected.

When upgrading, reapply only these differences. Run the graph/scope, real HTTP, and 100M aggregate regressions described in the repository's AGENTS.md.
