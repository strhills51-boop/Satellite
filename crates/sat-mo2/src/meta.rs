//! A mod's `meta.ini`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ini;

/// The subset of `meta.ini` Satellite uses. MO2 writes Nexus identity here, which bootstraps mod identity
/// for the knowledge layer (PHILOSOPHY §8 Фаза 4 «Идентичность модов»).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModMeta {
    pub game_name: Option<String>,
    pub mod_id: Option<u64>,
    pub version: Option<String>,
    pub newest_version: Option<String>,
    pub url: Option<String>,
    pub installation_file: Option<String>,
    pub category: Option<String>,
    pub notes: Option<String>,
    /// Separator color as `#rrggbb`.
    pub color: Option<String>,
}

impl ModMeta {
    pub fn parse(data: &[u8]) -> Self {
        let ini = ini::parse(data);
        let Some(g) = ini.get("general") else {
            return Self::default();
        };
        let s = |k: &str| g.get(k).map(|v| ini::unquote(v)).filter(|v| !v.is_empty());
        Self {
            game_name: s("gamename"),
            mod_id: s("modid").and_then(|v| v.parse().ok()).filter(|&id| id > 0),
            version: s("version"),
            newest_version: s("newestversion"),
            url: s("url"),
            installation_file: s("installationfile"),
            category: s("category"),
            notes: s("notes"),
            color: g.get("color").and_then(|v| ini::decode_qcolor(v)),
        }
    }

    /// Reads `<dir>/meta.ini`; a missing file yields the default.
    pub fn read(dir: &Path) -> Self {
        std::fs::read(dir.join("meta.ini"))
            .map(|d| Self::parse(&d))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nexus_identity_and_color() {
        let m = ModMeta::parse(
            b"[General]\r\ngameName=SkyrimSE\r\nmodid=12604\r\nversion=d1.0.2\r\ninstallationFile=SkyUI_5_2_SE.7z\r\ncolor=@Variant(\\0\\0\\0\\x43\\x1\\xff\\xff\\x80\\x80\\x30\\x30\\xd0\\xd0\\0\\0)\r\n",
        );
        assert_eq!(m.mod_id, Some(12604));
        assert_eq!(m.version.as_deref(), Some("d1.0.2"));
        assert_eq!(m.color.as_deref(), Some("#8030d0"));
        assert_eq!(ModMeta::parse(b"[General]\nmodid=0\n").mod_id, None);
    }
}
