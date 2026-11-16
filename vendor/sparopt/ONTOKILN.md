# Local changes to sparopt 0.3.9

Base: published [Sparopt 0.3.9](https://docs.rs/crate/sparopt/0.3.9/source/) from Oxigraph. Original crate SHA-256: `8c1d1327a49bef5f6f294a2ddcf01635c5449a0654e9a35d3f50d7a45e4ecc78`. The included MIT license is one of the upstream dual-license options. Sources and the normalized manifest come from that archive; only the changes below are local.

* Add `Optimizer::optimize_graph_pattern_with_initial_variables`; retain the original method as the empty-input entry point. Initial variable names seed guaranteed-bound, otherwise unrestricted RDF term types in normalization, join ordering and filter pushing. Exact values remain in Spareval's runtime binding map. BOUND may be folded only where the initial value is in scope; no concrete datatype is inferred. Existing projection/template admission keeps accepted initial variables available to the evaluator.
* Restrict input types to the projected variables in all three Project handlers, matching existing type inference and evaluator scope. Hidden variables do not receive an outer binding.
* When join ordering chooses a lateral RHS, replan that RHS with the LHS output types, matching the existing Lateral handler. Otherwise a nested RHS can scan an unbound leading pattern again for every left row even though a shared term is fixed. The existing lateral admission checks still apply.

Spareval has a direct relative dependency on this crate so an embedding workspace that already patches Spareval keeps this planner patch without relying on a transitive workspace patch. The root patch also unifies the crate in OntoKiln's own dependency graph. Keep the existing lockfile's Rand version.

When upgrading, compare against this exact published baseline and reapply only these changes. Run `tests/v4_bindings.rs`, explicit unoptimized differential cases, the complete Pack v4 gates and serial same-host bound-query benchmarks.
