//! MO2 instance resolution. @sat:mo2.instance
//!
//! Owns: MO2 instance resolution: folders, profiles, ini or layout fallback.

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
    /// Where MO2 keeps downloaded archives (`download_directory`).
    pub downloads_dir: PathBuf,
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
            // Non-portable MO2 keeps its ini in %LOCALAPPDATA%\ModOrganizer\<instance>, naming this folder as its
            // base directory; without it the folders themselves are enough (monolith model).
            if p.is_dir()
                && let Some(global) = global_ini_for(&p)
            {
                return Self::load(global);
            }
            if p.is_dir() && p.join("mods").is_dir() && p.join("profiles").is_dir() {
                let mut i = Self::from_dirs(p.join("mods"), p.join("profiles"), p.join("overwrite"), None);
                i.game_path = find_game(PathBuf::new(), &p, &p, "");
                return Ok(i);
            }
            return Err(Mo2Error::NotInstance(format!(
                "neither ModOrganizer.ini nor mods/ + profiles/ found at {}{}",
                p.display(),
                instances_inside(&p)
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
        let downloads_dir = resolve("download_directory", "downloads");
        let profiles = list_dirs(&profiles_dir);
        let mut profile = get(general, "selected_profile");
        if profile.is_empty() {
            profile = profiles.first().cloned().unwrap_or_default();
        }
        let game_path = find_game(
            PathBuf::from(get(general, "gamepath").replace('/', std::path::MAIN_SEPARATOR_STR)),
            &inst_dir,
            &base_dir,
            &get(general, "gamename"),
        );
        Ok(Self {
            ini_path,
            base_dir,
            game_name: get(general, "gamename"),
            game_path,
            mods_dir,
            overwrite_dir,
            profiles_dir,
            downloads_dir,
            profile,
            profiles,
        })
    }

    /// An instance from explicit folders, without `ModOrganizer.ini` — what the monolith asks the user for.
    /// The profile defaults to `Default` when present, otherwise the first one.
    pub fn from_dirs(
        mods_dir: PathBuf,
        profiles_dir: PathBuf,
        overwrite_dir: PathBuf,
        profile: Option<&str>,
    ) -> Self {
        let abs = |p: PathBuf| std::path::absolute(&p).unwrap_or(p);
        let (mods_dir, profiles_dir, overwrite_dir) = (abs(mods_dir), abs(profiles_dir), abs(overwrite_dir));
        let profiles = list_dirs(&profiles_dir);
        let profile = profile
            .map(str::to_string)
            .or_else(|| profiles.iter().find(|p| p.as_str() == "Default").cloned())
            .or_else(|| profiles.first().cloned())
            .unwrap_or_default();
        let base_dir = mods_dir.parent().map(Path::to_path_buf).unwrap_or_default();
        let downloads_dir = base_dir.join("downloads");
        Self {
            ini_path: PathBuf::new(),
            base_dir,
            game_name: String::new(),
            game_path: PathBuf::new(),
            mods_dir,
            overwrite_dir,
            profiles_dir,
            downloads_dir,
            profile,
            profiles,
        }
    }

    pub fn profile_dir(&self) -> PathBuf {
        self.profiles_dir.join(&self.profile)
    }

    // @sat:mo2.instance.missing
    /// Why the profile has no `modlist.txt`, said so the user can fix the path: which profile was read, which
    /// profiles there are, and MO2 instances in the subfolders (a folder of several instances is a common mistake).
    pub fn missing_modlist(&self) -> String {
        let mut out = format!(
            "{}: no modlist.txt (profile \"{}\")",
            self.profile_dir().display(),
            self.profile
        );
        if !self.profiles.is_empty() {
            out += &format!("; profiles here: {}", self.profiles.join(", "));
        }
        out + &instances_inside(&self.base_dir)
    }

    // @sat:mo2.instance.documents
    /// The game's folder in the user's documents (`My Games\\Skyrim Special Edition`: its ini, logs, saves),
    /// looked up in `Documents` and OneDrive's documents; the named game's first, else any known one there.
    pub fn documents_dir(&self) -> Option<PathBuf> {
        let home = PathBuf::from(std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?);
        let docs = [
            home.join("Documents"),
            home.join("OneDrive").join("Documents"),
            home.join("OneDrive").join("Документы"),
        ];
        let g = self.game_name.to_ascii_lowercase();
        let names: &[&str] = if g.contains("fallout 4") {
            &["Fallout4"]
        } else if g.contains("starfield") {
            &["Starfield"]
        } else if g == "skyrim" {
            &["Skyrim"]
        } else {
            &[
                "Skyrim Special Edition",
                "Skyrim Special Edition GOG",
                "Skyrim VR",
                "Fallout4",
                "Starfield",
            ]
        };
        names
            .iter()
            .flat_map(|n| docs.iter().map(move |d| d.join("My Games").join(n)))
            .find(|p| p.is_dir())
    }

    /// Folder of a mod. An instance has exactly one mods folder, as in MO2 and the monolith; other folders
    /// with mods are external donors (ADR 0006), never extra roots of the build.
    pub fn mod_dir(&self, name: &str) -> PathBuf {
        self.mods_dir.join(name)
    }
}

fn list_dirs(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Whether a folder looks like a game root: a `Data` folder with a master plugin in it.
fn is_game_root(p: &Path) -> bool {
    std::fs::read_dir(p.join("Data"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            e.file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".esm")
        })
}

// @sat:mo2.instance.game
/// The game's folder: the instance's `gamePath` when it holds the game, else a portable copy next to the instance
/// (`Stock Game`, `Game Root`, `Game`), else the game in a Steam library or a usual GOG folder, else what the instance
/// says (it may be empty; callers say so).
fn find_game(said: PathBuf, inst_dir: &Path, base_dir: &Path, game_name: &str) -> PathBuf {
    if !said.as_os_str().is_empty() && is_game_root(&said) {
        return said;
    }
    let near = ["Stock Game", "Game Root", "Game", "Stock Folder"];
    for dir in [inst_dir, base_dir] {
        for n in near {
            let p = dir.join(n);
            if is_game_root(&p) {
                return p;
            }
        }
    }
    installed_games(game_name).into_iter().next().unwrap_or(said)
}

/// Folder names a game installs to, the named game first (an instance without an ini names none).
fn game_folders(game_name: &str) -> Vec<&'static str> {
    const ALL: [(&str, &[&str]); 6] = [
        (
            "skyrim special edition",
            &["Skyrim Special Edition", "Skyrim Anniversary Edition"],
        ),
        ("fallout 4", &["Fallout 4"]),
        ("skyrim", &["Skyrim"]),
        ("starfield", &["Starfield"]),
        ("new vegas", &["Fallout New Vegas"]),
        ("fallout 3", &["Fallout 3", "Fallout 3 goty"]),
    ];
    let g = game_name.to_ascii_lowercase();
    let mut out: Vec<&str> = Vec::new();
    for (n, dirs) in ALL {
        if !g.is_empty() && g.contains(n) {
            out.extend(dirs.iter());
            return out;
        }
    }
    for (_, dirs) in ALL {
        out.extend(dirs.iter());
    }
    out
}

/// Steam libraries (`libraryfolders.vdf` of the usual Steam folders) and usual GOG folders holding the game.
fn installed_games(game_name: &str) -> Vec<PathBuf> {
    let mut libs: Vec<PathBuf> = Vec::new();
    let mut steams: Vec<PathBuf> = Vec::new();
    for v in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(d) = std::env::var_os(v) {
            steams.push(PathBuf::from(d).join("Steam"));
        }
    }
    for d in ['C', 'D', 'E', 'F'] {
        steams.push(PathBuf::from(format!("{d}:\\Steam")));
        steams.push(PathBuf::from(format!("{d}:\\SteamLibrary")));
    }
    for s in &steams {
        libs.push(s.clone());
        if let Ok(t) = std::fs::read_to_string(s.join("steamapps").join("libraryfolders.vdf")) {
            libs.extend(steam_libraries(&t));
        }
    }
    let mut out = Vec::new();
    for name in game_folders(game_name) {
        for l in &libs {
            let p = l.join("steamapps").join("common").join(name);
            if is_game_root(&p) && !out.contains(&p) {
                out.push(p);
            }
        }
        for d in ['C', 'D', 'E'] {
            let p = PathBuf::from(format!("{d}:\\GOG Games\\{name}"));
            if is_game_root(&p) && !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// The `"path"` values of Steam's `libraryfolders.vdf`.
fn steam_libraries(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|l| {
            let mut q = l.split('"').filter(|x| !x.trim().is_empty());
            if q.next()? != "path" {
                return None;
            }
            Some(PathBuf::from(q.next()?.replace("\\\\", "\\")))
        })
        .collect()
}

/// A global instance's ini (`%LOCALAPPDATA%\ModOrganizer\*\ModOrganizer.ini`) whose base directory is `dir`.
fn global_ini_for(dir: &Path) -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    global_ini_in(&PathBuf::from(local).join("ModOrganizer"), dir)
}

fn global_ini_in(root: &Path, dir: &Path) -> Option<PathBuf> {
    let norm = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_lowercase();
    let want = norm(&dir.to_string_lossy());
    std::fs::read_dir(root).ok()?.flatten().find_map(|e| {
        let ini_path = e.path().join("ModOrganizer.ini");
        let ini = ini::parse(&read(&ini_path).ok()?);
        let base = ini
            .get("settings")?
            .get("base_directory")
            .map(|v| ini::unquote(v))?;
        (norm(&base) == want).then_some(ini_path)
    })
}

/// "; MO2 instances inside: …" for the subfolders of `dir` that are instances, else empty.
fn instances_inside(dir: &Path) -> String {
    let mut near: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.join("ModOrganizer.ini").is_file() || (p.join("mods").is_dir() && p.join("profiles").is_dir())
        })
        .map(|p| p.display().to_string())
        .collect();
    near.sort();
    if near.is_empty() {
        String::new()
    } else {
        format!("; MO2 instances inside: {} — give one of them", near.join(", "))
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
    fn folder_layout_without_ini() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("profiles/Alt")).unwrap();
        std::fs::create_dir_all(root.join("profiles/Default")).unwrap();
        std::fs::create_dir_all(root.join("mods/A")).unwrap();
        let i = Instance::load(root).unwrap();
        assert_eq!(i.profile, "Default");
        assert_eq!(i.mods_dir, std::path::absolute(root.join("mods")).unwrap());
        assert!(i.overwrite_dir.ends_with("overwrite"));
        assert!(i.ini_path.as_os_str().is_empty());
    }

    #[test]
    fn missing_ini() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            Instance::load(dir.path()),
            Err(Mo2Error::NotInstance(_))
        ));
    }

    #[test]
    fn a_portable_game_next_to_the_instance_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("profiles/Default")).unwrap();
        std::fs::create_dir_all(root.join("mods")).unwrap();
        std::fs::create_dir_all(root.join("Stock Game/Data")).unwrap();
        std::fs::write(root.join("Stock Game/Data/Skyrim.esm"), b"x").unwrap();
        std::fs::write(
            root.join("ModOrganizer.ini"),
            "[General]\ngamePath=@ByteArray(Z:\\\\nowhere)\n",
        )
        .unwrap();
        let i = Instance::load(root).unwrap();
        assert!(i.game_path.ends_with("Stock Game"), "{}", i.game_path.display());
    }

    #[test]
    fn a_global_instance_is_found_by_its_base_directory_and_steam_libraries_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("ModOrganizer");
        std::fs::create_dir_all(local.join("Other")).unwrap();
        std::fs::create_dir_all(local.join("SSE")).unwrap();
        std::fs::write(
            local.join("Other/ModOrganizer.ini"),
            "[Settings]\nbase_directory=Y:/x\n",
        )
        .unwrap();
        std::fs::write(
            local.join("SSE/ModOrganizer.ini"),
            "[Settings]\nbase_directory=X:/MO2/SSE/\n",
        )
        .unwrap();
        let found = global_ini_in(&local, Path::new("X:\\MO2\\SSE")).unwrap();
        assert!(found.starts_with(local.join("SSE")));
        assert!(global_ini_in(&local, Path::new("X:\\MO2\\FO4")).is_none());
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t\t\"label\"\t\t\"\"\n";
        assert_eq!(steam_libraries(vdf), [PathBuf::from("D:\\SteamLibrary")]);
        assert_eq!(
            game_folders("Skyrim Special Edition")[0],
            "Skyrim Special Edition"
        );
        assert_eq!(game_folders("")[0], "Skyrim Special Edition");
    }

    #[test]
    fn a_folder_of_instances_names_them() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("SSE/profiles/Default")).unwrap();
        std::fs::create_dir_all(root.join("SSE/mods")).unwrap();
        let Err(Mo2Error::NotInstance(e)) = Instance::load(root) else {
            panic!()
        };
        assert!(e.contains("MO2 instances inside") && e.contains("SSE"), "{e}");
        // an instance whose profile has no modlist.txt says which profile and where to look
        std::fs::create_dir_all(root.join("profiles/Default")).unwrap();
        std::fs::create_dir_all(root.join("mods")).unwrap();
        let i = Instance::load(root).unwrap();
        let m = i.missing_modlist();
        assert!(
            m.contains("no modlist.txt") && m.contains("\"Default\"") && m.contains("SSE"),
            "{m}"
        );
    }
}
