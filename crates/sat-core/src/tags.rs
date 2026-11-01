//! Tag catalog: collections of tag definitions with colours, and the colour of any tag. @sat:tags
//!
//! Owns: tag collections (built-in «Системные теги», «Стикеры», the user's own), a tag's colour — explicit in the
//! catalog, else by its semantic group (monolith `TAG_GROUP_COLOR_RULES`), else the neutral default.
//! Not here: which entry carries which tag (`@sat:order.decor.marks`), tags found by analysis (future facts layer).
//!
//! Monolith reference: `createDefaultBuiltInTags`, `createDefaultStickerCollection`, `getTagColorBySemanticGroup`
//! (area 17). The monolith's per-name colour table (`TAG_COLORS`, ~200 names) is not copied: its groups give the
//! same hue for nearly every name, and a user's explicit colour in the catalog always wins.

use serde::{Deserialize, Serialize};

/// Neutral tag colour (monolith `DEFAULT_TAG_COLOR`).
pub const DEFAULT_TAG_COLOR: u32 = 0x475569;

/// One tag definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagDef {
    pub name: String,
    /// `None`: the semantic group decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,
    /// Short caption (stickers: «Избранное»).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagCollection {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub tags: Vec<TagDef>,
}

fn yes() -> bool {
    true
}

// @sat:tags.catalog
/// All collections. A project without a catalog gets the built-in ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TagCatalog(pub Vec<TagCollection>);

impl Default for TagCatalog {
    fn default() -> Self {
        let def = |n: &str| TagDef {
            name: n.to_string(),
            color: None,
            short: None,
        };
        let sticker = |n: &str, short: &str| TagDef {
            name: n.to_string(),
            color: None,
            short: Some(short.to_string()),
        };
        Self(vec![
            TagCollection {
                id: "builtins".into(),
                name: "Системные теги".into(),
                builtin: true,
                enabled: true,
                tags: [
                    "OAR",
                    "DAR",
                    "OStim",
                    "SL",
                    "SkyPatcher",
                    "BOS",
                    "MCO",
                    "RaceMenu",
                    "SkyUI",
                    "SkyHUD",
                    "Camera",
                    "SKSE",
                    "Inventory Injector",
                    "Payload Interpreter",
                    "Community Shaders",
                    "MCM",
                    "UI",
                    "animation",
                    "NPC",
                    "World",
                    "equipment",
                    "enemies",
                    "utilities",
                    "conversation",
                    "quest",
                    "skin",
                    "character-assets",
                ]
                .into_iter()
                .map(def)
                .collect(),
            },
            TagCollection {
                id: "stickers".into(),
                name: "Стикеры".into(),
                builtin: true,
                enabled: true,
                tags: vec![
                    sticker("⭐", "Избранное"),
                    sticker("🔥", "Важно"),
                    sticker("📌", "Закрепить"),
                    sticker("⚠️", "Предупреждение"),
                ],
            },
        ])
    }
}

/// Semantic groups: a tag whose words include one of these gets the group's colour (monolith, first match wins).
const GROUPS: &[(u32, &[&str])] = &[
    (
        0xf97316,
        &[
            "skse",
            "dll",
            "papyrus",
            "jcontainers",
            "consoleutil",
            "po3",
            "address library",
        ],
    ),
    (
        0x64748b,
        &[
            "patch",
            "compat",
            "addon",
            "fix",
            "synthesis",
            "bashed",
            "smashed",
            "xedit",
            "plugin",
            "archive",
            "fomod",
        ],
    ),
    (
        0x8b5cf6,
        &[
            "oar",
            "dar",
            "fnis",
            "nemesis",
            "pandora",
            "animation",
            "behavior",
            "idle",
            "gesture",
            "xpmsse",
            "skeleton",
        ],
    ),
    (
        0xef4444,
        &[
            "combat",
            "mco",
            "scar",
            "precision",
            "dodge",
            "valhalla",
            "wildcat",
            "smilodon",
            "blade",
            "mortal enemies",
        ],
    ),
    (
        0x3b82f6,
        &[
            "ui",
            "hud",
            "mcm",
            "skyui",
            "skyhud",
            "ihud",
            "morehud",
            "dear diary",
            "nordic",
            "untarnished",
            "scaleform",
            "menu",
            "compass",
        ],
    ),
    (
        0x22c55e,
        &[
            "enb",
            "reshade",
            "shader",
            "parallax",
            "texture",
            "mesh",
            "water",
            "weather",
            "lighting",
            "dyndolod",
            "texgen",
            "lod",
            "occlusion",
            "visual",
            "grass",
            "tree",
        ],
    ),
    (
        0x14b8a6,
        &[
            "world",
            "landscape",
            "architecture",
            "settlement",
            "city",
            "interior",
            "dungeon",
            "home",
            "road",
            "lux",
            "elfx",
            "embers",
            "cells",
            "worldspace",
            "house",
        ],
    ),
    (
        0xec4899,
        &[
            "racemenu",
            "character",
            "body",
            "skin",
            "cbbe",
            "3ba",
            "bhunp",
            "unp",
            "himbo",
            "sos",
            "smp",
            "cbpc",
            "bodyslide",
            "head",
            "facegen",
            "npc",
            "follower",
            "hair",
            "eyes",
            "overlay",
            "serana",
            "bijin",
            "pandorable",
            "kalilies",
        ],
    ),
    (
        0xf59e0b,
        &[
            "equipment",
            "armor",
            "weapon",
            "clothing",
            "jewelry",
            "cloak",
            "backpack",
            "accessories",
            "inventory",
            "loot",
            "container",
            "crafting",
        ],
    ),
    (
        0xa855f7,
        &[
            "quest",
            "dialogue",
            "conversation",
            "guild",
            "faction",
            "start",
            "lorkhan",
            "lotd",
        ],
    ),
    (
        0xfacc15,
        &["audio", "voice", "voiced", "dbvo", "xvasynth", "music", "dolby"],
    ),
    (
        0x6366f1,
        &[
            "perk",
            "magic",
            "spell",
            "shout",
            "enairim",
            "ordinator",
            "apocalypse",
            "summermyst",
            "imperious",
            "thunderchild",
            "wintersun",
            "simonrim",
        ],
    ),
    (
        0x92400e,
        &[
            "survival",
            "economy",
            "immersion",
            "frostfall",
            "campfire",
            "hunterborn",
            "rnd",
            "requiem",
        ],
    ),
    (
        0xf97316,
        &[
            "enemies",
            "creatures",
            "animals",
            "dragons",
            "draugr",
            "bandits",
            "mihail",
            "4thunknown",
            "obis",
            "pets",
            "wildlife",
            "zombie",
        ],
    ),
    (
        0xe11d48,
        &[
            "sexlab",
            "slal",
            "ostim",
            "osex",
            "devious",
            "zaz",
            "flower girls",
            "nsfw",
            "bdsm",
            "arousal",
        ],
    ),
];

/// Colours the monolith treats as "no colour chosen" (its neutral greys).
fn neutral(c: u32) -> bool {
    matches!(c, DEFAULT_TAG_COLOR | 0x64748b | 0x334155 | 0x374151)
}

/// Words of a tag: `_` and `-` are spaces (monolith `normalizeTagNameKey`), lower case.
fn words(tag: &str) -> String {
    let t: String = tag
        .chars()
        .map(|c| if c == '_' || c == '-' { ' ' } else { c })
        .collect();
    format!(
        " {} ",
        t.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
    )
}

/// The colour of a tag by its semantic group (word match, like the monolith's `\b…\b`).
pub fn group_color(tag: &str) -> Option<u32> {
    let w = words(tag);
    GROUPS
        .iter()
        .find(|(_, keys)| keys.iter().any(|k| w.contains(&format!(" {k} "))))
        .map(|(c, _)| *c)
}

impl TagCatalog {
    pub fn find(&self, name: &str) -> Option<&TagDef> {
        let k = words(name);
        self.0
            .iter()
            .filter(|c| c.enabled)
            .flat_map(|c| &c.tags)
            .find(|t| words(&t.name) == k)
    }

    // @sat:tags.color
    /// A tag's colour: an explicit, non-neutral one from the catalog, else its semantic group, else the default.
    pub fn color(&self, name: &str) -> u32 {
        match self.find(name).and_then(|t| t.color) {
            Some(c) if !neutral(c) => c,
            explicit => group_color(name).or(explicit).unwrap_or(DEFAULT_TAG_COLOR),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_come_from_the_catalog_then_the_group() {
        let mut cat = TagCatalog::default();
        assert_eq!(cat.color("SKSE"), 0xf97316);
        assert_eq!(
            cat.color("character-assets"),
            0xec4899,
            "`-` is a space: the word `character`"
        );
        assert_eq!(
            cat.color("texture landscape"),
            0x22c55e,
            "first group wins (texture before landscape)"
        );
        assert_eq!(
            cat.color("Textures Landscape"),
            0x14b8a6,
            "whole words: `textures` is not `texture`"
        );
        assert_eq!(
            cat.color("ui_fix"),
            0x64748b,
            "`fix` comes before `ui` in the monolith's order"
        );
        assert_eq!(cat.color("my stuff"), DEFAULT_TAG_COLOR);
        assert_eq!(cat.color("skyuiish"), DEFAULT_TAG_COLOR, "whole words only");
        cat.0[0].tags.push(TagDef {
            name: "My Stuff".into(),
            color: Some(0x123456),
            short: None,
        });
        assert_eq!(cat.color("my_stuff"), 0x123456);
        // the stickers are there by default
        assert_eq!(cat.find("⭐").and_then(|t| t.short.as_deref()), Some("Избранное"));
    }
}
