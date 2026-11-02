//! Baking a group into one mod folder: the group's resulting files, the last member winning. @sat:mo2.bake
//!
//! Owns: walking the members' folders in canon order, keeping for each path (any letter case) the file of the last
//! member that has it — the one the game would load — and putting them into a new folder through a temp folder
//! and a rename; hard links when asked (falling back to a copy where a link is refused).
//! Not here: when to bake (`@sat:factory`), the commit around it (`@sat:mo2.commit`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn walk(root: &Path, rel: &mut PathBuf, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for e in std::fs::read_dir(root.join(&*rel))? {
        let e = e?;
        let t = e.file_type()?;
        rel.push(e.file_name());
        if t.is_dir() {
            walk(root, rel, out)?;
        } else if t.is_file() {
            out.push(rel.clone());
        }
        rel.pop();
    }
    Ok(())
}

// @sat:mo2.bake.run
/// Builds `target` (which must not exist) from the folders of `members` (canon order) in `mods_dir`. Returns the
/// number of files. Members' `meta.ini` stay theirs; the output gets its own.
pub fn bake(mods_dir: &Path, members: &[String], target: &Path, link: bool) -> Result<usize, String> {
    if target.exists() {
        return Err(format!("{} already exists", target.display()));
    }
    let io = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    // path (folded) → (member folder, relative path); later members win
    let mut winner: HashMap<String, (PathBuf, PathBuf)> = HashMap::new();
    for m in members {
        let dir = mods_dir.join(m);
        let mut files = Vec::new();
        walk(&dir, &mut PathBuf::new(), &mut files).map_err(|e| io(&dir, e))?;
        for rel in files {
            let key = rel.to_string_lossy().replace('\\', "/").to_lowercase();
            if key == "meta.ini" {
                continue;
            }
            winner.insert(key, (dir.clone(), rel));
        }
    }
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = target.with_file_name(format!(".{name}.baking"));
    let _ = std::fs::remove_dir_all(&tmp);
    let r = (|| {
        std::fs::create_dir_all(&tmp).map_err(|e| io(&tmp, e))?;
        for (dir, rel) in winner.values() {
            let (src, dst) = (dir.join(rel), tmp.join(rel));
            if let Some(p) = dst.parent() {
                std::fs::create_dir_all(p).map_err(|e| io(p, e))?;
            }
            let linked = link && std::fs::hard_link(&src, &dst).is_ok();
            if !linked {
                std::fs::copy(&src, &dst).map_err(|e| io(&src, e))?;
            }
        }
        std::fs::write(
            tmp.join("meta.ini"),
            format!(
                "[General]\nnotes=Built by Satellite from: {}\n",
                members.join(", ")
            ),
        )
        .map_err(|e| io(&tmp, e))?;
        std::fs::rename(&tmp, target).map_err(|e| io(target, e))?;
        Ok(winner.len())
    })();
    if r.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_member_wins_and_links_save_space() {
        let dir = tempfile::tempdir().unwrap();
        let mods = dir.path();
        for (m, files) in [
            ("A", vec![("textures/a.dds", "A"), ("Meshes/x.nif", "A")]),
            ("B", vec![("textures/A.dds", "B")]),
        ] {
            for (f, body) in files {
                let p = mods.join(m).join(f);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, body).unwrap();
            }
            std::fs::write(mods.join(m).join("meta.ini"), "[General]\n").unwrap();
        }
        let out = mods.join("AB");
        assert_eq!(bake(mods, &["A".into(), "B".into()], &out, true).unwrap(), 2);
        let tex = std::fs::read_dir(out.join("textures"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            std::fs::read_to_string(tex).unwrap(),
            "B",
            "B is later: its texture wins"
        );
        assert_eq!(std::fs::read_to_string(out.join("Meshes/x.nif")).unwrap(), "A");
        assert!(
            std::fs::read_to_string(out.join("meta.ini"))
                .unwrap()
                .contains("A, B")
        );
        assert!(
            bake(mods, &["A".into()], &out, false).is_err(),
            "never over an existing folder"
        );
    }
}
