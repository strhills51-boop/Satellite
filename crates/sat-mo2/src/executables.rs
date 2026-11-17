//! The tools a build has registered in MO2 (`ModOrganizer.ini [customExecutables]`). @sat:mo2.executables
//!
//! Owns: reading the registered executables (title, binary, arguments, working folder) and the command line that
//! runs one of them inside MO2's virtual file system (`moshortcut://`), so a generator sees the build as the game
//! does.
//! Not here: what a processor is and when it is dirty (`@sat:processor`), running it in a commit (`@sat:mo2.commit`).
//!
//! Paths are this machine's: the project names a tool by its MO2 title, never by a path (PHILOSOPHY «Указатели»).

use std::path::{Path, PathBuf};

use crate::ini;

/// A tool registered in MO2.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Executable {
    /// The name MO2 shows and `moshortcut://` takes.
    pub title: String,
    pub binary: PathBuf,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub arguments: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<PathBuf>,
}

impl Executable {
    /// The file name of the binary, lowercased (what presets recognise a tool by).
    pub fn file_name(&self) -> String {
        self.binary
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    }
}

// @sat:mo2.executables.read
/// The executables of an instance's `ModOrganizer.ini`, in MO2's order. A missing or unreadable ini — none.
pub fn read(ini_path: &Path) -> Vec<Executable> {
    let Ok(data) = std::fs::read(ini_path) else {
        return Vec::new();
    };
    parse(&data)
}

/// Parses `[customExecutables]`: `size=N`, then `i\title`, `i\binary`, `i\arguments`, `i\workingDirectory`.
pub fn parse(data: &[u8]) -> Vec<Executable> {
    let ini = ini::parse(data);
    let Some(sec) = ini.get("customexecutables") else {
        return Vec::new();
    };
    let n: usize = sec.get("size").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    let get = |i: usize, k: &str| {
        sec.get(&format!("{i}\\{k}"))
            .map(|v| ini::unquote(v))
            .unwrap_or_default()
    };
    (1..=n)
        .filter_map(|i| {
            let title = get(i, "title");
            let binary = get(i, "binary");
            if title.is_empty() || binary.is_empty() {
                return None;
            }
            let wd = get(i, "workingdirectory");
            Some(Executable {
                title,
                binary: PathBuf::from(binary.replace('/', std::path::MAIN_SEPARATOR_STR)),
                arguments: get(i, "arguments"),
                working_dir: (!wd.is_empty())
                    .then(|| PathBuf::from(wd.replace('/', std::path::MAIN_SEPARATOR_STR))),
            })
        })
        .collect()
}

// @sat:mo2.executables.launch
/// The command that runs a registered tool inside MO2's virtual file system: MO2 itself with the profile and a
/// `moshortcut://` link to the tool (MO2 waits for the tool and exits). `mo2_exe` is `ModOrganizer.exe` of the build.
pub fn launch_command(mo2_exe: &Path, profile: &str, title: &str) -> (PathBuf, Vec<String>) {
    (
        mo2_exe.to_path_buf(),
        vec!["-p".into(), profile.into(), format!("moshortcut://:{title}")],
    )
}

/// `ModOrganizer.exe` of an instance: next to a portable instance's ini, else in its base folder.
pub fn mo2_exe(ini_path: &Path, base_dir: &Path) -> Option<PathBuf> {
    [ini_path.parent(), Some(base_dir)]
        .into_iter()
        .flatten()
        .map(|d| d.join("ModOrganizer.exe"))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_tools_are_read_as_mo2_writes_them() {
        let ini = b"[General]\r\ngameName=Skyrim Special Edition\r\n\r\n[customExecutables]\r\nsize=3\r\n\
1\\arguments=\r\n1\\binary=C:/Modding/Tools/BodySlide/BodySlide x64.exe\r\n1\\title=BodySlide x64\r\n1\\workingDirectory=\r\n\
2\\arguments=\"-o \\\"D:\\\\Out\\\"\"\r\n2\\binary=C:/Modding/Tools/Pandora/Pandora Behaviour Engine+.exe\r\n2\\title=Pandora\r\n\
2\\workingDirectory=C:/Modding/Tools/Pandora\r\n3\\binary=\r\n3\\title=broken\r\n";
        let ex = parse(ini);
        assert_eq!(ex.len(), 2, "an entry without a binary is skipped");
        assert_eq!(ex[0].title, "BodySlide x64");
        assert_eq!(ex[0].file_name(), "bodyslide x64.exe");
        assert_eq!(ex[1].title, "Pandora");
        assert!(ex[1].working_dir.is_some());
        let (exe, args) = launch_command(Path::new("C:/MO2/ModOrganizer.exe"), "Default", "Pandora");
        assert_eq!(exe, Path::new("C:/MO2/ModOrganizer.exe"));
        assert_eq!(args, ["-p", "Default", "moshortcut://:Pandora"]);
    }
}
