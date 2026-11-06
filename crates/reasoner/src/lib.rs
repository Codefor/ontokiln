//! Subject-bound forward chaining over the immutable SPO index.
mod entities;
pub mod sparql;
pub use entities::EntityMatches;
use ontokiln_pack::{Id, Pack, Triple};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::OnceLock;

pub const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
pub const SUB_CLASS: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
pub const SUB_PROPERTY: &str = "<http://www.w3.org/2000/01/rdf-schema#subPropertyOf>";
const MAX_WORK: usize = 100_000;
const MAX_RESULT_TEXT: usize = 1_048_576;

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
        let (Some(s), Some(wanted)) = (self.pack.lookup(subject), self.pack.lookup(predicate))
        else {
            return Ok(Vec::new());
        };
        let steps = self.derive(s)?;
        let mut answers = Vec::new();
        let mut text_left = MAX_RESULT_TEXT;
        for (index, step) in steps
            .iter()
            .enumerate()
            .filter(|(_, step)| step.fact[1] == wanted)
        {
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
                        terms.iter().map(|t| t.len()).sum::<usize>() + link.rule.len(),
                    )?;
                    chain.push(ProofStep {
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
        if !(1..=16).contains(&subjects.len()) {
            return Err("expected 1..16 subjects");
        }
        subjects
            .par_iter()
            .map(|s| self.query(s, predicate, explain))
            .collect()
    }

    fn derive(&self, subject: Id) -> Result<Vec<Step>, &'static str> {
        let mut search = Search::default();
        for &fact in self.pack.outgoing(subject) {
            search.record(fact, None, fact, "asserted")?;
        }

        // Breadth-first, with all asserted facts seeded first. Each proof links
        // only to an earlier step, even for cyclic subclass/property graphs.
        let mut cursor = 0;
        while cursor < search.steps.len() {
            let [s, p, o] = search.steps[cursor].fact;
            if let Some((schema, rule)) = self.object_hierarchy(p) {
                for &premise in self.pack.edges(o, schema) {
                    search.record([s, p, premise[2]], Some(cursor), premise, rule)?;
                }
            }
            if let Some(sub_property) = self.sub_property {
                for &premise in self.pack.edges(p, sub_property) {
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
