//! File patches in the window: a mod's ini edited as data, written by the commit into Satellite Patches. @sat:app.patches
//!
//! Owns: the project's file patches as the window edits them (keyed by mod name in the window, by entity id in the
//! project — `@sat:app.project` maps them), the command that edits one file's patch (set, remove, take a key
//! back, drop the patch, take the mod's current file as the new base) and the query that shows a file with its
//! patch: every key with the mod's value and the patched one, whether the mod's file changed since the patch.
//! Not here: patches as data and applying them (`@sat:patch`), building the output mod (`@sat:mo2.patches`), the
//! plan (`@sat:home.plan.patches`). The original file is never touched (PHILOSOPHY «Файлы»).

use serde::{Deserialize, Serialize};

use sat_core::patch::{FilePatch, PatchOp, apply, content_signature};

use crate::{App, AppError};

/// One key of an ini (`section` empty — before the first section).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IniKey {
    #[serde(default)]
    pub section: String,
    pub key: String,
}

/// One key set to a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IniSet {
    #[serde(default)]
    pub section: String,
    pub key: String,
    pub value: String,
}

/// A key of the file as the window shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchedKey {
    pub section: String,
    pub key: String,
    /// The mod's value (`None` — the patch adds the key).
    pub value: Option<String>,
    /// What the patch does to it: `set` (to `patched`), `remove`, nothing.
    pub op: Option<String>,
    pub patched: Option<String>,
}

/// A file with its patch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilePatchView {
    pub mod_name: String,
    pub path: String,
    /// The file is readable in the mod.
    pub found: bool,
    pub keys: Vec<PatchedKey>,
    /// The project has a patch of this file.
    pub patched: bool,
    /// The mod's file changed since the patch was made (the commit stops until the patch is looked at).
    pub base_changed: bool,
    /// The file as the commit will write it into Satellite Patches.
    pub text: Option<String>,
    /// How many files the project patches in all.
    pub total: usize,
}

impl App {
    /// The project's patches (sources by mod name), as the shell loads them.
    pub fn set_patches(&mut self, patches: Vec<FilePatch>) {
        self.patches = patches;
    }

    /// The patches as edited (sources by mod name).
    pub fn patches(&self) -> &[FilePatch] {
        &self.patches
    }

    fn mod_file(&self, mod_name: &str, path: &str) -> Option<Vec<u8>> {
        self.domain_inputs.reader.as_ref()?(mod_name, path)
    }

    // @sat:app.patches.edit
    /// Edits the patch of one file; `false` when nothing changed. A new patch records the mod's file as its base.
    pub(crate) fn patch_file(&mut self, mod_name: &str, path: &str, e: &PatchEdit) -> Result<bool, AppError> {
        let path = path.replace('\\', "/").trim_matches('/').to_string();
        let bytes = self.mod_file(mod_name, &path);
        let at = self
            .patches
            .iter()
            .position(|p| p.source.eq_ignore_ascii_case(mod_name) && p.path.eq_ignore_ascii_case(&path));
        let before = self.patches.clone();
        if e.drop {
            if let Some(i) = at {
                self.patches.remove(i);
            }
            return Ok(self.patches != before);
        }
        let Some(bytes) = bytes else {
            return Err(AppError::Record(format!(
                "{mod_name}/{path}: the file is not readable"
            )));
        };
        let i = at.unwrap_or_else(|| {
            self.patches.push(FilePatch {
                source: mod_name.to_string(),
                path: path.clone(),
                base: Some(content_signature(&bytes)),
                ops: Vec::new(),
            });
            self.patches.len() - 1
        });
        let p = &mut self.patches[i];
        if e.rebase {
            p.base = Some(content_signature(&bytes));
        }
        for s in &e.set {
            p.put(PatchOp::Set {
                section: s.section.clone(),
                key: s.key.clone(),
                value: s.value.clone(),
            });
        }
        for k in &e.remove {
            p.put(PatchOp::Remove {
                section: k.section.clone(),
                key: k.key.clone(),
            });
        }
        for k in &e.revert {
            p.revert(&k.section, &k.key);
        }
        // a patch with nothing in it is no patch
        if p.ops.is_empty() {
            self.patches.remove(i);
        }
        Ok(self.patches != before)
    }

    // @sat:app.patches.view
    pub(crate) fn file_patch_view(&self, mod_name: &str, path: &str) -> FilePatchView {
        let path = path.replace('\\', "/").trim_matches('/').to_string();
        let mut v = FilePatchView {
            mod_name: mod_name.to_string(),
            path: path.clone(),
            total: self.patches.len(),
            ..Default::default()
        };
        let Some(bytes) = self.mod_file(mod_name, &path) else {
            return v;
        };
        v.found = true;
        let patch = self
            .patches
            .iter()
            .find(|p| p.source.eq_ignore_ascii_case(mod_name) && p.path.eq_ignore_ascii_case(&path));
        let ops: &[PatchOp] = patch.map_or(&[], |p| &p.ops);
        v.patched = patch.is_some();
        v.base_changed = patch
            .and_then(|p| p.base.as_deref())
            .is_some_and(|b| b != content_signature(&bytes));
        let op_of = |section: &str, key: &str| {
            ops.iter().find(|o| {
                let (a, b) = o.target();
                a.eq_ignore_ascii_case(section) && b.eq_ignore_ascii_case(key)
            })
        };
        let file = sat_formats::ini::IniFile::parse(&bytes);
        let mut seen = std::collections::HashSet::new();
        for (section, key, value) in file.entries() {
            seen.insert((section.to_lowercase(), key.to_lowercase()));
            let o = op_of(&section, &key);
            v.keys.push(PatchedKey {
                op: o.map(|o| match o {
                    PatchOp::Set { .. } => "set".into(),
                    PatchOp::Remove { .. } => "remove".into(),
                }),
                patched: o.and_then(|o| match o {
                    PatchOp::Set { value, .. } => Some(value.clone()),
                    PatchOp::Remove { .. } => None,
                }),
                section,
                key,
                value: Some(value),
            });
        }
        // keys the patch adds
        for o in ops {
            if let PatchOp::Set { section, key, value } = o
                && !seen.contains(&(section.to_lowercase(), key.to_lowercase()))
            {
                v.keys.push(PatchedKey {
                    section: section.clone(),
                    key: key.clone(),
                    value: None,
                    op: Some("set".into()),
                    patched: Some(value.clone()),
                });
            }
        }
        if v.patched {
            v.text = Some(String::from_utf8_lossy(&apply(&bytes, ops)).into_owned());
        }
        v
    }
}

/// What to do to one file's patch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchEdit {
    #[serde(default)]
    pub set: Vec<IniSet>,
    #[serde(default)]
    pub remove: Vec<IniKey>,
    /// Take these keys back (the mod's value stays).
    #[serde(default)]
    pub revert: Vec<IniKey>,
    /// Drop the whole patch of the file.
    #[serde(default)]
    pub drop: bool,
    /// The mod's file now is the base (after looking at a patch whose base changed).
    #[serde(default)]
    pub rebase: bool,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::tests::app_of;
    use crate::{Command, Event, Query, Reply};

    fn view(a: &mut App) -> FilePatchView {
        match a
            .query(Query::FilePatch {
                mod_name: "A".into(),
                path: "SKSE/Plugins/x.ini".into(),
            })
            .unwrap()
        {
            Reply::FilePatch { file } => file,
            _ => unreachable!(),
        }
    }

    #[test]
    fn edit_a_mods_ini_as_a_patch() {
        let mut a = app_of(["A", "B"]);
        let file = std::sync::Mutex::new(b"; c\n[General]\nfSpeed = 1.0\nbOn=1\n".to_vec());
        let file = Arc::new(file);
        let f = file.clone();
        a.set_file_reader(Arc::new(move |m: &str, p: &str| {
            (m == "A" && p.eq_ignore_ascii_case("skse/plugins/x.ini")).then(|| f.lock().unwrap().clone())
        }));
        let v = view(&mut a);
        assert!(v.found && !v.patched);
        assert_eq!(v.keys.len(), 2);
        let ev = a
            .command(Command::PatchFile {
                mod_name: "A".into(),
                path: "SKSE\\Plugins\\x.ini".into(),
                edit: PatchEdit {
                    set: vec![
                        IniSet {
                            section: "general".into(),
                            key: "fspeed".into(),
                            value: "2.0".into(),
                        },
                        IniSet {
                            section: "New".into(),
                            key: "k".into(),
                            value: "v".into(),
                        },
                    ],
                    remove: vec![IniKey {
                        section: "General".into(),
                        key: "bOn".into(),
                    }],
                    ..Default::default()
                },
            })
            .unwrap();
        assert_ne!(ev, Event::Nothing);
        let v = view(&mut a);
        assert!(v.patched && !v.base_changed);
        let k = |n: &str| {
            v.keys
                .iter()
                .find(|k| k.key.eq_ignore_ascii_case(n))
                .unwrap()
                .clone()
        };
        assert_eq!(
            (k("fSpeed").op.as_deref(), k("fSpeed").patched.as_deref()),
            (Some("set"), Some("2.0"))
        );
        assert_eq!(k("bOn").op.as_deref(), Some("remove"));
        assert_eq!(k("k").value, None, "the patch adds it");
        let text = v.text.unwrap();
        assert!(
            text.contains("fSpeed = 2.0") && !text.contains("bOn") && text.contains("; c"),
            "{text}"
        );
        // the mod was updated: the patch says so
        *file.lock().unwrap() = b"[General]\nfSpeed = 1.5\n".to_vec();
        assert!(view(&mut a).base_changed);
        a.command(Command::PatchFile {
            mod_name: "A".into(),
            path: "SKSE/Plugins/x.ini".into(),
            edit: PatchEdit {
                rebase: true,
                ..Default::default()
            },
        })
        .unwrap();
        assert!(!view(&mut a).base_changed);
        // taking every key back leaves no patch
        a.command(Command::PatchFile {
            mod_name: "A".into(),
            path: "SKSE/Plugins/x.ini".into(),
            edit: PatchEdit {
                revert: vec![
                    IniKey {
                        section: "General".into(),
                        key: "fSpeed".into(),
                    },
                    IniKey {
                        section: "General".into(),
                        key: "bOn".into(),
                    },
                    IniKey {
                        section: "New".into(),
                        key: "k".into(),
                    },
                ],
                ..Default::default()
            },
        })
        .unwrap();
        assert!(!view(&mut a).patched);
        assert!(a.patches().is_empty());
    }
}
