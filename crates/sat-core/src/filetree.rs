//! File tree projections: a set of paths with conflict marks laid out as rows in three modes. @sat:filetree
//!
//! Owns: building the rows of a file explorer from leaves (path + mark) — folders summing what is under them,
//! the monolith's three modes, which folders are open, and the row limit.
//! Not here: where the leaves come from (a mod, a selection, a chain — `@sat:app.tree`), names and colours.
//!
//! Modes (monolith `renderVisualTreePanel` v486, `renderVisualTreeLines`, `renderConflictAwareGroupedLinesV486`):
//! - **grouped** (`simple`): folders; in a folder its conflict files by name, the rest as `N× .ext`; leaf folders
//!   without conflicts that hold the same kinds of files merge into one row `N папок: a, b…`;
//! - **classic** (`full`): every folder and file;
//! - **conflict**: only conflict files and the folders above them.
//!
//! A folder row sums its subtree: files, wins, losses, owned while disabled, focused (in the focused chain), and
//! carries the winner and the chain when every conflict file under it shares them. Open folders: the caller's
//! choice wins; otherwise a folder opens on its own when it is small (≤ [`AUTO_OPEN`] files) or the only folder
//! at its level, and in conflict mode always. Rows stop at a limit; `hidden` says how many did not fit.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// How a tree shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Grouped,
    Classic,
    Conflict,
}

/// A leaf's conflict state relative to the subject (the mod or mods the tree is of).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// No one else has it.
    #[default]
    Plain,
    /// The subject wins it.
    Win,
    /// The subject loses it.
    Lose,
    /// The subject has it but is disabled.
    Owned,
    /// Contested, the subject is not a side (a chain's tree).
    Contested,
}

impl State {
    fn conflict(self) -> bool {
        self != State::Plain
    }
}

/// No winner / no chain.
pub const NONE: u32 = u32::MAX;

/// One path.
#[derive(Debug, Clone)]
pub struct Leaf {
    pub path: String,
    pub state: State,
    /// The winning mod (caller's id) or [`NONE`].
    pub winner: u32,
    /// The chain (caller's id) or [`NONE`].
    pub chain: u32,
    /// In the focused chain.
    pub focus: bool,
}

/// What a row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Dir,
    File,
    /// Leaf folders merged (grouped mode).
    Merged,
    /// Files of one extension in a folder (grouped mode).
    Agg,
}

/// One row of the explorer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub depth: u16,
    pub kind: Kind,
    pub name: String,
    /// Full path (folders and files); merged rows — the first folder's.
    pub path: String,
    pub files: u32,
    pub wins: u32,
    pub loses: u32,
    pub owned: u32,
    /// Conflict files of any state.
    pub conflicts: u32,
    pub focused: u32,
    /// Shared by every conflict file under it, else [`NONE`].
    pub winner: u32,
    pub chain: u32,
    /// A file's state.
    pub state: State,
    /// Folders: open now.
    pub open: bool,
    /// Merged and aggregate rows: extensions with counts, most first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exts: Vec<(String, u32)>,
}

/// The explorer's answer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tree {
    pub rows: Vec<Row>,
    /// Rows that did not fit.
    pub hidden: u32,
    /// Leaves in the tree (after the mode's filter).
    pub files: u32,
    pub conflicts: u32,
}

/// Folders up to this many files open on their own.
pub const AUTO_OPEN: u32 = 24;

#[derive(Default)]
struct Node {
    name: String,
    path: String,
    dirs: BTreeMap<String, usize>,
    files: Vec<usize>,
    sum: Sum,
}

#[derive(Default, Clone, Copy)]
struct Sum {
    files: u32,
    wins: u32,
    loses: u32,
    owned: u32,
    conflicts: u32,
    focused: u32,
    /// NONE = none seen yet, NONE-1 = mixed
    winner: u32,
    chain: u32,
}

const MIXED: u32 = u32::MAX - 1;

impl Sum {
    fn new() -> Self {
        Self {
            winner: NONE,
            chain: NONE,
            ..Default::default()
        }
    }
    fn add_leaf(&mut self, l: &Leaf) {
        self.files += 1;
        match l.state {
            State::Win => self.wins += 1,
            State::Lose => self.loses += 1,
            State::Owned => self.owned += 1,
            _ => {}
        }
        if l.state.conflict() {
            self.conflicts += 1;
            self.winner = merge(self.winner, l.winner);
            self.chain = merge(self.chain, l.chain);
        }
        if l.focus {
            self.focused += 1;
        }
    }
    fn shown(v: u32) -> u32 {
        if v == MIXED { NONE } else { v }
    }
}

fn merge(a: u32, b: u32) -> u32 {
    if a == NONE {
        b
    } else if a == b {
        a
    } else {
        MIXED
    }
}

fn ext_of(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((_, e)) if !e.is_empty() => format!(".{}", e.to_lowercase()),
        _ => "(без расширения)".into(),
    }
}

/// Which folders are open: the caller's word first (`Some`), else the default.
pub type Opener<'a> = dyn Fn(&str) -> Option<bool> + 'a;

/// Builds the rows of `leaves` in `mode`; `limit` caps the rows.
pub fn build(leaves: &[Leaf], mode: Mode, opener: &Opener<'_>, limit: usize) -> Tree {
    let mut nodes: Vec<Node> = vec![Node::default()];
    nodes[0].sum = Sum::new();
    let mut files = 0u32;
    let mut conflicts = 0u32;
    for (i, l) in leaves.iter().enumerate() {
        if mode == Mode::Conflict && !l.state.conflict() {
            continue;
        }
        files += 1;
        if l.state.conflict() {
            conflicts += 1;
        }
        let parts: Vec<&str> = l.path.split('/').filter(|s| !s.is_empty()).collect();
        let Some((_, dirs)) = parts.split_last() else {
            continue;
        };
        let mut at = 0usize;
        nodes[0].sum.add_leaf(l);
        for (d, part) in dirs.iter().enumerate() {
            let key = part.to_lowercase();
            let next = match nodes[at].dirs.get(&key) {
                Some(&n) => n,
                None => {
                    let n = nodes.len();
                    nodes.push(Node {
                        name: part.to_string(),
                        path: parts[..=d].join("/"),
                        sum: Sum::new(),
                        ..Default::default()
                    });
                    nodes[at].dirs.insert(key, n);
                    n
                }
            };
            at = next;
            nodes[at].sum.add_leaf(l);
        }
        nodes[at].files.push(i);
    }
    let mut out = Tree {
        files,
        conflicts,
        ..Default::default()
    };
    let mut ctx = Ctx {
        leaves,
        nodes: &nodes,
        mode,
        opener,
        limit,
        out: &mut out,
    };
    ctx.children(0, 0);
    out
}

struct Ctx<'a, 'b> {
    leaves: &'a [Leaf],
    nodes: &'a [Node],
    mode: Mode,
    opener: &'a Opener<'b>,
    limit: usize,
    out: &'a mut Tree,
}

impl Ctx<'_, '_> {
    fn push(&mut self, r: Row) -> bool {
        if self.out.rows.len() >= self.limit {
            self.out.hidden += 1;
            return false;
        }
        self.out.rows.push(r);
        true
    }

    fn dir_row(&self, n: &Node, depth: u16, kind: Kind, open: bool) -> Row {
        Row {
            depth,
            kind,
            name: n.name.clone(),
            path: n.path.clone(),
            files: n.sum.files,
            wins: n.sum.wins,
            loses: n.sum.loses,
            owned: n.sum.owned,
            conflicts: n.sum.conflicts,
            focused: n.sum.focused,
            winner: Sum::shown(n.sum.winner),
            chain: Sum::shown(n.sum.chain),
            state: State::Plain,
            open,
            exts: Vec::new(),
        }
    }

    fn file_row(&self, l: &Leaf, depth: u16) -> Row {
        let name = l.path.rsplit('/').next().unwrap_or(&l.path).to_string();
        let mut s = Sum::new();
        s.add_leaf(l);
        Row {
            depth,
            kind: Kind::File,
            name,
            path: l.path.clone(),
            files: 1,
            wins: s.wins,
            loses: s.loses,
            owned: s.owned,
            conflicts: s.conflicts,
            focused: s.focused,
            winner: if l.state.conflict() { l.winner } else { NONE },
            chain: if l.state.conflict() { l.chain } else { NONE },
            state: l.state,
            open: false,
            exts: Vec::new(),
        }
    }

    /// Extensions of a folder's own files with counts, most first.
    fn exts(&self, files: &[usize], only_plain: bool) -> Vec<(String, u32)> {
        let mut by: BTreeMap<String, u32> = BTreeMap::new();
        for &i in files {
            let l = &self.leaves[i];
            if only_plain && l.state.conflict() {
                continue;
            }
            *by.entry(ext_of(&l.path)).or_default() += 1;
        }
        let mut v: Vec<(String, u32)> = by.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }

    fn is_open(&self, n: &Node, siblings: usize) -> bool {
        match (self.opener)(&n.path) {
            Some(o) => o,
            None => self.mode == Mode::Conflict || n.sum.files <= AUTO_OPEN || siblings == 1,
        }
    }

    fn children(&mut self, at: usize, depth: u16) {
        let node = &self.nodes[at];
        let dirs: Vec<usize> = node.dirs.values().copied().collect();
        // grouped: leaf folders without conflicts that hold the same kinds of files merge
        let mut merged: Vec<Vec<usize>> = Vec::new();
        let mut single: Vec<usize> = Vec::new();
        if self.mode == Mode::Grouped {
            let mut by: BTreeMap<String, Vec<usize>> = BTreeMap::new();
            for &d in &dirs {
                let n = &self.nodes[d];
                if n.dirs.is_empty() && n.sum.conflicts == 0 && n.sum.focused == 0 {
                    let sig: Vec<String> = self.exts(&n.files, false).into_iter().map(|(e, _)| e).collect();
                    by.entry(sig.join("|")).or_default().push(d);
                } else {
                    single.push(d);
                }
            }
            for (_, g) in by {
                if g.len() >= 2 {
                    merged.push(g);
                } else {
                    single.extend(g);
                }
            }
            single.sort_by_key(|&d| self.nodes[d].name.to_lowercase());
        } else {
            single = dirs.clone();
        }
        let siblings = single.len() + merged.len();
        for d in single {
            let open = self.is_open(&self.nodes[d], siblings);
            let row = self.dir_row(&self.nodes[d], depth, Kind::Dir, open);
            if !self.push(row) {
                return;
            }
            if open {
                self.children(d, depth + 1);
            }
        }
        for g in merged {
            let first = &self.nodes[g[0]];
            let mut row = self.dir_row(first, depth, Kind::Merged, false);
            let names: Vec<&str> = g.iter().map(|&d| self.nodes[d].name.as_str()).collect();
            row.name = format!("{} {}: {}", g.len(), folders(g.len()), names.join(", "));
            let all: Vec<usize> = g
                .iter()
                .flat_map(|&d| self.nodes[d].files.iter().copied())
                .collect();
            row.files = all.len() as u32;
            row.exts = self.exts(&all, false);
            if !self.push(row) {
                return;
            }
        }
        // files: by name (all in classic and conflict mode, conflict ones in grouped), the rest as aggregates
        let mut named: Vec<usize> = node
            .files
            .iter()
            .copied()
            .filter(|&i| {
                self.mode != Mode::Grouped || self.leaves[i].state.conflict() || self.leaves[i].focus
            })
            .collect();
        named.sort_by(|&a, &b| natural(&self.leaves[a].path, &self.leaves[b].path));
        for i in named {
            let row = self.file_row(&self.leaves[i], depth);
            if !self.push(row) {
                return;
            }
        }
        if self.mode == Mode::Grouped {
            for (e, n) in self.exts(&node.files, true) {
                let row = Row {
                    depth,
                    kind: Kind::Agg,
                    name: format!("{n}× {e}"),
                    path: node.path.clone(),
                    files: n,
                    wins: 0,
                    loses: 0,
                    owned: 0,
                    conflicts: 0,
                    focused: 0,
                    winner: NONE,
                    chain: NONE,
                    state: State::Plain,
                    open: false,
                    exts: vec![(e, n)],
                };
                if !self.push(row) {
                    return;
                }
            }
        }
    }
}

/// «папка» in agreement with a number.
fn folders(n: usize) -> &'static str {
    match (n % 10, n % 100) {
        (1, x) if x != 11 => "папка",
        (2..=4, x) if !(12..=14).contains(&x) => "папки",
        _ => "папок",
    }
}

/// Natural order of paths: digits as numbers, case-insensitive (`a2` before `a10`).
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let mut n = String::new();
                while let Some(&c) = x.peek().filter(|c| c.is_ascii_digit()) {
                    n.push(c);
                    x.next();
                }
                let mut m = String::new();
                while let Some(&d) = y.peek().filter(|c| c.is_ascii_digit()) {
                    m.push(d);
                    y.next();
                }
                let o = n
                    .trim_start_matches('0')
                    .len()
                    .cmp(&m.trim_start_matches('0').len())
                    .then(n.cmp(&m));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_ascii_lowercase().cmp(&d.to_ascii_lowercase());
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(p: &str, state: State, winner: u32) -> Leaf {
        Leaf {
            path: p.into(),
            state,
            winner,
            chain: if state == State::Plain { NONE } else { 1 },
            focus: false,
        }
    }

    fn names(t: &Tree) -> Vec<String> {
        t.rows
            .iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth as usize), r.name))
            .collect()
    }

    fn sample() -> Vec<Leaf> {
        vec![
            leaf("meshes/a/x.nif", State::Lose, 7),
            leaf("meshes/a/y.nif", State::Plain, NONE),
            leaf("meshes/a/z.dds", State::Plain, NONE),
            leaf("meshes/b/1.nif", State::Plain, NONE),
            leaf("meshes/c/2.nif", State::Plain, NONE),
            leaf("meshes/c/10.nif", State::Plain, NONE),
            leaf("textures/t.dds", State::Win, 3),
        ]
    }

    #[test]
    fn grouped_names_conflict_files_aggregates_the_rest_and_merges_alike_folders() {
        let t = build(&sample(), Mode::Grouped, &|_| None, 100);
        assert_eq!(
            names(&t),
            [
                "meshes",
                "  a",
                "    x.nif",
                "    1× .dds",
                "    1× .nif",
                "  2 папки: b, c",
                "textures",
                "  t.dds",
            ]
        );
        let m = &t.rows[0];
        assert_eq!((m.files, m.loses, m.wins, m.conflicts, m.winner), (6, 1, 0, 1, 7));
        assert_eq!(t.rows[5].exts, [(".nif".to_string(), 3)]);
        assert_eq!((t.files, t.conflicts), (7, 2));
    }

    #[test]
    fn classic_shows_every_file_in_natural_order_and_conflict_only_the_contested() {
        let t = build(&sample(), Mode::Classic, &|_| None, 100);
        assert!(names(&t).contains(&"    2.nif".to_string()));
        let c = names(&t);
        let (i2, i10) = (
            c.iter().position(|x| x.ends_with("2.nif")).unwrap(),
            c.iter().position(|x| x.ends_with("10.nif")).unwrap(),
        );
        assert!(i2 < i10, "2 before 10");
        let t = build(&sample(), Mode::Conflict, &|_| None, 100);
        assert_eq!(names(&t), ["meshes", "  a", "    x.nif", "textures", "  t.dds"]);
        assert_eq!(t.rows[0].winner, 7);
    }

    #[test]
    fn folders_open_by_the_callers_word_and_rows_stop_at_the_limit() {
        let t = build(&sample(), Mode::Classic, &|p| Some(p != "meshes"), 100);
        assert_eq!(names(&t), ["meshes", "textures", "  t.dds"]);
        assert!(!t.rows[0].open);
        let t = build(&sample(), Mode::Classic, &|_| None, 3);
        assert_eq!(t.rows.len(), 3);
        assert!(t.hidden > 0);
    }

    #[test]
    fn mixed_winners_under_a_folder_show_none() {
        let mut l = sample();
        l.push(leaf("meshes/a/w.nif", State::Lose, 9));
        let t = build(&l, Mode::Conflict, &|_| None, 100);
        assert_eq!(t.rows[0].winner, NONE);
    }
}
