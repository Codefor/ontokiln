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
    fn build(pack: &Pack, revision: Option<&str>) -> Self {
        let mut index = Self::default();
        let type_id = pack.lookup(RDF_TYPE);
        let sub_class = pack.lookup(SUB_CLASS);
        for (row, &[s, p, o]) in pack.triples().iter().enumerate() {
            if revision
                .is_some_and(|v| !pack.sources(row).any(|d| d.revision.as_deref() == Some(v)))
            {
                continue;
            }
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
    /// Build only the requested postings from the shared POS index. This is
    /// used by disposable HTTP workers, so no whole-graph cache is necessary.
    pub fn entities_indexed(
        &self,
        types: &[String],
        subclasses: bool,
        limit: usize,
        revision: Option<&str>,
        max_visits: usize,
    ) -> Result<EntityMatches> {
        self.check_revision(revision).map_err(anyhow::Error::msg)?;
        ensure!(
            !types.is_empty() && types.len() <= 32 && limit <= 1000,
            "expected 1..32 types and limit <= 1000"
        );
        let mut remaining = max_visits;
        let mut intersection: Option<RoaringBitmap> = None;
        for name in types {
            let mut posting = RoaringBitmap::new();
            let mut pending: Vec<_> = self.pack.lookup(name).into_iter().collect();
            let mut seen = HashSet::new();
            while let Some(class) = pending.pop() {
                if !seen.insert(class) {
                    continue;
                }
                for (pred, children) in [
                    (self.rdf_type, false),
                    (self.sub_class.filter(|_| subclasses), true),
                ] {
                    let Some(pred) = pred else { continue };
                    for (row, [s, _, _]) in self.pack.candidates([None, Some(pred), Some(class)]) {
                        remaining = remaining.checked_sub(1).ok_or_else(|| {
                            anyhow::anyhow!("type candidate scan budget exceeded")
                        })?;
                        if self.visible(row, revision) {
                            if children {
                                pending.push(s);
                            } else {
                                posting.insert(s);
                            }
                        }
                    }
                }
            }
            match &mut intersection {
                Some(previous) => *previous &= posting,
                None => intersection = Some(posting),
            }
            if intersection.as_ref().is_some_and(RoaringBitmap::is_empty) {
                break;
            }
        }
        self.render_entities(intersection.expect("at least one type"), limit)
    }
    /// AND across explicit rdf:type postings, optionally expanded along asserted subclass edges.
    pub fn entities(
        &self,
        types: &[String],
        subclasses: bool,
        limit: usize,
    ) -> Result<EntityMatches> {
        self.entities_scoped(types, subclasses, limit, None)
    }
    pub fn entities_scoped(
        &self,
        types: &[String],
        subclasses: bool,
        limit: usize,
        revision: Option<&str>,
    ) -> Result<EntityMatches> {
        self.check_revision(revision).map_err(anyhow::Error::msg)?;
        ensure!(
            !types.is_empty() && types.len() <= 32 && limit <= 1000,
            "expected 1..32 types and limit <= 1000"
        );
        let mut candidates: Option<RoaringBitmap> = None;
        let scoped = revision.map(|v| TypeIndex::build(&self.pack, Some(v)));
        for ty in types {
            let Some(id) = self.pack.lookup(ty) else {
                return Ok(EntityMatches {
                    total: 0,
                    entities: vec![],
                });
            };
            let index = scoped.as_ref().unwrap_or_else(|| {
                self.types
                    .get_or_init(|| TypeIndex::build(&self.pack, None))
            });
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
        self.render_entities(candidates.expect("nonempty type list"), limit)
    }
    fn render_entities(&self, candidates: RoaringBitmap, limit: usize) -> Result<EntityMatches> {
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
