//! Subject-bound forward chaining over the immutable SPO index.
mod entities;
pub mod parameters;
mod proofs;
pub mod sparql;
pub use entities::EntityMatches;
use ontokiln_pack::Source;
use ontokiln_pack::{Id, Pack, Triple};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::OnceLock;

pub const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
pub const SUB_CLASS: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
pub const SUB_PROPERTY: &str = "<http://www.w3.org/2000/01/rdf-schema#subPropertyOf>";
const MAX_WORK: usize = 100_000;
const MAX_RESULT_TEXT: usize = 1_048_576;

pub const RULE_VERSION: &str = "rdfs-hierarchy-v1";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    Asserted,
    Inferred,
    #[default]
    Closure,
}
pub struct Reasoner {
    types: OnceLock<entities::TypeIndex>,
    pack: Pack,
    rdf_type: Option<Id>,
    sub_class: Option<Id>,
    sub_property: Option<Id>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Answer {
    pub object: String,
    pub inferred: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof: Option<Vec<ProofStep>>,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct ProofStep {
    pub rule: &'static str,
    // An asserted triple from the pack. Later steps apply it to the previous result.
    pub premise: [String; 3],
    pub conclusion: [String; 3],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_fact: Option<[String; 3]>,
    pub sources: Vec<Source>,
    pub rule_version: &'static str,
}

struct Step {
    fact: Triple,
    previous: Option<usize>,
    premise: Triple,
    rule: &'static str,
}

impl Reasoner {
    pub fn new(pack: Pack) -> Self {
        Self {
            types: OnceLock::new(),
            rdf_type: pack.lookup(RDF_TYPE),
            sub_class: pack.lookup(SUB_CLASS),
            sub_property: pack.lookup(SUB_PROPERTY),
            pack,
        }
    }
    pub fn triple_count(&self) -> usize {
        self.pack.triple_count()
    }
    pub fn term_count(&self) -> usize {
        self.pack.term_count()
    }

    /// Keys use N-Triples notation. An unknown term yields an empty result.
    pub fn query(
        &self,
        subject: &str,
        predicate: &str,
        explain: bool,
    ) -> Result<Vec<Answer>, &'static str> {
        self.query_view(subject, predicate, explain, View::Closure, None)
    }
    pub fn snapshot_file(&self) -> Option<&std::fs::File> {
        self.pack.snapshot_file()
    }
    pub fn documents(&self) -> &[Source] {
        self.pack.documents()
    }
    pub fn check_revision(&self, revision: Option<&str>) -> Result<(), &'static str> {
        if let Some(rev) = revision {
            if !self
                .documents()
                .iter()
                .any(|d| d.revision.as_deref() == Some(rev))
            {
                return Err("unknown source revision");
            }
        } else if self.documents().iter().any(|d| d.revision.is_some()) {
            return Err("source revision required");
        }
        Ok(())
    }
    fn visible(&self, row: usize, revision: Option<&str>) -> bool {
        revision.is_none_or(|v| {
            self.pack
                .sources(row)
                .any(|d| d.revision.as_deref() == Some(v))
        })
    }
    pub fn query_view(
        &self,
        subject: &str,
        predicate: &str,
        explain: bool,
        view: View,
        revision: Option<&str>,
    ) -> Result<Vec<Answer>, &'static str> {
        self.query_counted(subject, predicate, explain, view, revision, &mut || Ok(()))
    }
    pub fn query_counted(
        &self,
        subject: &str,
        predicate: &str,
        explain: bool,
        view: View,
        revision: Option<&str>,
        check: &mut impl FnMut() -> Result<(), &'static str>,
    ) -> Result<Vec<Answer>, &'static str> {
        self.check_revision(revision)?;
        let (Some(s), Some(wanted)) = (self.pack.lookup(subject), self.pack.lookup(predicate))
        else {
            return Ok(Vec::new());
        };
        let steps = self.derive(s, revision, view != View::Asserted, check)?;
        let mut answers = Vec::new();
        let mut text_left = MAX_RESULT_TEXT;
        for (index, step) in steps.iter().enumerate().filter(|(_, step)| {
            step.fact[1] == wanted && (view != View::Inferred || step.previous.is_some())
        }) {
            let object = self.pack.term(step.fact[2]);
            charge(&mut text_left, object.len())?;
            let proof = if explain {
                let mut chain = Vec::new();
                let mut at = Some(index);
                while let Some(i) = at {
                    let link = &steps[i];
                    let terms = link.premise.map(|id| self.pack.term(id));
                    charge(
                        &mut text_left,
                        terms.iter().map(|t| t.len()).sum::<usize>()
                            + link.rule.len()
                            + RULE_VERSION.len(),
                    )?;
                    let strings = |fact: Triple| fact.map(|id| self.pack.term(id));
                    let conclusion = strings(link.fact);
                    let previous = link.previous.map(|index| strings(steps[index].fact));
                    charge(
                        &mut text_left,
                        conclusion
                            .iter()
                            .chain(previous.iter().flatten())
                            .map(|s| s.len())
                            .sum(),
                    )?;
                    let mut sources = Vec::new();
                    let row = self
                        .pack
                        .triples()
                        .binary_search(&link.premise)
                        .expect("asserted premise");
                    for source in self
                        .pack
                        .sources(row)
                        .filter(|d| revision.is_none_or(|v| d.revision.as_deref() == Some(v)))
                    {
                        charge(
                            &mut text_left,
                            source.file.len()
                                + source.module.len()
                                + source.sha256.len()
                                + source.revision.as_ref().map_or(0, String::len)
                                + source.source_file.as_ref().map_or(0, String::len)
                                + source.source_sha256.as_ref().map_or(0, String::len),
                        )?;
                        sources.push(source.clone());
                    }
                    chain.push(ProofStep {
                        rule_version: RULE_VERSION,
                        sources,
                        conclusion: conclusion.map(str::to_owned),
                        previous_fact: previous.map(|t| t.map(str::to_owned)),
                        rule: link.rule,
                        premise: terms.map(str::to_owned),
                    });
                    at = link.previous;
                }
                chain.reverse();
                Some(chain)
            } else {
                None
            };
            answers.push(Answer {
                object: object.to_owned(),
                inferred: step.previous.is_some(),
                proof,
            });
        }
        answers.sort_unstable_by(|a, b| a.object.cmp(&b.object));
        Ok(answers)
    }
    /// Evaluate independent traversals in parallel, preserving the input order.
    pub fn query_batch(
        &self,
        subjects: &[String],
        predicate: &str,
        explain: bool,
    ) -> Result<Vec<Vec<Answer>>, &'static str> {
        self.query_batch_view(subjects, predicate, explain, View::Closure, None)
    }
    pub fn query_batch_view(
        &self,
        subjects: &[String],
        predicate: &str,
        explain: bool,
        view: View,
        revision: Option<&str>,
    ) -> Result<Vec<Vec<Answer>>, &'static str> {
        if !(1..=16).contains(&subjects.len()) {
            return Err("expected 1..16 subjects");
        }
        subjects
            .par_iter()
            .map(|s| self.query_view(s, predicate, explain, view, revision))
            .collect()
    }

    pub fn batch_counted(
        &self,
        subjects: &[String],
        predicate: &str,
        explain: bool,
        view: View,
        revision: Option<&str>,
        max_visits: usize,
    ) -> Result<Vec<Vec<Answer>>, &'static str> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        if !(1..=16).contains(&subjects.len()) {
            return Err("expected 1..16 subjects");
        }
        let visits = AtomicUsize::new(0);
        let evaluate = |s: &String| {
            self.query_counted(s, predicate, explain, view, revision, &mut || {
                if visits.fetch_add(1, Ordering::Relaxed) >= max_visits {
                    Err("query candidate scan budget exceeded")
                } else {
                    Ok(())
                }
            })
        };
        if subjects.len() == 1 {
            return evaluate(&subjects[0]).map(|v| vec![v]);
        }
        subjects.par_iter().map(evaluate).collect()
    }
    fn derive(
        &self,
        subject: Id,
        revision: Option<&str>,
        infer: bool,
        check: &mut impl FnMut() -> Result<(), &'static str>,
    ) -> Result<Vec<Step>, &'static str> {
        let mut search = Search::default();
        for (row, fact) in self.pack.candidates([Some(subject), None, None]) {
            check()?;
            if !self.visible(row, revision) {
                continue;
            }
            search.record(fact, None, fact, "asserted")?;
        }

        // Breadth-first, with all asserted facts seeded first. Each proof links
        // only to an earlier step, even for cyclic subclass/property graphs.
        let mut cursor = 0;
        while infer && cursor < search.steps.len() {
            let [s, p, o] = search.steps[cursor].fact;
            if let Some((schema, rule)) = self.object_hierarchy(p) {
                for (row, premise) in self.pack.candidates([Some(o), Some(schema), None]) {
                    check()?;
                    if !self.visible(row, revision) {
                        continue;
                    }
                    search.record([s, p, premise[2]], Some(cursor), premise, rule)?;
                }
            }
            if let Some(sub_property) = self.sub_property {
                for (row, premise) in self.pack.candidates([Some(p), Some(sub_property), None]) {
                    check()?;
                    if !self.visible(row, revision) {
                        continue;
                    }
                    if self.pack.term(premise[2]).starts_with('<') {
                        search.record(
                            [s, premise[2], o],
                            Some(cursor),
                            premise,
                            "inherited_property",
                        )?;
                    }
                }
            }
            cursor += 1;
        }
        Ok(search.steps)
    }

    fn object_hierarchy(&self, predicate: Id) -> Option<(Id, &'static str)> {
        if Some(predicate) == self.rdf_type {
            Some((self.sub_class?, "inherited_type"))
        } else if Some(predicate) == self.sub_class {
            Some((self.sub_class?, "transitive_subclass"))
        } else if Some(predicate) == self.sub_property {
            Some((self.sub_property?, "transitive_subproperty"))
        } else {
            None
        }
    }
}

#[derive(Default)]
struct Search {
    steps: Vec<Step>,
    seen: HashSet<Triple>,
    work: usize,
}

impl Search {
    fn record(
        &mut self,
        fact: Triple,
        previous: Option<usize>,
        premise: Triple,
        rule: &'static str,
    ) -> Result<(), &'static str> {
        self.work += 1;
        if self.work > MAX_WORK {
            return Err("query exceeds the 100000-step budget");
        }
        if self.seen.insert(fact) {
            self.steps.push(Step {
                fact,
                previous,
                premise,
                rule,
            });
        }
        Ok(())
    }
}

fn charge(left: &mut usize, bytes: usize) -> Result<(), &'static str> {
    *left = left
        .checked_sub(bytes)
        .ok_or("query exceeds the 1 MiB result term-text budget")?;
    Ok(())
}
