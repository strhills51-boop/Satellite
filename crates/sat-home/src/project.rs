//! The project of a build on this machine: open, import, re-import, save. @sat:home.project
//!
//! Owns: finding and opening the project of a build (`<profile>/satellite/project.stll`), the first import from
//! `modlist.txt`, the re-import that keeps what the user decided, converting the early layout once (only when the
//! caller may write), and saving after a change.
//! Not here: the file itself (`@sat:stll.file`), the project's meaning (`@sat:project`).

use std::path::{Path, PathBuf};

use sat_core::canon::Modlist;
use sat_core::project::{ManagerDiff, PROJECT_SCHEMA, Project};
use sat_mo2::{Instance, profile};
use sat_stll::{FILE_NAME, Stll};

/// Where the project of an instance's profile lives.
pub fn default_path(inst: &Instance) -> PathBuf {
    inst.profile_dir().join("satellite").join(FILE_NAME)
}

/// The manager's list of the profile (`modlist.txt`).
pub fn manager_list(inst: &Instance) -> Result<Modlist, String> {
    let path = inst.profile_dir().join("modlist.txt");
    match std::fs::read(&path) {
        Ok(data) => Ok(profile::parse_modlist(&data).0),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(inst.missing_modlist()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

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

/// [`open`] without writing anything: the early layout is read as it is, not converted (the sandbox).
pub fn open_read_only(path: &Path) -> Result<Option<(Stll, Project)>, String> {
    if path.exists() {
        return open(path);
    }
    let early = path.with_file_name("project.v0.json");
    Ok(std::fs::read(early)
        .ok()
        .and_then(|b| Project::from_json(&b).ok())
        .map(|p| (Stll::new(path), p)))
}

/// [`open`], for commands that need a project.
pub fn require(path: &Path) -> Result<(Stll, Project), String> {
    open(path)?.ok_or_else(|| format!("{}: no project — run `sat-cli analyze` first", path.display()))
}

/// Saves a changed project (stored facts are kept).
pub fn save(stll: &mut Stll, project: &Project) -> Result<(), String> {
    stll.save(project).map(|_| ()).map_err(|e| e.to_string())
}

/// What the project file was when a program last read or wrote it: size and modification time. Another program
/// writing it in between (`sat-cli analyze` while the window is open) changes the stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub len: u64,
    pub modified: Option<std::time::SystemTime>,
}

/// The stamp of the file now; `None` when there is no file.
pub fn stamp_of(path: &Path) -> Option<Stamp> {
    let md = std::fs::metadata(path).ok()?;
    Some(Stamp {
        len: md.len(),
        modified: md.modified().ok(),
    })
}

/// Why a save did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    /// The file changed since `expected` (another program wrote it): saving would overwrite that.
    ChangedOutside,
    Io(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ChangedOutside => f.write_str(
                "the project file changed since it was read (another program wrote it) — saving would overwrite that",
            ),
            Self::Io(e) => f.write_str(e),
        }
    }
}

// @sat:home.project.save
/// Saves `project` to `path` unless the file changed since `expected` (the stamp when it was read or last saved;
/// `None` — there was no file) and `force` is not given. Stored facts are kept. Returns the new stamp.
pub fn save_checked(
    path: &Path,
    project: &Project,
    expected: Option<Stamp>,
    force: bool,
) -> Result<Stamp, SaveError> {
    if !force && stamp_of(path) != expected {
        return Err(SaveError::ChangedOutside);
    }
    let mut stll = if path.exists() {
        Stll::open(path).map_err(|e| SaveError::Io(e.to_string()))?.0
    } else {
        Stll::new(path)
    };
    stll.save(project).map_err(|e| SaveError::Io(e.to_string()))?;
    stamp_of(path).ok_or_else(|| SaveError::Io(format!("{}: not there after saving", path.display())))
}

/// How the project of a run came to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// The stored project.
    Stored,
    /// The canon taken anew from `modlist.txt`, everything else kept.
    Reimported,
    /// No project yet: imported from `modlist.txt`.
    Imported,
}

/// A build's project, ready for a run.
#[derive(Debug)]
pub struct Opened {
    pub path: PathBuf,
    pub stll: Stll,
    pub project: Project,
    pub origin: Origin,
    /// `modlist.txt` as the manager has it: a side, not the canon (ADR 0006).
    pub manager: Modlist,
    pub manager_diff: ManagerDiff,
}

/// How to open the project for a run.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenOptions {
    /// Take the canon from `modlist.txt` again (`--reimport`).
    pub reimport: bool,
    /// Nothing may be written: the early layout is read, not converted (the sandbox).
    pub read_only: bool,
}

/// Opens (or imports) the project at `path` against the manager's list. The project is the canon (ADR 0006);
/// `modlist.txt` is a side — the first run imports it, `reimport` takes its canon again.
pub fn open_for_run(path: &Path, manager: Modlist, o: OpenOptions) -> Result<Opened, String> {
    let found = if o.read_only {
        open_read_only(path)?
    } else {
        open(path)?
    };
    let (stll, stored) = match found {
        Some((f, p)) if p.schema == PROJECT_SCHEMA || !o.read_only => (f, Some(p)),
        // the sandbox reads an early project it cannot convert as no project
        Some((f, _)) => (f, None),
        None => (Stll::new(path), None),
    };
    let (project, origin) = match stored {
        Some(p) if !o.reimport => (p, Origin::Stored),
        Some(old) => (reimport(&manager, old), Origin::Reimported),
        None => (Project::import(manager.clone()), Origin::Imported),
    };
    let manager_diff = project.manager_diff(&manager);
    Ok(Opened {
        path: path.to_path_buf(),
        stll,
        project,
        origin,
        manager,
        manager_diff,
    })
}

/// Re-import keeps what the project learned and what the user decided (signatures, sync decisions, decoration,
/// settled chains), replaces only the canon. Decoration of names no longer in the canon is reported when the book
/// applies it.
fn reimport(manager: &Modlist, old: Project) -> Project {
    let mut p = Project::import(manager.clone());
    // entities keep their ids by name, so everything keyed by id still finds them
    p.entities = old.entities;
    p.entities.follow(&p.canon);
    p.signatures = old.signatures;
    p.sync = old.sync;
    p.decor = old.decor;
    p.decisions = old.decisions;
    p.plugins = old.plugins;
    p.conflict_ignore = old.conflict_ignore;
    p.tags = old.tags;
    p.intents = old.intents;
    p.sticker_rules = old.sticker_rules;
    p.counters = old.counters;
    p.sources = old.sources;
    p.new_mods = old.new_mods;
    p.extra = old.extra;
    p
}

impl Opened {
    /// What the shell says about where the project came from.
    pub fn origin_line(&self) -> String {
        match self.origin {
            Origin::Stored => format!("project: {}", self.path.display()),
            Origin::Reimported => format!(
                "project: re-imported canon from modlist.txt into {}",
                self.path.display()
            ),
            Origin::Imported => format!(
                "project: imported from modlist.txt (first run) → {}",
                self.path.display()
            ),
        }
    }
}
