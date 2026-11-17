//! Running a processor's tool and collecting what it wrote (ADR 0021 §3–5). @sat:mo2.processors
//!
//! Owns: one run of an external generator after the commit wrote the profile — a snapshot of Overwrite, the tool
//! started through MO2 (it sees the build in MO2's virtual file system and writes to Overwrite), what is new or
//! changed in Overwrite since the snapshot moved into the processor's output mod, the old output kept in
//! `deleted/<commit>` and put back when the run fails or brings nothing; whether MO2 is open (a running MO2 would
//! take the launch over and return at once, and would rewrite the profile).
//! Not here: what is dirty and the order (`@sat:processor`, `@sat:home.plan.processors`), MO2's tool list
//! (`@sat:mo2.executables`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// What to run: a processor whose inputs changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    /// The tool's MO2 title (`moshortcut://`).
    pub executable: String,
    /// The output mod's folder name.
    pub output: String,
    /// The fingerprint recorded when the run is promoted.
    pub fingerprint: String,
}

/// What a run did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    pub id: String,
    /// Files collected into the output.
    pub files: usize,
    /// Promoted: the new output is in place and the fingerprint can be recorded.
    pub promoted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Where runs happen.
#[derive(Debug, Clone)]
pub struct Place {
    pub mods_dir: PathBuf,
    pub overwrite_dir: PathBuf,
    /// `deleted/<commit>` of this commit: old outputs go there.
    pub deleted_dir: PathBuf,
}

type Snapshot = BTreeMap<PathBuf, (u64, Option<SystemTime>)>;

fn snapshot(root: &Path) -> Snapshot {
    let mut out = Snapshot::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            match e.metadata() {
                Ok(m) if m.is_dir() => stack.push(p),
                Ok(m) => {
                    if let Ok(rel) = p.strip_prefix(root) {
                        out.insert(rel.to_path_buf(), (m.len(), m.modified().ok()));
                    }
                }
                Err(_) => {}
            }
        }
    }
    out
}

fn io(p: &Path, e: std::io::Error) -> String {
    format!("{}: {e}", p.display())
}

// @sat:mo2.processors.run
/// Runs one processor: `launch` starts the tool and returns when it has finished. On success the new output
/// replaces the old one (kept in `deleted/`); on failure or an empty result the old output stays and Overwrite
/// gets back what was taken from it.
pub fn run(place: &Place, r: &Run, launch: &dyn Fn(&Run) -> Result<(), String>) -> Done {
    let done = |files, promoted, error: Option<String>| Done {
        id: r.id.clone(),
        files,
        promoted,
        error,
    };
    let before = snapshot(&place.overwrite_dir);
    if let Err(e) = launch(r) {
        return done(0, false, Some(format!("the tool did not run: {e}")));
    }
    let after = snapshot(&place.overwrite_dir);
    let fresh: Vec<&PathBuf> = after
        .iter()
        .filter(|(p, v)| before.get(*p) != Some(v))
        .map(|(p, _)| p)
        .collect();
    if fresh.is_empty() {
        return done(
            0,
            false,
            Some("the tool wrote nothing to Overwrite — the old output stays".into()),
        );
    }
    // collect into a new folder beside the output, then swap
    let target = place.mods_dir.join(&r.output);
    let staging = place.mods_dir.join(format!("{}.sat-new", r.output));
    let _ = std::fs::remove_dir_all(&staging);
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    let back = |moved: &[(PathBuf, PathBuf)]| {
        for (from, to) in moved.iter().rev() {
            if let Some(d) = from.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::rename(to, from);
        }
    };
    for rel in &fresh {
        let from = place.overwrite_dir.join(rel);
        let to = staging.join(rel);
        let step = to
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::rename(&from, &to));
        if let Err(e) = step {
            back(&moved);
            let _ = std::fs::remove_dir_all(&staging);
            return done(0, false, Some(io(&from, e)));
        }
        moved.push((from, to));
    }
    // the old output into deleted/, the new one in its place
    let kept = place.deleted_dir.join(&r.output);
    let had_old = target.exists();
    if had_old {
        let swap = std::fs::create_dir_all(&place.deleted_dir).and_then(|()| std::fs::rename(&target, &kept));
        if let Err(e) = swap {
            back(&moved);
            let _ = std::fs::remove_dir_all(&staging);
            return done(0, false, Some(io(&target, e)));
        }
    }
    if let Err(e) = std::fs::rename(&staging, &target) {
        if had_old {
            let _ = std::fs::rename(&kept, &target);
        }
        back(&moved);
        let _ = std::fs::remove_dir_all(&staging);
        return done(0, false, Some(io(&staging, e)));
    }
    done(fresh.len(), true, None)
}

// @sat:mo2.processors.launch
/// The real launcher: MO2 with the profile and the tool's `moshortcut://`, waited for.
pub fn launcher(mo2_exe: PathBuf, profile: String) -> impl Fn(&Run) -> Result<(), String> {
    move |r: &Run| {
        let (exe, args) = crate::executables::launch_command(&mo2_exe, &profile, &r.executable);
        let status = std::process::Command::new(&exe)
            .args(&args)
            .status()
            .map_err(|e| io(&exe, e))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("{} exited with {status}", exe.display()))
        }
    }
}

// @sat:mo2.processors.open
/// Whether MO2 is running on this machine (Windows: the process list). A running MO2 takes a `moshortcut` launch
/// over and returns at once, and writes its own profile back: processors do not run then.
pub fn mo2_running() -> bool {
    if !cfg!(windows) {
        return false;
    }
    std::process::Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq ModOrganizer.exe", "/NH"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .to_lowercase()
                .contains("modorganizer.exe")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(base: &Path) -> Place {
        let p = Place {
            mods_dir: base.join("mods"),
            overwrite_dir: base.join("overwrite"),
            deleted_dir: base.join("deleted").join("c1"),
        };
        std::fs::create_dir_all(&p.mods_dir).unwrap();
        std::fs::create_dir_all(&p.overwrite_dir).unwrap();
        p
    }

    fn run_of(output: &str) -> Run {
        Run {
            id: "pandora".into(),
            executable: "Pandora".into(),
            output: output.into(),
            fingerprint: "fp".into(),
        }
    }

    #[test]
    fn what_the_tool_writes_goes_into_the_output_and_the_old_one_is_kept() {
        let t = tempfile::tempdir().unwrap();
        let p = place(t.path());
        // something of the user's in Overwrite stays there
        std::fs::write(p.overwrite_dir.join("user.ini"), "x").unwrap();
        std::fs::create_dir_all(p.mods_dir.join("Pandora Output")).unwrap();
        std::fs::write(p.mods_dir.join("Pandora Output").join("old.hkx"), "old").unwrap();
        let ow = p.overwrite_dir.clone();
        let tool = move |_: &Run| {
            let d = ow.join("meshes").join("actors");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("0_master.hkx"), "new").unwrap();
            Ok(())
        };
        let d = run(&p, &run_of("Pandora Output"), &tool);
        assert!(d.promoted && d.files == 1, "{d:?}");
        let out = p.mods_dir.join("Pandora Output");
        assert!(out.join("meshes/actors/0_master.hkx").exists());
        assert!(!out.join("old.hkx").exists());
        assert!(
            p.deleted_dir.join("Pandora Output/old.hkx").exists(),
            "the old output is kept for the rollback"
        );
        assert!(
            p.overwrite_dir.join("user.ini").exists(),
            "what was there before stays"
        );
        assert!(!p.overwrite_dir.join("meshes/actors/0_master.hkx").exists());
    }

    #[test]
    fn a_failed_or_empty_run_keeps_the_old_output() {
        let t = tempfile::tempdir().unwrap();
        let p = place(t.path());
        std::fs::create_dir_all(p.mods_dir.join("Out")).unwrap();
        std::fs::write(p.mods_dir.join("Out").join("old"), "old").unwrap();
        let d = run(&p, &run_of("Out"), &|_| Err("no exe".into()));
        assert!(!d.promoted && d.error.is_some());
        let d = run(&p, &run_of("Out"), &|_| Ok(()));
        assert!(!d.promoted, "nothing written — nothing promoted");
        assert!(p.mods_dir.join("Out/old").exists());
    }
}
