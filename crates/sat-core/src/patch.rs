//! Patches: edits of a mod's file kept in the project, written into an output mod at commit (ADR 0014 §3). @sat:patch
//!
//! Owns: a file patch as data (which mod, which file, the content signature of the base it was made on, the
//! operations), the content signature itself, and the output mod's name.
//! Not here: file models (`@sat:formats.ini`), building the output mod at commit (`@sat:mo2.patches`).
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

impl FilePatch {
    // @sat:patch.put
    /// Puts an operation: one per key (a later one replaces an earlier one for the same section and key).
    pub fn put(&mut self, op: PatchOp) {
        let (s, k) = op.target();
        self.ops.retain(|o| {
            let (a, b) = o.target();
            !(a.eq_ignore_ascii_case(s) && b.eq_ignore_ascii_case(k))
        });
        self.ops.push(op);
    }

    /// Takes the operation on a key back (the key keeps the mod's value).
    pub fn revert(&mut self, section: &str, key: &str) {
        self.ops.retain(|o| {
            let (a, b) = o.target();
            !(a.eq_ignore_ascii_case(section) && b.eq_ignore_ascii_case(key))
        });
    }
}

impl PatchOp {
    /// (section, key) the operation changes.
    pub fn target(&self) -> (&str, &str) {
        match self {
            Self::Set { section, key, .. } | Self::Remove { section, key } => (section, key),
        }
    }
}

// @sat:patch.apply
/// The patched bytes of one file (the ini model keeps comments, order and line ends: only changed lines differ).
pub fn apply(bytes: &[u8], ops: &[PatchOp]) -> Vec<u8> {
    let mut f = sat_formats::ini::IniFile::parse(bytes);
    for op in ops {
        match op {
            PatchOp::Set { section, key, value } => {
                f.set(section, key, value);
            }
            PatchOp::Remove { section, key } => {
                f.remove(section, key);
            }
        }
    }
    f.to_bytes()
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
