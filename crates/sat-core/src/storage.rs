//! The storage layer: how the build's files lie on disk (ADR 0022). @sat:storage
//!
//! One file for identical copies, compression by temperature.
//!
//! Owns: what a file is for storage (its class: what may share bytes through a hard link, what may be compressed),
//! the temperature of a mod (hot — enabled, cold — disabled, the owner's word over both), the compression each
//! temperature gets, which copies of a set of identical files become links to which keeper, what a file still
//! needs (a codec, or nothing), and the expected size after compression (measured on the owner's build).
//! Not here: reading the disk and acting on it (`@sat:mo2.storage`), finding identical copies (the content-hash
//! cache `@sat:scan.content-hash`), the command (`@sat:cli.store`).
//!
//! ADR 0022. Storage changes nothing MO2 or the game see: a linked file is a file, a WOF-compressed file reads as
//! one. Everything here is undone by unlinking and decompressing.

use serde::{Deserialize, Serialize};

/// How warm a file is: what reads it and how often. The order is hottest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    /// The game reads it: an enabled mod's file.
    Hot,
    /// The game may read it, rarely (an outfit only `AddItemMenu` hands out).
    Warm,
    /// The game does not read it: a disabled mod's file.
    Cold,
}

/// A WOF compression algorithm (`compact /exe:<name>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Codec {
    Xpress4k,
    Xpress8k,
    Xpress16k,
    Lzx,
}

impl Codec {
    /// The name `compact.exe /exe:` takes.
    pub fn compact_name(self) -> &'static str {
        match self {
            Codec::Xpress4k => "XPRESS4K",
            Codec::Xpress8k => "XPRESS8K",
            Codec::Xpress16k => "XPRESS16K",
            Codec::Lzx => "LZX",
        }
    }

    pub fn from_name(s: &str) -> Option<Codec> {
        match s.to_ascii_uppercase().as_str() {
            "XPRESS4K" => Some(Codec::Xpress4k),
            "XPRESS8K" => Some(Codec::Xpress8k),
            "XPRESS16K" => Some(Codec::Xpress16k),
            "LZX" => Some(Codec::Lzx),
            _ => None,
        }
    }
}

/// What a file is for storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    Textures,
    Meshes,
    Animations,
    Sound,
    Archives,
    Scripts,
    Interface,
    BodySlide,
    Plugins,
    /// Settings the game and tools write in place: ini, json, toml, txt, xml, yaml, cfg.
    Config,
    /// Archives left inside a mod (7z, zip, rar).
    Packed,
    Other,
}

// @sat:storage.class
/// The class of a file by its extension.
pub fn class(rel: &str) -> Class {
    let ext = rel.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "dds" | "png" | "tga" => Class::Textures,
        "nif" | "tri" | "btr" | "bto" => Class::Meshes,
        "hkx" => Class::Animations,
        "wav" | "xwm" | "fuz" | "lip" | "mp3" | "ogg" => Class::Sound,
        "bsa" | "ba2" => Class::Archives,
        "pex" | "psc" => Class::Scripts,
        "swf" => Class::Interface,
        "osd" | "osp" => Class::BodySlide,
        "esp" | "esm" | "esl" => Class::Plugins,
        "ini" | "json" | "toml" | "txt" | "xml" | "yaml" | "yml" | "cfg" => Class::Config,
        "7z" | "zip" | "rar" => Class::Packed,
        _ => Class::Other,
    }
}

impl Class {
    /// Assets nothing writes in place: copies may share their bytes through a hard link.
    pub fn linkable(self) -> bool {
        matches!(
            self,
            Class::Textures
                | Class::Meshes
                | Class::Animations
                | Class::Sound
                | Class::Archives
                | Class::Scripts
                | Class::Interface
                | Class::BodySlide
        )
    }

    /// Files worth compressing (plugins and settings are written in place — a write undoes WOF anyway).
    pub fn compressible(self) -> bool {
        self.linkable()
    }

    /// What is left of the size after `codec` — measured on a 3 GB sample of the owner's build (2026-11-19,
    /// `wof_sample.tsv`); an estimate for the plan, not a promise.
    pub fn kept(self, codec: Codec) -> f64 {
        let (xpress, lzx) = match self {
            Class::Textures => (0.39, 0.31),
            Class::Meshes => (0.65, 0.57),
            Class::Animations => (0.44, 0.34),
            Class::Sound => (0.91, 0.88),
            Class::Archives => (0.69, 0.62),
            Class::Scripts => (0.51, 0.41),
            Class::Interface => (0.6, 0.5),
            Class::BodySlide => (0.83, 0.77),
            _ => (1.0, 1.0),
        };
        match codec {
            Codec::Lzx => lzx,
            _ => xpress,
        }
    }
}

// @sat:storage.policy
/// Which compression each temperature gets, and what is left alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    pub hot: Option<Codec>,
    pub warm: Option<Codec>,
    pub cold: Option<Codec>,
    /// Copies smaller than this are not linked (the saving is not worth a link).
    pub link_min: u64,
    /// Files smaller than this are not compressed (one cluster or so).
    pub compress_min: u64,
    /// Separators whose mods are generator outputs: generators rewrite them in place — neither linked nor compressed.
    pub untouched_separators: Vec<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            hot: Some(Codec::Xpress8k),
            warm: Some(Codec::Lzx),
            cold: Some(Codec::Lzx),
            link_min: 64 * 1024,
            compress_min: 8 * 1024,
            untouched_separators: vec!["800 GENERATED OUTPUT".into(), "990 GENERATED TOP".into()],
        }
    }
}

impl Policy {
    pub fn codec(&self, t: Tier) -> Option<Codec> {
        match t {
            Tier::Hot => self.hot,
            Tier::Warm => self.warm,
            Tier::Cold => self.cold,
        }
    }

    /// A mod under one of the generator separators.
    pub fn untouched(&self, separator: &str) -> bool {
        self.untouched_separators
            .iter()
            .any(|s| s.eq_ignore_ascii_case(separator.trim_end_matches("_separator")))
    }
}

/// Why a mod has its temperature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Why {
    Enabled,
    Disabled,
    /// The owner's tag `хранение: …`.
    Owner,
}

/// The owner's tag that sets a mod's temperature: `хранение: горячее | тёплое | холодное`.
pub const TAG: &str = "хранение:";

// @sat:storage.tier
/// A mod's temperature: enabled — hot, disabled — cold; the owner's tag over both.
pub fn mod_tier(enabled: bool, tags: &[String]) -> (Tier, Why) {
    for t in tags {
        let Some(v) = t.trim().strip_prefix(TAG) else {
            continue;
        };
        let v = v.trim().to_lowercase();
        let tier = match v.as_str() {
            "горячее" | "hot" => Tier::Hot,
            "тёплое" | "теплое" | "warm" => Tier::Warm,
            "холодное" | "cold" => Tier::Cold,
            _ => continue,
        };
        return (tier, Why::Owner);
    }
    if enabled {
        (Tier::Hot, Why::Enabled)
    } else {
        (Tier::Cold, Why::Disabled)
    }
}

/// One copy in a set of identical files (same path, size and bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Copy {
    /// Its mod is enabled.
    pub enabled: bool,
    pub tier: Tier,
    /// The file it is on disk (volume and file index; `0` — not known): copies with the same id are one file already.
    pub file: u128,
}

/// Which copies a link pass may replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Only copies of disabled mods: the files the game reads stay as they are.
    Disabled,
    /// Every copy but the keeper.
    All,
}

// @sat:storage.links
/// What a set of identical copies becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkPlan {
    /// The copy the others become links to: an enabled one where there is one.
    pub keeper: usize,
    /// Copies to replace by a link to the keeper.
    pub replace: Vec<usize>,
    /// The temperature of the keeper's bytes once linked: the hottest copy that shares them.
    pub tier: Tier,
}

/// The links for one set of identical copies (`None` — nothing to do).
pub fn link_set(copies: &[Copy], scope: Scope) -> Option<LinkPlan> {
    if copies.len() < 2 {
        return None;
    }
    let keeper = copies.iter().position(|c| c.enabled).unwrap_or(0);
    let k = copies[keeper];
    let same = |c: &Copy| k.file != 0 && c.file == k.file;
    let replace: Vec<usize> = (0..copies.len())
        .filter(|&i| i != keeper)
        .filter(|&i| !same(&copies[i]))
        .filter(|&i| scope == Scope::All || !copies[i].enabled)
        .collect();
    // the shared bytes are as hot as the hottest copy reading them
    let tier = copies
        .iter()
        .enumerate()
        .filter(|(i, c)| *i == keeper || same(c) || replace.contains(i))
        .map(|(_, c)| c.tier)
        .min()
        .unwrap_or(k.tier);
    if replace.is_empty() {
        return None;
    }
    Some(LinkPlan {
        keeper,
        replace,
        tier,
    })
}

// @sat:storage.compress
/// The codec a file still needs (`None` — leave it): compressible, big enough, not compressed yet.
pub fn needs(policy: &Policy, class: Class, size: u64, tier: Tier, compressed: bool) -> Option<Codec> {
    if compressed || size < policy.compress_min || !class.compressible() {
        return None;
    }
    policy.codec(tier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_say_what_may_share_bytes_and_be_compressed() {
        assert_eq!(class("Textures/Armor/x.DDS"), Class::Textures);
        assert!(class("meshes/a.nif").linkable());
        assert!(!class("Foo.esp").linkable() && !class("Foo.esp").compressible());
        assert!(!class("SKSE/Plugins/x.ini").compressible());
        assert!(class("CalienteTools/BodySlide/ShapeData/x.osd").compressible());
        assert!(Class::Textures.kept(Codec::Lzx) < Class::Textures.kept(Codec::Xpress8k));
    }

    #[test]
    fn temperature_follows_the_state_and_the_owner_overrides_it() {
        assert_eq!(mod_tier(true, &[]), (Tier::Hot, Why::Enabled));
        assert_eq!(mod_tier(false, &[]), (Tier::Cold, Why::Disabled));
        assert_eq!(
            mod_tier(true, &["хранение: холодное".into()]),
            (Tier::Cold, Why::Owner)
        );
        assert_eq!(
            mod_tier(false, &["хранение:теплое".into()]),
            (Tier::Warm, Why::Owner)
        );
        assert_eq!(mod_tier(true, &["хранение: ?".into()]), (Tier::Hot, Why::Enabled));
        let p = Policy::default();
        assert_eq!(p.codec(Tier::Hot), Some(Codec::Xpress8k));
        assert!(p.untouched("800 GENERATED OUTPUT_separator"));
    }

    #[test]
    fn disabled_copies_become_links_to_an_enabled_keeper() {
        let c = |enabled, file| Copy {
            enabled,
            tier: if enabled { Tier::Hot } else { Tier::Cold },
            file,
        };
        // off, on, off (already one file with the keeper), on
        let set = [c(false, 1), c(true, 2), c(false, 2), c(true, 3)];
        let p = link_set(&set, Scope::Disabled).unwrap();
        assert_eq!(p.keeper, 1, "the game's copy keeps its bytes");
        assert_eq!(
            p.replace,
            [0],
            "the other enabled copy is left alone in the first pass"
        );
        assert_eq!(p.tier, Tier::Hot, "the shared bytes are as hot as the keeper");
        let all = link_set(&set, Scope::All).unwrap();
        assert_eq!(all.replace, [0, 3]);
        // all disabled: the first is the keeper, the bytes stay cold
        let off = link_set(&[c(false, 0), c(false, 0)], Scope::Disabled).unwrap();
        assert_eq!(
            (off.keeper, off.replace.as_slice(), off.tier),
            (0, [1].as_slice(), Tier::Cold)
        );
        // nothing to do: every copy already one file
        assert!(link_set(&[c(true, 7), c(false, 7)], Scope::All).is_none());
    }

    #[test]
    fn a_file_needs_a_codec_once() {
        let p = Policy::default();
        assert_eq!(
            needs(&p, Class::Textures, 1 << 20, Tier::Cold, false),
            Some(Codec::Lzx)
        );
        assert_eq!(
            needs(&p, Class::Textures, 1 << 20, Tier::Hot, false),
            Some(Codec::Xpress8k)
        );
        assert_eq!(needs(&p, Class::Textures, 1 << 20, Tier::Cold, true), None);
        assert_eq!(needs(&p, Class::Config, 1 << 20, Tier::Cold, false), None);
        assert_eq!(needs(&p, Class::Meshes, 100, Tier::Cold, false), None);
        assert_eq!(Codec::from_name("lzx"), Some(Codec::Lzx));
    }
}
