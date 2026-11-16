//! Home: the chain over the build on this machine — project, run, plan, commit (ADR 0019). @sat:home
//!
//! PHILOSOPHY «Спутник»: Satellite changes mods only by a commit, and the commit happens at home.
//!
//! Owns: the chain «sync → processing → commit» over one build, once for every shell — the CLI and the window call
//! the same functions: the project (`@sat:home.project`), the run (`@sat:home.run`), the full plan
//! (`@sat:home.plan`), the commit (`@sat:home.commit`).
//! Not here: what commands and queries mean (`@sat:app` — it does not depend on this crate, ADR 0004), the plan's
//! rules (`@sat:sync.plan`), the manager's files and the commit transaction (`@sat:mo2`), formats, the scan.

pub mod commit;
pub mod plan;
pub mod project;
pub mod run;

/// Folder names directly under `dir`, sorted (none when it is not there).
pub fn subfolders(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}
