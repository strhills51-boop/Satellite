//! Intents: what the user wants done to the build, recorded anywhere, fulfilled by a commit (ADR 0007). @sat:intent
//!
//! Owns: intent records of a project (install a mod from a source), what makes a mod *planned* (in the canon, with
//! a pending install, no folder yet), and the commit operations they produce (`@sat:sync.plan`).
//! Not here: running an install (`@sat:mo2.install`), sync records — mismatches the disk shows
//! (`@sat:sync.records`).
//!
//! An intent is written where the user is — the site, another computer, a rule — and the project carries it until
//! a commit on the machine with the build fulfils it. A planned mod has its place in the order, its enabled flag,
//! its decoration from the start; the folder appears at the commit. Its folder being absent is the plan, not a
//! mismatch, so it makes no sync record.

use serde::{Deserialize, Serialize};

use crate::entity::EntityId;

/// Where a mod comes from (PHILOSOPHY «Указатели»). Only portable references: no absolute paths of a machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// An archive in the build's downloads folder (MO2 `download_directory`), by file name.
    Downloads {
        file: String,
        /// Size when the intent was made: a different file of the same name is not taken silently.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<u64>,
    },
}

/// What to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// Put the mod's files into its folder from the source (the place in the order is the canon's).
    Install { source: Source },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentStatus {
    Pending,
    /// Fulfilled by a commit (kept: it is where the mod came from).
    Done,
}

// @sat:intent.record
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Intent {
    pub entity: EntityId,
    #[serde(flatten)]
    pub action: Action,
    pub status: IntentStatus,
}

impl Intent {
    pub fn install(entity: EntityId, source: Source) -> Self {
        Self {
            entity,
            action: Action::Install { source },
            status: IntentStatus::Pending,
        }
    }

    pub fn pending(&self) -> bool {
        self.status == IntentStatus::Pending
    }

    /// The source of a pending install.
    pub fn pending_install(&self) -> Option<&Source> {
        match (&self.action, self.status) {
            (Action::Install { source }, IntentStatus::Pending) => Some(source),
            _ => None,
        }
    }
}
