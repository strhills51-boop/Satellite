//! Cold tier: per-mod scan cache (bincode files keyed by a hash of the mod name). @sat:scan.cache
//!
//! An entry records the content signature and the analyzer stamps the facts were computed with
//! ([`crate::analyzer::Stamp`]); the route (`reuse` / `indexed` / `scan`) is decided from them. Entries of the
//! first format (one global version, options folded into the signature) are still read and adopted.
//!
//! Owns: the per-mod cache file format (current and first) .

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use sat_core::facts::ModFacts;

use crate::analyzer::Stamp;
use crate::signature::Fnv;

// @sat:scan.cache.format
/// Layout of a cache file. Analyzer changes do **not** bump it: their stamps do.
pub const CACHE_FORMAT: u32 = 2;
/// The first format's only version.
const LEGACY_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Envelope {
    format: u32,
    signature: u64,
    stamps: Vec<Stamp>,
    facts: ModFacts,
}

#[derive(Serialize, Deserialize)]
struct Legacy {
    version: u32,
    signature: u64,
    facts: ModFacts,
}

pub(crate) enum Cached {
    Current {
        signature: u64,
        stamps: Vec<Stamp>,
        facts: ModFacts,
    },
    /// Written by the first format: signature included the options, facts made by every enabled analyzer at v1.
    Legacy { signature: u64, facts: ModFacts },
}

fn path_for(dir: &Path, name: &str) -> PathBuf {
    let mut h = Fnv::new();
    h.write(name.to_lowercase().as_bytes());
    dir.join(format!("{:016x}.scan", h.finish()))
}

fn config() -> impl bincode::config::Config {
    bincode::config::standard()
}

pub(crate) fn load(dir: &Path, name: &str) -> Option<Cached> {
    let data = std::fs::read(path_for(dir, name)).ok()?;
    let (tag, _): (u32, usize) = bincode::serde::decode_from_slice(&data, config()).ok()?;
    let c = match tag {
        CACHE_FORMAT => {
            let (e, _): (Envelope, usize) = bincode::serde::decode_from_slice(&data, config()).ok()?;
            Cached::Current {
                signature: e.signature,
                stamps: e.stamps,
                facts: e.facts,
            }
        }
        LEGACY_VERSION => {
            let (e, _): (Legacy, usize) = bincode::serde::decode_from_slice(&data, config()).ok()?;
            Cached::Legacy {
                signature: e.signature,
                facts: e.facts,
            }
        }
        _ => return None,
    };
    let facts_name = match &c {
        Cached::Current { facts, .. } | Cached::Legacy { facts, .. } => &facts.name,
    };
    (facts_name == name).then_some(c)
}

/// Best effort: a failed write only costs a rescan next time.
pub(crate) fn store(dir: &Path, facts: &ModFacts, signature: u64, stamps: &[Stamp]) {
    let env = Envelope {
        format: CACHE_FORMAT,
        signature,
        stamps: stamps.to_vec(),
        facts: facts.clone(),
    };
    write(dir, &facts.name, &env);
}

#[cfg(test)]
pub(crate) fn store_legacy(dir: &Path, facts: &ModFacts, signature: u64) {
    let env = Legacy {
        version: LEGACY_VERSION,
        signature,
        facts: facts.clone(),
    };
    write(dir, &facts.name, &env);
}

fn write(dir: &Path, name: &str, value: &impl Serialize) {
    let Ok(bytes) = bincode::serde::encode_to_vec(value, config()) else {
        return;
    };
    let target = path_for(dir, name);
    let tmp = target.with_extension("tmp");
    if std::fs::create_dir_all(dir).is_ok() && std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &target);
    }
}
