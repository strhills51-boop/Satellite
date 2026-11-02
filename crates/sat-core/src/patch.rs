//! Patches: edits of a mod's file kept in the project, written into an output mod at commit (ADR 0014 §3). @sat:patch
//!
//! Owns: a file patch as data (which mod, which file, the content signature of the base it was made on, the
//! operations), the content signature itself, and the output mod's name.
//! Not here: file models (`@sat:formats.ini`), applying at commit (`@sat:mo2.patches`).
//!
//! The original file is never touched: the commit writes the patched copy into **Satellite Patches**, a mod at the
//! end of the order (it wins). Taking the patches back = switching that mod off. When the base changed (the mod
//! was updated), the patch is not applied blindly: the commit stops and asks to look again.

use serde::{Deserialize, Serialize};

/// The output mod of patches.
pub const PATCH_MOD: &str = "Satellite Patches";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PatchOp {
    /// Set an ini value (`section` empty — before the first section).
    Set {
        section: String,
        key: String,
        value: String,
    },
    Remove {
        section: String,
        key: String,
    },
}

// @sat:patch.file
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilePatch {
    /// The mod the file comes from (entity id in the project file, name in a session).
    pub source: String,
    /// The file inside the mod (`/`-separated).
    pub path: String,
    /// Content signature of the file the patch was made on (`content_signature`); `None` — any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub ops: Vec<PatchOp>,
}

// @sat:patch.signature
/// Content signature of a file's bytes (FNV-1a 64, hex): enough to notice that a base changed.
pub fn content_signature(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Fingerprint of all patches with their bases: the output is rebuilt when it changes.
pub fn fingerprint(patches: &[FilePatch]) -> String {
    let json = serde_json::to_vec(patches).unwrap_or_default();
    content_signature(&json)
}
