//! Common RDF representation for online hierarchy and offline rule witnesses.
use crate::Source;
use oxrdf::Literal;
use sha2::{Digest, Sha256};
use std::io::{self, Write};
const NS: &str = "urn:ontokiln:proof:";
pub type Terms<'a> = [&'a str; 3];
fn key(prefix: &str, parts: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        let b = part.as_ref().as_bytes();
        hash.update((b.len() as u64).to_be_bytes());
        hash.update(b);
    }
    format!("<{NS}{prefix}:{:x}>", hash.finalize())
}
pub struct ProofGraph<'a, W> {
    out: W,
    version: &'a str,
    revision: Option<&'a str>,
}
impl<'a, W: Write> ProofGraph<'a, W> {
    pub fn new(out: W, version: &'a str, revision: Option<&'a str>) -> Self {
        Self {
            out,
            version,
            revision,
        }
    }
    fn id(&self, [s, p, o]: Terms<'_>) -> String {
        key("fact", [self.version, self.revision.unwrap_or(""), s, p, o])
    }
    pub fn record<'s>(
        &mut self,
        fact: Terms<'_>,
        rule: &str,
        premises: &[Terms<'_>],
        sources: impl IntoIterator<Item = &'s Source>,
    ) -> io::Result<()> {
        let id = self.id(fact);
        for (p, t) in ["subject", "predicate", "object"].into_iter().zip(fact) {
            writeln!(self.out, "{id} <{NS}{p}> {t} .")?
        }
        writeln!(self.out, "{id} <{NS}rule> {} .", Literal::from(rule))?;
        writeln!(
            self.out,
            "{id} <{NS}ruleVersion> {} .",
            Literal::from(self.version)
        )?;
        for &p in premises {
            let premise = self.id(p);
            writeln!(self.out, "{id} <{NS}premise> {premise} .")?
        }
        for d in sources {
            let source = key(
                "source",
                [
                    d.file.as_str(),
                    &d.sha256,
                    d.revision.as_deref().unwrap_or(""),
                ],
            );
            writeln!(self.out, "{id} <{NS}source> {source} .")?;
            for (p, v) in [
                ("file", Some(d.file.as_str())),
                ("sha256", Some(&d.sha256)),
                ("module", Some(&d.module)),
                ("revision", d.revision.as_deref()),
                ("sourceFile", d.source_file.as_deref()),
                ("sourceSha256", d.source_sha256.as_deref()),
            ] {
                if let Some(v) = v {
                    writeln!(self.out, "{source} <{NS}{p}> {} .", Literal::from(v))?
                }
            }
        }
        Ok(())
    }
    pub fn finish(mut self) -> io::Result<()> {
        self.out.flush()
    }
}
