//! The commit as the window sees it: asked by a command, run by the shell, its result shown. @sat:app.commit
//!
//! Owns: the commit's state in the app — a request the shell takes (`Command::Commit` with the steps the user
//! confirmed), that it runs, and how it ended (committed, refused by a gate, rolled back, rollback incomplete).
//! The app writes nothing (ADR 0004): the shell checks the gates and runs the commit (`@sat:home.commit`), then
//! opens the build again — the run that follows is the commit's check (ADR 0019 §4).
//! Not here: the plan (`@sat:app.sync.full`), the transaction (`@sat:mo2.commit`).

use serde::{Deserialize, Serialize};

use crate::App;

/// Where the commit is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommitState {
    #[default]
    Idle,
    Requested,
    Running,
    /// Written; the build was opened again.
    Committed,
    /// Not started: a gate refused (blockers, stale records, the project file changed outside).
    Refused,
    /// A step failed; everything done was undone.
    RolledBack,
    /// A step failed and undoing failed too: see the journal.
    Incomplete,
}

/// How the last commit ended.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitResult {
    pub state: CommitState,
    /// Why it was refused or failed; what was left not undone.
    pub error: Option<String>,
    pub left: Vec<String>,
    /// The version folder (backups, intent, journal) and the steps done.
    pub version_dir: Option<String>,
    pub steps: usize,
    /// Processors run after the commit: (id, files collected, promoted, why not).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub processed: Vec<(String, usize, bool, Option<String>)>,
    /// After a commit: mismatches the next run found (expected none).
    pub records_after: Option<usize>,
}

/// The commit as the window shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitView {
    /// The shell can commit (a build is open and `--write` was given).
    pub available: bool,
    pub state: CommitState,
    pub last: Option<CommitResult>,
}

#[derive(Debug, Default)]
pub(crate) struct CommitSlot {
    pub view: CommitView,
    request: Option<Vec<String>>,
}

impl App {
    /// The shell can commit to the build.
    pub fn enable_commits(&mut self) {
        self.commit.view.available = true;
    }

    pub(crate) fn request_commit(&mut self, confirmed: Vec<String>) -> bool {
        let v = &self.commit.view;
        if !v.available || matches!(v.state, CommitState::Requested | CommitState::Running) {
            return false;
        }
        self.commit.request = Some(confirmed);
        self.commit.view.state = CommitState::Requested;
        true
    }

    /// The shell takes a commit request: the steps the user confirmed (sealing a group…).
    pub fn take_commit_request(&mut self) -> Option<Vec<String>> {
        self.commit.request.take()
    }

    /// The shell started the commit.
    pub fn commit_started(&mut self) {
        self.commit.view.state = CommitState::Running;
    }

    /// The commit ended (refused, rolled back, committed — then the build is opened again and the new app is told).
    pub fn commit_finished(&mut self, r: CommitResult) {
        self.commit.view.state = r.state;
        self.commit.view.last = Some(r);
        self.revision += 1;
    }

    pub(crate) fn commit_view(&self) -> CommitView {
        self.commit.view.clone()
    }

    /// A commit is asked or running: the shell starts no run meanwhile (one writer, ADR 0010 law 9).
    pub fn committing(&self) -> bool {
        matches!(
            self.commit.view.state,
            CommitState::Requested | CommitState::Running
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::app_of;
    use crate::{Command, Event, Query, Reply};

    fn view(a: &mut App) -> CommitView {
        match a.query(Query::Commit).unwrap() {
            Reply::Commit { commit } => commit,
            _ => unreachable!(),
        }
    }

    #[test]
    fn asked_taken_ended() {
        let mut a = app_of(["A", "B"]);
        // the sandbox does not commit
        assert_eq!(
            a.command(Command::Commit { confirmed: vec![] }).unwrap(),
            Event::Nothing
        );
        a.enable_commits();
        assert_eq!(
            a.command(Command::Commit {
                confirmed: vec!["Out".into()]
            })
            .unwrap(),
            Event::Decided
        );
        assert!(a.committing());
        // not twice while one is asked
        assert_eq!(
            a.command(Command::Commit { confirmed: vec![] }).unwrap(),
            Event::Nothing
        );
        assert_eq!(a.take_commit_request(), Some(vec!["Out".to_string()]));
        a.commit_started();
        assert_eq!(view(&mut a).state, CommitState::Running);
        a.commit_finished(CommitResult {
            state: CommitState::Refused,
            error: Some("stale".into()),
            ..Default::default()
        });
        let v = view(&mut a);
        assert_eq!(v.state, CommitState::Refused);
        assert!(!a.committing());
        assert_eq!(v.last.unwrap().error.as_deref(), Some("stale"));
    }
}
