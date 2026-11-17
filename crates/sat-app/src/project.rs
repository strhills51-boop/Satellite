//! The app's truth and the project: what a project gives the app, what the app gives back. @sat:app.project
//!
//! Owns: the project as loaded (its sections the window does not edit) and the one mapping between the app's truth (book, settled chains, ignores, plugin canon, sync records, tags,
//! chain identities / layout / lineage, pins, file patches) and the sections of a [`Project`] — read when a build or a project
//! opens, written back before a run (the run reconciles the canon the user has now) and when the project is saved.
//! Not here: the project file (`@sat:stll.file`, `@sat:home.project`), the project's meaning (`@sat:project`).

use sat_core::project::Project;
use serde::{Deserialize, Serialize};

use crate::App;

/// Saving the project, as the window shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectState {
    /// The shell may write the project (`--write`); otherwise the window is a sandbox.
    pub writable: bool,
    /// Where the project file is (empty without one).
    pub path: String,
    /// The project now differs from what was last saved (or read).
    pub dirty: bool,
    /// A save is asked and not done yet.
    pub saving: bool,
    /// Why the last save did not happen.
    pub error: Option<String>,
    /// The file changed outside the window since it was read: saving overwrites that (`force`).
    pub changed_outside: bool,
    /// How many saves this window made.
    pub saves: u32,
}

#[derive(Debug, Default)]
pub(crate) struct SaveSlot {
    pub state: ProjectState,
    /// The project as last saved or read: `dirty` compares with it.
    saved: Option<Project>,
    request: Option<bool>,
}

impl App {
    /// What the app takes from a project besides the canon (the session is built from its book): settled chains,
    /// member keys, conflict ignores, the plugin canon, sync records, tags, chain identities, layout and lineage,
    /// pins, stickers, sources, counters. Returns the problems of rules that did not compile.
    pub fn load_project(&mut self, p: &Project, disk: Vec<String>) -> Vec<String> {
        self.project_base = Some(p.clone());
        let session = self.session();
        session.set_member_keys(p.member_keys());
        session.set_decisions(p.decisions.clone());
        session.set_conflict_ignore(p.conflict_ignore.clone());
        session.set_plugin_canon(p.plugins.clone());
        self.set_sync(p.sync.clone(), disk);
        self.set_tag_catalog(p.tags.clone());
        self.set_chain_identities(p.chain_identities.clone());
        self.set_chain_layout(p.chain_layout.clone());
        self.set_chain_lineage(p.chain_lineage.clone());
        self.set_pinned_files(p.pinned_files.clone());
        // patches name their mod by entity id in the project, by name in the window
        let by_name = |id: &str| {
            p.entities
                .name_of(&sat_core::entity::EntityId(id.to_string()))
                .map_or_else(|| id.to_string(), str::to_string)
        };
        self.set_patches(
            p.patches
                .iter()
                .map(|x| sat_core::patch::FilePatch {
                    source: by_name(&x.source),
                    ..x.clone()
                })
                .collect(),
        );
        let mut problems = self.set_sticker_rules(&p.sticker_rules);
        self.set_system_stickers(p.system_stickers());
        self.set_sources(p.sources_by_name());
        problems.extend(self.set_counters(&p.counters));
        problems.extend(self.set_indicators(&p.indicators));
        self.space = p.space.clone().filter(|s| s.problems().is_empty());
        problems
    }

    /// The shell can save the project at `path` (`--write`).
    pub fn enable_saving(&mut self, path: String) {
        self.save.state.writable = true;
        self.save.state.path = path;
    }

    /// The project as it is on disk now (read at opening, written by a save or a run): what `dirty` compares with.
    pub fn project_saved(&mut self, p: Project) {
        self.save.saved = Some(p);
        self.save.state.saving = false;
        self.save.state.error = None;
        self.save.state.changed_outside = false;
    }

    pub(crate) fn request_save(&mut self, force: bool) -> bool {
        if !self.save.state.writable || self.project_base.is_none() {
            return false;
        }
        self.save.request = Some(force);
        self.save.state.saving = true;
        true
    }

    /// The shell takes a save request (`true` — overwrite a file changed outside).
    pub fn take_save_request(&mut self) -> Option<bool> {
        self.save.request.take()
    }

    /// A save the shell made: what was written.
    pub fn save_done(&mut self, p: Project) {
        self.project_saved(p);
        self.save.state.saves += 1;
    }

    /// A save the shell could not make.
    pub fn save_failed(&mut self, error: String, changed_outside: bool) {
        self.save.state.saving = false;
        self.save.state.error = Some(error);
        self.save.state.changed_outside = changed_outside;
    }

    // @sat:app.project.state
    /// Saving as the window shows it; `dirty` compares the project now with the one last saved or read.
    pub(crate) fn project_state(&mut self) -> ProjectState {
        let now = self.project();
        let mut st = self.save.state.clone();
        st.dirty = match (&now, &self.save.saved) {
            (Some(n), Some(s)) => n != s,
            (Some(_), None) => true,
            _ => false,
        };
        st
    }

    // @sat:app.project.now
    /// The project as it is now: the sections the window does not edit as loaded, the app's truth written over
    /// them. `None` when no project was loaded.
    pub fn project(&mut self) -> Option<Project> {
        let mut p = self.project_base.clone()?;
        self.store_project(&mut p);
        Some(p)
    }

    // @sat:app.project.store
    /// Writes the app's truth back into `p`: the canon and decoration of the book, settled chains, ignores, the
    /// plugin canon, sync records with their decisions, tags, chain identities, layout and lineage, pins. Sections
    /// the window does not edit (factories, patches, intents, rules, sources, facts) stay as `p` has them.
    pub fn store_project(&mut self, p: &mut Project) {
        let session = self.session();
        p.commit_book(session.book());
        p.decisions = session.decisions().clone();
        p.conflict_ignore = session.conflict_ignore().clone();
        p.plugins = session.plugin_canon().clone();
        p.sync = self.sync_records().to_vec();
        p.tags = self.tags.clone();
        p.chain_identities = self.chain_identities().to_vec();
        p.chain_layout = self.chain_layout().clone();
        p.chain_lineage = self.chain_lineage().clone();
        p.pinned_files = self.pinned_files().to_vec();
        p.indicators = self.indicator_defs.clone();
        p.space = self.space.clone();
        let ids = &p.entities;
        p.patches = self
            .patches()
            .iter()
            .map(|x| sat_core::patch::FilePatch {
                source: ids
                    .id_of(&x.source)
                    .map_or_else(|| x.source.clone(), |i| i.0.clone()),
                ..x.clone()
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use crate::tests::app_of;
    use crate::{Command, Event, Query, Reply};

    fn state(a: &mut crate::App) -> super::ProjectState {
        match a.query(Query::Project).unwrap() {
            Reply::Project { project } => project,
            _ => unreachable!(),
        }
    }

    #[test]
    fn donor_mods_are_taken_as_planned_installs() {
        let mut a = app_of(["A", "B"]);
        let p = sat_core::project::Project::import(a.session().book().to_modlist());
        a.load_project(&p, vec!["A".into(), "B".into()]);
        a.set_donor(
            "Old",
            crate::DonorData {
                mods_dir: "/old/mods".into(),
                modlist: None,
                folders: vec!["A".into(), "New One".into()],
            },
        );
        let Reply::Donors { donors } = a.query(Query::Donors).unwrap() else {
            panic!()
        };
        let kinds: Vec<_> = donors[0]
            .candidates
            .iter()
            .map(|c| (c.name.as_str(), c.kind))
            .collect();
        assert!(kinds.contains(&("New One", sat_core::sync::donor::CandidateKind::FolderOnly)));
        a.command(crate::Command::TakeFromDonor {
            donor: "Old".into(),
            names: vec!["New One".into()],
            separator: None,
            moved: true,
        })
        .unwrap();
        let p = a.project().unwrap();
        assert!(
            p.canon.entries.iter().any(|e| e.name == "New One"),
            "a planned mod in the canon"
        );
        let planned = p.planned();
        assert_eq!(planned.len(), 1);
        assert!(
            matches!(&planned[0].source, sat_core::intent::Source::Donor { donor, folder, moved: true, .. } if donor == "Old" && folder == "New One")
        );
        // what is already in the project cannot be taken
        assert!(
            a.command(crate::Command::TakeFromDonor {
                donor: "Old".into(),
                names: vec!["A".into()],
                separator: None,
                moved: false,
            })
            .is_err()
        );
    }

    #[test]
    fn unsaved_changes_and_saving() {
        let mut a = app_of(["A", "B"]);
        let p = sat_core::project::Project::import(a.session().book().to_modlist());
        a.load_project(&p, Vec::new());
        let loaded = a.project().unwrap();
        a.project_saved(loaded);
        // read back as loaded: nothing to save
        assert!(!state(&mut a).dirty);
        a.command(Command::SetConflictIgnore {
            rules: vec!["meta.ini".into()],
        })
        .unwrap();
        assert!(state(&mut a).dirty);
        // the sandbox does not save
        assert_eq!(
            a.command(Command::SaveProject { force: false }).unwrap(),
            Event::Nothing
        );
        assert_eq!(a.take_save_request(), None);
        a.enable_saving("p.stll".into());
        assert_eq!(
            a.command(Command::SaveProject { force: false }).unwrap(),
            Event::Decided
        );
        assert!(state(&mut a).saving);
        assert_eq!(a.take_save_request(), Some(false));
        a.save_failed("changed".into(), true);
        let st = state(&mut a);
        assert!(st.dirty && st.changed_outside && !st.saving);
        a.command(Command::SaveProject { force: true }).unwrap();
        assert_eq!(a.take_save_request(), Some(true));
        let now = a.project().unwrap();
        a.save_done(now);
        let st = state(&mut a);
        assert!(!st.dirty && !st.changed_outside && st.saves == 1 && st.error.is_none());
    }
}
