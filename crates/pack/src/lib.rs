//! Versioned offline data shared by the compiler and the read-only reasoner.
use anyhow::{Context, Result, ensure};
use memmap2::Mmap;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use tempfile::{NamedTempFile, tempfile};

pub type Id = u32;
pub type Triple = [Id; 3];

const MAGIC: &[u8; 8] = b"ONTOKILN";
pub const PACK_VERSION: u32 = 2;
const PREFIX_LEN: usize = 8 + 4;
const BODY_START: usize = PREFIX_LEN + 32;
const OFFSETS_START: usize = BODY_START + 4 + 8;
pub const MAX_PACK_BYTES: usize = 256 * 1024 * 1024;

// This is an operational memory/disk limit. The v2 layout already uses u64 offsets.
fn max_pack_bytes() -> Result<usize> {
    match std::env::var("ONTOKILN_MAX_PACK_MIB") {
        Ok(value) => {
            let mib: usize = value
                .parse()
                .context("ONTOKILN_MAX_PACK_MIB must be a positive integer")?;
            let bytes = mib
                .checked_mul(1024 * 1024)
                .context("pack limit overflow")?;
            ensure!(
                bytes > 0 && bytes <= isize::MAX as usize,
                "invalid pack limit for this platform"
            );
            Ok(bytes)
        }
        Err(std::env::VarError::NotPresent) => Ok(MAX_PACK_BYTES),
        Err(e) => Err(e.into()),
    }
}

// Body: u32 term count, u64 triple count, u64 offsets (including sentinel),
// little-endian SPO rows, then packed UTF-8 terms. Both typed tables are aligned.
#[derive(Debug)]
struct Layout {
    term_count: usize,
    spo_start: usize,
    text_start: usize,
}

impl Layout {
    fn new(terms: u32, triples: u64) -> Result<Self> {
        let spo = OFFSETS_START as u64 + (u64::from(terms) + 1) * 8;
        let text = triples
            .checked_mul(12)
            .and_then(|bytes| spo.checked_add(bytes))
            .context("pack section size overflow")?;
        ensure!(text <= isize::MAX as u64, "pack exceeds addressable size");
        Ok(Self {
            term_count: terms as usize,
            spo_start: spo as usize,
            text_start: text as usize,
        })
    }

    fn offsets<'a>(&self, bytes: &'a [u8]) -> &'a [u64] {
        bytemuck::cast_slice(&bytes[OFFSETS_START..self.spo_start])
    }
}

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
        layout: Layout,
    },
}

impl Pack {
    pub fn from_sorted(terms: Vec<String>, triples: Vec<Triple>) -> Result<Self> {
        let layout = Layout::new(terms.len().try_into()?, triples.len().try_into()?)?;
        let text_len = terms.iter().map(String::len).sum::<usize>();
        let limit = max_pack_bytes()?;
        ensure!(
            layout
                .text_start
                .checked_add(text_len)
                .is_some_and(|size| size <= limit),
            "pack exceeds configured size limit ({limit} bytes)"
        );
        ensure!(
            terms.iter().all(|term| !term.is_empty()),
            "empty dictionary term"
        );
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
            Storage::Mapped { layout, .. } => layout.term_count,
        }
    }
    pub fn triple_count(&self) -> usize {
        self.triples().len()
    }
    pub fn triples(&self) -> &[Triple] {
        match &self.0 {
            Storage::Owned { triples, .. } => triples,
            Storage::Mapped { map, layout } => {
                bytemuck::cast_slice(&map[layout.spo_start..layout.text_start])
            }
        }
    }
    pub fn term(&self, id: Id) -> &str {
        match &self.0 {
            Storage::Owned { terms, .. } => &terms[id as usize],
            Storage::Mapped { map, layout } => {
                let offsets = layout.offsets(map);
                let start = u64::from_le(offsets[id as usize]) as usize;
                let end = u64::from_le(offsets[id as usize + 1]) as usize;
                std::str::from_utf8(&map[layout.text_start + start..layout.text_start + end])
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
            Storage::Mapped { map, .. } => file.write_all(map)?,
            Storage::Owned { terms, triples } => {
                let layout = Layout::new(terms.len().try_into()?, triples.len().try_into()?)?;
                let text_len = terms.iter().map(String::len).sum::<usize>();
                let mut body = Vec::with_capacity(layout.text_start - BODY_START + text_len);
                body.extend((terms.len() as u32).to_le_bytes());
                body.extend((triples.len() as u64).to_le_bytes());
                let mut offset = 0_u64;
                body.extend(offset.to_le_bytes());
                for term in terms {
                    offset += term.len() as u64;
                    body.extend(offset.to_le_bytes());
                }
                for triple in triples {
                    for id in triple {
                        body.extend(id.to_le_bytes());
                    }
                }
                for term in terms {
                    body.extend_from_slice(term.as_bytes());
                }
                file.write_all(MAGIC)?;
                file.write_all(&PACK_VERSION.to_le_bytes())?;
                file.write_all(&Sha256::digest(&body))?;
                file.write_all(&body)?;
            }
        }
        file.as_file().sync_all()?;
        file.persist(path).context("publish pack")?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let mut source = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let mut prefix = [0; PREFIX_LEN];
        source
            .read_exact(&mut prefix)
            .context("invalid pack header")?;
        ensure!(prefix.starts_with(MAGIC), "invalid pack header");
        let version = u32::from_le_bytes(prefix[8..].try_into().unwrap());
        ensure!(
            version == PACK_VERSION,
            "unsupported pack version {version} (expected {PACK_VERSION}); rebuild from .ttl/.nt with ontokiln compile"
        );

        let len = source.metadata()?.len();
        let limit = max_pack_bytes()?;
        ensure!(
            len <= limit as u64,
            "pack exceeds configured size limit ({limit} bytes)"
        );
        ensure!(len >= (OFFSETS_START + 8) as u64, "truncated pack");
        let mut backing = tempfile()?;
        backing.write_all(&prefix)?;
        let remaining = len - PREFIX_LEN as u64;
        ensure!(
            std::io::copy(&mut source.take(remaining + 1), &mut backing)? == remaining,
            "pack changed during load"
        );
        // SAFETY: only this private anonymous file is mapped. The caller's source
        // may be truncated or rewritten after loading. No writable handle escapes.
        let map = unsafe { Mmap::map(&backing)? };
        drop(backing);
        ensure!(
            Sha256::digest(&map[BODY_START..]).as_slice() == &map[PREFIX_LEN..BODY_START],
            "pack checksum mismatch"
        );
        let terms = u32::from_le_bytes(map[BODY_START..BODY_START + 4].try_into().unwrap());
        let triples = u64::from_le_bytes(map[BODY_START + 4..OFFSETS_START].try_into().unwrap());
        let layout = Layout::new(terms, triples)?;
        ensure!(layout.text_start <= map.len(), "truncated pack sections");
        let offsets = layout.offsets(&map);
        let text = std::str::from_utf8(&map[layout.text_start..])?;
        ensure!(
            offsets[0] == 0 && u64::from_le(offsets[layout.term_count]) == text.len() as u64,
            "invalid dictionary endpoints"
        );
        let mut previous = 0;
        for &end in &offsets[1..] {
            let end = u64::from_le(end);
            ensure!(
                previous < end && end <= text.len() as u64 && text.is_char_boundary(end as usize),
                "invalid dictionary offset"
            );
            previous = end;
        }
        let mapped = Self(Storage::Mapped { map, layout });
        #[cfg(target_endian = "big")]
        {
            let terms = (0..mapped.term_count())
                .map(|id| mapped.term(id as Id).to_owned())
                .collect();
            let triples = mapped
                .triples()
                .iter()
                .map(|t| t.map(Id::from_le))
                .collect();
            Self::from_sorted(terms, triples)
        }
        #[cfg(target_endian = "little")]
        {
            mapped.validate()?;
            Ok(mapped)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_v2_has_a_single_dictionary_sentinel() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.pack");
        Pack::from_sorted(vec![], vec![])
            .unwrap()
            .save(&path)
            .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[OFFSETS_START..], &[0; 8]);
        let pack = Pack::load(&path).unwrap();
        assert_eq!((pack.term_count(), pack.triple_count()), (0, 0));
    }

    #[test]
    fn invalid_v2_sections_fail_even_with_a_matching_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("graph.pack");
        let terms = vec!["<urn:a>".to_string(), "<urn:é>".to_string()];
        Pack::from_sorted(terms, vec![[0, 0, 1]])
            .unwrap()
            .save(&path)
            .unwrap();
        let original = std::fs::read(&path).unwrap();
        let layout = Layout::new(2, 1).unwrap();
        let reject = |pos: usize, value: &[u8]| {
            let mut bytes = original.clone();
            bytes[pos..pos + value.len()].copy_from_slice(value);
            let digest = Sha256::digest(&bytes[BODY_START..]);
            bytes[PREFIX_LEN..BODY_START].copy_from_slice(&digest);
            std::fs::write(&path, bytes).unwrap();
            assert!(
                Pack::load(&path).is_err(),
                "accepted invalid section at {pos}"
            );
        };
        reject(BODY_START, &u32::MAX.to_le_bytes());
        reject(BODY_START + 4, &u64::MAX.to_le_bytes());
        reject(OFFSETS_START, &1_u64.to_le_bytes());
        reject(OFFSETS_START + 8, &0_u64.to_le_bytes()); // repeated offset
        reject(OFFSETS_START + 8, &13_u64.to_le_bytes()); // inside é
        reject(OFFSETS_START + 8, &u64::MAX.to_le_bytes());
        reject(OFFSETS_START + 16, &14_u64.to_le_bytes()); // wrong sentinel
        reject(layout.spo_start, &2_u32.to_le_bytes()); // missing subject
        reject(layout.text_start, &[0xff]);
    }
}
