//! The project file of a build, for the commands. @sat:cli.project
//!
//! Owns: finding and opening the project of a build (`<profile>/satellite/project.stll`), converting the early
//! layout once, and saving after a command changed the project.
//! Not here: the file itself (`@sat:stll.file`).

use std::path::Path;

use sat_core::project::{PROJECT_SCHEMA, Project};
use sat_stll::{FILE_NAME, Stll};

/// Opens the project at `path`; `None` when there is none yet. The early layout (`project.v0.json` and
/// `facts/` in the same folder) is converted into `project.stll` the first time.
pub fn open(path: &Path) -> Result<Option<(Stll, Project)>, String> {
    if path.exists() {
        return Stll::open(path)
            .map(Some)
            .map_err(|e| format!("{e} (this build reads project schema {PROJECT_SCHEMA})"));
    }
    match path.parent() {
        Some(dir) if path.file_name().is_some_and(|n| n == FILE_NAME) => sat_stll::open_dir(dir),
        _ => Ok(None),
    }
}

/// [`open`], for commands that need a project.
pub fn require(path: &Path) -> Result<(Stll, Project), String> {
    open(path)?.ok_or_else(|| format!("{}: no project — run `sat-cli analyze` first", path.display()))
}

/// Saves a changed project (stored facts are kept).
pub fn save(stll: &mut Stll, project: &Project) -> Result<(), String> {
    stll.save(project).map(|_| ()).map_err(|e| e.to_string())
}
