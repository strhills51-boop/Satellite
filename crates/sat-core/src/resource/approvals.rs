//! Approvals: what the user allowed each resource to do, stored on this machine. @sat:resource.approvals
//!
//! Owns: the approval record and its storage format (a JSON file in the user's profile, never in a project).
//! Not here: the decision itself (`@sat:resource.policy`).
//!
//! An approval names the resource id, version and content digest; a different version or a changed manifest
//! is a different thing and asks again. Revoking removes the record.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Capability, Entry, ResourceId};

/// One approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub version: String,
    /// Hex digest of the manifest (see `Manifest::digest`).
    pub digest: String,
    pub granted: BTreeSet<Capability>,
}

/// All approvals of this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approvals {
    format: u32,
    by_id: BTreeMap<ResourceId, Approval>,
}

fn hex(d: u64) -> String {
    format!("{d:016x}")
}

impl Approvals {
    /// Capabilities granted to exactly this id + version + digest.
    pub fn granted(&self, id: &str, version: &str, digest: u64) -> BTreeSet<Capability> {
        match self.by_id.get(id) {
            Some(a) if a.version == version && a.digest == hex(digest) => a.granted.clone(),
            _ => BTreeSet::new(),
        }
    }

    /// Grants `caps` (added to what this exact resource already has; a record for another version is replaced).
    pub fn approve(&mut self, e: &Entry, caps: &[Capability]) {
        let digest = hex(e.digest);
        let rec = self
            .by_id
            .entry(e.manifest.id.clone())
            .or_insert_with(|| Approval {
                version: e.manifest.version.clone(),
                digest: digest.clone(),
                granted: BTreeSet::new(),
            });
        if rec.version != e.manifest.version || rec.digest != digest {
            rec.version = e.manifest.version.clone();
            rec.digest = digest;
            rec.granted.clear();
        }
        rec.granted.extend(caps.iter().copied());
    }

    pub fn revoke(&mut self, id: &str) -> bool {
        self.by_id.remove(id).is_some()
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        match std::fs::read(path) {
            Ok(b) => serde_json::from_slice(&b).map_err(std::io::Error::other),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut me = self.clone();
        me.format = 1;
        let tmp = path.with_extension("tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&me).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Kind, Manifest, Registry, Source};
    use super::*;

    #[test]
    fn roundtrip_and_new_version_resets() {
        let mut r = Registry::default();
        let m = Manifest::new("x.tool", Kind::Command, "t").requiring(&[Capability::ReadCanon]);
        r.register(m.clone(), Source::Machine).unwrap();
        let mut ap = Approvals::default();
        ap.approve(r.get("x.tool").unwrap(), &[Capability::ReadCanon]);

        let dir = std::env::temp_dir().join(format!("sat-approvals-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("approvals.json");
        ap.save(&p).unwrap();
        let back = Approvals::load(&p).unwrap();
        r.resolve(&back);
        assert!(r.get("x.tool").unwrap().allows(Capability::ReadCanon));
        std::fs::remove_dir_all(&dir).unwrap();

        let mut v2 = m;
        v2.version = "2".into();
        r.register(v2, Source::Machine).unwrap(); // update from the same source
        r.resolve(&back);
        assert!(!r.get("x.tool").unwrap().decision.is_active());
        assert!(
            Approvals::load(Path::new("/nonexistent/sat/approvals.json"))
                .unwrap()
                .by_id
                .is_empty()
        );
    }
}
