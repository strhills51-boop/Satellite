//! The Satellite project on disk (ADR 0007). @sat:stll
//!
//! - [`file`] — the `.stll` file: one zip with a manifest, the project as NDJSON sections and each mod's facts;
//!   opened without the build, saved atomically, keeps what it does not know;
//! - [`sections`] — the rule that cuts the project tree into sections and joins them back;
//! - [`facts`] — the per-mod facts format (paths, sizes, metrics, archive entries, plugin headers) by entity id,
//!   with the content signature they were made from; also the early folder layout, read to migrate it.
//!
//! Owns: the on-disk layout of a project. Machine binding (where the build is) stays outside the file.

pub mod facts;
pub mod file;
pub mod sections;

pub use file::{FILE_NAME, Stll, StllError};

/// Opens the project of a satellite folder: `project.stll`, or — once — the early folder layout
/// (`project.v0.json` + `facts/`), converted into `project.stll` (the old files are left as they were).
/// `Ok(None)` when the folder holds no project.
pub fn open_dir(dir: &std::path::Path) -> Result<Option<(Stll, sat_core::project::Project)>, String> {
    let path = dir.join(FILE_NAME);
    if path.exists() {
        return Stll::open(&path).map(Some).map_err(|e| e.to_string());
    }
    let old = dir.join("project.v0.json");
    let Ok(bytes) = std::fs::read(&old) else {
        return Ok(None);
    };
    let project =
        sat_core::project::Project::from_json(&bytes).map_err(|e| format!("{}: {e}", old.display()))?;
    migrate(dir, &path, &project)?;
    Stll::open(&path).map(Some).map_err(|e| e.to_string())
}

/// Rewrites a project file without its facts (kept: everything else). A commit's version folder keeps the
/// user's truth this way; facts are rebuildable by a scan.
pub fn strip_facts(path: &std::path::Path) -> Result<(), String> {
    let (mut f, project) = Stll::open(path).map_err(|e| e.to_string())?;
    let w = f.begin().map_err(|e| e.to_string())?;
    w.finish(&project, |_| false)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// @sat:stll.migrate
fn migrate(
    dir: &std::path::Path,
    path: &std::path::Path,
    project: &sat_core::project::Project,
) -> Result<(), String> {
    let store = facts::FactsStore::new(dir.join("facts"));
    let mut f = Stll::new(path);
    let mut w = f.begin().map_err(|e| e.to_string())?;
    let ids = project
        .entities
        .iter()
        .map(|e| e.id.clone())
        .chain(std::iter::once(sat_core::entity::EntityId(
            file::OVERWRITE_ID.into(),
        )));
    for id in ids {
        if let Some((sig, facts)) = store.get(&id) {
            w.put_facts(&id.0, sig, &facts).map_err(|e| e.to_string())?;
        }
    }
    let saved = w
        .finish(project, file::keep_entities(project))
        .map_err(|e| e.to_string())?;
    eprintln!(
        "project: {} converted into {} ({} mods' facts); the old files stay as they were",
        dir.join("project.v0.json").display(),
        path.display(),
        saved.facts_written
    );
    Ok(())
}
