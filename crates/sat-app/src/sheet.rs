//! The mod sheet: one standard quick look at a mod, at three depths (ADR 0015). @sat:app.sheet
//!
//! Owns: the sheet schema (sections of fields, each with who says it — [`Origin`] — and an optional
//! [`Signal`]) and assembling it from what is here: the observed layer (facts, overwrite, plugins) and the user's
//! layer (marks, enabled). The author's and the community's layers join later through the same schema, by section.
//! Not here: the summary panel with opponents and chain lists (`ModDetail`, `@sat:app.mod-detail`), how a window
//! draws the sheet (labels by field id: the core has no words for people).
//!
//! Levels: **badge** — the name and the strongest one or two signals (a list row, a tile); **mini** — every section,
//! a few fields each (a hover cloud, the summary); **full** — everything (a listing on the site, a window).
//!
//! Field ids are stable and shared with the site and the author's manifest: add new ones, never rename.

use serde::{Deserialize, Serialize};

use sat_core::order::Key;

use crate::App;

/// Version of the sheet schema.
pub const SHEET_SCHEMA: u32 = 1;
/// Fields per section at the mini level.
pub const MINI_FIELDS: usize = 6;
/// Signals at the badge level.
pub const BADGE_SIGNALS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SheetLevel {
    Badge,
    Mini,
    Full,
}

/// Who says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Seen in this build (scan facts, overwrite, plugin lists).
    Observed,
    /// Set by the user (marks, enabled).
    User,
    /// Declared by the mod's author (later: `satellite.yaml`, the mod's representation on the site).
    Author,
    /// Seen across many builds (later).
    Community,
    /// Computed by a rule set: found tags (`@sat:tag-rules`), conditional stickers (`@sat:sticker-rules`).
    Rules,
}

/// How much a field asks for attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    Note,
    Warn,
    Problem,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Value {
    Count { n: u64 },
    Bytes { n: u64 },
    Flag { on: bool },
    Text { text: String },
    Names { items: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub id: String,
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<Signal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub id: String,
    pub origin: Origin,
    pub fields: Vec<Field>,
    /// Fields left out at this level.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModSheet {
    pub schema: u32,
    pub key: Key,
    pub name: String,
    /// The entity id when the project knows the mod (`@sat:entity`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub separator: Option<String>,
    pub enabled: bool,
    pub level: SheetLevel,
    /// The strongest signals of all sections, strongest first (the badge shows these).
    pub signals: Vec<Field>,
    pub sections: Vec<Section>,
}

fn count(id: &str, n: impl Into<u64>) -> Field {
    Field {
        id: id.into(),
        value: Value::Count { n: n.into() },
        signal: None,
    }
}

fn bytes(id: &str, n: u64) -> Field {
    Field {
        id: id.into(),
        value: Value::Bytes { n },
        signal: None,
    }
}

fn names(id: &str, items: Vec<String>) -> Field {
    Field {
        id: id.into(),
        value: Value::Names { items },
        signal: None,
    }
}

fn signal(mut f: Field, s: Signal) -> Field {
    f.signal = Some(s);
    f
}

/// Counts that are zero say nothing on a quick look.
fn nonzero(f: &Field) -> bool {
    !matches!(f.value, Value::Count { n: 0 } | Value::Bytes { n: 0 })
        && !matches!(&f.value, Value::Names { items } if items.is_empty())
}

impl App {
    // @sat:app.sheet.build
    /// The sheet of the mod at `key`, at `level`. A separator or an unknown mod gets the user's layer only.
    pub(crate) fn mod_sheet(&mut self, key: Key, level: SheetLevel) -> ModSheet {
        let book = self.session.book();
        let name = book.name(key).to_string();
        let separator = book
            .separator_of(key)
            .map(|s| book.name(s).trim_end_matches("_separator").to_string());
        let enabled = book.enabled(key);
        let marks = book.marks(key).cloned();
        let mut sections = Vec::new();
        let mut entity = None;
        if let Some(m) = self.session.mod_id(&name) {
            entity = self.session.member_id(m).map(str::to_string);
            let sum = self.session.table().summary(m).clone();
            let x = &sum.metrics;
            // what is in it
            let mut content = vec![
                bytes("bytes", x.bytes),
                count("files", x.files),
                count("archives", x.archives),
                count("archive_files", x.archive_files),
                count("plugins", x.plugins),
                count("dds", x.dds),
                bytes("vram", x.vram_bytes),
                count("nif", x.nif),
                count("pex", x.pex),
                count("hkx", x.hkx),
                count("dll", x.dll),
                count("source_files", x.source_files),
            ];
            if x.nested_archives > 0 {
                content.push(signal(count("nested_archives", x.nested_archives), Signal::Note));
            }
            if sum.missing {
                content.insert(
                    0,
                    signal(
                        Field {
                            id: "missing_folder".into(),
                            value: Value::Flag { on: true },
                            signal: None,
                        },
                        Signal::Problem,
                    ),
                );
            } else if self.session.table().mod_providers(m).is_empty() && x.plugins == 0 {
                content.insert(
                    0,
                    signal(
                        Field {
                            id: "empty".into(),
                            value: Value::Flag { on: true },
                            signal: None,
                        },
                        Signal::Warn,
                    ),
                );
            }
            if let Some(e) = &sum.error {
                content.insert(
                    0,
                    signal(
                        Field {
                            id: "scan_error".into(),
                            value: Value::Text { text: e.clone() },
                            signal: None,
                        },
                        Signal::Problem,
                    ),
                );
            }
            sections.push(("content", Origin::Observed, content));

            // what it is made of (`@sat:profile`)
            if !sum.missing {
                let p = &sum.profile;
                let text = |id: &str, text: String| Field {
                    id: id.into(),
                    value: Value::Text { text },
                    signal: None,
                };
                let list = |v: Vec<&str>| v.into_iter().map(str::to_string).collect::<Vec<_>>();
                let mut profile = vec![
                    text("kind", p.primary_kind().into()),
                    text("package", p.package_shape().into()),
                    names("coverage80", list(p.coverage80())),
                    names("labels", list(p.labels())),
                    names(
                        "top_ext",
                        p.top_extensions(12)
                            .into_iter()
                            .map(|(e, n)| format!("{} {n}", if e.is_empty() { "(none)" } else { e }))
                            .collect(),
                    ),
                    names("domains", list(p.domain_names())),
                    names("presentation", list(p.presentation_kinds())),
                    count("deep_paths", p.deep),
                    count("long_paths", p.long),
                    count("data_wrapper", p.data_wrapper()),
                ];
                if p.tracked() > 0 {
                    profile.insert(2, text("archived_share", format!("{}%", p.archived_share())));
                }
                let cargo = p.cargo_signals();
                if !cargo.is_empty() {
                    profile.push(signal(names("cargo_signals", list(cargo)), Signal::Note));
                }
                if p.junk > 0 {
                    profile.push(signal(count("junk", p.junk), Signal::Note));
                }
                sections.push(("profile", Origin::Observed, profile));
            }

            // how it meets the other mods
            let ow = self.session.overwrite();
            let s = ow.stat(m);
            let heads = self.heads();
            let parts = crate::chains::parts(&ow, &heads, m);
            let settled = parts
                .iter()
                .filter(|p| heads[p.group as usize].done(self.session.decisions()))
                .count();
            let mut conflicts = Vec::new();
            if s.fully_overridden {
                conflicts.push(signal(
                    Field {
                        id: "fully_overridden".into(),
                        value: Value::Flag { on: true },
                        signal: None,
                    },
                    Signal::Warn,
                ));
            }
            conflicts.extend([
                count("wins", s.win),
                count("loses", s.lose),
                count("middle", s.mid),
                count("unique", s.unique),
            ]);
            let open = parts.len() - settled;
            let mut chains = count("chains", parts.len() as u64);
            if open > 0 {
                chains.signal = Some(Signal::Note);
            }
            conflicts.push(chains);
            conflicts.push(count("chains_settled", settled as u64));
            // losing copies against the winner's bytes (`@sat:identical.mods`): still conflicts, still disk
            if let Some(c) = self.mod_content().and_then(|c| c.get(m.idx()).copied()) {
                if c.equal_files > 0 {
                    conflicts.push(signal(count("equal_copies", c.equal_files), Signal::Note));
                    conflicts.push(bytes("equal_bytes", c.equal_bytes));
                }
                if c.unknown_files > 0 {
                    conflicts.push(count("unknown_copies", c.unknown_files));
                }
            }
            sections.push(("conflicts", Origin::Observed, conflicts));

            // its plugins
            let plugins = self.session.plugins();
            let (mut active, mut inactive, mut hidden, mut missing) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for p in plugins
                .plugins
                .iter()
                .filter(|p| p.versions.iter().any(|v| v.owner == m))
            {
                if p.blacklisted {
                    hidden.push(p.name.clone());
                } else if p.active {
                    active.push(p.name.clone());
                } else {
                    inactive.push(p.name.clone());
                }
                missing.extend(p.missing_masters.iter().map(|mm| format!("{} → {mm}", p.name)));
            }
            let mut fields = Vec::new();
            if !missing.is_empty() {
                fields.push(signal(names("missing_masters", missing), Signal::Problem));
            }
            if enabled && active.is_empty() && !inactive.is_empty() {
                fields.push(signal(names("inactive", inactive), Signal::Warn));
            } else {
                fields.push(names("inactive", inactive));
            }
            fields.insert(0, names("active", active));
            fields.push(names("hidden", hidden));
            sections.push(("plugins", Origin::Observed, fields));
        }
        // where it comes from (as the manager recorded it)
        if let Some(src) = self.sources.get(&name.to_lowercase()).cloned() {
            let text = |id: &str, v: Option<String>| {
                v.map(|text| Field {
                    id: id.into(),
                    value: Value::Text { text },
                    signal: None,
                })
            };
            let mut fields: Vec<Field> = [
                text("site", Some(src.site.clone())),
                src.mod_id.map(|n| count("mod_id", n)),
                text("version", src.version.clone()),
                text("newest_version", src.newest_version.clone()).map(|f| {
                    if src.update_available() {
                        signal(f, Signal::Note)
                    } else {
                        f
                    }
                }),
                text("file", src.file.clone()),
                text("link", src.link()),
            ]
            .into_iter()
            .flatten()
            .collect();
            if src.update_available() {
                fields.insert(
                    0,
                    signal(
                        Field {
                            id: "update".into(),
                            value: Value::Flag { on: true },
                            signal: None,
                        },
                        Signal::Note,
                    ),
                );
            }
            sections.push(("source", Origin::Observed, fields));
        }
        // what the rules say
        let mut rules = Vec::new();
        if let Some(m) = self.session.mod_id(&name) {
            let found: Vec<String> = self
                .session
                .table()
                .summary(m)
                .found_tags
                .iter()
                .map(|(t, _)| t.clone())
                .collect();
            rules.push(names("found_tags", found));
        }
        rules.push(names("rule_stickers", self.rule_stickers(key)));
        sections.push(("rules", Origin::Rules, rules));
        // the user's layer
        let mut user = vec![Field {
            id: "enabled".into(),
            value: Value::Flag { on: enabled },
            signal: None,
        }];
        if let Some(mk) = marks {
            user.push(names("tags", mk.tags));
            user.push(names("stickers", mk.stickers));
            if !mk.note.is_empty() {
                user.push(Field {
                    id: "note".into(),
                    value: Value::Text { text: mk.note },
                    signal: None,
                });
            }
        }
        sections.push(("user", Origin::User, user));

        // signals of every section, strongest first
        let mut signals: Vec<Field> = sections
            .iter()
            .flat_map(|(_, _, fs)| fs.iter().filter(|f| f.signal.is_some()).cloned())
            .collect();
        signals.sort_by_key(|f| std::cmp::Reverse(f.signal));
        let sections = match level {
            SheetLevel::Badge => {
                signals.truncate(BADGE_SIGNALS);
                Vec::new()
            }
            SheetLevel::Mini | SheetLevel::Full => sections
                .into_iter()
                .map(|(id, origin, fields)| {
                    // signals first, then what has something to say
                    let mut fields: Vec<Field> = fields
                        .into_iter()
                        .filter(|f| f.signal.is_some() || nonzero(f) || matches!(f.value, Value::Flag { .. }))
                        .collect();
                    fields.sort_by_key(|f| std::cmp::Reverse(f.signal));
                    let more = if level == SheetLevel::Mini {
                        let more = fields.len().saturating_sub(MINI_FIELDS);
                        fields.truncate(MINI_FIELDS);
                        more
                    } else {
                        0
                    };
                    Section {
                        id: id.into(),
                        origin,
                        fields,
                        more,
                    }
                })
                .filter(|s: &Section| !s.fields.is_empty())
                .collect(),
        };
        ModSheet {
            schema: SHEET_SCHEMA,
            key,
            name,
            entity,
            separator,
            enabled,
            level,
            signals,
            sections,
        }
    }
}
