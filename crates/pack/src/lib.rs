//! Versioned offline data shared by the compiler and the read-only reasoner.
use anyhow::{Context, Result, bail, ensure};
use memmap2::Mmap;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use tempfile::{NamedTempFile, tempfile};

pub type Id = u32;
pub type Triple = [Id; 3];

const MAGIC: &[u8; 8] = b"ONTOKILN";
const VERSION: u32 = 1;
const HEADER: usize = 8 + 4 + 32;
pub const MAX_PACK_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug)]
pub struct Pack(Storage);

#[derive(Debug)]
enum Storage {
    Owned {
        terms: Vec<String>,
        triples: Vec<Triple>,
    },
    Mapped {
        map: Mmap,
        start: usize,
        terms: Vec<u32>,
        spo: usize,
    },
}

impl Pack {
    pub fn from_sorted(terms: Vec<String>, triples: Vec<Triple>) -> Result<Self> {
        let size =
            HEADER + 8 + terms.iter().map(|s| 4 + s.len()).sum::<usize>() + 12 * triples.len();
        ensure!(size <= MAX_PACK_BYTES, "pack exceeds demo size limit");
        let pack = Self(Storage::Owned { terms, triples });
        pack.validate()?;
        Ok(pack)
    }

    fn validate(&self) -> Result<()> {
        for i in 1..self.term_count() {
            ensure!(
                self.term(i as Id - 1) < self.term(i as Id),
                "unsorted or duplicate terms"
            );
        }
        let triples = self.triples();
        ensure!(
            triples.windows(2).all(|w| w[0] < w[1]),
            "unsorted or duplicate triples"
        );
        for &[s, p, o] in triples {
            ensure!(
                [s, p, o]
                    .iter()
                    .all(|&id| (id as usize) < self.term_count()),
                "invalid term ID"
            );
            ensure!(
                self.term(s).starts_with(['<', '_']) && self.term(p).starts_with('<'),
                "invalid subject or predicate"
            );
        }
        Ok(())
    }

    pub fn term_count(&self) -> usize {
        match &self.0 {
            Storage::Owned { terms, .. } => terms.len(),
            Storage::Mapped { terms, .. } => terms.len(),
        }
    }
    pub fn triple_count(&self) -> usize {
        self.triples().len()
    }
    pub fn triples(&self) -> &[Triple] {
        match &self.0 {
            Storage::Owned { triples, .. } => triples,
            Storage::Mapped { map, spo, .. } => bytemuck::cast_slice(&map[*spo..]),
        }
    }
    pub fn term(&self, id: Id) -> &str {
        match &self.0 {
            Storage::Owned { terms, .. } => &terms[id as usize],
            Storage::Mapped { map, terms, .. } => {
                let pos = terms[id as usize] as usize;
                let len = u32::from_le_bytes(map[pos..pos + 4].try_into().unwrap()) as usize;
                std::str::from_utf8(&map[pos + 4..pos + 4 + len])
                    .expect("validated immutable dictionary")
            }
        }
    }
    pub fn lookup(&self, term: &str) -> Option<Id> {
        let mut range = 0..self.term_count();
        while range.start < range.end {
            let mid = range.start + (range.end - range.start) / 2;
            match self.term(mid as Id).cmp(term) {
                std::cmp::Ordering::Less => range.start = mid + 1,
                std::cmp::Ordering::Greater => range.end = mid,
                std::cmp::Ordering::Equal => return Some(mid as Id),
            }
        }
        None
    }
    pub fn outgoing(&self, s: Id) -> &[Triple] {
        let triples = self.triples();
        let first = triples.partition_point(|t| t[0] < s);
        let len = triples[first..].partition_point(|t| t[0] == s);
        &triples[first..first + len]
    }
    pub fn edges(&self, s: Id, p: Id) -> &[Triple] {
        let row = self.outgoing(s);
        let first = row.partition_point(|t| t[1] < p);
        let len = row[first..].partition_point(|t| t[1] == p);
        &row[first..first + len]
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        let dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let mut file = NamedTempFile::new_in(dir)?;
        match &self.0 {
            Storage::Mapped { map, start, .. } => file.write_all(&map[*start..])?,
            Storage::Owned { terms, triples } => {
                let mut body = Vec::with_capacity(
                    8 + terms.iter().map(|t| 4 + t.len()).sum::<usize>() + 12 * triples.len(),
                );
                body.extend((terms.len() as u32).to_le_bytes());
                body.extend((triples.len() as u32).to_le_bytes());
                for term in terms {
                    body.extend((term.len() as u32).to_le_bytes());
                    body.extend_from_slice(term.as_bytes());
                }
                for t in triples {
                    for id in t {
                        body.extend(id.to_le_bytes());
                    }
                }
                file.write_all(MAGIC)?;
                file.write_all(&VERSION.to_le_bytes())?;
                file.write_all(&Sha256::digest(&body))?;
                file.write_all(&body)?;
            }
        }
        file.as_file().sync_all()?;
        file.persist(path).context("publish pack")?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let source = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let len = source.metadata()?.len();
        ensure!(len <= MAX_PACK_BYTES as u64, "pack exceeds demo size limit");
        ensure!(len >= HEADER as u64, "invalid pack header");
        // v1 has variable-width terms followed by a whole number of 12-byte rows.
        // Align EOF in the private backing file to align the start of its SPO rows.
        let start = (4 - len as usize % 4) % 4;
        let mut backing = tempfile()?;
        backing.write_all(&[0; 3][..start])?;
        ensure!(
            std::io::copy(&mut source.take(len + 1), &mut backing)? == len,
            "pack changed during load"
        );
        // SAFETY: this anonymous temporary file is private. Drop its only writable
        // handle immediately; callers may freely overwrite/truncate the source path.
        let map = unsafe { Mmap::map(&backing)? };
        drop(backing);
        let bytes = &map[start..];
        ensure!(bytes.starts_with(MAGIC), "invalid pack header");
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
            term_count as u64 * 4 + triple_count as u64 * 12 <= body.len() as u64,
            "impossible pack counts"
        );
        let mut terms = Vec::new();
        for _ in 0..term_count {
            terms.push((map.len() - body.len()) as u32);
            let len = number(&mut body)? as usize;
            ensure!(len > 0 && len <= body.len(), "invalid term length");
            std::str::from_utf8(&body[..len])?;
            body = &body[len..];
        }
        ensure!(
            body.len() as u64 == triple_count as u64 * 12,
            "invalid triple section length"
        );
        let spo = map.len() - body.len();
        bytemuck::try_cast_slice::<u8, Triple>(body)
            .map_err(|_| anyhow::anyhow!("unaligned SPO section"))?;
        let mapped = Self(Storage::Mapped {
            map,
            start,
            terms,
            spo,
        });
        #[cfg(target_endian = "big")]
        {
            let terms = (0..term_count)
                .map(|i| mapped.term(i as Id).to_owned())
                .collect();
            let triples = mapped
                .triples()
                .iter()
                .map(|row| row.map(Id::from_le))
                .collect();
            return Self::from_sorted(terms, triples);
        }
        #[cfg(target_endian = "little")]
        {
            mapped.validate()?;
            Ok(mapped)
        }
    }
}

fn number(bytes: &mut &[u8]) -> Result<u32> {
    let Some((word, rest)) = bytes.split_first_chunk::<4>() else {
        bail!("truncated pack");
    };
    *bytes = rest;
    Ok(u32::from_le_bytes(*word))
}
