//! Cold tier: per-mod scan cache (bincode files keyed by a hash of the mod name).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use sat_core::facts::ModFacts;

use crate::signature::Fnv;

/// Bump to invalidate every cached scan after a change of [`ModFacts`] semantics.
pub const CACHE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    signature: u64,
    facts: ModFacts,
}

fn path_for(dir: &Path, name: &str) -> PathBuf {
    let mut h = Fnv::new();
    h.write(name.to_lowercase().as_bytes());
    dir.join(format!("{:016x}.scan", h.finish()))
}

fn config() -> impl bincode::config::Config {
    bincode::config::standard()
}

pub(crate) fn load(dir: &Path, name: &str, signature: u64) -> Option<ModFacts> {
    let data = std::fs::read(path_for(dir, name)).ok()?;
    let (env, _): (Envelope, usize) = bincode::serde::decode_from_slice(&data, config()).ok()?;
    (env.version == CACHE_VERSION && env.signature == signature && env.facts.name == name)
        .then_some(env.facts)
}

/// Best effort: a failed write only costs a rescan next time.
pub(crate) fn store(dir: &Path, facts: &ModFacts, signature: u64) {
    let env = Envelope {
        version: CACHE_VERSION,
        signature,
        facts: facts.clone(),
    };
    let Ok(bytes) = bincode::serde::encode_to_vec(&env, config()) else {
        return;
    };
    let target = path_for(dir, &facts.name);
    let tmp = target.with_extension("tmp");
    if std::fs::create_dir_all(dir).is_ok() && std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &target);
    }
}
