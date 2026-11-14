//! TES4 plugin header (`.esp` / `.esm` / `.esl`) and record-signature counting. @sat:formats.plugin
//!
//! Layout reference: UESP "Skyrim Mod:Mod File Format". Records and groups use 24-byte headers
//! in Skyrim/Fallout 4 (TES4 header record included).
//!
//! Owns: TES4 header, record counting, plugin kind and the monolith plugin type.

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
    // @sat:formats.plugin.kind
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

// @sat:formats.plugin.monolith-type
/// The monolith's plugin type (`resolvePluginType` 100529 / `resolvePluginTypeV503` 172032), for parity views and
/// imports: ESL flag → `espfe` on `.esp`, else `esl`; then the extension; then the ESM flag; else `unknown`.
///
/// It differs from [`PluginHeader::kind`] on an ESM-flagged `.esp`: the monolith says `esp`, the game loads it with
/// the masters, so NG's kind is `Esm` (docs/parity/run/01-plugins.md).
pub fn monolith_plugin_type(file_name: &str, header: Option<&PluginHeader>) -> &'static str {
    let lower = file_name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map_or("", |(_, e)| e);
    let (esm, esl) = header.map_or((false, false), |h| (h.master_flag, h.light_flag));
    if esl {
        return if ext == "esp" { "espfe" } else { "esl" };
    }
    match ext {
        "esl" => "esl",
        "esm" => "esm",
        "esp" => "esp",
        _ if esm => "esm",
        _ => "unknown",
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

// @sat:formats.plugin.records
/// One record of a plugin as the record layer needs it (`@sat:records`): signature, FormID as written in the file
/// (its top byte indexes the plugin's masters, the plugin itself after them), flags, and the record's data —
/// unpacked when the record is compressed (`None` when it does not unpack).
#[derive(Debug)]
pub struct RecordData<'a> {
    pub signature: [u8; 4],
    pub form_id: u32,
    pub flags: u32,
    pub data: Option<&'a [u8]>,
}

/// Calls `each` for every record after the `TES4` header, in file order (groups are walked into).
pub fn read_records<R: BufRead>(mut r: R, mut each: impl FnMut(RecordData<'_>)) -> Result<()> {
    let mut hdr = [0u8; 24];
    let mut first = true;
    let mut buf = Vec::new();
    let mut unpacked = Vec::new();
    loop {
        if !read_full_or_eof(&mut r, &mut hdr)? {
            break;
        }
        if &hdr[0..4] == b"GRUP" {
            first = false;
            continue;
        }
        let size = u32le(&hdr, 4) as usize;
        if size > 1 << 28 {
            return Err(FormatError::Corrupt("record larger than 256 MiB"));
        }
        buf.resize(size, 0);
        r.read_exact(&mut buf)?;
        if std::mem::take(&mut first) && &hdr[0..4] == b"TES4" {
            continue;
        }
        let flags = u32le(&hdr, 8);
        let data = if flags & 0x0004_0000 != 0 && buf.len() >= 4 {
            unpacked.clear();
            let ok = std::io::Read::read_to_end(
                &mut std::io::Read::take(flate2::read::ZlibDecoder::new(&buf[4..]), 1 << 28),
                &mut unpacked,
            )
            .is_ok();
            ok.then_some(unpacked.as_slice())
        } else {
            Some(buf.as_slice())
        };
        each(RecordData {
            signature: hdr[0..4].try_into().expect("4 bytes"),
            form_id: u32le(&hdr, 12),
            flags,
            data,
        });
    }
    Ok(())
}

// @sat:formats.plugin.subrecords
/// The fields (subrecords) of a record's data, in order: (signature, bytes). `XXXX` gives the size of the next
/// field when it does not fit 16 bits; a torn tail ends the walk.
pub fn subrecords(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0usize;
    let mut big: Option<usize> = None;
    while at + 6 <= data.len() {
        let sig: [u8; 4] = data[at..at + 4].try_into().expect("4 bytes");
        let mut size = u16::from_le_bytes([data[at + 4], data[at + 5]]) as usize;
        at += 6;
        if let Some(b) = big.take() {
            size = b;
        }
        if at + size > data.len() {
            break;
        }
        if &sig == b"XXXX" && size == 4 {
            big = Some(u32le(data, at) as usize);
        } else {
            out.push((sig, &data[at..at + size]));
        }
        at += size;
    }
    out
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

/// Test/fixture writers, public for other crates' tests.
pub mod fixtures {
    use std::io::Write;

    /// A plugin with `masters` and `records` as (signature, FormID, data, compressed), all in one group.
    pub fn plugin(esm: bool, masters: &[&str], records: &[(&[u8; 4], u32, &[u8], bool)]) -> Vec<u8> {
        let mut body = Vec::new();
        let mut sub = |sig: &[u8; 4], data: &[u8]| {
            body.extend_from_slice(sig);
            body.extend_from_slice(&(data.len() as u16).to_le_bytes());
            body.extend_from_slice(data);
        };
        let mut hedr = Vec::new();
        hedr.extend_from_slice(&1.71f32.to_bits().to_le_bytes());
        hedr.extend_from_slice(&(records.len() as u32).to_le_bytes());
        hedr.extend_from_slice(&0x800u32.to_le_bytes());
        sub(b"HEDR", &hedr);
        for m in masters {
            let mut z = m.as_bytes().to_vec();
            z.push(0);
            sub(b"MAST", &z);
            sub(b"DATA", &[0; 8]);
        }
        let mut out = b"TES4".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&u32::from(esm).to_le_bytes());
        out.extend_from_slice(&[0; 12]);
        out.extend_from_slice(&body);
        let mut recs = Vec::new();
        for (sig, id, data, packed) in records {
            let stored = if *packed {
                let mut w = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                w.write_all(data).expect("in memory");
                let mut v = (data.len() as u32).to_le_bytes().to_vec();
                v.extend(w.finish().expect("in memory"));
                v
            } else {
                data.to_vec()
            };
            recs.extend_from_slice(*sig);
            recs.extend_from_slice(&(stored.len() as u32).to_le_bytes());
            recs.extend_from_slice(&(if *packed { 0x0004_0000u32 } else { 0 }).to_le_bytes());
            recs.extend_from_slice(&id.to_le_bytes());
            recs.extend_from_slice(&[0; 8]);
            recs.extend(stored);
        }
        out.extend_from_slice(b"GRUP");
        out.extend_from_slice(&((24 + recs.len()) as u32).to_le_bytes());
        out.extend_from_slice(&[0; 16]);
        out.extend(recs);
        out
    }
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
    fn records_come_with_their_form_ids_and_unpacked_data() {
        let bytes = fixtures::plugin(
            false,
            &["Skyrim.esm"],
            &[
                (b"NPC_", 0x0000_0D0E, b"edited npc", false),
                (b"WEAP", 0x0100_0800, b"new weapon data", true),
            ],
        );
        let mut got = Vec::new();
        read_records(&bytes[..], |r| {
            got.push((r.signature, r.form_id, r.data.map(<[u8]>::to_vec)))
        })
        .unwrap();
        assert_eq!(
            got,
            [
                (*b"NPC_", 0x0000_0D0E, Some(b"edited npc".to_vec())),
                (*b"WEAP", 0x0100_0800, Some(b"new weapon data".to_vec()))
            ]
        );
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
        assert_eq!(
            monolith_plugin_type("x.esp", Some(&h)),
            if h.light_flag { "espfe" } else { "esp" }
        );
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

#[cfg(test)]
mod monolith_type_tests {
    use super::*;

    #[test]
    fn matches_the_monolith_table() {
        let h = |esm, esl| PluginHeader {
            master_flag: esm,
            light_flag: esl,
            ..Default::default()
        };
        let t = |n: &str, esm, esl| monolith_plugin_type(n, Some(&h(esm, esl)));
        assert_eq!(t("A.esp", false, true), "espfe");
        assert_eq!(t("A.esm", false, true), "esl");
        assert_eq!(t("A.esl", false, false), "esl");
        assert_eq!(t("A.esm", false, false), "esm");
        assert_eq!(
            t("A.esp", true, false),
            "esp",
            "monolith: extension before the ESM flag"
        );
        assert_eq!(
            h(true, false).kind("A.esp"),
            PluginKind::Esm,
            "NG: the game loads it as a master"
        );
        assert_eq!(t("A.bak", true, false), "esm");
        assert_eq!(monolith_plugin_type("A.bak", None), "unknown");
    }
}

#[cfg(test)]
mod subrecord_tests {
    #[test]
    fn fields_are_walked_and_xxxx_sizes_the_next_one() {
        let mut d = Vec::new();
        d.extend_from_slice(b"EDID\x03\x00ab\0");
        d.extend_from_slice(b"XXXX\x04\x00");
        d.extend_from_slice(&5u32.to_le_bytes());
        d.extend_from_slice(b"DATA\x00\x00hello");
        d.extend_from_slice(b"TORN\x09\x00x");
        let f = super::subrecords(&d);
        assert_eq!(f, vec![(*b"EDID", &b"ab\0"[..]), (*b"DATA", &b"hello"[..])]);
    }
}
