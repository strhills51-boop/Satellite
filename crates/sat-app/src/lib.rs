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
    /// Move the focus to an entry (a link from another block: "go to this mod").
    Focus {
        view: ViewId,
        key: Key,
    },
    /// Mark a chain settled ("done") or not, by its content signature (hex string: JSON numbers lose 64-bit
    /// precision).
    SetChainDone {
        signature: String,
        done: bool,
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
        /// Chain icons per context badge on mod cards (the "density"); 0 = no badges.
        #[serde(default)]
        badges: u8,
    },
    Totals,
    /// The plugin list (full; a few thousand entries).
    Plugins,
    /// Active key bindings of the list context.
    Keymap,
    /// Everything the summary shows about one mod: with whom and in what it conflicts, its plugins.
    ModDetail {
        key: Key,
    },
    /// Separator tiles of the left column.
    Separators,
}

/// One separator tile: its block's size and how many mods in it are on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeparatorTile {
    pub key: Key,
    pub name: String,
    pub index: u32,
    /// Position of the first mod of the block (MO2 priority), if the block has any.
    pub first: Option<u32>,
    pub mods: u32,
    pub enabled: u32,
}

// @sat:app.mod-detail
/// One opponent of a mod: who overrides whom and on how many files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Opponent {
    /// Book key (absent for Overwrite, which is not an entry of the list).
    pub key: Option<Key>,
    pub name: String,
    pub pos: Option<u32>,
    /// Separator of the participant's block (participants are grouped by it, like the monolith).
    pub separator: Option<String>,
    /// This row is the focused mod itself.
    pub me: bool,
    pub enabled: bool,
    /// Files where this mod overrides the focused one.
    pub beats_me: u32,
    /// Files where the focused mod overrides this one.
    pub i_beat: u32,
    /// One tooth per listed chain of the focused mod (same order as `chains`): `w` this participant wins that
    /// chain, `l` it is overridden there, `-` it is not in that chain.
    pub teeth: String,
}

/// One exact chain of the focused mod.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainView {
    pub id: u32,
    pub signature: String,
    pub done: bool,
    pub emoji: String,
    pub hue: u16,
    /// Generated from the paths: top folder and the most common extension.
    pub title: String,
    /// Generated name: title · first mod → last mod (monolith chain names).
    pub name: String,
    pub files: u32,
    pub bytes: u64,
    /// Stack, winner first.
    pub stack: Vec<String>,
    /// Place of the focused mod in the stack (0 = winner).
    pub place: usize,
    pub samples: Vec<String>,
}

/// The summary of one mod.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModDetail {
    pub key: Key,
    pub name: String,
    pub counters: Option<Counters>,
    pub opponents: Vec<Opponent>,
    pub chains: Vec<ChainView>,
    /// Chains not listed (the list keeps the largest).
    pub more_chains: usize,
    pub plugins: Vec<PluginView>,
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
    /// Overwrite badges, one per context of the mod (empty when not asked or no conflicts).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub badges: Vec<ContextBadge>,
}

// @sat:app.badges
/// One context badge of a mod card (monolith "classic" overwrite badges): the context's colour and the icons of
/// the mod's largest chains in it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextBadge {
    pub hue: u16,
    pub chains: Vec<ChainBadge>,
    /// Chains of this mod in the context not shown as icons.
    pub more: u32,
    /// Conflict files of this mod in the context.
    pub files: u32,
}

/// One chain icon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainBadge {
    /// Content signature, hex (`@sat:identity.of`).
    pub signature: String,
    /// Settled by the user: shown as a check mark.
    pub done: bool,
    pub emoji: String,
    pub hue: u16,
    pub files: u32,
    /// Place of the mod in the chain's stack (0 = it wins there).
    pub place: u8,
    /// Stack, winner first (tooltip).
    pub stack: Vec<String>,
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
    /// `focus_row`: the row of the keyboard focus in this view (the window scrolls to keep it visible).
    Rows {
        view: ViewId,
        len: usize,
        focus_row: Option<usize>,
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
    ModDetail {
        detail: Box<ModDetail>,
    },
    Separators {
        tiles: Vec<SeparatorTile>,
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
    /// A decision changed: badges and the summary re-read.
    Decided,
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
    #[error("bad chain signature {0:?}")]
    Signature(String),
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
            C::Focus { view, key } => self.session.command(|b| Ok(b.focus_entry(view, key)))?,
            C::SetChainDone { signature, done } => {
                let sig = u64::from_str_radix(&signature, 16)
                    .map_err(|_| AppError::Signature(signature.clone()))?;
                return Ok(if self.session.set_chain_done(sig, done) {
                    self.revision += 1;
                    Event::Decided
                } else {
                    Event::Nothing
                });
            }
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
            Query::Rows {
                view,
                start,
                count,
                badges,
            } => {
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
                            badges: match (badges, index.get(&r.name.to_lowercase())) {
                                (n, Some(&m)) if n > 0 && r.kind == RowKind::Mod => {
                                    mod_badges(&ow, &names, self.session.decisions(), m, n)
                                }
                                _ => Vec::new(),
                            },
                            counters,
                        }
                    })
                    .collect();
                Reply::Rows {
                    view,
                    len,
                    focus_row: book.focus_row(view),
                    rows,
                }
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
            Query::ModDetail { key } => Reply::ModDetail {
                detail: Box::new(self.mod_detail(key)),
            },
            Query::Separators => {
                let book = self.session.book();
                let mut tiles: Vec<SeparatorTile> = Vec::new();
                for (pos, (name, kind, enabled)) in book.entries().enumerate() {
                    match kind {
                        sat_core::canon::EntryKind::Separator => tiles.push(SeparatorTile {
                            key: book.key_at(pos),
                            name: name.trim_end_matches("_separator").to_string(),
                            index: tiles.len() as u32 + 1,
                            first: None,
                            mods: 0,
                            enabled: 0,
                        }),
                        sat_core::canon::EntryKind::Mod => {
                            if let Some(t) = tiles.last_mut() {
                                t.first.get_or_insert(pos as u32);
                                t.mods += 1;
                                t.enabled += u32::from(enabled);
                            }
                        }
                        sat_core::canon::EntryKind::Foreign => {}
                    }
                }
                Reply::Separators { tiles }
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

/// Context badges of one mod: chains grouped by context, largest first, `limit` icons per context.
fn mod_badges(
    ow: &sat_core::overwrite::OverwriteProjection,
    table: &sat_core::table::ProviderTable,
    decisions: &sat_core::decisions::ChainDecisions,
    m: ModId,
    limit: u8,
) -> Vec<ContextBadge> {
    let mut chains = ow.mod_chains(m);
    if chains.is_empty() {
        return Vec::new();
    }
    chains.sort_by_key(|&c| std::cmp::Reverse((ow.chain(c).files, c.0)));
    let mut by_ctx: Vec<(Option<sat_core::ids::ContextId>, Vec<sat_core::ids::ChainId>)> = Vec::new();
    for c in chains {
        let ctx = ow.chain_context(c);
        match by_ctx.iter_mut().find(|(k, _)| *k == ctx) {
            Some((_, v)) => v.push(c),
            None => by_ctx.push((ctx, vec![c])),
        }
    }
    let name = |x: ModId| table.mod_name(x);
    by_ctx
        .into_iter()
        .map(|(ctx, cs)| {
            let hue = ctx.map_or(210, |k| {
                sat_core::identity::identity(ow.contexts()[k.idx()].mods.iter().map(|&x| name(x))).hue
            });
            let files = cs.iter().map(|&c| ow.chain(c).files).sum();
            let shown: Vec<ChainBadge> = cs
                .iter()
                .take(limit as usize)
                .map(|&c| {
                    let stack = ow.chain_mods(c);
                    let id = sat_core::identity::identity(stack.iter().map(|&x| name(x)));
                    ChainBadge {
                        signature: format!("{:016x}", id.signature),
                        done: decisions.is_done(id.signature),
                        emoji: id.emoji.to_string(),
                        hue: id.hue,
                        files: ow.chain(c).files,
                        place: stack.iter().position(|&x| x == m).unwrap_or(0) as u8,
                        stack: stack.iter().map(|&x| name(x).to_string()).collect(),
                    }
                })
                .collect();
            ContextBadge {
                hue,
                more: (cs.len() - shown.len()) as u32,
                files,
                chains: shown,
            }
        })
        .collect()
}

const CHAIN_LIMIT: usize = 60;
const SAMPLE_LIMIT: usize = 6;

/// Title of a chain from its paths: the most common top folder and extension (`textures · dds`).
fn chain_title(paths: &[String]) -> String {
    let mut top: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut ext: std::collections::BTreeMap<&str, usize> = Default::default();
    for p in paths {
        *top.entry(p.split('/').next().unwrap_or("")).or_default() += 1;
        if let Some((_, e)) = p.rsplit_once('.') {
            *ext.entry(e).or_default() += 1;
        }
    }
    let best = |m: &std::collections::BTreeMap<&str, usize>| {
        m.iter()
            .max_by_key(|(k, n)| (**n, std::cmp::Reverse(**k)))
            .map(|(k, _)| k.to_string())
    };
    match (best(&top), best(&ext)) {
        (Some(t), Some(e)) if !t.contains('.') => format!("{t} · {e}"),
        (Some(t), _) => t,
        _ => String::new(),
    }
}

impl App {
    fn mod_detail(&mut self, key: Key) -> ModDetail {
        let ow = self.session.overwrite();
        let plugins = self.session.plugins();
        let table = self.session.table().clone();
        let book = self.session.book();
        let name = book.name(key).to_string();
        let index: std::collections::HashMap<String, ModId> = table
            .mod_names()
            .iter()
            .enumerate()
            .map(|(i, n)| (n.to_lowercase(), ModId::from_idx(i)))
            .collect();
        let Some(&m) = index.get(&name.to_lowercase()) else {
            return ModDetail {
                key,
                name,
                counters: None,
                opponents: Vec::new(),
                chains: Vec::new(),
                more_chains: 0,
                plugins: Vec::new(),
            };
        };
        let mut chains = ow.mod_chains(m);
        chains.sort_by_key(|&c| std::cmp::Reverse((ow.chain(c).files, c.0)));
        let more_chains = chains.len().saturating_sub(CHAIN_LIMIT);
        // opponents over every chain; teeth over the listed ones
        let mut opp: std::collections::HashMap<ModId, (u32, u32)> = Default::default();
        for &c in &chains {
            let stack = ow.chain_mods(c);
            let me = stack.iter().position(|&x| x == m).unwrap_or(0);
            let files = ow.chain(c).files;
            for (i, &o) in stack.iter().enumerate() {
                if o == m {
                    continue;
                }
                let e = opp.entry(o).or_default();
                if i < me {
                    e.0 += files;
                } else {
                    e.1 += files;
                }
            }
        }
        let listed: Vec<_> = chains.iter().copied().take(CHAIN_LIMIT).collect();
        let chain_views: Vec<ChainView> = listed
            .iter()
            .map(|&c| {
                let stack = ow.chain_mods(c);
                let paths: Vec<String> = ow
                    .chain_paths(c)
                    .iter()
                    .take(64)
                    .map(|&p| table.path_string(p))
                    .collect();
                let id = sat_core::identity::identity(stack.iter().map(|&x| table.mod_name(x)));
                let title = chain_title(&paths);
                let first = table.mod_name(stack[stack.len() - 1]);
                let last = table.mod_name(stack[0]);
                ChainView {
                    id: c.0,
                    signature: format!("{:016x}", id.signature),
                    done: self.session.decisions().is_done(id.signature),
                    emoji: id.emoji.to_string(),
                    hue: id.hue,
                    name: if title.is_empty() {
                        format!("{first} → {last}")
                    } else {
                        format!("{title} · {first} → {last}")
                    },
                    title,
                    files: ow.chain(c).files,
                    bytes: ow.chain(c).bytes,
                    stack: stack.iter().map(|&x| table.mod_name(x).to_string()).collect(),
                    place: stack.iter().position(|&x| x == m).unwrap_or(0),
                    samples: paths.into_iter().take(SAMPLE_LIMIT).collect(),
                }
            })
            .collect();
        opp.entry(m).or_default();
        let book = self.session.book();
        let mut opponents: Vec<Opponent> = opp
            .into_iter()
            .map(|(o, (beats_me, i_beat))| {
                let oname = table.mod_name(o).to_string();
                let okey = book.key_of(&oname);
                let teeth = listed
                    .iter()
                    .map(|&c| match ow.chain_mods(c).iter().position(|&x| x == o) {
                        Some(0) => 'w',
                        Some(_) => 'l',
                        None => '-',
                    })
                    .collect();
                Opponent {
                    key: okey,
                    pos: okey.map(|k| book.pos(k) as u32),
                    separator: okey
                        .and_then(|k| book.separator_of(k))
                        .map(|s| book.name(s).trim_end_matches("_separator").to_string()),
                    me: o == m,
                    enabled: ow.priorities().is_active(o),
                    name: oname,
                    beats_me,
                    i_beat,
                    teeth,
                }
            })
            .collect();
        opponents.sort_by_key(|o| (o.pos.is_none(), o.pos, o.name.clone()));
        let s = ow.stat(m);
        let mod_plugins: Vec<PluginView> = plugins
            .plugins
            .iter()
            .filter(|p| p.versions.iter().any(|v| v.owner == m))
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
        ModDetail {
            key,
            name,
            counters: Some(Counters {
                win: s.win,
                lose: s.lose,
                mid: s.mid,
                unique: s.unique,
                x: s.fully_overridden,
                plugins: mod_plugins.len() as u32,
            }),
            opponents,
            chains: chain_views,
            more_chains,
            plugins: mod_plugins,
        }
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
                badges: 4,
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
        // one context, one chain icon; the same chain shows the same icon on both mods
        assert_eq!(a_row.badges.len(), 1);
        let icon = &a_row.badges[0].chains[0];
        assert_eq!((icon.place, icon.files, icon.stack.len()), (0, 1, 2));
        assert_eq!(rows[1].badges[0].chains[0].emoji, icon.emoji);
        assert_eq!(rows[1].badges[0].chains[0].place, 1);
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
        let key_a = rows[2].key;
        let Reply::ModDetail { detail } = a.query(Query::ModDetail { key: key_a }).unwrap() else {
            panic!()
        };
        // participants: B, then A itself (list order), grouped under separator S
        let names: Vec<&str> = detail.opponents.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["B", "A"]);
        let b = &detail.opponents[0];
        assert_eq!(
            (
                b.i_beat,
                b.beats_me,
                b.teeth.as_str(),
                b.me,
                b.separator.as_deref()
            ),
            (1, 0, "l", false, Some("S"))
        );
        assert_eq!(detail.opponents[1].teeth, "w");
        assert!(detail.opponents[1].me);
        let ch = &detail.chains[0];
        assert_eq!(ch.title, "textures · dds");
        assert_eq!(ch.name, "textures · dds · B → A");
        assert_eq!(ch.stack, ["A", "B"]);
        // settle the chain: the badge shows it on both mods, and it survives a move
        assert!(matches!(
            a.command(Command::SetChainDone {
                signature: ch.signature.clone(),
                done: true
            })
            .unwrap(),
            Event::Decided
        ));
        let b_key = b.key.unwrap();
        a.command(Command::Move {
            keys: vec![key_a],
            before: Some(b_key),
        })
        .unwrap();
        let Reply::Rows { rows: after, .. } = a
            .query(Query::Rows {
                view: 0,
                start: 0,
                count: 10,
                badges: 4,
            })
            .unwrap()
        else {
            panic!()
        };
        assert!(
            after
                .iter()
                .filter(|r| r.kind == RowKind::Mod)
                .all(|r| r.badges[0].chains[0].done)
        );
        assert!(matches!(
            a.command(Command::SetChainDone {
                signature: "zz".into(),
                done: true
            }),
            Err(AppError::Signature(_))
        ));
        // a link from the summary moves the focus
        a.command(Command::Focus {
            view: 0,
            key: b.key.unwrap(),
        })
        .unwrap();
        let Reply::Rows { focus_row, .. } = a
            .query(Query::Rows {
                view: 0,
                start: 0,
                count: 1,
                badges: 0,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(focus_row, Some(2), "B is below A after the move");
    }
}
