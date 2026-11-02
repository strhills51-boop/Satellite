//! Donor builds on this machine, and taking a mod folder out of one. @sat:mo2.donors
//!
//! Owns: the machine binding of donors (`<profile>/satellite/donors.json`: name → mods folder — the project names a
//! donor, this file says where it is), counting a folder, and copying or moving a donor's mod folder into the build
//! with a check that what arrived is what left.
//! Not here: matching a donor against the project (`@sat:sync.donor`), the commit around it (`@sat:mo2.commit`).
//!
//! Paths of a machine never go into the project (PHILOSOPHY «Указатели»): the same project on another computer
//! names the same donor, bound there to its own path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// File of the machine binding, in `<profile>/satellite/`.
pub const DONORS_FILE: &str = "donors.json";

/// Donors this machine knows: name → mods folder.
pub fn load(profile_dir: &Path) -> BTreeMap<String, PathBuf> {
    std::fs::read(profile_dir.join("satellite").join(DONORS_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Binds a donor name to a mods folder on this machine.
pub fn bind(profile_dir: &Path, name: &str, mods_dir: &Path) -> Result<(), String> {
    let mut all = load(profile_dir);
    all.insert(name.to_string(), mods_dir.to_path_buf());
    let dir = profile_dir.join("satellite");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let data = serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(DONORS_FILE), data).map_err(|e| format!("{DONORS_FILE}: {e}"))
}

// @sat:mo2.donors.count
/// Files and bytes under a folder.
pub fn count(dir: &Path) -> std::io::Result<(u64, u64)> {
    let (mut files, mut bytes) = (0, 0);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let e = e?;
            let t = e.file_type()?;
            if t.is_dir() {
                stack.push(e.path());
            } else if t.is_file() {
                files += 1;
                bytes += e.metadata()?.len();
            }
        }
    }
    Ok((files, bytes))
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let t = e.file_type()?;
        if t.is_dir() {
            copy_tree(&src, &dst)?;
        } else if t.is_file() {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

// @sat:mo2.donors.take
/// Copies the donor folder `from` to `to` (a new folder) and checks that the copy holds as many files and bytes as
/// the source; a copy that differs is removed and reported. With `moved`, the folder is renamed instead (the same
/// drive only: a move across drives would need a copy and a delete that cannot be undone as one step).
pub fn take(from: &Path, to: &Path, moved: bool) -> Result<(), String> {
    let io = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    if to.exists() {
        return Err(format!("{} already exists", to.display()));
    }
    if moved {
        return std::fs::rename(from, to).map_err(|e| {
            format!(
                "{}: {e} — a donor on another drive can only be copied (drop --move)",
                from.display()
            )
        });
    }
    let want = count(from).map_err(|e| io(from, e))?;
    if let Err(e) = copy_tree(from, to) {
        let _ = std::fs::remove_dir_all(to);
        return Err(io(from, e));
    }
    let got = count(to).map_err(|e| io(to, e))?;
    if got != want {
        let _ = std::fs::remove_dir_all(to);
        return Err(format!(
            "{}: the copy has {} files / {} bytes, the donor {} / {} — removed",
            to.display(),
            got.0,
            got.1,
            want.0,
            want.1
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_donor_folder_is_copied_and_checked_or_moved() {
        let t = std::env::temp_dir().join(format!("sat-donors-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let src = t.join("donor").join("Mod A");
        std::fs::create_dir_all(src.join("textures")).unwrap();
        std::fs::write(src.join("a.esp"), b"TES4").unwrap();
        std::fs::write(src.join("textures").join("x.dds"), vec![0u8; 300]).unwrap();
        assert_eq!(count(&src).unwrap(), (2, 304));

        let dst = t.join("mods").join("Mod A");
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        take(&src, &dst, false).unwrap();
        assert_eq!(count(&dst).unwrap(), (2, 304));
        assert!(src.exists(), "a copy leaves the donor as it was");
        assert!(take(&src, &dst, false).is_err(), "never over an existing folder");

        let moved = t.join("mods").join("Mod A2");
        take(&src, &moved, true).unwrap();
        assert!(!src.exists() && moved.join("a.esp").exists());

        let profile = t.join("profile");
        bind(&profile, "Old build", &t.join("donor")).unwrap();
        assert_eq!(load(&profile)["Old build"], t.join("donor"));
        std::fs::remove_dir_all(&t).unwrap();
    }
}
