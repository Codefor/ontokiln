//! Versioned offline data shared by the compiler and the read-only reasoner.
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use tempfile::NamedTempFile;

pub type Id = u32;
pub type Triple = [Id; 3];

const MAGIC: &[u8; 8] = b"ONTOKILN";
const VERSION: u32 = 1;
const HEADER: usize = 8 + 4 + 32;
pub const MAX_PACK_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug)]
pub struct Pack {
    // Sorted N-Triples terms. Table position is the snapshot-local ID.
    terms: Vec<String>,
    // Unique (subject, predicate, object) tuples in lexicographic order.
    triples: Vec<Triple>,
}

impl Pack {
    pub fn from_sorted(terms: Vec<String>, triples: Vec<Triple>) -> Result<Self> {
        ensure!(
            terms.windows(2).all(|w| w[0] < w[1]),
            "unsorted or duplicate terms"
        );
        ensure!(
            triples.windows(2).all(|w| w[0] < w[1]),
            "unsorted or duplicate triples"
        );
        for &[s, p, o] in &triples {
            ensure!(
                [s, p, o].iter().all(|&id| (id as usize) < terms.len()),
                "invalid term ID"
            );
            ensure!(
                terms[s as usize].starts_with(['<', '_']) && terms[p as usize].starts_with('<'),
                "invalid subject or predicate"
            );
        }
        let pack = Self { terms, triples };
        ensure!(
            pack.encoded_size() <= MAX_PACK_BYTES,
            "pack exceeds demo size limit"
        );
        Ok(pack)
    }

    pub fn term_count(&self) -> usize {
        self.terms.len()
    }
    pub fn triple_count(&self) -> usize {
        self.triples.len()
    }
    pub fn term(&self, id: Id) -> &str {
        &self.terms[id as usize]
    }
    pub fn lookup(&self, term: &str) -> Option<Id> {
        self.terms
            .binary_search_by(|s| s.as_str().cmp(term))
            .ok()
            .map(|i| i as Id)
    }
    pub fn outgoing(&self, s: Id) -> &[Triple] {
        let first = self.triples.partition_point(|t| t[0] < s);
        let len = self.triples[first..].partition_point(|t| t[0] == s);
        &self.triples[first..first + len]
    }
    pub fn edges(&self, s: Id, p: Id) -> &[Triple] {
        let row = self.outgoing(s);
        let first = row.partition_point(|t| t[1] < p);
        let len = row[first..].partition_point(|t| t[1] == p);
        &row[first..first + len]
    }
    fn encoded_size(&self) -> usize {
        HEADER + 8 + self.terms.iter().map(|s| 4 + s.len()).sum::<usize>() + 12 * self.triples.len()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let mut body = Vec::with_capacity(self.encoded_size() - HEADER);
        body.extend((self.terms.len() as u32).to_le_bytes());
        body.extend((self.triples.len() as u32).to_le_bytes());
        for term in &self.terms {
            body.extend((term.len() as u32).to_le_bytes());
            body.extend_from_slice(term.as_bytes());
        }
        for triple in &self.triples {
            for id in triple {
                body.extend(id.to_le_bytes());
            }
        }
        let dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let mut file = NamedTempFile::new_in(dir)?;
        file.write_all(MAGIC)?;
        file.write_all(&VERSION.to_le_bytes())?;
        file.write_all(&Sha256::digest(&body))?;
        file.write_all(&body)?;
        file.as_file().sync_all()?;
        file.persist(path).context("publish pack")?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let mut bytes = Vec::new();
        file.take(MAX_PACK_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= MAX_PACK_BYTES,
            "pack exceeds demo size limit"
        );
        ensure!(
            bytes.len() >= HEADER && bytes.starts_with(MAGIC),
            "invalid pack header"
        );
        let mut fields = &bytes[MAGIC.len()..];
        ensure!(number(&mut fields)? == VERSION, "unsupported pack version");
        let (digest, mut body) = fields.split_at(32);
        ensure!(
            Sha256::digest(body).as_slice() == digest,
            "pack checksum mismatch"
        );
        let term_count = number(&mut body)? as usize;
        let triple_count = number(&mut body)? as usize;
        ensure!(
            (term_count as u64) * 4 + (triple_count as u64) * 12 <= body.len() as u64,
            "impossible pack counts"
        );
        // Grow only as valid entries are read. Counts in a file are untrusted.
        let mut terms = Vec::new();
        for _ in 0..term_count {
            let len = number(&mut body)? as usize;
            ensure!(len > 0 && len <= body.len(), "invalid term length");
            let (term, rest) = body.split_at(len);
            terms.push(std::str::from_utf8(term)?.to_owned());
            body = rest;
        }
        ensure!(
            body.len() as u64 == triple_count as u64 * 12,
            "invalid triple section length"
        );
        let mut triples = Vec::with_capacity(triple_count);
        for _ in 0..triple_count {
            triples.push([number(&mut body)?, number(&mut body)?, number(&mut body)?]);
        }
        Self::from_sorted(terms, triples)
    }
}

fn number(bytes: &mut &[u8]) -> Result<u32> {
    let Some((word, rest)) = bytes.split_first_chunk::<4>() else {
        bail!("truncated pack");
    };
    *bytes = rest;
    Ok(u32::from_le_bytes(*word))
}
