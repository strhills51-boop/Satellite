//! The game pack: everything Satellite knows about one game, as data (ADR 0018 §4). @sat:game
//!
//! Owns: the pack's shape (base plugins and their order, the Creation Club rule, index limits, archive rules,
//! dangerous record signatures, the FaceGen layout, the game's conflict domain specs), the built-in packs (embedded
//! JSON, Skyrim SE/AE first), and the active pack of the process — chosen once from the manager's game name.
//! Not here: what uses it (`@sat:plugins`, `@sat:conflict.kinds`, `@sat:conflict.spec`), reading plugins
//! (`@sat:formats.plugin`).
//!
//! The core reads the pack, never a constant about a game: a second game is a second pack (and, when its formats
//! differ, an adapter the pack names).

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::conflict_spec::DomainSpec;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BasePlugin {
    pub name: String,
    #[serde(default)]
    pub masters: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginRules {
    /// The plugin format adapter (`tes4`: Bethesda TES4-style plugins).
    pub format: String,
    /// The game's own masters, in their fixed load order.
    pub base: Vec<BasePlugin>,
    /// Creation Club plugins start with it (shipped in the game's data, listed in `loadorder.txt` only).
    #[serde(default)]
    pub creation_club_prefix: Option<String>,
    /// How many full plugins load (runtime indexes `00`…`limit-1`).
    pub full_limit: u32,
    /// How many light plugins load.
    pub light_limit: u32,
    /// Plugins with an older header version are of another edition of the game (Skyrim LE in SE).
    #[serde(default)]
    pub min_header: Option<f32>,
    /// Object ids a light plugin's new records may take, by header version (the last that applies counts).
    #[serde(default)]
    pub light_ranges: Vec<LightRange>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LightRange {
    /// From this header version on.
    pub from_header: f32,
    pub low: u32,
    pub high: u32,
}

impl PluginRules {
    /// The object id range of a light plugin with this header version.
    pub fn light_range(&self, header: Option<f32>) -> Option<(u32, u32)> {
        let h = header.unwrap_or(0.0);
        self.light_ranges
            .iter()
            .rfind(|r| h >= r.from_header)
            .map(|r| (r.low, r.high))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArchiveRules {
    /// Archive file extensions.
    pub exts: Vec<String>,
    /// A loose file beats any archive copy, whatever the mod order.
    pub loose_over_archive: bool,
    /// An archive loads with the plugin of its name (`Name.bsa`, `Name - Textures.bsa` with `Name.esp`), in the
    /// plugin order; an archive without a loaded plugin does not load.
    pub by_plugin: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordRules {
    /// The record header flag "deleted".
    pub deleted_flag: u32,
    /// Signatures whose deletion breaks the game (references, navmeshes): to be disabled, never deleted.
    pub deleted_watch: Vec<String>,
    /// Signatures two plugins must not both edit (navmeshes): no merge exists.
    pub exclusive: Vec<String>,
    /// Fields that are arrays of fixed-size elements (keywords: 4-byte references): compared as collections.
    #[serde(default)]
    pub arrays: std::collections::BTreeMap<String, usize>,
    /// Fields derived from others (counts of a list): not compared.
    #[serde(default)]
    pub derived: Vec<String>,
}

/// A tool that builds an output mod from the build (LOD, behaviors, patches): its output must come after every
/// input, or it was built without them (ADR 0018 §5, generations).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Generator {
    pub id: String,
    pub title: String,
    /// A mod with a path matching one of these is the tool's output (`*` — within a folder, `**` — any depth).
    pub markers: Vec<String>,
    /// A mod with a path matching one of these is an input.
    pub inputs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FaceGen {
    /// The record whose face files must come with it.
    pub record: String,
    /// Face files, `{plugin}` — the record's own plugin, `{formid}` — its object id, 8 hex digits.
    pub files: Vec<String>,
}

// @sat:game.pack
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GamePack {
    pub id: String,
    pub title: String,
    /// Names the manager uses for the game (MO2 `gameName`).
    pub mo2_names: Vec<String>,
    pub plugins: PluginRules,
    #[serde(default)]
    pub archives: Option<ArchiveRules>,
    #[serde(default)]
    pub records: Option<RecordRules>,
    #[serde(default)]
    pub facegen: Option<FaceGen>,
    /// The game's conflict domains described by data (`@sat:conflict.spec`).
    #[serde(default)]
    pub domains: Vec<DomainSpec>,
    /// Tools whose outputs go stale (`@sat:conflict.kinds.generations`).
    #[serde(default)]
    pub generators: Vec<Generator>,
    /// The script extender and its address library (`@sat:conflict.kinds.runtime`).
    #[serde(default)]
    pub runtime: Option<RuntimeRules>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeRules {
    /// The game's executable in its folder (its file version is the game version).
    pub exe: String,
    /// The extender's loader library in the game folder; its captures are the game version it is for.
    pub loader: String,
    /// Extender plugins in mods.
    pub plugins: String,
    /// Address library files, `{v}` — the game version as `1-6-1170`; one of them must be in an active mod.
    pub address_library: Vec<String>,
}

// @sat:game.glob
/// A path glob as a case-insensitive regex: `**` any depth, `*` within one segment, `?` one character.
pub fn glob(g: &str) -> regex::Regex {
    let mut re = String::from("(?i)^");
    let mut it = g.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '*' if it.peek() == Some(&'*') => {
                it.next();
                if it.peek() == Some(&'/') {
                    it.next();
                    re.push_str("(?:.*/)?");
                } else {
                    re.push_str(".*");
                }
            }
            '*' => re.push_str("[^/]*"),
            '?' => re.push_str("[^/]"),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    regex::Regex::new(&re).expect("a glob compiles")
}

const BUILTIN: &[&str] = &[include_str!("../games/skyrimse.json")];

/// The built-in packs.
pub fn builtin() -> Vec<GamePack> {
    BUILTIN
        .iter()
        .map(|s| serde_json::from_str(s).expect("a built-in game pack parses"))
        .collect()
}

/// The built-in pack for the manager's game name, if any.
pub fn for_game_name(name: &str) -> Option<GamePack> {
    let n = name.trim().to_lowercase();
    builtin()
        .into_iter()
        .find(|p| p.id == n || p.mo2_names.iter().any(|m| m.to_lowercase() == n))
}

static ACTIVE: OnceLock<GamePack> = OnceLock::new();

/// Chooses the process's pack (once; later calls are ignored). Unknown games keep the default.
pub fn select(name: &str) {
    if let Some(p) = for_game_name(name) {
        let _ = ACTIVE.set(p);
    }
}

/// The process's pack: the one selected, else Skyrim SE/AE.
pub fn active() -> &'static GamePack {
    ACTIVE.get_or_init(|| builtin().remove(0))
}

impl GamePack {
    pub fn is_creation_club(&self, name: &str) -> bool {
        let Some(prefix) = &self.plugins.creation_club_prefix else {
            return false;
        };
        let n = name.to_ascii_lowercase();
        let Some((stem, ext)) = n.rsplit_once('.') else {
            return false;
        };
        matches!(ext, "esl" | "esm" | "esp")
            && stem.len() > prefix.len()
            && stem.starts_with(prefix.as_str())
            && stem
                .bytes()
                .skip(prefix.len())
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    }

    /// Position of a base plugin in the game's fixed order.
    pub fn base_position(&self, name: &str) -> Option<usize> {
        self.plugins
            .base
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case(name.trim()))
    }

    /// Whether `file_name` is an archive of this game.
    pub fn is_archive(&self, file_name: &str) -> bool {
        let l = file_name.to_ascii_lowercase();
        self.archives
            .as_ref()
            .is_some_and(|a| a.exts.iter().any(|e| l.ends_with(&format!(".{e}"))))
    }

    /// The plugin stem an archive loads with: `Name - Textures.bsa` → `name`, `Name.bsa` → `name`.
    pub fn archive_stem(file_name: &str) -> String {
        let l = file_name.to_ascii_lowercase();
        let stem = l.rsplit_once('.').map_or(l.as_str(), |x| x.0);
        stem.split_once(" - ").map_or(stem, |x| x.0).trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skyrim_pack_parses_and_is_found_by_the_managers_name() {
        let p = for_game_name("Skyrim Special Edition").unwrap();
        assert_eq!(p.id, "skyrimse");
        assert_eq!(p.plugins.base[0].name, "Skyrim.esm");
        assert_eq!(p.plugins.full_limit, 254);
        assert!(p.is_creation_club("ccBGSSSE001-Fish.esm"));
        assert!(!p.is_creation_club("cc.esp"));
        assert_eq!(p.base_position("update.esm"), Some(1));
        assert!(p.is_archive("Foo - Textures.BSA"));
        assert_eq!(GamePack::archive_stem("Foo - Textures.bsa"), "foo");
        assert_eq!(GamePack::archive_stem("Foo.bsa"), "foo");
        assert!(p.domains.iter().any(|d| d.id == "oar"));
        assert!(for_game_name("Starfield").is_none());
        assert_eq!(p.plugins.light_range(Some(1.7)), Some((0x800, 0xFFF)));
        assert_eq!(p.plugins.light_range(Some(1.71)), Some((0, 0xFFF)));
        assert!(glob("meshes/**/*.nif").is_match("Meshes/a/b/c.NIF"));
        assert!(glob("meshes/**/*.nif").is_match("meshes/c.nif"));
        assert!(!glob("*.esp").is_match("a/b.esp"));
        assert!(glob("*.esp").is_match("DynDOLOD.esp"));
        assert_eq!(active().id, "skyrimse");
    }
}
