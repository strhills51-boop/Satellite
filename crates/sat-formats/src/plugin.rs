//! TES4 plugin header (`.esp` / `.esm` / `.esl`) and record-signature counting.
//!
//! Layout reference: UESP "Skyrim Mod:Mod File Format". Records and groups use 24-byte headers
//! in Skyrim/Fallout 4 (TES4 header record included).

use std::collections::BTreeMap;
use std::io::{BufRead, Read};

use serde::{Deserialize, Serialize};

use crate::read::{u16le, u32le, zstring};
use crate::{FormatError, Result};

const FLAG_MASTER: u32 = 0x1;
const FLAG_LOCALIZED: u32 = 0x80;
const FLAG_LIGHT: u32 = 0x200;
const MAX_HEADER_SIZE: u32 = 64 << 20;

/// Effective plugin kind as the game treats it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginKind {
    Esm,
    Esp,
    Esl,
}

/// Parsed TES4 header record.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PluginHeader {
    pub master_flag: bool,
    pub light_flag: bool,
    pub localized: bool,
    pub version: f32,
    pub num_records: u32,
    pub next_object_id: u32,
    pub author: Option<String>,
    pub description: Option<String>,
    pub masters: Vec<String>,
}

impl PluginHeader {
    /// Kind derived from flags and file extension (`.esl` is always light, `.esm` always master).
    pub fn kind(&self, file_name: &str) -> PluginKind {
        let lower = file_name.to_ascii_lowercase();
        if lower.ends_with(".esl") || self.light_flag {
            PluginKind::Esl
        } else if lower.ends_with(".esm") || self.master_flag {
            PluginKind::Esm
        } else {
            PluginKind::Esp
        }
    }
}

/// Reads the TES4 header record from the start of a plugin.
pub fn read_plugin_header<R: Read>(mut r: R) -> Result<PluginHeader> {
    let mut hdr = [0u8; 24];
    r.read_exact(&mut hdr)?;
    let sig: [u8; 4] = hdr[0..4].try_into().expect("4 bytes");
    if &sig != b"TES4" {
        return Err(FormatError::Signature {
            found: sig,
            expected: "TES4",
        });
    }
    let size = u32le(&hdr, 4);
    let flags = u32le(&hdr, 8);
    if size > MAX_HEADER_SIZE {
        return Err(FormatError::Corrupt("TES4 header too large"));
    }
    let mut data = vec![0u8; size as usize];
    r.read_exact(&mut data)?;

    let mut h = PluginHeader {
        master_flag: flags & FLAG_MASTER != 0,
        light_flag: flags & FLAG_LIGHT != 0,
        localized: flags & FLAG_LOCALIZED != 0,
        ..Default::default()
    };
    let mut o = 0usize;
    let mut xxxx: Option<usize> = None; // oversized next subrecord
    while o + 6 <= data.len() {
        let sub = &data[o..o + 4];
        let mut len = u16le(&data, o + 4) as usize;
        o += 6;
        if let Some(big) = xxxx.take() {
            len = big;
        }
        if o + len > data.len() {
            return Err(FormatError::Corrupt("TES4 subrecord overruns header"));
        }
        let v = &data[o..o + len];
        match sub {
            b"HEDR" if v.len() >= 12 => {
                h.version = f32::from_bits(u32le(v, 0));
                h.num_records = u32le(v, 4);
                h.next_object_id = u32le(v, 8);
            }
            b"CNAM" => h.author = Some(zstring(v)),
            b"SNAM" => h.description = Some(zstring(v)),
            b"MAST" => h.masters.push(zstring(v)),
            b"XXXX" if v.len() == 4 => xxxx = Some(u32le(v, 0) as usize),
            _ => {}
        }
        o += len;
    }
    Ok(h)
}

/// Counts of record signatures in a plugin (`REFR`, `ACHR`, `NPC_`, …).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordCounts {
    pub by_signature: BTreeMap<String, u32>,
    pub total: u32,
}

impl RecordCounts {
    /// Placed references (`REFR` + `ACHR`) — the budget that matters for engine reference limits.
    pub fn placed_references(&self) -> u32 {
        self.get("REFR") + self.get("ACHR")
    }
    pub fn get(&self, sig: &str) -> u32 {
        self.by_signature.get(sig).copied().unwrap_or(0)
    }
}

/// Walks every record header without decompressing record data.
/// `GRUP` headers are entered (their contents follow immediately), records are skipped by size.
pub fn scan_records<R: BufRead>(mut r: R) -> Result<RecordCounts> {
    let mut counts = RecordCounts::default();
    let mut hdr = [0u8; 24];
    let mut first = true;
    loop {
        if !read_full_or_eof(&mut r, &mut hdr)? {
            break;
        }
        let sig = &hdr[0..4];
        if sig == b"GRUP" {
            first = false;
            continue;
        }
        let size = u32le(&hdr, 4) as u64;
        skip(&mut r, size)?;
        if first {
            first = false;
            if sig == b"TES4" {
                continue;
            }
        }
        let key = String::from_utf8_lossy(sig).into_owned();
        *counts.by_signature.entry(key).or_insert(0) += 1;
        counts.total += 1;
    }
    Ok(counts)
}

/// Reads exactly `buf.len()` bytes; `Ok(false)` on clean EOF before the first byte, error on a torn header.
fn read_full_or_eof<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<bool> {
    let mut n = 0;
    while n < buf.len() {
        let k = r.read(&mut buf[n..])?;
        if k == 0 {
            return if n == 0 {
                Ok(false)
            } else {
                Err(FormatError::Corrupt("truncated record header"))
            };
        }
        n += k;
    }
    Ok(true)
}

fn skip<R: BufRead>(r: &mut R, mut n: u64) -> Result<()> {
    while n > 0 {
        let avail = r.fill_buf()?;
        if avail.is_empty() {
            return Err(FormatError::Corrupt("record overruns file"));
        }
        let k = (avail.len() as u64).min(n) as usize;
        r.consume(k);
        n -= k as u64;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds a minimal plugin: TES4 header + one GRUP with two REFR + one NPC_.
    pub(crate) fn sample_plugin(esm: bool, masters: &[&str]) -> Vec<u8> {
        let mut body = Vec::new();
        let mut sub = |sig: &[u8; 4], data: &[u8]| {
            body.extend_from_slice(sig);
            body.extend_from_slice(&(data.len() as u16).to_le_bytes());
            body.extend_from_slice(data);
        };
        let mut hedr = Vec::new();
        hedr.extend_from_slice(&1.7f32.to_bits().to_le_bytes());
        hedr.extend_from_slice(&3u32.to_le_bytes());
        hedr.extend_from_slice(&0x800u32.to_le_bytes());
        sub(b"HEDR", &hedr);
        sub(b"CNAM", b"author\0");
        for m in masters {
            let mut z = m.as_bytes().to_vec();
            z.push(0);
            sub(b"MAST", &z);
            sub(b"DATA", &[0; 8]);
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"TES4");
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(if esm { 1u32 } else { 0 }).to_le_bytes());
        out.extend_from_slice(&[0; 12]);
        out.extend_from_slice(&body);
        let rec = |sig: &[u8; 4]| {
            let mut v = Vec::new();
            v.extend_from_slice(sig);
            v.extend_from_slice(&4u32.to_le_bytes());
            v.extend_from_slice(&[0; 16]);
            v.extend_from_slice(&[1, 2, 3, 4]);
            v
        };
        let mut refrs = rec(b"REFR");
        refrs.extend(rec(b"REFR"));
        out.extend_from_slice(b"GRUP");
        out.extend_from_slice(&((24 + refrs.len()) as u32).to_le_bytes());
        out.extend_from_slice(&[0; 16]);
        out.extend(refrs);
        out.extend(rec(b"NPC_"));
        out
    }

    #[test]
    fn header_and_records() {
        let bytes = sample_plugin(true, &["Skyrim.esm", "Update.esm"]);
        let h = read_plugin_header(&bytes[..]).unwrap();
        assert!(h.master_flag && !h.light_flag);
        assert_eq!(h.num_records, 3);
        assert_eq!(h.masters, vec!["Skyrim.esm", "Update.esm"]);
        assert_eq!(h.author.as_deref(), Some("author"));
        assert!((h.version - 1.7).abs() < 1e-6);
        assert_eq!(h.kind("x.esp"), PluginKind::Esm);
        let c = scan_records(&bytes[..]).unwrap();
        assert_eq!(c.placed_references(), 2);
        assert_eq!(c.get("NPC_"), 1);
        assert_eq!(c.total, 3);
    }

    #[test]
    fn rejects_non_plugin() {
        let err = read_plugin_header(&b"BSA\0aaaaaaaaaaaaaaaaaaaaaaaa"[..]).unwrap_err();
        assert!(matches!(err, FormatError::Signature { .. }));
    }

    #[test]
    fn truncated_record_is_error() {
        let mut bytes = sample_plugin(false, &[]);
        bytes.truncate(bytes.len() - 2);
        assert!(scan_records(&bytes[..]).is_err());
    }
}
