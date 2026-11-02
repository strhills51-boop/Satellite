//! Where a mod comes from: its identity on a site, as the manager recorded it (ADR 0007, 0015). @sat:source
//!
//! Owns: a mod's external identity (site, game, mod id, version, the newest version known, installation file),
//! whether an update is known, and the link to its page — the pointer the site and future intents use.
//! Not here: reading it from MO2 (`@sat:mo2.meta`), entities (`@sat:entity`), installing (`@sat:intent`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModSource {
    /// `nexus` for now (MO2 records Nexus identity).
    pub site: String,
    /// The manager's game name (`SkyrimSE`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mod_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newest_version: Option<String>,
    /// The archive it was installed from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// A page the manager recorded for a mod not on Nexus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Nexus game domains for the manager's game names.
const NEXUS_GAMES: &[(&str, &str)] = &[
    ("skyrimse", "skyrimspecialedition"),
    ("skyrimspecialedition", "skyrimspecialedition"),
    ("skyrim", "skyrim"),
    ("skyrimvr", "skyrimspecialedition"),
    ("fallout4", "fallout4"),
    ("fallout4vr", "fallout4"),
    ("falloutnv", "newvegas"),
    ("fallout3", "fallout3"),
    ("oblivion", "oblivion"),
    ("morrowind", "morrowind"),
    ("starfield", "starfield"),
    ("enderal", "enderal"),
    ("enderalse", "enderalspecialedition"),
];

fn clean(v: &str) -> &str {
    v.trim().trim_start_matches(['d', 'v', 'V'])
}

impl ModSource {
    pub fn is_empty(&self) -> bool {
        self.mod_id.is_none() && self.url.is_none() && self.file.is_none()
    }

    // @sat:source.update
    /// The manager knows a newer version than the installed one.
    pub fn update_available(&self) -> bool {
        match (&self.version, &self.newest_version) {
            (Some(v), Some(n)) => !n.trim().is_empty() && clean(v) != clean(n),
            _ => false,
        }
    }

    // @sat:source.link
    /// The mod's page: Nexus by game and id, else the recorded URL.
    pub fn link(&self) -> Option<String> {
        if self.site == "nexus"
            && let (Some(id), Some(game)) = (self.mod_id, &self.game)
            && let Some((_, domain)) = NEXUS_GAMES.iter().find(|(g, _)| g.eq_ignore_ascii_case(game))
        {
            return Some(format!("https://www.nexusmods.com/{domain}/mods/{id}"));
        }
        self.url.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_and_update() {
        let s = ModSource {
            site: "nexus".into(),
            game: Some("SkyrimSE".into()),
            mod_id: Some(12604),
            version: Some("d1.0.2".into()),
            newest_version: Some("1.0.3".into()),
            ..ModSource::default()
        };
        assert_eq!(
            s.link().as_deref(),
            Some("https://www.nexusmods.com/skyrimspecialedition/mods/12604")
        );
        assert!(s.update_available());
        let same = ModSource {
            newest_version: Some("v1.0.2".into()),
            ..s.clone()
        };
        assert!(!same.update_available());
    }
}
