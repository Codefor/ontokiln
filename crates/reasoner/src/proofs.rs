use super::{RULE_VERSION, Reasoner};
use anyhow::Result;
use ontokiln_pack::{Triple, proof::ProofGraph};
use std::io::Write;

impl Reasoner {
    /// Emit a deterministic breadth-first witness. A subject-scoped export
    /// also includes the schema assertions needed to replay its proof.
    pub fn export_proofs(
        &self,
        subject: Option<&str>,
        revision: Option<&str>,
        out: impl Write,
    ) -> Result<()> {
        self.check_revision(revision).map_err(anyhow::Error::msg)?;
        let mut graph = ProofGraph::new(out, RULE_VERSION, revision);
        let terms = |fact: Triple| fact.map(|v| self.pack.term(v));
        let mut cursor = 0;
        let subjects = std::iter::from_fn(|| {
            if let Some(s) = subject {
                if cursor > 0 {
                    return None;
                }
                cursor = 1;
                return self.pack.lookup(s);
            }
            let rest = &self.pack.triples()[cursor..];
            let s = rest.first()?[0];
            cursor += rest.partition_point(|t| t[0] == s);
            Some(s)
        });
        for s in subjects {
            let steps = self
                .derive(s, revision, true, &mut || Ok(()))
                .map_err(anyhow::Error::msg)?;
            for step in &steps {
                let row = self
                    .pack
                    .triples()
                    .binary_search(&step.premise)
                    .expect("asserted premise");
                let sources = self
                    .pack
                    .sources(row)
                    .filter(|d| revision.is_none_or(|r| d.revision.as_deref() == Some(r)));
                graph.record(terms(step.premise), "asserted", &[], sources)?;
                if let Some(prev) = step.previous {
                    graph.record(
                        terms(step.fact),
                        step.rule,
                        &[terms(steps[prev].fact), terms(step.premise)],
                        [],
                    )?;
                }
            }
        }
        graph.finish()?;
        Ok(())
    }
}
