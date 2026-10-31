//! Resource registry: everything that extends Satellite enters as a resource (ADR 0010, law 10). @sat:resource
//!
//! Owns: the manifest format, load sources and the trust they imply, capabilities, the policy that decides what
//! a resource may do, and the registry of active resources. Path: `manifest → registry → policy → runtime`.
//! Not here: approvals storage (`@sat:resource.approvals`), built-in command operations (`@sat:resource.command`),
//! key bindings (`@sat:resource.hotkeys`), the
//! analyzer contract itself (`@sat:scan.analyzer`).
//!
//! Lessons of the old Satellite prototype, fixed by construction:
//! - trust comes from **where the resource was loaded from** ([`Source`]), never from its id prefix;
//! - capabilities are granted **to a resource** (and version + digest), never to the whole project;
//! - approvals live **on the user's machine** (`approvals`), never in the project file, so opening someone's
//!   project cannot grant its resources anything;
//! - manifests are data: no code is evaluated (no `new Function`); behaviour comes from built-in kinds.

pub mod approvals;
pub mod command;
pub mod hotkeys;

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

pub use approvals::{Approval, Approvals};
pub use command::{Action, CommandError};
pub use hotkeys::{Chord, HotkeyConflict, Keymap};

/// Manifest format version understood by this build.
pub const MANIFEST_FORMAT: u32 = 1;

/// Resource id: `namespace.name` in lowercase (`core.cursor.down`, `vadimich.conflict-cleanup`).
/// The namespace is a name only; it carries no trust.
pub type ResourceId = String;

// @sat:resource.kind
/// What a resource is. Behaviour of each kind is built into Satellite; a manifest only parameterises it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// An operation a user can trigger (by key, menu, palette).
    Command,
    /// A key chord bound to a command in a context.
    Hotkey,
    /// A window kind (ADR 0009).
    Window,
    /// A file analyzer (sat-scan).
    Analyzer,
    /// A focus-group provider (ADR 0009).
    FocusProvider,
    /// A saved layout / filter / workspace preset (data only).
    Preset,
}

// @sat:resource.source
/// Where a resource was loaded from. This, and only this, decides the default trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// Compiled into Satellite.
    BuiltIn,
    /// Installed by the user on this machine (profile folder).
    Machine,
    /// Came inside a project container (someone else's project, ADR 0007).
    Project,
    /// Downloaded from a shared catalogue (future: modders' network).
    Network,
}

impl Source {
    /// Built-in resources are trusted with what they declare; everything else needs an approval.
    pub fn trusted(self) -> bool {
        self == Source::BuiltIn
    }

    /// Precedence when two resources compete (e.g. the same key chord): the user's own choices win.
    pub fn precedence(self) -> u8 {
        match self {
            Source::Machine => 3,
            Source::Project => 2,
            Source::Network => 1,
            Source::BuiltIn => 0,
        }
    }
}

// @sat:resource.capability
/// What a resource may touch. Granted per resource; the runtime checks them on every call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// Read the canon (order, enabled, separators).
    ReadCanon,
    /// Read decorations.
    ReadDecor,
    /// Read facts (provider table, analyzer results).
    ReadFacts,
    /// Change decorations (colour, groups, muted).
    WriteDecor,
    /// Change the canon (moves, enable/disable) — through commands.
    WriteCanon,
    /// Read files of the build (analyzers).
    ReadFiles,
    /// Start long jobs (runs, commits).
    RunJobs,
    /// Network access.
    Network,
}

impl Capability {
    /// Capabilities that can change what goes into the mod manager or leave the machine.
    pub fn dangerous(self) -> bool {
        matches!(
            self,
            Capability::WriteCanon | Capability::RunJobs | Capability::Network
        )
    }
}

// @sat:resource.manifest
/// A resource description. Pure data; `params` are interpreted by the built-in kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub id: ResourceId,
    pub version: String,
    pub kind: Kind,
    /// Human name (for lists and approval prompts).
    pub title: String,
    #[serde(default)]
    pub requires: BTreeSet<Capability>,
    /// Other resources this one needs (e.g. a hotkey needs its command).
    #[serde(default)]
    pub depends: BTreeSet<ResourceId>,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

impl Manifest {
    pub fn new(id: &str, kind: Kind, title: &str) -> Self {
        Self {
            format: MANIFEST_FORMAT,
            id: id.into(),
            version: "1".into(),
            kind,
            title: title.into(),
            requires: BTreeSet::new(),
            depends: BTreeSet::new(),
            params: BTreeMap::new(),
        }
    }

    pub fn requiring(mut self, caps: &[Capability]) -> Self {
        self.requires.extend(caps.iter().copied());
        self
    }

    pub fn depending(mut self, ids: &[&str]) -> Self {
        self.depends.extend(ids.iter().map(|s| s.to_string()));
        self
    }

    pub fn param(mut self, k: &str, v: &str) -> Self {
        self.params.insert(k.into(), v.into());
        self
    }

    // @sat:resource.digest
    /// Content digest (FNV-1a 64 over the canonical JSON). An approval is bound to it: any change to the
    /// manifest asks again.
    pub fn digest(&self) -> u64 {
        let bytes = serde_json::to_vec(self).expect("manifest serialises");
        bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
        })
    }
}

/// Why a manifest was not accepted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResourceError {
    #[error("manifest format {0} is not supported (this build reads {MANIFEST_FORMAT})")]
    Format(u32),
    #[error("bad resource id {0:?}: expected lowercase `namespace.name`")]
    BadId(String),
    #[error("resource {0} is already registered from {1:?}")]
    Duplicate(ResourceId, Source),
    #[error("resource {id}: {why}")]
    Invalid { id: ResourceId, why: String },
}

fn valid_id(id: &str) -> bool {
    let mut parts = id.split('.');
    let ok_part = |p: &str| {
        !p.is_empty()
            && p.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !p.starts_with('-')
            && !p.ends_with('-')
    };
    parts.clone().count() >= 2 && parts.all(ok_part)
}

// @sat:resource.decision
/// What the policy decided for one resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Active with these capabilities (exactly what it declared).
    Active(BTreeSet<Capability>),
    /// Waiting for the user: these declared capabilities are not approved for this version/digest.
    NeedsApproval(BTreeSet<Capability>),
    /// Cannot run: a dependency is missing or not active.
    Blocked(Vec<ResourceId>),
}

impl Decision {
    pub fn is_active(&self) -> bool {
        matches!(self, Decision::Active(_))
    }
}

/// A registered resource.
#[derive(Debug, Clone)]
pub struct Entry {
    pub manifest: Manifest,
    pub source: Source,
    pub digest: u64,
    pub decision: Decision,
}

impl Entry {
    /// Runtime check: may this resource use `cap` right now?
    pub fn allows(&self, cap: Capability) -> bool {
        matches!(&self.decision, Decision::Active(g) if g.contains(&cap))
    }
}

// @sat:resource.registry
/// All known resources and the policy's decision for each. Decisions are recomputed by [`Registry::resolve`].
#[derive(Debug, Default, Clone)]
pub struct Registry {
    entries: BTreeMap<ResourceId, Entry>,
}

impl Registry {
    /// Registry with the built-in resources (default commands and key bindings).
    pub fn with_builtins() -> Self {
        let mut r = Self::default();
        for m in command::builtin_commands()
            .into_iter()
            .chain(hotkeys::builtin_hotkeys())
        {
            r.register(m, Source::BuiltIn).expect("built-ins are valid");
        }
        r.resolve(&Approvals::default());
        r
    }

    /// Accepts a manifest (validation only; call [`Registry::resolve`] to decide). A built-in id can never be
    /// shadowed; a non-built-in may be replaced by the same source (an update).
    pub fn register(&mut self, m: Manifest, source: Source) -> Result<(), ResourceError> {
        if m.format != MANIFEST_FORMAT {
            return Err(ResourceError::Format(m.format));
        }
        if !valid_id(&m.id) {
            return Err(ResourceError::BadId(m.id));
        }
        if m.kind == Kind::Hotkey {
            hotkeys::validate(&m).map_err(|why| ResourceError::Invalid {
                id: m.id.clone(),
                why,
            })?;
        }
        if let Some(e) = self.entries.get(&m.id)
            && (e.source == Source::BuiltIn || e.source != source)
        {
            return Err(ResourceError::Duplicate(m.id, e.source));
        }
        let digest = m.digest();
        self.entries.insert(
            m.id.clone(),
            Entry {
                manifest: m,
                source,
                digest,
                decision: Decision::NeedsApproval(BTreeSet::new()),
            },
        );
        Ok(())
    }

    pub fn unregister(&mut self, id: &str) -> Option<Entry> {
        match self.entries.get(id) {
            Some(e) if e.source != Source::BuiltIn => self.entries.remove(id),
            _ => None,
        }
    }

    // @sat:resource.policy
    /// Decides every resource: built-ins get what they declare; others only what an approval on this machine
    /// grants for this exact id + version + digest; then dependencies must be active (to a fixed point).
    pub fn resolve(&mut self, approvals: &Approvals) {
        for e in self.entries.values_mut() {
            let wanted = e.manifest.requires.clone();
            e.decision = if e.source.trusted() {
                Decision::Active(wanted)
            } else {
                let granted = approvals.granted(&e.manifest.id, &e.manifest.version, e.digest);
                let missing: BTreeSet<_> = wanted.difference(&granted).copied().collect();
                if missing.is_empty() {
                    Decision::Active(wanted)
                } else {
                    Decision::NeedsApproval(missing)
                }
            };
        }
        loop {
            let mut changed = false;
            let ids: Vec<ResourceId> = self.entries.keys().cloned().collect();
            for id in ids {
                let e = &self.entries[&id];
                if !e.decision.is_active() {
                    continue;
                }
                let missing: Vec<ResourceId> = e
                    .manifest
                    .depends
                    .iter()
                    .filter(|d| !self.entries.get(*d).is_some_and(|x| x.decision.is_active()))
                    .cloned()
                    .collect();
                if !missing.is_empty() {
                    self.entries.get_mut(&id).expect("listed").decision = Decision::Blocked(missing);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.get(id)
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }

    /// Active resources of a kind, in id order.
    pub fn active(&self, kind: Kind) -> impl Iterator<Item = &Entry> {
        self.entries
            .values()
            .filter(move |e| e.manifest.kind == kind && e.decision.is_active())
    }

    /// Resources waiting for the user, with what they ask.
    pub fn pending(&self) -> impl Iterator<Item = (&Entry, &BTreeSet<Capability>)> {
        self.entries.values().filter_map(|e| match &e.decision {
            Decision::NeedsApproval(c) => Some((e, c)),
            _ => None,
        })
    }

    /// The active key map built from active hotkey resources.
    pub fn keymap(&self) -> Keymap {
        Keymap::build(self.active(Kind::Hotkey))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn foreign() -> Manifest {
        Manifest::new("someone.colorize", Kind::Command, "Colorize conflicts")
            .requiring(&[Capability::ReadFacts, Capability::WriteDecor])
    }

    #[test]
    fn ids_are_checked_and_carry_no_trust() {
        let mut r = Registry::default();
        assert!(matches!(
            r.register(Manifest::new("Bad", Kind::Command, "x"), Source::Machine),
            Err(ResourceError::BadId(_))
        ));
        // a project resource that names itself `core.*` is still a project resource: not trusted
        let fake = Manifest::new("core.evil", Kind::Command, "x").requiring(&[Capability::WriteCanon]);
        r.register(fake, Source::Project).unwrap();
        r.resolve(&Approvals::default());
        assert!(!r.get("core.evil").unwrap().decision.is_active());
    }

    #[test]
    fn builtins_cannot_be_shadowed() {
        let mut r = Registry::with_builtins();
        let m = Manifest::new("core.cursor.down", Kind::Command, "x");
        assert!(matches!(
            r.register(m, Source::Machine),
            Err(ResourceError::Duplicate(..))
        ));
        assert!(r.unregister("core.cursor.down").is_none());
    }

    #[test]
    fn approval_is_per_resource_version_and_digest() {
        let mut r = Registry::default();
        r.register(foreign(), Source::Project).unwrap();
        let mut ap = Approvals::default();
        r.resolve(&ap);
        let e = r.get("someone.colorize").unwrap();
        assert_eq!(
            e.decision,
            Decision::NeedsApproval([Capability::ReadFacts, Capability::WriteDecor].into())
        );
        assert!(!e.allows(Capability::ReadFacts));

        ap.approve(e, &[Capability::ReadFacts]);
        r.resolve(&ap);
        assert_eq!(
            r.get("someone.colorize").unwrap().decision,
            Decision::NeedsApproval([Capability::WriteDecor].into())
        );
        let e = r.get("someone.colorize").unwrap().clone();
        ap.approve(&e, &[Capability::WriteDecor]);
        r.resolve(&ap);
        let e = r.get("someone.colorize").unwrap();
        assert!(e.allows(Capability::WriteDecor));
        assert!(!e.allows(Capability::WriteCanon), "only what was declared");

        // the manifest changes (same id and version): the approval no longer applies
        let changed = foreign().param("color", "red");
        r.unregister("someone.colorize");
        r.register(changed, Source::Project).unwrap();
        r.resolve(&ap);
        assert!(!r.get("someone.colorize").unwrap().decision.is_active());
    }

    #[test]
    fn dependencies_block_transitively() {
        let mut r = Registry::default();
        r.register(
            Manifest::new("a.base", Kind::Command, "base").requiring(&[Capability::Network]),
            Source::Machine,
        )
        .unwrap();
        r.register(
            Manifest::new("a.mid", Kind::Window, "mid").depending(&["a.base"]),
            Source::Machine,
        )
        .unwrap();
        r.register(
            Manifest::new("a.top", Kind::Preset, "top").depending(&["a.mid"]),
            Source::Machine,
        )
        .unwrap();
        r.resolve(&Approvals::default());
        assert!(matches!(r.get("a.mid").unwrap().decision, Decision::Blocked(_)));
        assert!(matches!(r.get("a.top").unwrap().decision, Decision::Blocked(_)));
        assert_eq!(r.pending().count(), 1);
    }
}
