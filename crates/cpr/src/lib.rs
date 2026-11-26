//! Ascaron archive V0.9 (`.cpr`), the archive format that `archiver.dll` of Patrician II / III reads:
//! - header 0x20 bytes: `ASCARON_ARCHIVE V0.9`, zero padded;
//! - blocks from 0x20, each `{u32 bsz, u32 used, u32 count, u32 data size, entries}` (directory, `bsz` bytes),
//!   followed by `data size` bytes of data; the next block starts at block + bsz + data size, up to the end of the
//!   file; a block without entries ends the archive for `archiver.dll`;
//! - entry `{u32 offset (from the file start), u32 size, u32 type, name\0}`; type 1 = stored, 2 = compressed (not
//!   read or written here); offset -1 = deleted; `used` = 16 + the bytes of the entries.
//!
//! `archiver.dll` maps the whole file, checks only offset + size <= file length and serves stored data in place, so
//! entries may share data: [`write`] stores equal contents once.
use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};

pub const MAGIC: &[u8] = b"ASCARON_ARCHIVE V0.9";
const HEADER: usize = 0x20;
const BLOCK: usize = 0x1000;
const DIR_HEAD: usize = 16;
const ALIGN: usize = 0x20;
const STORED: u32 = 1;

/// Stored entry of an archive.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Name as written (Latin-1 bytes, `\` separators).
    pub name: Vec<u8>,
    pub offset: u32,
    pub size: u32,
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Stored entries of the archive, in file order (compressed and deleted entries are left out). Reads the
/// directories only.
pub fn index<R: Read + Seek>(r: &mut R) -> io::Result<Vec<Entry>> {
    let len = r.seek(SeekFrom::End(0))?;
    let mut head = [0u8; HEADER];
    r.seek(SeekFrom::Start(0))?;
    r.read_exact(&mut head).map_err(|_| bad("not an ASCARON_ARCHIVE V0.9"))?;
    if !head.starts_with(MAGIC) {
        return Err(bad("not an ASCARON_ARCHIVE V0.9"));
    }
    let mut out = Vec::new();
    let mut at = HEADER as u64;
    while at + DIR_HEAD as u64 <= len {
        let mut h = [0u8; DIR_HEAD];
        r.seek(SeekFrom::Start(at))?;
        r.read_exact(&mut h)?;
        let (bsz, used, count, dsz) = (u32le(&h, 0), u32le(&h, 4), u32le(&h, 8), u32le(&h, 12));
        if count == 0 {
            break;
        }
        if (used as usize) < DIR_HEAD || used > bsz || at + bsz as u64 + dsz as u64 > len {
            return Err(bad("broken directory block"));
        }
        let mut dir = vec![0u8; used as usize - DIR_HEAD];
        r.read_exact(&mut dir)?;
        let mut p = 0;
        for _ in 0..count {
            if p + 13 > dir.len() {
                return Err(bad("broken directory entry"));
            }
            let (offset, size, kind) = (u32le(&dir, p), u32le(&dir, p + 4), u32le(&dir, p + 8));
            let end = dir[p + 12..].iter().position(|&c| c == 0).ok_or_else(|| bad("broken directory entry"))? + p + 12;
            let name = dir[p + 12..end].to_vec();
            p = end + 1;
            if offset == u32::MAX || kind != STORED {
                continue;
            }
            if offset as u64 + size as u64 > len {
                return Err(bad("entry past the end of the archive"));
            }
            out.push(Entry { name, offset, size });
        }
        at += bsz as u64 + dsz as u64;
    }
    Ok(out)
}

/// Contents of a stored entry.
pub fn read<R: Read + Seek>(r: &mut R, e: &Entry) -> io::Result<Vec<u8>> {
    let mut data = vec![0u8; e.size as usize];
    r.seek(SeekFrom::Start(e.offset as u64))?;
    r.read_exact(&mut data)?;
    Ok(data)
}

/// Archive of `files` (name, contents), entries in the given order, equal contents stored once. A name must be
/// non-empty and without NUL; at least one file (`archiver.dll` rejects an archive without entries).
pub fn write(files: &[(Vec<u8>, Vec<u8>)]) -> io::Result<Vec<u8>> {
    if files.is_empty() {
        return Err(bad("no files"));
    }
    if let Some((name, _)) = files.iter().find(|(n, _)| n.is_empty() || n.contains(&0) || DIR_HEAD + 13 + n.len() > BLOCK) {
        return Err(bad(&format!("bad entry name {:?}", String::from_utf8_lossy(name))));
    }
    let mut out = vec![0u8; HEADER];
    out[..MAGIC.len()].copy_from_slice(MAGIC);
    let mut stored: HashMap<&[u8], u32> = HashMap::new();
    let mut i = 0;
    while i < files.len() {
        let (start, mut used) = (i, DIR_HEAD);
        while i < files.len() && used + 13 + files[i].0.len() <= BLOCK {
            used += 13 + files[i].0.len();
            i += 1;
        }
        let blk = out.len();
        out.resize(blk + BLOCK, 0);
        let mut p = blk + DIR_HEAD;
        for (name, data) in &files[start..i] {
            let offset = match stored.get(data.as_slice()) {
                Some(&o) => o,
                None => {
                    let o = u32::try_from(out.len()).map_err(|_| bad("archive over 4 GB"))?;
                    out.extend_from_slice(data);
                    out.resize(out.len().next_multiple_of(ALIGN), 0);
                    stored.insert(data.as_slice(), o);
                    o
                }
            };
            let size = u32::try_from(data.len()).map_err(|_| bad("file over 4 GB"))?;
            out[p..p + 4].copy_from_slice(&offset.to_le_bytes());
            out[p + 4..p + 8].copy_from_slice(&size.to_le_bytes());
            out[p + 8..p + 12].copy_from_slice(&STORED.to_le_bytes());
            out[p + 12..p + 12 + name.len()].copy_from_slice(name);
            p += 13 + name.len();
        }
        let dsz = u32::try_from(out.len() - blk - BLOCK).map_err(|_| bad("archive over 4 GB"))?;
        let head = [BLOCK as u32, used as u32, (i - start) as u32, dsz];
        for (k, v) in head.iter().enumerate() {
            out[blk + 4 * k..blk + 4 * k + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
    u32::try_from(out.len()).map_err(|_| bad("archive over 4 GB"))?;
    Ok(out)
}
