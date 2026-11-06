use super::{RDF_TYPE, Reasoner, SUB_CLASS};
use anyhow::{Result, ensure};
use ontokiln_pack::{Id, Pack};
use roaring::RoaringBitmap;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(crate) struct TypeIndex {
    direct: HashMap<Id, RoaringBitmap>,
    children: HashMap<Id, Vec<Id>>,
}

impl TypeIndex {
    fn build(pack: &Pack) -> Self {
        let mut index = Self::default();
        let type_id = pack.lookup(RDF_TYPE);
        let sub_class = pack.lookup(SUB_CLASS);
        for &[s, p, o] in pack.triples() {
            if Some(p) == type_id {
                index.direct.entry(o).or_default().insert(s);
            } else if Some(p) == sub_class {
                index.children.entry(o).or_default().push(s);
            }
        }
        index
    }
    fn instances(&self, class: Id, subclasses: bool) -> RoaringBitmap {
        let mut out = RoaringBitmap::new();
        let mut pending = vec![class];
        let mut seen = HashSet::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            if let Some(posting) = self.direct.get(&id) {
                out |= posting;
            }
            if subclasses && let Some(children) = self.children.get(&id) {
                pending.extend(children);
            }
        }
        out
    }
}

#[derive(Serialize, Debug)]
pub struct EntityMatches {
    pub total: u64,
    pub entities: Vec<String>,
}

impl Reasoner {
    /// AND across explicit rdf:type postings, optionally expanded along asserted subclass edges.
    pub fn entities(
        &self,
        types: &[String],
        subclasses: bool,
        limit: usize,
    ) -> Result<EntityMatches> {
        ensure!(
            !types.is_empty() && types.len() <= 32 && limit <= 1000,
            "expected 1..32 types and limit <= 1000"
        );
        let mut candidates: Option<RoaringBitmap> = None;
        for ty in types {
            let Some(id) = self.pack.lookup(ty) else {
                return Ok(EntityMatches {
                    total: 0,
                    entities: vec![],
                });
            };
            let index = self.types.get_or_init(|| TypeIndex::build(&self.pack));
            let matches = index.instances(id, subclasses);
            let intersection = match candidates.take() {
                None => matches,
                Some(mut previous) => {
                    previous &= matches;
                    previous
                }
            };
            let empty = intersection.is_empty();
            candidates = Some(intersection);
            if empty {
                break;
            }
        }
        let candidates = candidates.expect("nonempty type list");
        let mut entities = Vec::new();
        let mut remaining = super::MAX_RESULT_TEXT;
        for id in candidates.iter().take(limit) {
            let term = self.pack.term(id);
            super::charge(&mut remaining, term.len()).map_err(anyhow::Error::msg)?;
            entities.push(term.to_owned());
        }
        Ok(EntityMatches {
            total: candidates.len(),
            entities,
        })
    }
}
