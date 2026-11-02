//! Writing the patches output mod: each patched file read from its mod, edited, written into one folder. @sat:mo2.patches
//!
//! Owns: applying the project's file patches (`@sat:patch`) to the current files of their mods and building the
//! output mod (temp folder and a rename, as every output); checking a patch's base.
//! Not here: patches as data (`@sat:patch`), the ini model (`@sat:formats.ini`), the commit (`@sat:mo2.commit`).

use std::path::Path;

use sat_core::patch::{FilePatch, PatchOp, content_signature};
use sat_formats::ini::IniFile;

/// Whether the file a patch was made on is still the one in its mod (`Ok(false)` — it changed).
pub fn base_holds(mods_dir: &Path, p: &FilePatch) -> Result<bool, String> {
    let file = mods_dir.join(&p.source).join(&p.path);
    let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(p.base.as_deref().is_none_or(|b| b == content_signature(&bytes)))
}

/// The patched bytes of one file.
pub fn apply(bytes: &[u8], ops: &[PatchOp]) -> Vec<u8> {
    let mut f = IniFile::parse(bytes);
    for op in ops {
        match op {
            PatchOp::Set { section, key, value } => {
                f.set(section, key, value);
            }
            PatchOp::Remove { section, key } => {
                f.remove(section, key);
            }
        }
    }
    f.to_bytes()
}

// @sat:mo2.patches.build
/// Builds the output mod `target` (must not exist) from `patches` (sources by mod name). Returns the file count.
pub fn build(mods_dir: &Path, patches: &[FilePatch], target: &Path) -> Result<usize, String> {
    if target.exists() {
        return Err(format!("{} already exists", target.display()));
    }
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = target.with_file_name(format!(".{name}.writing"));
    let _ = std::fs::remove_dir_all(&tmp);
    let r = (|| {
        for p in patches {
            if p.path.split('/').any(|c| c == ".." || c.is_empty()) {
                return Err(format!("{}: unsafe path", p.path));
            }
            let src = mods_dir.join(&p.source).join(&p.path);
            let bytes = std::fs::read(&src).map_err(|e| format!("{}: {e}", src.display()))?;
            let out = tmp.join(&p.path);
            if let Some(d) = out.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            std::fs::write(&out, apply(&bytes, &p.ops)).map_err(|e| format!("{}: {e}", out.display()))?;
        }
        std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::write(
            tmp.join("meta.ini"),
            "[General]\nnotes=Patches kept in the Satellite project\n",
        )
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, target).map_err(|e| format!("{}: {e}", target.display()))?;
        Ok(patches.len())
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
    fn a_patched_copy_goes_to_the_output_and_the_original_stays() {
        let dir = tempfile::tempdir().unwrap();
        let mods = dir.path();
        let ini = mods.join("3BA").join("SKSE/Plugins/CBPConfig.txt");
        std::fs::create_dir_all(ini.parent().unwrap()).unwrap();
        std::fs::write(&ini, "[Breast]\nfStiffness=0.5\n").unwrap();
        let p = FilePatch {
            source: "3BA".into(),
            path: "SKSE/Plugins/CBPConfig.txt".into(),
            base: Some(content_signature(b"[Breast]\nfStiffness=0.5\n")),
            ops: vec![PatchOp::Set {
                section: "Breast".into(),
                key: "fStiffness".into(),
                value: "0.8".into(),
            }],
        };
        assert!(base_holds(mods, &p).unwrap());
        let out = mods.join("Satellite Patches");
        assert_eq!(build(mods, std::slice::from_ref(&p), &out).unwrap(), 1);
        assert_eq!(
            std::fs::read_to_string(out.join("SKSE/Plugins/CBPConfig.txt")).unwrap(),
            "[Breast]\nfStiffness=0.8\n"
        );
        assert_eq!(
            std::fs::read_to_string(&ini).unwrap(),
            "[Breast]\nfStiffness=0.5\n",
            "the original stays"
        );
        std::fs::write(&ini, "[Breast]\nfStiffness=0.6\n").unwrap();
        assert!(
            !base_holds(mods, &p).unwrap(),
            "the mod was updated: the base changed"
        );
    }
}
