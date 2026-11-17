//! `sat-cli analyze --records --conflicts --facegen-fix NAME`: a generated mod with the record mods' own NPC faces. @sat:cli.facegen-fix
//!
//! Owns: writing the face files of `@sat:conflict.kinds.facegen.fixes` — each NPC's face as its record mod has it,
//! loose or unpacked from its archive — into a mod folder of its own, with a list of where every file came from.
//! Not here: which files (`@sat:conflict.kinds.facegen.fixes`), reading a copy (`@sat:scan.fetch.bytes`), placing
//! the mod in the order (the next `analyze` finds the new folder; `sat-cli sync` and `commit` add it).
//!
//! The fix is additive: nothing in the owner's mods changes, and unchecking the generated mod undoes it. It is a
//! snapshot of the order it was made for: after the order changes, it is made again (against the order without the
//! earlier fix). A folder is rewritten only
//! when it carries the list of a previous fix — a folder of anything else is never touched.

use std::path::Path;

use sat_core::table::{ProviderTable, Source};

/// The list in the generated folder: what it is, and every file with its source.
pub const MARKER: &str = "satellite-facegen-fix.txt";

/// Writes `fixes` (path, the copy to take) into `dir`; returns how many files were written.
pub fn write(
    dir: &Path,
    folders: &sat_scan::fetch::Folders,
    table: &ProviderTable,
    fixes: &[(sat_core::ids::PathId, usize)],
) -> Result<usize, String> {
    if dir.exists() {
        if !dir.join(MARKER).is_file() {
            return Err(format!(
                "{} exists and is not a face fix made by Satellite (no {MARKER}): not touched",
                dir.display()
            ));
        }
        std::fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut list = String::from(
        "Satellite: NPC faces as their record mods have them, above the copies that beat them.\n\
         Generated; uncheck the mod to undo, make it again after the order changes.\n\n",
    );
    let mut written = 0;
    for &(p, i) in fixes {
        let rel = table.path_string(p);
        let m = table.mod_name(table.owner(i));
        let archive = match table.source(i) {
            Source::Archive(a) => Some(table.archives()[a.0 as usize].file_name.clone()),
            Source::Loose => None,
        };
        let Some(bytes) = sat_scan::fetch::read_copy(folders, m, &rel, archive.as_deref()) else {
            list.push_str(&format!(
                "NOT READ  {rel}  <- {m}{}\n",
                archive.as_deref().map(|a| format!(" [{a}]")).unwrap_or_default()
            ));
            continue;
        };
        let out = rel.split(['/', '\\']).fold(dir.to_path_buf(), |d, s| d.join(s));
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&out, bytes).map_err(|e| format!("{}: {e}", out.display()))?;
        list.push_str(&format!(
            "{rel}  <- {m}{}\n",
            archive.as_deref().map(|a| format!(" [{a}]")).unwrap_or_default()
        ));
        written += 1;
    }
    std::fs::write(dir.join(MARKER), list).map_err(|e| format!("{MARKER}: {e}"))?;
    Ok(written)
}
