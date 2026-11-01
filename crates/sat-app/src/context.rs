//! The conflict context table (monolith "Модалка конфликтов" with the colour-types flag). @sat:app.context-table
//!
//! Owns: the list of contexts for the selector, and the table window of one context — rows are the context's mods
//! (priority order, grouped by separator), columns are its conflict paths (by chain or by strength), a cell says
//! whether the row's mod wins that path, loses it, or does not take part.
//! Not here: chains, contexts and stacks (`@sat:overwrite.projection`), colours of file types (the page).
//!
//! Both axes come in windows: a context of 300 mods × 40 000 paths is never sent whole. A chain is one exact stack,
//! so all its paths share one column pattern; a cell is looked up through the path's chain, not stored. The sorted
//! column list of the open context is cached per app revision.

use serde::{Deserialize, Serialize};

use sat_core::ids::{ChainId, ContextId, ModId, PathId};
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
    /// One char per column of the window: `w` wins, `l` loses, `-` not in the path's stack.
    pub cells: String,
}

/// One column: a path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TableCol {
    pub path: String,
    /// Index into `chains` of the reply.
    pub chain: u32,
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
    /// Columns: (path, index into `heads`, the path's exact chain — its stack gives the cells).
    cols: Vec<(PathId, u32, ChainId)>,
    chain_starts: Vec<u32>,
    /// Rows: separator headings and context mods with wins / loses.
    rows: Vec<RowItem>,
}

#[derive(Debug, Clone, Copy)]
enum RowItem {
    Header(Option<Key>),
    Mod(ModId, u32, u32),
}

impl App {
    /// Contexts, largest first.
    pub(crate) fn contexts(&mut self) -> Vec<ContextItem> {
        let ow = self.session.overwrite();
        let table = self.session.table().clone();
        let book = self.session.book();
        let mut out: Vec<ContextItem> = ow
            .contexts()
            .iter()
            .map(|cx| {
                let id = sat_core::identity::identity(cx.mods.iter().map(|&m| table.mod_name(m)));
                let first = cx.mods.first().map(|&m| table.mod_name(m)).unwrap_or("");
                let last = cx.mods.last().map(|&m| table.mod_name(m)).unwrap_or("");
                ContextItem {
                    key: cx.mods.first().and_then(|&m| book.key_of(table.mod_name(m))),
                    name: format!("{first} → {last}"),
                    hue: id.hue,
                    mods: cx.mods.len(),
                    chains: {
                        let mut gs: Vec<u32> = cx
                            .chains
                            .iter()
                            .filter_map(|&c| ow.routes().of_chain(c))
                            .collect();
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

    // @sat:app.context-table.window
    /// The window `[row_start, +row_count) × [col_start, +col_count)` of the context of mod `key`.
    pub(crate) fn context_table(
        &mut self,
        key: Key,
        order: ColumnOrder,
        rows: (usize, usize),
        cols: (usize, usize),
    ) -> Option<ContextTable> {
        let ow = self.session.overwrite();
        let table = self.session.table().clone();
        let m = {
            let book = self.session.book();
            let name = book.name(key).to_lowercase();
            table
                .mod_names()
                .iter()
                .position(|n| n.to_lowercase() == name)
                .map(ModId::from_idx)?
        };
        let cx = ow.mod_context(m)?;
        let fresh = self
            .open_context
            .as_ref()
            .is_some_and(|o| o.revision == self.revision && o.context == cx && o.order == order);
        if !fresh {
            let heads = self.heads();
            let decisions = self.session.decisions();
            let book = self.session.book();
            let sep_of = |m: ModId| book.key_of(table.mod_name(m)).and_then(|k| book.separator_of(k));
            self.open_context = Some(open(
                &ow,
                &table,
                &heads,
                decisions,
                &sep_of,
                cx,
                order,
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
                let (rm, wins, loses) = match item {
                    RowItem::Mod(m, w, l) => (m, w, l),
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
                            cells: String::new(),
                        };
                    }
                };
                let name = table.mod_name(rm).to_string();
                let key = book.key_of(&name);
                let cells = window
                    .iter()
                    .map(
                        |&(_, _, c)| match ow.chain_mods(c).iter().position(|&x| x == rm) {
                            Some(0) => 'w',
                            Some(_) => 'l',
                            None => '-',
                        },
                    )
                    .collect();
                TableRow {
                    header: false,
                    pos: key.map(|k| book.pos(k) as u32),
                    separator: key.and_then(|k| book.separator_of(k)).map(sep_name),
                    enabled: key.is_some_and(|k| book.enabled(k)),
                    key,
                    name,
                    wins,
                    loses,
                    cells,
                }
            })
            .collect();
        let id = sat_core::identity::identity(context.mods.iter().map(|&x| table.mod_name(x)));
        let first = context.mods.first().map(|&x| table.mod_name(x)).unwrap_or("");
        let last = context.mods.last().map(|&x| table.mod_name(x)).unwrap_or("");
        Some(ContextTable {
            name: format!("{first} → {last}"),
            hue: id.hue,
            rows_total: oc.rows.len(),
            cols_total: oc.cols.len(),
            chains: {
                let mut ids: Vec<u32> = window.iter().map(|&(_, ci, _)| ci).collect();
                ids.sort_unstable();
                ids.dedup();
                ids.into_iter().map(|i| oc.heads[i as usize].clone()).collect()
            },
            chains_total: oc.heads.len(),
            chain_starts: oc.chain_starts.clone(),
            rows: row_views,
            cols: window
                .iter()
                .map(|&(p, ci, _)| TableCol {
                    path: table.path_string(p),
                    chain: ci,
                })
                .collect(),
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn open(
    ow: &sat_core::overwrite::OverwriteProjection,
    table: &sat_core::table::ProviderTable,
    heads: &[crate::chains::Head],
    decisions: &sat_core::decisions::ChainDecisions,
    sep_of: &dyn Fn(ModId) -> Option<Key>,
    cx: ContextId,
    order: ColumnOrder,
    revision: u64,
) -> OpenContext {
    let context = &ow.contexts()[cx.idx()];
    let routes = ow.routes();
    // normalized chains of the context, larger first
    let mut groups: Vec<u32> = context
        .chains
        .iter()
        .filter_map(|&c| routes.of_chain(c))
        .collect();
    groups.sort_unstable();
    groups.dedup();
    groups.sort_by_key(|&g| (std::cmp::Reverse(heads[g as usize].files), g));
    let mut cols: Vec<(PathId, u32, ChainId)> = Vec::new();
    let mut chain_starts = Vec::with_capacity(groups.len());
    for (i, &g) in groups.iter().enumerate() {
        chain_starts.push(cols.len() as u32);
        let mut ps: Vec<(String, PathId, ChainId)> = heads[g as usize]
            .chains
            .iter()
            .flat_map(|&c| ow.chain_paths(c).into_iter().map(move |p| (p, c)))
            .map(|(p, c)| (table.path_string(p), p, c))
            .collect();
        ps.sort_unstable();
        cols.extend(ps.into_iter().map(|(_, p, c)| (p, i as u32, c)));
    }
    if order == ColumnOrder::Strength {
        // deeper stacks first; stable, so within a depth the chain order stays
        cols.sort_by_key(|&(_, _, c)| std::cmp::Reverse(ow.chain_mods(c).len()));
        chain_starts.clear();
    }
    // wins / loses per member over the whole context (exact stacks)
    let mut tally: std::collections::HashMap<ModId, (u32, u32)> = Default::default();
    for &c in &context.chains {
        let files = ow.chain(c).files;
        for (i, &m) in ow.chain_mods(c).iter().enumerate() {
            let e = tally.entry(m).or_insert((0, 0));
            if i == 0 {
                e.0 += files;
            } else {
                e.1 += files;
            }
        }
    }
    // context mods are in priority order = list order, so a separator's mods are contiguous
    let mut rows = Vec::with_capacity(context.mods.len() + 8);
    let mut last: Option<Option<Key>> = None;
    for &m in &context.mods {
        let sep = sep_of(m);
        if last != Some(sep) {
            rows.push(RowItem::Header(sep));
            last = Some(sep);
        }
        let (w, l) = tally.get(&m).copied().unwrap_or((0, 0));
        rows.push(RowItem::Mod(m, w, l));
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
        a.command(crate::Command::SetChainDone {
            signature: detail(&mut a).chains[0].signature.clone(),
            done: false,
        })
        .unwrap();
        assert!(a.session().decisions().is_empty());
    }
}
