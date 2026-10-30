//! MO2 instance resolution.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Mo2Error, Result, ini, read};

/// A resolved MO2 instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    pub ini_path: PathBuf,
    pub base_dir: PathBuf,
    pub game_name: String,
    pub game_path: PathBuf,
    pub mods_dir: PathBuf,
    pub overwrite_dir: PathBuf,
    pub profiles_dir: PathBuf,
    pub profile: String,
    pub profiles: Vec<String>,
}

impl Instance {
    /// Loads from a `ModOrganizer.ini` path or the instance directory containing it.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let p = path.as_ref();
        let p = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        let ini_path = if p.is_dir() {
            p.join("ModOrganizer.ini")
        } else {
            p.clone()
        };
        if !ini_path.is_file() {
            return Err(Mo2Error::NotInstance(format!(
                "ModOrganizer.ini not found at {}",
                ini_path.display()
            )));
        }
        let ini = ini::parse(&read(&ini_path)?);
        let inst_dir = ini_path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let empty = Default::default();
        let settings = ini.get("settings").unwrap_or(&empty);
        let general = ini.get("general").unwrap_or(&empty);
        let get = |m: &std::collections::BTreeMap<String, String>, k: &str| {
            m.get(k).map(|v| ini::unquote(v)).unwrap_or_default()
        };

        let base = get(settings, "base_directory");
        let base_dir = if base.is_empty() {
            inst_dir.clone()
        } else {
            normalize(&inst_dir, &base)
        };
        let resolve = |key: &str, default: &str| -> PathBuf {
            let v = get(settings, key);
            if v.is_empty() {
                return base_dir.join(default);
            }
            let v = v.replace("%BASE_DIR%", &base_dir.to_string_lossy());
            normalize(&inst_dir, &v)
        };
        let mods_dir = resolve("mod_directory", "mods");
        let overwrite_dir = resolve("overwrite_directory", "overwrite");
        let profiles_dir = resolve("profiles_directory", "profiles");
        let mut profiles: Vec<String> = std::fs::read_dir(&profiles_dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        profiles.sort();
        let mut profile = get(general, "selected_profile");
        if profile.is_empty() {
            profile = profiles.first().cloned().unwrap_or_default();
        }
        Ok(Self {
            ini_path,
            base_dir,
            game_name: get(general, "gamename"),
            game_path: PathBuf::from(get(general, "gamepath").replace('/', std::path::MAIN_SEPARATOR_STR)),
            mods_dir,
            overwrite_dir,
            profiles_dir,
            profile,
            profiles,
        })
    }

    pub fn profile_dir(&self) -> PathBuf {
        self.profiles_dir.join(&self.profile)
    }

    /// Folder of a mod. An instance has exactly one mods folder, as in MO2 and the monolith; other folders
    /// with mods are external donors (ADR 0006), never extra roots of the build.
    pub fn mod_dir(&self, name: &str) -> PathBuf {
        self.mods_dir.join(name)
    }
}

fn normalize(inst_dir: &Path, v: &str) -> PathBuf {
    let v = v.replace(['/', '\\'], std::path::MAIN_SEPARATOR_STR);
    let p = PathBuf::from(v);
    if p.is_absolute() { p } else { inst_dir.join(p) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_defaults_and_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("profiles/Default")).unwrap();
        std::fs::create_dir_all(root.join("profiles/Alt")).unwrap();
        std::fs::create_dir_all(root.join("mods/A")).unwrap();
        std::fs::write(
            root.join("ModOrganizer.ini"),
            "[General]\r\ngameName=Skyrim Special Edition\r\nselected_profile=@ByteArray(Default)\r\n[Settings]\r\noverwrite_directory=%BASE_DIR%/ow\r\n",
        )
        .unwrap();
        let i = Instance::load(root).unwrap();
        assert_eq!(i.game_name, "Skyrim Special Edition");
        assert_eq!(i.profile, "Default");
        assert_eq!(i.profiles, ["Alt", "Default"]);
        assert_eq!(i.mods_dir, std::path::absolute(root.join("mods")).unwrap());
        assert!(i.overwrite_dir.ends_with("ow"));
        assert_eq!(i.mod_dir("A"), i.mods_dir.join("A"));
    }

    #[test]
    fn missing_ini() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            Instance::load(dir.path()),
            Err(Mo2Error::NotInstance(_))
        ));
    }
}
