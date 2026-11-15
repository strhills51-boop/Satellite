//! The conflict context table (monolith "Модалка конфликтов" with the colour-types flag). @sat:app.context-table
//!
//! Owns: the list of contexts for the selector, and the table window of one context — rows are the context's mods
//! and the disabled mods that own its paths (list order, grouped by separator), columns are its owner stacks
//! (`@sat:app.participants`: by chain, by strength, or by the monolith's activity score around a selected chain), a
//! cell says whether the row's mod wins the stack, loses it, owns it while disabled, or does not take part.
//! Not here: chains, contexts and stacks (`@sat:overwrite.projection`), colours of file types (the page).
//!
//! Both axes come in windows: a context of 300 mods × 40 000 paths is never sent whole. A chain is one exact stack,
//! so all its paths share one column pattern; a cell is looked up through the path's chain, not stored. The sorted
//! column list of the open context is cached per app revision.

use serde::{Deserialize, Serialize};

use sat_core::ids::{ContextId, ModId};
use sat_core::order::Key;

use crate::App;

/// Column order of the table (monolith: "Сила конфликта" / "Релевантность цепочке").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ColumnOrder {
    /// Paths grouped by chain, larger chains first.
    #[default]
    Chain,
    /// Deeper stacks first (more mods fight for the path), then by chain.
    Strength,
    /// The monolith's matrix around a selected chain (S17 147 956, `activityScore`): the selected chain's stacks
    /// first, then every stack by `strong·100 + linked·70 + owned·30 + present·10` — strong are wins and losses
    /// inside the selected chain, linked outside it, owned the disabled owners, present everyone in the stack.
    /// Without a selection every cell is strong.
    Activity,
}

/// One context in the selector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextItem {
    /// A mod of the context: the table is asked by it (context numbers change with the order, mods do not).
    pub key: Option<Key>,
    pub name: String,
    pub hue: u16,
    pub mods: usize,
    pub chains: usize,
    pub files: u32,
}

/// A chain heading a group of columns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableChain {
    /// Position of the chain in the context's column order (what [`TableCol::chain`] refers to).
    pub index: u32,
    pub signature: String,
    pub done: bool,
    pub emoji: String,
    pub hue: u16,
    pub name: String,
    pub files: u32,
    /// Stack depth.
    pub depth: usize,
}

/// One row: a mod of the context, or the heading of a separator group (monolith groups rows by separator).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableRow {
    /// A separator heading: `name` is the separator, `key` its entry, no cells.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub header: bool,
    pub key: Option<Key>,
    pub name: String,
    pub pos: Option<u32>,
    pub separator: Option<String>,
    pub enabled: bool,
    /// Paths of the context this mod wins / loses (whole row, not the window).
    pub wins: u32,
    pub loses: u32,
    /// Paths of the context it owns while disabled.
    #[serde(default)]
    pub owned: u32,
    /// One char per column of the window: `w` wins, `l` loses, `u` owns it while disabled, `-` not there.
    pub cells: String,
}

/// One column: a stack — targets that go through their chain the same way (`@sat:conflict.stacks`); a single
/// target is a stack of one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableCol {
    /// The target, or what the stack's targets share (`textures/armor/iron/ … 118 .dds`).
    pub path: String,
    /// Index into `chains` of the reply.
    pub chain: u32,
    /// Targets in the stack.
    #[serde(default = "one")]
    pub count: u32,
    /// The stack's place in its chain (for `StackTargets`).
    #[serde(default)]
    pub stack: u32,
}

fn one() -> u32 {
    1
}

// @sat:app.stack-label
/// What a stack's targets share: the one target, or their common folder with the extensions by count.
pub fn stack_label(targets: &[String]) -> String {
    match targets {
        [] => String::new(),
        [one] => one.clone(),
        [first, ..] if !targets.iter().all(|t| t.contains('/')) => format!("{first} …"),
        [first, rest @ ..] => {
            let mut common: Vec<&str> = first.split('/').collect();
            common.pop();
            for t in rest {
                let segs: Vec<&str> = t.split('/').collect();
                let n = common
                    .iter()
                    .zip(&segs[..segs.len().saturating_sub(1)])
                    .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
                    .count();
                common.truncate(n);
            }
            let mut ext: std::collections::BTreeMap<String, u32> = Default::default();
            for t in targets {
                let name = t.rsplit('/').next().unwrap_or(t);
                let e = name
                    .rsplit_once('.')
                    .map_or(String::new(), |x| format!(".{}", x.1.to_lowercase()));
                *ext.entry(e).or_default() += 1;
            }
            let mut ext: Vec<(String, u32)> = ext.into_iter().collect();
            ext.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let kinds: Vec<String> = ext
                .iter()
                .take(3)
                .map(|(e, n)| {
                    if e.is_empty() {
                        format!("{n}")
                    } else {
                        format!("{n} {e}")
                    }
                })
                .collect();
            let head = if common.is_empty() {
                String::new()
            } else {
                format!("{}/ ", common.join("/"))
            };
            format!(
                "{head}… {}{}",
                kinds.join(", "),
                if ext.len() > 3 { ", …" } else { "" }
            )
        }
    }
}

/// A window of the context table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextTable {
    pub name: String,
    pub hue: u16,
    pub rows_total: usize,
    pub cols_total: usize,
    /// Chains of the columns in the window (a context may have thousands; the window names only its own).
    pub chains: Vec<TableChain>,
    pub chains_total: usize,
    /// First column of each chain in the column order (`Chain` order only; empty for `Strength`).
    pub chain_starts: Vec<u32>,
    pub rows: Vec<TableRow>,
    pub cols: Vec<TableCol>,
}

/// Sorted columns of the open context.
#[derive(Debug)]
pub(crate) struct OpenContext {
    revision: u64,
    context: ContextId,
    order: ColumnOrder,
    /// Normalized chains in column order, and their headings.
    heads: Vec<TableChain>,
    /// Selected chain (index into `heads`) the `Activity` order was computed for.
    focus: Option<u32>,
    /// Columns: (the stack's place in its normalized chain, index into `heads`, the owner stack — it gives the
    /// cells).
    cols: Vec<(u32, u32, crate::participants::OwnerStack)>,
    chain_starts: Vec<u32>,
    /// Rows: separator headings and context mods with wins / loses.
    rows: Vec<RowItem>,
}

#[derive(Debug, Clone, Copy)]
enum RowItem {
    Header(Option<Key>),
    /// A mod with its wins, losses and owned-while-disabled paths in the context.
    Mod(ModId, u32, u32, u32),
}

impl App {
    // @sat:app.context-name
    /// A context's name: what it is about (`@sat:naming.title` over its mods, their separators and sample paths) and
    /// its lowest and highest mods — `title · first → last`, like a chain's.
    fn context_name(
        &self,
        ow: &sat_core::overwrite::OverwriteProjection,
        cx: &sat_core::overwrite::Context,
    ) -> String {
        let table = self.session.table();
        let book = self.session.book();
        let labels: Vec<&str> = cx.mods.iter().map(|&m| self.session.unit_label(m)).collect();
        let seps: Vec<String> = cx
            .mods
            .iter()
            .filter_map(|&m| book.key_of(table.mod_name(m)))
            .filter_map(|k| book.separator_of(k))
            .map(|k| book.name(k).trim_end_matches("_separator").to_string())
            .collect();
        let seps: Vec<&str> = seps.iter().map(String::as_str).collect();
        let mut paths: Vec<String> = Vec::new();
        for &c in &cx.chains {
            if paths.len() >= 64 {
                break;
            }
            paths.extend(
                ow.chain_paths(c)
                    .iter()
                    .take(64 - paths.len())
                    .map(|&p| table.path_string(p)),
            );
        }
        let title = sat_core::naming::title(
            &sat_core::naming::ChainFeatures {
                mods: &labels,
                separators: &seps,
                paths: &paths,
                files: cx.files,
            },
            sat_core::naming::STRENGTH_DEFAULT,
        );
        let ends = format!(
            "{} → {}",
            labels.first().copied().unwrap_or(""),
            labels.last().copied().unwrap_or("")
        );
        if title.is_empty() {
            ends
        } else {
            format!("{title} · {ends}")
        }
    }

    /// Contexts, largest first.
    pub(crate) fn contexts(&mut self) -> Vec<ContextItem> {
        let ow = self.session.overwrite();
        let heads = self.heads();
        let table = self.session.table().clone();
        let book = self.session.book();
        let mut out: Vec<ContextItem> = ow
            .contexts()
            .iter()
            .map(|cx| {
                let id = sat_core::identity::identity(cx.mods.iter().map(|&m| self.session.member_key(m)));
                ContextItem {
                    key: cx.mods.first().and_then(|&m| book.key_of(table.mod_name(m))),
                    name: self.context_name(&ow, cx),
                    hue: id.hue,
                    mods: cx.mods.len(),
                    chains: {
                        let mut gs: Vec<u32> = cx.chains.iter().filter_map(|&c| heads.of_chain(c)).collect();
                        gs.sort_unstable();
                        gs.dedup();
                        gs.len()
                    },
                    files: cx.files,
                }
            })
            .collect();
        out.sort_by_key(|c| std::cmp::Reverse((c.files, c.mods)));
        out
    }

    /// The context of mod `key`.
    pub(crate) fn context_of(&mut self, key: Key) -> Option<ContextId> {
        let table = self.session.table();
        let name = self.session.book().name(key).to_lowercase();
        let m = table
            .mod_names()
            .iter()
            .position(|n| n.to_lowercase() == name)
            .map(ModId::from_idx)?;
        self.session.overwrite().mod_context(m)
    }

    // @sat:app.context-table.window
    /// The window `[row_start, +row_count) × [col_start, +col_count)` of the context of mod `key`.
    pub(crate) fn context_table(
        &mut self,
        key: Key,
        order: ColumnOrder,
        focus: Option<u32>,
        rows: (usize, usize),
        cols: (usize, usize),
    ) -> Option<ContextTable> {
        let ow = self.session.overwrite();
        let table = self.session.table().clone();
        let cx = self.context_of(key)?;
        let fresh = self.open_context.as_ref().is_some_and(|o| {
            o.revision == self.revision
                && o.context == cx
                && o.order == order
                && (order != ColumnOrder::Activity || o.focus == focus)
        });
        if !fresh {
            let heads = self.heads();
            let decisions = self.session.decisions();
            let book = self.session.book();
            let sep_of = |m: ModId| book.key_of(table.mod_name(m)).and_then(|k| book.separator_of(k));
            let pos_of = |m: ModId| book.key_of(table.mod_name(m)).map_or(usize::MAX, |k| book.pos(k));
            let lists = Lists {
                sep_of: &sep_of,
                pos_of: &pos_of,
            };
            self.open_context = Some(open(
                &ow,
                &table,
                &heads,
                decisions,
                &lists,
                cx,
                order,
                focus,
                self.revision,
            ));
        }
        let oc = self.open_context.as_ref().expect("opened");
        let context = &ow.contexts()[cx.idx()];
        let book = self.session.book();
        let (c0, c1) = (
            cols.0.min(oc.cols.len()),
            cols.0.saturating_add(cols.1).min(oc.cols.len()),
        );
        let window = &oc.cols[c0..c1];
        let (r0, r1) = (
            rows.0.min(oc.rows.len()),
            rows.0.saturating_add(rows.1).min(oc.rows.len()),
        );
        let sep_name = |k: Key| book.name(k).trim_end_matches("_separator").to_string();
        let row_views = oc.rows[r0..r1]
            .iter()
            .map(|&item| {
                let (rm, wins, loses, owned) = match item {
                    RowItem::Mod(m, w, l, u) => (m, w, l, u),
                    RowItem::Header(k) => {
                        return TableRow {
                            header: true,
                            key: k,
                            name: k.map_or_else(|| "—".to_string(), sep_name),
                            pos: k.map(|k| book.pos(k) as u32),
                            separator: None,
                            enabled: true,
                            wins: 0,
                            loses: 0,
                            owned: 0,
                            cells: String::new(),
                        };
                    }
                };
                let name = table.mod_name(rm).to_string();
                let key = book.key_of(&name);
                let cells = window.iter().map(|(_, _, st)| st.cell(&ow, rm)).collect();
                TableRow {
                    header: false,
                    pos: key.map(|k| book.pos(k) as u32),
                    separator: key.and_then(|k| book.separator_of(k)).map(sep_name),
                    enabled: key.is_some_and(|k| book.enabled(k)),
                    key,
                    name,
                    wins,
                    loses,
                    owned,
                    cells,
                }
            })
            .collect();
        let id = sat_core::identity::identity(context.mods.iter().map(|&x| self.session.member_key(x)));
        Some(ContextTable {
            name: self.context_name(&ow, context),
            hue: id.hue,
            rows_total: oc.rows.len(),
            cols_total: oc.cols.len(),
            chains: {
                let mut ids: Vec<u32> = window.iter().map(|(_, ci, _)| *ci).collect();
                ids.sort_unstable();
                ids.dedup();
                ids.into_iter().map(|i| oc.heads[i as usize].clone()).collect()
            },
            chains_total: oc.heads.len(),
            chain_starts: oc.chain_starts.clone(),
            rows: row_views,
            cols: window
                .iter()
                .map(|(k, ci, st)| TableCol {
                    path: stack_label(&st.path_strings(&table)),
                    chain: *ci,
                    count: st.files(),
                    stack: *k,
                })
                .collect(),
        })
    }
}

// @sat:app.context-table.groups
/// The chains (headings) of a context in their order (`@sat:arrange`): load order, bridged chains together — the
/// order of the table's chain bands and of the cards — with the link each one hangs on.
pub(crate) fn context_order(
    ow: &sat_core::overwrite::OverwriteProjection,
    heads: &crate::chains::Heads,
    cx: ContextId,
    pos_of: &dyn Fn(ModId) -> usize,
) -> Vec<(u32, Option<sat_core::arrange::Link>)> {
    let mut groups: Vec<u32> = ow.contexts()[cx.idx()]
        .chains
        .iter()
        .filter_map(|&c| heads.of_chain(c))
        .collect();
    groups.sort_unstable();
    groups.dedup();
    let items: Vec<sat_core::arrange::Item> = groups
        .iter()
        .map(|&g| {
            let route = &heads[g as usize].route;
            let mut key: Vec<usize> = route.iter().map(|&m| pos_of(m)).collect();
            key.sort_unstable();
            sat_core::arrange::Item {
                mods: route.iter().map(|m| m.0).collect(),
                key,
            }
        })
        .collect();
    sat_core::arrange::arrange(&items)
        .into_iter()
        .map(|(i, l)| (groups[i], l))
        .collect()
}

/// The chains of a context in their order (`context_order` without the links).
pub(crate) fn context_groups(
    ow: &sat_core::overwrite::OverwriteProjection,
    heads: &crate::chains::Heads,
    cx: ContextId,
    pos_of: &dyn Fn(ModId) -> usize,
) -> Vec<u32> {
    context_order(ow, heads, cx, pos_of)
        .into_iter()
        .map(|x| x.0)
        .collect()
}

/// How the list places a mod: its separator and its position.
pub(crate) struct Lists<'a> {
    pub sep_of: &'a dyn Fn(ModId) -> Option<Key>,
    pub pos_of: &'a dyn Fn(ModId) -> usize,
}

#[allow(clippy::too_many_arguments)]
fn open(
    ow: &sat_core::overwrite::OverwriteProjection,
    table: &sat_core::table::ProviderTable,
    heads: &crate::chains::Heads,
    decisions: &sat_core::decisions::ChainDecisions,
    lists: &Lists<'_>,
    cx: ContextId,
    order: ColumnOrder,
    focus: Option<u32>,
    revision: u64,
) -> OpenContext {
    let context = &ow.contexts()[cx.idx()];
    let groups = context_groups(ow, heads, cx, lists.pos_of);
    let paths_of = ow.context_chain_paths(cx);
    let mut cols: Vec<(u32, u32, crate::participants::OwnerStack)> = Vec::new();
    let mut chain_starts = Vec::with_capacity(groups.len());
    // one column per owner stack: its paths go through the chain the same way, disabled owners included
    for (i, &g) in groups.iter().enumerate() {
        chain_starts.push(cols.len() as u32);
        let st = crate::participants::head_stacks(ow, table, &heads[g as usize], Some(&paths_of));
        cols.extend(st.into_iter().enumerate().map(|(k, c)| (k as u32, i as u32, c)));
    }
    match order {
        ColumnOrder::Chain => {}
        ColumnOrder::Strength => {
            // deeper stacks first; stable, so within a depth the chain order stays
            cols.sort_by_key(|(_, _, st)| std::cmp::Reverse(ow.chain_mods(st.chain).len() + st.idle.len()));
            chain_starts.clear();
        }
        ColumnOrder::Activity => {
            let score = |ci: u32, st: &crate::participants::OwnerStack| {
                let active = ow.chain_mods(st.chain).len() as u64;
                let idle = st.idle.len() as u64;
                let inside = focus.is_none_or(|f| f == ci);
                let (strong, linked) = if inside { (active, 0) } else { (0, active) };
                strong * 100 + linked * 70 + idle * 30 + (active + idle) * 10
            };
            cols.sort_by_cached_key(|(_, ci, st)| {
                (
                    focus.is_some_and(|f| f != *ci),
                    std::cmp::Reverse(score(*ci, st)),
                    std::cmp::Reverse(st.files()),
                )
            });
            chain_starts.clear();
        }
    }
    // wins / losses / owned-while-disabled per member over the whole context
    let mut tally: std::collections::HashMap<ModId, (u32, u32, u32)> = Default::default();
    for (_, _, st) in &cols {
        let files = st.files();
        for (i, &m) in ow.chain_mods(st.chain).iter().enumerate() {
            let e = tally.entry(m).or_default();
            if i == 0 {
                e.0 += files;
            } else {
                e.1 += files;
            }
        }
        for &m in &st.idle {
            tally.entry(m).or_default().2 += files;
        }
    }
    // rows in list order (disabled owners where they sit), a separator heading before each block
    let mut members: Vec<ModId> = context.mods.clone();
    members.extend(tally.keys().copied().filter(|m| !context.mods.contains(m)));
    members.sort_by_key(|&m| ((lists.pos_of)(m), m));
    members.dedup();
    let mut rows = Vec::with_capacity(members.len() + 8);
    let mut last: Option<Option<Key>> = None;
    for &m in &members {
        let sep = (lists.sep_of)(m);
        if last != Some(sep) {
            rows.push(RowItem::Header(sep));
            last = Some(sep);
        }
        let (w, l, u) = tally.get(&m).copied().unwrap_or_default();
        rows.push(RowItem::Mod(m, w, l, u));
    }
    let heads: Vec<TableChain> = groups
        .iter()
        .enumerate()
        .map(|(i, &g)| {
            let h = &heads[g as usize];
            TableChain {
                index: i as u32,
                signature: h.hex(),
                done: h.done(decisions),
                emoji: h.emoji.to_string(),
                hue: h.hue,
                name: h.name.clone(),
                files: h.files,
                depth: h.route.len(),
            }
        })
        .collect();
    OpenContext {
        heads,
        revision,
        context: cx,
        order,
        focus,
        cols,
        chain_starts,
        rows,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sat_core::canon::{Entry, EntryKind, Modlist};
    use sat_core::facts::{LooseDir, LooseFile, ModFacts};
    use sat_core::order::OrderBook;
    use sat_core::resource::Registry;
    use sat_core::session::Session;
    use sat_core::table::ProviderTable;

    use crate::{App, ColumnOrder, Query, Reply};

    fn app(mods: &[(&str, &[&str])]) -> App {
        let facts: Vec<ModFacts> = mods
            .iter()
            .map(|(n, files)| ModFacts {
                name: n.to_string(),
                dirs: vec![LooseDir {
                    rel: "meshes".into(),
                    files: files
                        .iter()
                        .map(|f| LooseFile {
                            name: f.to_string(),
                            size: 1,
                            vram: 0,
                        })
                        .collect(),
                }],
                ..Default::default()
            })
            .collect();
        let mut entries = vec![Entry {
            name: "S_separator".into(),
            kind: EntryKind::Separator,
            enabled: false,
        }];
        entries.extend(mods.iter().map(|(n, _)| Entry {
            name: n.to_string(),
            kind: EntryKind::Mod,
            enabled: true,
        }));
        App::new(
            Session::new(
                OrderBook::from_modlist(&Modlist::new(entries)),
                Arc::new(ProviderTable::build(&facts)),
            ),
            Registry::with_builtins(),
        )
    }

    fn window(a: &mut App, order: ColumnOrder, cols: (usize, usize)) -> crate::ContextTable {
        let key = a.session().book().key_of("A").unwrap();
        let Reply::ContextTable { table: Some(t) } = a
            .query(Query::ContextTable {
                key,
                order,
                focus: None,
                row_start: 0,
                row_count: 10,
                col_start: cols.0,
                col_count: cols.1,
            })
            .unwrap()
        else {
            panic!("no table")
        };
        *t
    }

    #[test]
    fn cells_follow_the_stacks_and_both_axes_window() {
        // x: stack C, B, A (depth 3); y: stack C, A; z: only B → not a conflict
        let mut a = app(&[
            ("A", &["x.nif", "y.nif"]),
            ("B", &["x.nif", "z.nif"]),
            ("C", &["x.nif", "y.nif"]),
        ]);
        let Reply::Contexts { contexts } = a.query(Query::Contexts).unwrap() else {
            panic!()
        };
        assert_eq!(
            (contexts.len(), contexts[0].mods, contexts[0].chains),
            (1, 3, 1),
            "x and y are both won by C: one normalized chain"
        );
        // named by what it is about (folder, extension), then its ends
        assert_eq!(contexts[0].name, "meshes · nif · A → C");
        let t = window(&mut a, ColumnOrder::Strength, (0, 10));
        // a separator heading, then the three mods
        assert_eq!((t.rows_total, t.cols_total), (4, 2));
        assert!(t.rows[0].header && t.rows[0].name == "S" && t.rows[0].cells.is_empty());
        let t = crate::ContextTable {
            rows: t.rows.into_iter().filter(|r| !r.header).collect(),
            ..t
        };
        assert_eq!(t.cols[0].path, "meshes/x.nif", "the deeper stack first");
        let cells: Vec<(&str, &str)> = t
            .rows
            .iter()
            .map(|r| (r.name.as_str(), r.cells.as_str()))
            .collect();
        assert_eq!(cells, [("A", "ll"), ("B", "l-"), ("C", "ww")]);
        assert_eq!((t.rows[2].wins, t.rows[0].loses), (2, 2));
        assert_eq!(t.rows[0].separator.as_deref(), Some("S"));
        // a column window
        let t = window(&mut a, ColumnOrder::Strength, (1, 5));
        assert_eq!(t.cols.len(), 1);
        assert_eq!(
            t.rows.iter().map(|r| r.cells.as_str()).collect::<Vec<_>>(),
            ["", "l", "-", "w"]
        );
        // after a move the cached columns are rebuilt: C below A → A wins y and x
        let (ka, kc) = {
            let b = a.session().book();
            (b.key_of("A").unwrap(), b.key_of("C").unwrap())
        };
        a.command(crate::Command::Move {
            keys: vec![kc],
            before: Some(ka),
            after: None,
        })
        .unwrap();
        // order now C, A, B (B highest): x → B, A, C; y → A, C
        let t = window(&mut a, ColumnOrder::Chain, (0, 10));
        let cell = |m: &str, path: &str| {
            let c = t.cols.iter().position(|c| c.path == path).unwrap();
            t.rows.iter().find(|r| r.name == m).unwrap().cells.as_bytes()[c] as char
        };
        assert_eq!(
            [
                cell("A", "meshes/x.nif"),
                cell("A", "meshes/y.nif"),
                cell("B", "meshes/x.nif"),
                cell("B", "meshes/y.nif")
            ],
            ['l', 'w', 'w', '-']
        );
        assert_eq!(t.chain_starts.len(), t.chains.len());
    }

    #[test]
    fn settling_a_normalized_chain_settles_its_exact_chains() {
        let mut a = app(&[
            ("A", &["x.nif", "y.nif"]),
            ("B", &["x.nif"]),
            ("C", &["x.nif", "y.nif"]),
        ]);
        let key = a.session().book().key_of("A").unwrap();
        let detail = |a: &mut App| {
            let Reply::ModDetail { detail } = a.query(Query::ModDetail { key }).unwrap() else {
                panic!()
            };
            detail
        };
        let d = detail(&mut a);
        assert_eq!(d.chains.len(), 1, "C → B → A and C → A are one normalized chain");
        assert_eq!(d.chains[0].stack, ["C", "B", "A"]);
        assert_eq!(d.chains[0].files, 2);
        a.command(crate::Command::SetChainDone {
            signature: d.chains[0].signature.clone(),
            done: true,
        })
        .unwrap();
        assert_eq!(a.session().decisions().len(), 2, "both exact chains settled");
        assert!(detail(&mut a).chains[0].done);
        let signature = detail(&mut a).chains[0].signature.clone();
        a.command(crate::Command::SetChainDone {
            signature,
            done: false,
        })
        .unwrap();
        assert!(a.session().decisions().is_empty());
    }
}
