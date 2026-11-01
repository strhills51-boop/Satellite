//! Application API: commands, queries and events over one session, independent of transport. @sat:app
//!
//! Owns: the message vocabulary every shell speaks (ADR 0004) — [`Command`] (intent to change), [`Query`] (read a
//! projection window), [`Event`] (what changed) — and the dispatcher that runs them on a [`Session`].
//! Not here: transports (`apps/devserver`, future desktop / web worker), loading a build from disk (shells),
//! the operations themselves (`@sat:session.command`, `@sat:order.book`).
//!
//! Rules (ADR 0004 §4–5): a command never returns rows, it returns an [`Event`] with row patches per view and the
//! new revision; the UI re-reads only the window it shows. Queries never change anything.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use sat_core::ids::ModId;
use sat_core::order::{self, Filter, Key, MAIN, Nav, OrderError, RowKind, Select, Shift, ViewChange, ViewId};
use sat_core::overwrite::OverwriteStats;
use sat_core::plugins::PluginStats;
use sat_core::resource::{Chord, CommandError, Registry};
use sat_core::session::Session;

// @sat:app.command
/// An intent to change state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Command {
    /// A key press in a list window: looked up in the key map, run as a command resource.
    Key {
        view: ViewId,
        chord: String,
    },
    /// A command resource by id (menu, palette).
    Run {
        view: ViewId,
        id: String,
    },
    Navigate {
        view: ViewId,
        nav: Nav,
        select: Select,
    },
    ShiftSelection {
        view: ViewId,
        how: Shift,
    },
    Move {
        keys: Vec<Key>,
        before: Option<Key>,
    },
    SetEnabled {
        keys: Vec<Key>,
        enabled: Option<bool>,
    },
    SetColor {
        keys: Vec<Key>,
        color: Option<u32>,
    },
    SetCollapsed {
        view: ViewId,
        separator: Key,
        collapsed: bool,
    },
    SetFilter {
        view: ViewId,
        filter: Filter,
    },
    Undo,
    Redo,
    OpenView,
    CloseView {
        view: ViewId,
    },
}

// @sat:app.query
/// A read of a projection window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Query {
    /// Rows `[start, start + count)` of a view, with the mod card counters.
    Rows {
        view: ViewId,
        start: usize,
        count: usize,
    },
    Totals,
    /// The plugin list (full; a few thousand entries).
    Plugins,
    /// Active key bindings of the list context.
    Keymap,
}

/// One row as a window shows it (ADR 0008 row + the mod card counters).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RowView {
    pub key: Key,
    pub pos: u32,
    pub kind: RowKind,
    pub name: String,
    pub enabled: bool,
    pub depth: u8,
    pub collapsed: bool,
    pub color: Option<u32>,
    pub size: u32,
    pub muted: bool,
    pub focused: bool,
    pub selected: bool,
    /// Overwrite counters of a mod: wins, losses, middle, unique; X = fully overridden.
    pub counters: Option<Counters>,
}

/// The counter group at the bottom right of a mod card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    pub win: u32,
    pub lose: u32,
    pub mid: u32,
    pub unique: u32,
    pub x: bool,
    pub plugins: u32,
}

/// One plugin as the plugin window shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginView {
    pub name: String,
    pub index: Option<String>,
    pub colour: sat_core::plugins::Colour,
    pub listed: sat_core::plugins::Listed,
    pub active: bool,
    pub owner: Option<String>,
    pub versions: usize,
    pub missing_masters: Vec<String>,
}

/// Answer to a query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Reply {
    Rows {
        view: ViewId,
        len: usize,
        rows: Vec<RowView>,
    },
    Totals {
        overwrite: OverwriteStats,
        plugins: PluginStats,
        revision: u64,
    },
    Plugins {
        plugins: Vec<PluginView>,
    },
    Keymap {
        bindings: Vec<(String, String)>,
    },
}

// @sat:app.event
/// What a command changed: per-view row patches (the UI re-reads its window) and whether the counters and
/// plugin list may have changed (canon / facts touched).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Event {
    Changed {
        revision: u64,
        views: Vec<ViewChange>,
        canon: bool,
    },
    ViewOpened {
        view: ViewId,
    },
    ViewClosed {
        view: ViewId,
    },
    /// The command was valid but changed nothing.
    Nothing,
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Order(#[from] OrderError),
    #[error(transparent)]
    Command(#[from] CommandError),
    #[error("no key binding for {0}")]
    Unbound(String),
    #[error("bad chord {0:?}: {1}")]
    Chord(String, String),
}

// @sat:app.dispatch
/// One open build: the session and the resources active on this machine.
#[derive(Debug)]
pub struct App {
    session: Session,
    registry: Registry,
    revision: u64,
}

impl App {
    pub fn new(session: Session, registry: Registry) -> Self {
        Self {
            session,
            registry,
            revision: 0,
        }
    }

    pub fn session(&mut self) -> &mut Session {
        &mut self.session
    }

    /// Runs a command; returns what changed.
    pub fn command(&mut self, c: Command) -> Result<Event, AppError> {
        use Command as C;
        let changed: Option<order::Change> = match c {
            C::Key { view, chord } => {
                let ch = Chord::parse(&chord).map_err(|e| AppError::Chord(chord.clone(), e))?;
                let id = self
                    .registry
                    .keymap()
                    .lookup("list", &ch)
                    .map(str::to_string)
                    .ok_or_else(|| AppError::Unbound(ch.to_string()))?;
                self.session.run(&self.registry, &id, view)?
            }
            C::Run { view, id } => self.session.run(&self.registry, &id, view)?,
            C::Navigate { view, nav, select } => self.session.command(|b| b.navigate(view, nav, select))?,
            C::ShiftSelection { view, how } => self.session.command(|b| b.shift_selection(view, how))?,
            C::Move { keys, before } => self.session.command(|b| b.move_keys(&keys, before))?,
            C::SetEnabled { keys, enabled } => self.session.command(|b| b.set_enabled(&keys, enabled))?,
            C::SetColor { keys, color } => self.session.command(|b| b.set_color(&keys, color))?,
            C::SetCollapsed {
                view,
                separator,
                collapsed,
            } => self
                .session
                .command(|b| b.set_collapsed(view, separator, collapsed))?,
            C::SetFilter { view, filter } => self.session.command(|b| b.set_filter(view, filter))?,
            C::Undo => self.session.command(|b| Ok(b.undo()))?,
            C::Redo => self.session.command(|b| Ok(b.redo()))?,
            C::OpenView => {
                let mut opened = None;
                self.session.command(|b| {
                    opened = Some(b.open_view()?);
                    Ok(None::<order::Change>)
                })?;
                return Ok(Event::ViewOpened {
                    view: opened.expect("opened"),
                });
            }
            C::CloseView { view } => {
                self.session
                    .command(|b| b.close_view(view).map(|()| None::<order::Change>))?;
                return Ok(Event::ViewClosed { view });
            }
        };
        Ok(match changed {
            None => Event::Nothing,
            Some(ch) => {
                self.revision += 1;
                let canon = ch.truth.iter().any(|t| {
                    matches!(
                        t.truth,
                        sat_core::engine::Truth::Canon | sat_core::engine::Truth::Facts
                    )
                });
                Event::Changed {
                    revision: self.revision,
                    views: ch.views,
                    canon,
                }
            }
        })
    }

    /// Answers a query (projections are brought up to date lazily, only what the query needs).
    pub fn query(&mut self, q: Query) -> Result<Reply, AppError> {
        Ok(match q {
            Query::Rows { view, start, count } => {
                let ow = self.session.overwrite();
                let plugins = self.session.plugins();
                let names = self.session.table().clone();
                let index: std::collections::HashMap<String, ModId> = names
                    .mod_names()
                    .iter()
                    .enumerate()
                    .map(|(i, n)| (n.to_lowercase(), ModId::from_idx(i)))
                    .collect();
                let mut per_mod = vec![0u32; names.mod_count()];
                for p in &plugins.plugins {
                    if let Some(w) = p.winner {
                        per_mod[p.versions[w].owner.idx()] += 1;
                    }
                }
                let book = self.session.book();
                let len = book.row_count(view);
                let rows = book
                    .rows(view, start, count)
                    .into_iter()
                    .map(|r| {
                        let counters = (r.kind == RowKind::Mod)
                            .then(|| index.get(&r.name.to_lowercase()))
                            .flatten()
                            .map(|&m| {
                                let s = ow.stat(m);
                                Counters {
                                    win: s.win,
                                    lose: s.lose,
                                    mid: s.mid,
                                    unique: s.unique,
                                    x: s.fully_overridden,
                                    plugins: per_mod[m.idx()],
                                }
                            });
                        RowView {
                            key: r.key,
                            pos: r.pos,
                            kind: r.kind,
                            name: r.name.to_string(),
                            enabled: r.enabled,
                            depth: r.depth,
                            collapsed: r.collapsed,
                            color: r.color,
                            size: r.size,
                            muted: r.muted,
                            focused: r.focused,
                            selected: r.selected,
                            counters,
                        }
                    })
                    .collect();
                Reply::Rows { view, len, rows }
            }
            Query::Totals => {
                let ow = self.session.overwrite();
                let pl = self.session.plugins();
                Reply::Totals {
                    overwrite: ow.totals(),
                    plugins: pl.stats,
                    revision: self.revision,
                }
            }
            Query::Plugins => {
                let pl: Arc<_> = self.session.plugins();
                let table = self.session.table().clone();
                let plugins = pl
                    .plugins
                    .iter()
                    .map(|p| PluginView {
                        name: p.name.clone(),
                        index: p.index.map(|i| i.to_string()),
                        colour: p.colour,
                        listed: p.listed,
                        active: p.active,
                        owner: p.winner.map(|w| table.mod_name(p.versions[w].owner).to_string()),
                        versions: p.versions.len(),
                        missing_masters: p.missing_masters.clone(),
                    })
                    .collect();
                Reply::Plugins { plugins }
            }
            Query::Keymap => Reply::Keymap {
                bindings: self
                    .registry
                    .keymap()
                    .bindings()
                    .filter(|(ctx, _, _)| *ctx == "list")
                    .map(|(_, ch, cmd)| (ch.to_string(), cmd.to_string()))
                    .collect(),
            },
        })
    }
}

/// The main view (always open).
pub const MAIN_VIEW: ViewId = MAIN;

#[cfg(test)]
mod tests {
    use super::*;
    use sat_core::canon::{Entry, EntryKind, Modlist};
    use sat_core::order::OrderBook;
    use sat_core::table::ProviderTable;

    fn app() -> App {
        let facts: Vec<sat_core::facts::ModFacts> = ["A", "B"]
            .iter()
            .map(|n| {
                let mut f = sat_core::facts::ModFacts {
                    name: n.to_string(),
                    ..Default::default()
                };
                f.dirs.push(sat_core::facts::LooseDir {
                    rel: "textures".into(),
                    files: vec![sat_core::facts::LooseFile {
                        name: "a.dds".into(),
                        size: 10,
                        vram: 0,
                    }],
                });
                f
            })
            .collect();
        let table = Arc::new(ProviderTable::build(&facts));
        let list = Modlist::new(
            ["S_separator", "A", "B"]
                .iter()
                .map(|n| Entry {
                    name: n.to_string(),
                    kind: if n.ends_with("_separator") {
                        EntryKind::Separator
                    } else {
                        EntryKind::Mod
                    },
                    enabled: true,
                })
                .collect(),
        );
        App::new(
            Session::new(OrderBook::from_modlist(&list), table),
            Registry::with_builtins(),
        )
    }

    #[test]
    fn json_round_trip_and_keys_drive_the_list() {
        let mut a = app();
        let c: Command = serde_json::from_str(r#"{"type":"key","view":0,"chord":"ArrowDown"}"#).unwrap();
        assert!(matches!(
            a.command(c).unwrap(),
            Event::Changed { canon: false, .. }
        ));
        a.command(Command::Key {
            view: 0,
            chord: "ArrowDown".into(),
        })
        .unwrap(); // focus A
        let ev = a
            .command(Command::Key {
                view: 0,
                chord: "Alt+ArrowDown".into(),
            })
            .unwrap();
        assert!(matches!(ev, Event::Changed { canon: true, .. }));
        let Reply::Rows { rows, len, .. } = a
            .query(Query::Rows {
                view: 0,
                start: 0,
                count: 10,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(len, 3);
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["S_separator", "B", "A"]
        );
        // A is now below B = higher priority: it wins the shared texture
        let a_row = &rows[2];
        assert_eq!(a_row.counters.unwrap().win, 1);
        assert!(a_row.focused && a_row.selected);
        let json = serde_json::to_string(&a.query(Query::Totals).unwrap()).unwrap();
        assert!(json.contains("\"conflict_paths\":1"), "{json}");
        assert!(matches!(
            a.command(Command::Key {
                view: 0,
                chord: "F9".into()
            }),
            Err(AppError::Unbound(_))
        ));
    }
}
