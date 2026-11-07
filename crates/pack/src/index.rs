//! Row IDs in POS and OSP order point into the canonical SPO table.
use super::{Id, Pack, Triple};
use std::ops::Range;

#[derive(Clone, Copy, Debug)]
pub enum Order {
    Spo,
    Pos,
    Osp,
}

impl Order {
    pub(crate) fn key<T: Copy>(self, [s, p, o]: [T; 3]) -> [T; 3] {
        match self {
            Self::Spo => [s, p, o],
            Self::Pos => [p, o, s],
            Self::Osp => [o, s, p],
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Spo => "SPO",
            Self::Pos => "POS",
            Self::Osp => "OSP",
        }
    }
}

pub struct Candidates<'a> {
    triples: &'a [Triple],
    permutation: Option<&'a [Id]>,
    range: Range<usize>,
    pub order: Order,
}
impl Iterator for Candidates<'_> {
    type Item = (usize, Triple);
    fn next(&mut self) -> Option<Self::Item> {
        let n = self.range.next()?;
        let row = self.permutation.map_or(n, |ids| ids[n] as usize);
        Some((row, self.triples[row]))
    }
}
impl Pack {
    pub fn candidates(&self, pattern: [Option<Id>; 3]) -> Candidates<'_> {
        let order = match pattern {
            [Some(_), None, Some(_)] | [None, None, Some(_)] => Order::Osp,
            [None, Some(_), _] => Order::Pos,
            _ => Order::Spo,
        };
        let permutation = self.permutation(order);
        let triples = self.triples();
        let key = order.key(pattern);
        let length = key.iter().take_while(|x| x.is_some()).count();
        let low = key.map(|x| x.unwrap_or(0));
        let high = key.map(|x| x.unwrap_or(Id::MAX));
        let key_at = |i| order.key(triples[permutation.map_or(i, |ids| ids[i] as usize)]);
        let bound = |key, inclusive| {
            let (mut l, mut r) = (0, triples.len());
            while l < r {
                let m = l + (r - l) / 2;
                let v = key_at(m);
                if v < key || (inclusive && v == key) {
                    l = m + 1;
                } else {
                    r = m;
                }
            }
            l
        };
        let range = if length == 0 {
            0..triples.len()
        } else {
            bound(low, false)..bound(high, true)
        };
        Candidates {
            triples,
            permutation,
            range,
            order,
        }
    }
}
