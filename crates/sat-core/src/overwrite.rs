//! Hot tier: the overwrite projection of a [`ProviderTable`] under given priorities. @sat:overwrite
//!
//! For every virtual path the active providers are ordered into a **stack**:
//! loose files beat archive entries (game rule for Bethesda titles), then higher priority wins.
//! A mod appears at most once per stack (its loose copy shadows its own archive copy).
//!
//! - **Exact chain** — the set of conflicting paths sharing one identical stack of mods.
//! - **Context** — a connected component of mods linked through chains.
//! - **W/L/M/X/+n** per mod — wins, losses, middle-of-stack, fully overridden, unique paths.
//!
//! The projection is a pure function of `(table, priorities)`, split in two (lesson of the old Satellite: the
//! order changes winners, never the structure of conflicts):
//! - [`ConflictIndex`] — facts only: contested paths (two or more distinct mods) and mod → contested paths;
//! - [`OverwriteProjection`] — stacks and winners of contested paths under priorities; single-mod paths are
//!   summed per mod once. [`OverwriteProjection::update_in_place`] recomputes only the paths the changed mods
//!   provide; chains are interned with reference counts, statistics change by deltas, contexts are lazy.
//!
//! Owns: the conflict index and the overwrite projection: stacks, chains, contexts, W/L/M/X statistics.

use std::sync::Arc;

use rayon::prelude::*;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::ids::{ChainId, ContextId, ModId, PathId};
use crate::table::ProviderTable;

/// Priority per mod (index = [`ModId`]); higher wins, `< 0` = inactive (disabled / not in the list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Priorities(pub Vec<i32>);

impl Priorities {
    pub const INACTIVE: i32 = -1;

    pub fn all_inactive(mods: usize) -> Self {
        Self(vec![Self::INACTIVE; mods])
    }
    #[inline]
    pub fn get(&self, m: ModId) -> i32 {
        self.0[m.idx()]
    }
    #[inline]
    pub fn is_active(&self, m: ModId) -> bool {
        self.0[m.idx()] >= 0
    }
}

// @sat:overwrite.units
/// Mods computed as one (logical groups, ADR 0008 / 0016): every member of a group maps to the group's
/// representative — the member with the lowest table id, which a move inside the group does not change (the outside
/// must not notice the inside) — so a path the members share is no conflict between them, and against other mods
/// the group stands as one resulting mod. Empty = every mod is its own unit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Units(Vec<u32>);

impl Units {
    /// Groups of mods; each group's representative is its lowest id.
    pub fn of_groups(mods: usize, groups: &[Vec<ModId>]) -> Self {
        if groups.iter().all(|g| g.len() < 2) {
            return Self::default();
        }
        let mut map: Vec<u32> = (0..mods as u32).collect();
        for g in groups {
            if let Some(&rep) = g.iter().min() {
                for &m in g {
                    map[m.idx()] = rep.0;
                }
            }
        }
        Self(map)
    }

    #[inline]
    pub fn rep(&self, m: ModId) -> ModId {
        self.0.get(m.idx()).map_or(m, |&r| ModId(r))
    }

    /// A member that is not its group's representative: it has no stack positions of its own.
    #[inline]
    pub fn absorbed(&self, m: ModId) -> bool {
        self.rep(m) != m
    }

    /// Mods whose unit differs between `self` and `other` (and their representatives).
    pub fn changed(&self, other: &Units, mods: usize) -> Vec<ModId> {
        let mut out = Vec::new();
        for i in 0..mods {
            let m = ModId::from_idx(i);
            let (a, b) = (self.rep(m), other.rep(m));
            if a != b {
                out.extend([m, a, b]);
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Per-mod overwrite summary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModStat {
    /// Providers of this mod (loose + archive entries).
    pub providers: u32,
    /// Paths only this active mod provides (+n).
    pub unique: u32,
    /// Conflict paths this mod wins (W).
    pub win: u32,
    /// Conflict paths where it is overridden (L).
    pub lose: u32,
    /// Overridden, but itself overrides someone below (M).
    pub mid: u32,
    /// Active, has providers, wins nothing and has nothing unique — invisible in game (X).
    pub fully_overridden: bool,
    /// Chains this mod is a member of.
    pub chains: u32,
    /// VRAM of textures where this mod is the winner, KiB.
    pub vram_win_kib: u64,
    /// Bytes of files where this mod is the winner.
    pub bytes_win: u64,
}

/// One exact chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chain {
    pub files: u32,
    /// Winner bytes across the chain.
    pub bytes: u64,
}

/// One context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    /// Members ordered by priority, lowest first (MO2 display order).
    pub mods: Vec<ModId>,
    pub chains: Vec<ChainId>,
    pub files: u32,
}

/// Totals for logs and budgets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverwriteStats {
    pub active_paths: usize,
    pub conflict_paths: usize,
    pub chains: usize,
    /// Normalized chains (route groups of exact chains, `@sat:routes`).
    #[serde(default)]
    pub normalized: usize,
    pub contexts: usize,
    pub fully_overridden: usize,
    pub vram_win_kib: u64,
}

const NONE: u32 = u32::MAX;
/// High bit of [`ConflictIndex::path_kind`]: the path is contested, the rest is its slot.
const CONTESTED: u32 = 1 << 31;

/// Paths owned by exactly one mod, summed per mod (they cannot conflict, whatever the order).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Solo {
    paths: u32,
    bytes: u64,
    vram_kib: u64,
}

// @sat:overwrite.conflicts
/// Facts-only structure of conflicts: which paths are provided by two or more distinct mods (contested), and
/// which mods share each of them. Independent of order and enabled flags, so it is rebuilt only when the facts
/// change; priorities then decide stacks and winners for contested paths only (a few percent of all paths).
#[derive(Debug)]
pub struct ConflictIndex {
    /// Per path: `CONTESTED | slot`, or the owner mod of a single-mod path.
    path_kind: Vec<u32>,
    /// Slot → path, in path order.
    contested: Vec<PathId>,
    /// Mod → contested slots it provides (CSR).
    mod_slot_offsets: Vec<u32>,
    mod_slots: Vec<u32>,
    solo: Vec<Solo>,
    ignored: usize,
}

/// Picks the provider of `m` that represents it on a single-mod path: loose before archive, then table order.
fn own_best(table: &ProviderTable, range: std::ops::Range<usize>) -> usize {
    range
        .clone()
        .find(|&i| !table.is_archive(i))
        .unwrap_or(range.start)
}

impl ConflictIndex {
    pub fn build(table: &ProviderTable) -> Self {
        Self::build_with(table, &crate::ignore::IgnoreRules::default())
    }

    /// Builds the index leaving out contested paths the user ignores (`@sat:ignore`): they are no conflict and
    /// no part of any chain; a path only one mod provides is never affected.
    pub fn build_with(table: &ProviderTable, ignore: &crate::ignore::IgnoreRules) -> Self {
        let mods = table.mod_count();
        let paths = table.path_count();
        let mut path_kind = vec![NONE; paths];
        let mut contested = Vec::new();
        let mut solo = vec![Solo::default(); mods];
        let mut mod_counts = vec![0u32; mods + 1];
        let mut seen: Vec<ModId> = Vec::new();
        let mut ignored = 0;
        for (pi, kind) in path_kind.iter_mut().enumerate() {
            let p = PathId::from_idx(pi);
            let range = table.path_providers(p);
            seen.clear();
            for i in range.clone() {
                let m = table.owner(i);
                if !seen.contains(&m) {
                    seen.push(m);
                }
            }
            if seen.len() == 1 {
                let m = seen[0];
                let i = own_best(table, range);
                let sl = &mut solo[m.idx()];
                sl.paths += 1;
                sl.bytes += table.size(i);
                sl.vram_kib += u64::from(table.vram_kib(i));
                *kind = m.0;
            } else if seen.len() > 1 && !ignore.is_empty() && ignore.matches(&table.path_string(p)) {
                ignored += 1;
            } else if seen.len() > 1 {
                *kind = CONTESTED | contested.len() as u32;
                contested.push(p);
                for m in &seen {
                    mod_counts[m.idx() + 1] += 1;
                }
            }
        }
        for i in 1..=mods {
            mod_counts[i] += mod_counts[i - 1];
        }
        let mut mod_slots = vec![0u32; mod_counts[mods] as usize];
        let mut fill = mod_counts[..mods].to_vec();
        for (slot, &p) in contested.iter().enumerate() {
            seen.clear();
            for i in table.path_providers(p) {
                let m = table.owner(i);
                if !seen.contains(&m) {
                    seen.push(m);
                    mod_slots[fill[m.idx()] as usize] = slot as u32;
                    fill[m.idx()] += 1;
                }
            }
        }
        Self {
            path_kind,
            contested,
            mod_slot_offsets: mod_counts,
            mod_slots,
            solo,
            ignored,
        }
    }

    /// Contested paths left out by the ignore rules.
    pub fn ignored_count(&self) -> usize {
        self.ignored
    }

    pub fn contested_count(&self) -> usize {
        self.contested.len()
    }

    fn slots_of(&self, m: ModId) -> &[u32] {
        &self.mod_slots[self.mod_slot_offsets[m.idx()] as usize..self.mod_slot_offsets[m.idx() + 1] as usize]
    }
}

/// Stack of one contested path under priorities: active distinct mods, loose before archive, then priority
/// descending, then table order (deterministic). Returns `(hash, winner bytes, winner VRAM)`.
fn stack_of(
    table: &ProviderTable,
    p: PathId,
    prio: &Priorities,
    units: &Units,
    cand: &mut Vec<(bool, i32, usize, ModId)>,
    out: &mut Vec<ModId>,
) -> (u64, u64, u32) {
    cand.clear();
    for i in table.path_providers(p) {
        let m = table.owner(i);
        let pr = prio.get(m);
        if pr >= 0 {
            cand.push((table.is_archive(i), -pr, i, m));
        }
    }
    if cand.is_empty() {
        return (0, 0, 0);
    }
    cand.sort_unstable();
    let wi = cand[0].2;
    let start = out.len();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &(_, _, _, m) in cand.iter() {
        let m = units.rep(m);
        if out[start..].contains(&m) {
            continue;
        }
        out.push(m);
        h ^= u64::from(m.0).wrapping_add(0x9e37_79b9_7f4a_7c15);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    (h, table.size(wi), table.vram_kib(wi))
}

/// A freshly computed stack for one slot.
struct Fresh {
    slot: u32,
    hash: u64,
    bytes: u64,
    vram: u32,
    start: u32,
    len: u32,
}

/// Computes stacks for `slots` in parallel; the stacks' mods go into the returned pool.
fn compute_stacks(
    table: &ProviderTable,
    index: &ConflictIndex,
    prio: &Priorities,
    units: &Units,
    slots: &[u32],
) -> (Vec<Fresh>, Vec<ModId>) {
    let parts: Vec<(Vec<Fresh>, Vec<ModId>)> = slots
        .par_chunks(PART)
        .map(|chunk| {
            let mut fresh = Vec::with_capacity(chunk.len());
            let mut pool = Vec::new();
            let mut cand = Vec::new();
            for &slot in chunk {
                let start = pool.len() as u32;
                let (hash, bytes, vram) = stack_of(
                    table,
                    index.contested[slot as usize],
                    prio,
                    units,
                    &mut cand,
                    &mut pool,
                );
                fresh.push(Fresh {
                    slot,
                    hash,
                    bytes,
                    vram,
                    start,
                    len: pool.len() as u32 - start,
                });
            }
            (fresh, pool)
        })
        .collect();
    let mut all = Vec::with_capacity(slots.len());
    let mut pool = Vec::new();
    for (fresh, p) in parts {
        let base = pool.len() as u32;
        all.extend(fresh.into_iter().map(|f| Fresh {
            start: f.start + base,
            ..f
        }));
        pool.extend(p);
    }
    (all, pool)
}

/// Interned chain: an exact stack shared by `files` contested paths.
#[derive(Debug, Clone)]
struct ChainRec {
    chain: Chain,
    hash: u64,
    start: u32,
    len: u32,
    /// Next chain with the same hash (or `NONE`).
    same_hash: u32,
}

/// Contexts are needed by the contexts window and the report, not by every move: computed on first read.
#[derive(Debug, Clone)]
struct Contexts {
    contexts: Vec<Context>,
    of_mod: Vec<u32>,
    of_chain: Vec<u32>,
}

// @sat:overwrite.projection
/// The overwrite projection: persistent state updated in place by [`OverwriteProjection::update_in_place`].
#[derive(Debug, Clone)]
pub struct OverwriteProjection {
    index: Arc<ConflictIndex>,
    priorities: Priorities,
    units: Units,
    // per contested slot: stack (start, len) into `pool`, winner bytes / VRAM, chain
    slot_start: Vec<u32>,
    slot_len: Vec<u32>,
    slot_bytes: Vec<u64>,
    slot_vram: Vec<u32>,
    slot_chain: Vec<u32>,
    pool: Vec<ModId>,
    garbage: usize,
    // chains (ids are stable while a chain lives; dead ids are reused)
    chains: Vec<ChainRec>,
    chain_pool: Vec<ModId>,
    by_hash: FxHashMap<u64, u32>,
    free: Vec<u32>,
    alive: usize,
    // mods
    stats: Vec<ModStat>,
    active_paths: usize,
    conflict_paths: usize,
    fully: usize,
    contexts: std::sync::OnceLock<Contexts>,
    routes: std::sync::OnceLock<crate::routes::RouteGroups>,
}

const PART: usize = 1 << 14;

impl OverwriteProjection {
    /// Full computation, building the conflict index on the way (one-shot callers).
    pub fn compute(table: &ProviderTable, priorities: Priorities) -> Self {
        Self::compute_with(Arc::new(ConflictIndex::build(table)), table, priorities)
    }

    /// Full computation over a prebuilt conflict index (`index` must be built from `table`).
    pub fn compute_with(index: Arc<ConflictIndex>, table: &ProviderTable, priorities: Priorities) -> Self {
        assert_eq!(
            priorities.0.len(),
            table.mod_count(),
            "priorities must cover every mod"
        );
        let mods = table.mod_count();
        let slots = index.contested.len();
        let mut me = Self {
            priorities: Priorities::all_inactive(mods),
            units: Units::default(),
            slot_start: vec![0; slots],
            slot_len: vec![0; slots],
            slot_bytes: vec![0; slots],
            slot_vram: vec![0; slots],
            slot_chain: vec![NONE; slots],
            pool: Vec::new(),
            garbage: 0,
            chains: Vec::new(),
            chain_pool: Vec::new(),
            by_hash: FxHashMap::with_capacity_and_hasher(slots, Default::default()),
            free: Vec::new(),
            alive: 0,
            stats: (0..mods)
                .map(|m| ModStat {
                    providers: table.mod_providers(ModId::from_idx(m)).len() as u32,
                    ..ModStat::default()
                })
                .collect(),
            active_paths: 0,
            conflict_paths: 0,
            fully: 0,
            contexts: std::sync::OnceLock::new(),
            routes: std::sync::OnceLock::new(),
            index,
        };
        let all: Vec<u32> = (0..slots as u32).collect();
        let touched: Vec<ModId> = (0..mods).map(ModId::from_idx).collect();
        me.apply(table, priorities, &touched, &all);
        me
    }

    // @sat:overwrite.update
    /// Incremental recomputation after the priorities of `touched` mods changed (moved, enabled, disabled,
    /// added, removed). Other mods must keep their relative order and active state: then only contested paths
    /// that one of `touched` provides can change their stack. Everything else is kept; chains and per-mod
    /// statistics change by deltas.
    pub fn update_in_place(&mut self, table: &ProviderTable, priorities: Priorities, touched: &[ModId]) {
        assert_eq!(
            priorities.0.len(),
            table.mod_count(),
            "priorities must cover every mod"
        );
        let mut affected: Vec<u32> = touched
            .iter()
            .flat_map(|&m| self.index.slots_of(m).iter().copied())
            .collect();
        affected.sort_unstable();
        affected.dedup();
        self.apply(table, priorities, touched, &affected);
    }

    /// Full computation with logical groups computed as one (`@sat:overwrite.units`).
    pub fn compute_units(
        index: Arc<ConflictIndex>,
        table: &ProviderTable,
        priorities: Priorities,
        units: Units,
    ) -> Self {
        let mut me = Self::compute_with(index, table, Priorities::all_inactive(table.mod_count()));
        me.units = units;
        let all: Vec<u32> = (0..me.index.contested.len() as u32).collect();
        let touched: Vec<ModId> = (0..table.mod_count()).map(ModId::from_idx).collect();
        me.apply(table, priorities, &touched, &all);
        me
    }

    /// [`OverwriteProjection::update_in_place`] with new units: the mods whose unit changed join `touched`.
    pub fn update_units_in_place(
        &mut self,
        table: &ProviderTable,
        priorities: Priorities,
        units: Units,
        touched: &[ModId],
    ) {
        let mut touched = touched.to_vec();
        if units != self.units {
            // members' own paths move to their new group's result
            for i in 0..table.mod_count() {
                let m = ModId::from_idx(i);
                let (old, new) = (self.units.rep(m), units.rep(m));
                if old != new && self.priorities.is_active(m) {
                    let sl = self.index.solo[m.idx()];
                    for (r, sign) in [(old, -1i64), (new, 1)] {
                        let st = &mut self.stats[r.idx()];
                        st.unique = (i64::from(st.unique) + sign * i64::from(sl.paths)) as u32;
                        st.bytes_win = (st.bytes_win as i64 + sign * sl.bytes as i64) as u64;
                        st.vram_win_kib = (st.vram_win_kib as i64 + sign * sl.vram_kib as i64) as u64;
                    }
                }
            }
            touched.extend(self.units.changed(&units, table.mod_count()));
            touched.sort_unstable();
            touched.dedup();
            self.units = units;
        }
        self.update_in_place(table, priorities, &touched);
    }

    pub fn units(&self) -> &Units {
        &self.units
    }

    // @sat:overwrite.inside
    /// The inside of a group: the overwrite among its `members` alone (each its own unit), as if nothing else were
    /// in the build — the group's own conflicts and chains, hidden from the outside.
    pub fn inside(&self, table: &ProviderTable, members: &[ModId]) -> Self {
        let mut prio = Priorities::all_inactive(table.mod_count());
        for &m in members {
            prio.0[m.idx()] = self.priorities.get(m);
        }
        Self::compute_with(self.index.clone(), table, prio)
    }

    /// [`OverwriteProjection::update_in_place`] on a copy.
    pub fn update(&self, table: &ProviderTable, priorities: Priorities, touched: &[ModId]) -> Self {
        let mut next = self.clone();
        next.update_in_place(table, priorities, touched);
        next
    }

    fn apply(&mut self, table: &ProviderTable, priorities: Priorities, touched: &[ModId], slots: &[u32]) {
        let (fresh, fresh_pool) = compute_stacks(table, &self.index, &priorities, &self.units, slots);
        let mut dirty: Vec<ModId> = touched.to_vec();
        // single-mod paths follow the active state of their owner
        for &m in touched {
            let (was, now) = (self.priorities.is_active(m), priorities.is_active(m));
            if was != now {
                let sl = self.index.solo[m.idx()];
                // a member's own paths are its group's result (`@sat:overwrite.units`)
                let st = &mut self.stats[self.units.rep(m).idx()];
                if now {
                    st.unique += sl.paths;
                    st.bytes_win += sl.bytes;
                    st.vram_win_kib += sl.vram_kib;
                    self.active_paths += sl.paths as usize;
                } else {
                    st.unique -= sl.paths;
                    st.bytes_win -= sl.bytes;
                    st.vram_win_kib -= sl.vram_kib;
                    self.active_paths -= sl.paths as usize;
                }
            }
        }
        for f in &fresh {
            let slot = f.slot as usize;
            let new = &fresh_pool[f.start as usize..(f.start + f.len) as usize];
            let old_range =
                self.slot_start[slot] as usize..(self.slot_start[slot] + self.slot_len[slot]) as usize;
            if self.pool[old_range.clone()] == *new {
                // same stack of units; inside a group the winning file can still change (a move inside the
                // group): only the winner's bytes follow — the change that leaves the group is its result
                let (ob, ov) = (self.slot_bytes[slot], self.slot_vram[slot]);
                if (ob, ov) != (f.bytes, f.vram) && !new.is_empty() {
                    let st = &mut self.stats[new[0].idx()];
                    st.bytes_win = st.bytes_win - ob + f.bytes;
                    st.vram_win_kib = st.vram_win_kib - u64::from(ov) + u64::from(f.vram);
                    let c = self.slot_chain[slot];
                    if c != NONE {
                        let rec = &mut self.chains[c as usize];
                        rec.chain.bytes = rec.chain.bytes - ob + f.bytes;
                    }
                    self.slot_bytes[slot] = f.bytes;
                    self.slot_vram[slot] = f.vram;
                }
                continue;
            }
            self.remove_slot(slot, &mut dirty);
            let start = self.pool.len() as u32;
            self.pool.extend_from_slice(new);
            self.garbage += old_range.len();
            self.slot_start[slot] = start;
            self.slot_len[slot] = f.len;
            self.slot_bytes[slot] = f.bytes;
            self.slot_vram[slot] = f.vram;
            self.add_slot(slot, f.hash, &mut dirty);
        }
        self.priorities = priorities;
        dirty.sort_unstable();
        dirty.dedup();
        for m in dirty {
            let active = self.priorities.is_active(m);
            let st = &mut self.stats[m.idx()];
            // a member of a group computed as one is judged with its group (its representative), not alone
            let x = active && !self.units.absorbed(m) && st.providers > 0 && st.win == 0 && st.unique == 0;
            if x != st.fully_overridden {
                st.fully_overridden = x;
                if x {
                    self.fully += 1;
                } else {
                    self.fully -= 1;
                }
            }
        }
        if self.garbage > self.pool.len() / 2 + 4096 {
            self.compact();
        }
        self.contexts = std::sync::OnceLock::new();
        self.routes = std::sync::OnceLock::new();
    }

    fn stack(&self, slot: usize) -> &[ModId] {
        &self.pool[self.slot_start[slot] as usize..(self.slot_start[slot] + self.slot_len[slot]) as usize]
    }

    fn remove_slot(&mut self, slot: usize, dirty: &mut Vec<ModId>) {
        let len = self.slot_len[slot] as usize;
        if len == 0 {
            return;
        }
        let start = self.slot_start[slot] as usize;
        let w = self.pool[start];
        self.active_paths -= 1;
        let st = &mut self.stats[w.idx()];
        st.bytes_win -= self.slot_bytes[slot];
        st.vram_win_kib -= u64::from(self.slot_vram[slot]);
        if len == 1 {
            st.unique -= 1;
            dirty.push(w);
            return;
        }
        for i in 0..len {
            let m = self.pool[start + i];
            dirty.push(m);
            let st = &mut self.stats[m.idx()];
            if i == 0 {
                st.win -= 1;
            } else {
                st.lose -= 1;
                if i < len - 1 {
                    st.mid -= 1;
                }
            }
        }
        self.conflict_paths -= 1;
        let c = std::mem::replace(&mut self.slot_chain[slot], NONE) as usize;
        let rec = &mut self.chains[c];
        rec.chain.files -= 1;
        rec.chain.bytes -= self.slot_bytes[slot];
        if rec.chain.files == 0 {
            self.kill_chain(c as u32);
        }
    }

    fn add_slot(&mut self, slot: usize, hash: u64, dirty: &mut Vec<ModId>) {
        let len = self.slot_len[slot] as usize;
        if len == 0 {
            return;
        }
        let start = self.slot_start[slot] as usize;
        let w = self.pool[start];
        self.active_paths += 1;
        let st = &mut self.stats[w.idx()];
        st.bytes_win += self.slot_bytes[slot];
        st.vram_win_kib += u64::from(self.slot_vram[slot]);
        if len == 1 {
            st.unique += 1;
            dirty.push(w);
            return;
        }
        for i in 0..len {
            let m = self.pool[start + i];
            dirty.push(m);
            let st = &mut self.stats[m.idx()];
            if i == 0 {
                st.win += 1;
            } else {
                st.lose += 1;
                if i < len - 1 {
                    st.mid += 1;
                }
            }
        }
        self.conflict_paths += 1;
        let c = self.intern(slot, hash);
        let rec = &mut self.chains[c as usize];
        rec.chain.files += 1;
        rec.chain.bytes += self.slot_bytes[slot];
        self.slot_chain[slot] = c;
    }

    /// Finds or creates the chain of the stack at `slot`.
    fn intern(&mut self, slot: usize, hash: u64) -> u32 {
        let start = self.slot_start[slot] as usize;
        let len = self.slot_len[slot] as usize;
        let mut c = self.by_hash.get(&hash).copied().unwrap_or(NONE);
        while c != NONE {
            let r = &self.chains[c as usize];
            if self.chain_pool[r.start as usize..(r.start + r.len) as usize] == self.pool[start..start + len]
            {
                return c;
            }
            c = r.same_hash;
        }
        let rec_start = self.chain_pool.len() as u32;
        self.chain_pool.extend_from_slice(&self.pool[start..start + len]);
        let head = self.by_hash.get(&hash).copied().unwrap_or(NONE);
        let rec = ChainRec {
            chain: Chain { files: 0, bytes: 0 },
            hash,
            start: rec_start,
            len: len as u32,
            same_hash: head,
        };
        let id = match self.free.pop() {
            Some(id) => {
                self.chains[id as usize] = rec;
                id
            }
            None => {
                self.chains.push(rec);
                self.chains.len() as u32 - 1
            }
        };
        self.by_hash.insert(hash, id);
        for i in 0..len {
            let m = self.pool[start + i];
            self.stats[m.idx()].chains += 1;
        }
        self.alive += 1;
        id
    }

    fn kill_chain(&mut self, c: u32) {
        let (hash, next, start, len) = {
            let r = &self.chains[c as usize];
            (r.hash, r.same_hash, r.start as usize, r.len as usize)
        };
        for i in 0..len {
            let m = self.chain_pool[start + i];
            self.stats[m.idx()].chains -= 1;
        }
        // unlink from the hash list
        let head = self.by_hash[&hash];
        if head == c {
            if next == NONE {
                self.by_hash.remove(&hash);
            } else {
                self.by_hash.insert(hash, next);
            }
        } else {
            let mut x = head;
            while self.chains[x as usize].same_hash != c {
                x = self.chains[x as usize].same_hash;
            }
            self.chains[x as usize].same_hash = next;
        }
        self.chains[c as usize].len = 0;
        self.free.push(c);
        self.alive -= 1;
    }

    /// Drops stack and chain mods no longer referenced.
    fn compact(&mut self) {
        let mut pool = Vec::with_capacity(self.pool.len() - self.garbage);
        for slot in 0..self.slot_start.len() {
            let start = pool.len() as u32;
            pool.extend_from_slice(self.stack(slot));
            self.slot_start[slot] = start;
        }
        self.pool = pool;
        self.garbage = 0;
        let mut cp = Vec::new();
        for r in &mut self.chains {
            let start = cp.len() as u32;
            cp.extend_from_slice(&self.chain_pool[r.start as usize..(r.start + r.len) as usize]);
            r.start = start;
        }
        self.chain_pool = cp;
    }

    fn live(&self, c: usize) -> bool {
        self.chains[c].chain.files > 0
    }

    fn contexts_data(&self) -> &Contexts {
        self.contexts.get_or_init(|| {
            let mods = self.stats.len();
            let mut parent: Vec<u32> = (0..mods as u32).collect();
            fn find(parent: &mut [u32], mut x: u32) -> u32 {
                while parent[x as usize] != x {
                    parent[x as usize] = parent[parent[x as usize] as usize];
                    x = parent[x as usize];
                }
                x
            }
            for c in 0..self.chains.len() {
                if !self.live(c) {
                    continue;
                }
                let ms = self.chain_mods(ChainId::from_idx(c));
                for &m in &ms[1..] {
                    let (a, b) = (find(&mut parent, ms[0].0), find(&mut parent, m.0));
                    if a != b {
                        parent[b as usize] = a;
                    }
                }
            }
            let mut contexts: Vec<Context> = Vec::new();
            let mut of_mod = vec![NONE; mods];
            let mut ctx_of_root: FxHashMap<u32, u32> = FxHashMap::default();
            for (m, st) in self.stats.iter().enumerate() {
                if st.chains == 0 {
                    continue;
                }
                let root = find(&mut parent, m as u32);
                let next = contexts.len() as u32;
                let id = *ctx_of_root.entry(root).or_insert(next);
                if id == next {
                    contexts.push(Context {
                        mods: Vec::new(),
                        chains: Vec::new(),
                        files: 0,
                    });
                }
                of_mod[m] = id;
                contexts[id as usize].mods.push(ModId::from_idx(m));
            }
            // chains in order of their first path, as a fresh computation would number them
            let mut first_slot = vec![NONE; self.chains.len()];
            for (slot, &c) in self.slot_chain.iter().enumerate() {
                if c != NONE && first_slot[c as usize] == NONE {
                    first_slot[c as usize] = slot as u32;
                }
            }
            let mut live: Vec<u32> = (0..self.chains.len() as u32)
                .filter(|&c| self.live(c as usize))
                .collect();
            live.sort_unstable_by_key(|&c| first_slot[c as usize]);
            let mut of_chain = vec![NONE; self.chains.len()];
            for c in live {
                let first = self.chain_mods(ChainId(c))[0];
                let ctx = of_mod[first.idx()];
                of_chain[c as usize] = ctx;
                let cx = &mut contexts[ctx as usize];
                cx.chains.push(ChainId(c));
                cx.files += self.chains[c as usize].chain.files;
            }
            for cx in &mut contexts {
                cx.mods.sort_unstable_by_key(|&m| self.priorities.get(m));
            }
            Contexts {
                contexts,
                of_mod,
                of_chain,
            }
        })
    }

    /// Normalized chains (route groups, `@sat:routes`): lazy, like contexts.
    pub fn routes(&self) -> &crate::routes::RouteGroups {
        self.routes
            .get_or_init(|| crate::routes::RouteGroups::build(self))
    }

    pub fn index(&self) -> &Arc<ConflictIndex> {
        &self.index
    }
    pub fn priorities(&self) -> &Priorities {
        &self.priorities
    }
    pub fn totals(&self) -> OverwriteStats {
        OverwriteStats {
            active_paths: self.active_paths,
            conflict_paths: self.conflict_paths,
            chains: self.alive,
            normalized: self.routes().len(),
            contexts: self.contexts_data().contexts.len(),
            fully_overridden: self.fully,
            vram_win_kib: self.stats.iter().map(|s| s.vram_win_kib).sum(),
        }
    }
    pub fn stat(&self, m: ModId) -> &ModStat {
        &self.stats[m.idx()]
    }
    pub fn stats(&self) -> &[ModStat] {
        &self.stats
    }
    /// Live chains in order of their first path (ids are stable while a chain lives, not dense).
    pub fn chain_ids(&self) -> Vec<ChainId> {
        self.contexts_data()
            .contexts
            .iter()
            .flat_map(|c| c.chains.iter().copied())
            .collect::<Vec<_>>()
    }
    pub fn chain_count(&self) -> usize {
        self.alive
    }
    pub fn chain(&self, c: ChainId) -> &Chain {
        &self.chains[c.idx()].chain
    }
    pub fn chain_context(&self, c: ChainId) -> Option<ContextId> {
        match self.contexts_data().of_chain[c.idx()] {
            NONE => None,
            x => Some(ContextId(x)),
        }
    }
    pub fn mod_context(&self, m: ModId) -> Option<ContextId> {
        match self.contexts_data().of_mod[m.idx()] {
            NONE => None,
            x => Some(ContextId(x)),
        }
    }
    // @sat:overwrite.opponents
    /// Per mod (index = `ModId`): how many other mods share an overwrite chain with it (the monolith's opponents).
    pub fn opponents(&self) -> Vec<u32> {
        let mut sets: Vec<rustc_hash::FxHashSet<u32>> = vec![Default::default(); self.stats.len()];
        for c in self.chain_ids() {
            let ms = self.chain_mods(c);
            for &a in ms {
                for &b in ms {
                    if a != b {
                        sets[a.idx()].insert(b.0);
                    }
                }
            }
        }
        sets.iter().map(|s| s.len() as u32).collect()
    }

    /// Stack of the chain, winner first.
    pub fn chain_mods(&self, c: ChainId) -> &[ModId] {
        let r = &self.chains[c.idx()];
        &self.chain_pool[r.start as usize..(r.start + r.len) as usize]
    }
    /// Paths of a chain, in path order (on demand: scans the winner's contested paths).
    pub fn chain_paths(&self, c: ChainId) -> Vec<PathId> {
        let Some(&w) = self.chain_mods(c).first() else {
            return Vec::new();
        };
        self.index
            .slots_of(w)
            .iter()
            .filter(|&&s| self.slot_chain[s as usize] == c.0)
            .map(|&s| self.index.contested[s as usize])
            .collect()
    }
    /// Chains a mod is a member of, by id.
    pub fn mod_chains(&self, m: ModId) -> Vec<ChainId> {
        let mut out: Vec<ChainId> = self
            .index
            .slots_of(m)
            .iter()
            .map(|&s| self.slot_chain[s as usize])
            .filter(|&c| c != NONE)
            .map(ChainId)
            .filter(|&c| self.chain_mods(c).contains(&m))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
    pub fn contexts(&self) -> &[Context] {
        &self.contexts_data().contexts
    }
    pub fn path_chain(&self, p: PathId) -> Option<ChainId> {
        let k = self.index.path_kind[p.idx()];
        if k == NONE || k & CONTESTED == 0 {
            return None;
        }
        match self.slot_chain[(k & !CONTESTED) as usize] {
            NONE => None,
            c => Some(ChainId(c)),
        }
    }
    pub fn path_winner(&self, p: PathId) -> Option<ModId> {
        match self.index.path_kind[p.idx()] {
            NONE => None,
            k if k & CONTESTED != 0 => self.stack((k & !CONTESTED) as usize).first().copied(),
            m => self.priorities.is_active(ModId(m)).then_some(ModId(m)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::tests::{mk, with_archive};

    fn names(t: &ProviderTable, ms: &[ModId]) -> Vec<String> {
        ms.iter().map(|&m| t.mod_name(m).to_string()).collect()
    }

    #[test]
    fn stacks_chains_contexts_and_badges() {
        let t = ProviderTable::build(&[
            mk("A", &["textures/a.dds", "textures/b.dds", "meshes/x.nif"]),
            mk("B", &["Textures/A.dds", "textures/c.dds"]),
            mk("C", &["textures/a.dds", "textures/b.dds", "textures/c.dds"]),
            mk("D", &["other/only.txt"]),
            mk("E", &["meshes/x.nif"]),
        ]);
        // E disabled
        let p = OverwriteProjection::compute(&t, Priorities(vec![0, 1, 2, 3, -1]));
        // a.dds: C>B>A, b.dds: C>A, c.dds: C>B, x.nif: only A
        assert_eq!(p.totals().chains, 3);
        assert_eq!(p.totals().conflict_paths, 3);
        let (a, b, c) = (ModId(0), ModId(1), ModId(2));
        assert_eq!(p.stat(c).win, 3);
        assert_eq!(p.stat(a).lose, 2);
        assert_eq!(p.stat(b).mid, 1);
        assert_eq!(p.stat(a).unique, 1);
        assert!(!p.stat(a).fully_overridden);
        assert!(
            p.stat(b).fully_overridden,
            "B wins nothing and has nothing unique"
        );
        assert_eq!(p.contexts().len(), 1);
        assert_eq!(names(&t, &p.contexts()[0].mods), ["A", "B", "C"]);
        let pa = t.find_path("textures/a.dds").unwrap();
        let ch = p.path_chain(pa).unwrap();
        assert_eq!(names(&t, p.chain_mods(ch)), ["C", "B", "A"]);
        assert_eq!(p.path_winner(pa), Some(c));

        // enable E above everything: x.nif becomes a conflict, A loses its unique
        let p2 = OverwriteProjection::compute(&t, Priorities(vec![0, 1, 2, 3, 4]));
        assert_eq!(p2.stat(ModId(4)).win, 1);
        assert_eq!(p2.stat(a).unique, 0);
    }

    #[test]
    fn loose_beats_archive_regardless_of_priority() {
        let t = ProviderTable::build(&[
            mk("Low", &["meshes/x.nif"]),
            with_archive(mk("High", &[]), "High.bsa", &["meshes/x.nif"]),
        ]);
        let p = OverwriteProjection::compute(&t, Priorities(vec![0, 10]));
        let x = t.find_path("meshes/x.nif").unwrap();
        assert_eq!(p.path_winner(x), Some(ModId(0)));
        assert_eq!(names(&t, p.chain_mods(p.path_chain(x).unwrap())), ["Low", "High"]);
    }

    #[test]
    fn own_archive_copy_is_not_a_conflict() {
        let t = ProviderTable::build(&[with_archive(mk("M", &["a/b.dds"]), "M.bsa", &["a/b.dds"])]);
        let p = OverwriteProjection::compute(&t, Priorities(vec![0]));
        assert_eq!(p.totals().chains, 0);
        assert_eq!(p.stat(ModId(0)).unique, 1);
        assert_eq!(p.stat(ModId(0)).providers, 2);
    }

    fn same(a: &OverwriteProjection, b: &OverwriteProjection, t: &ProviderTable, what: &str) {
        assert_eq!(a.totals(), b.totals(), "{what}: totals");
        assert_eq!(a.stats(), b.stats(), "{what}: stats");
        // chains by their stack (ids are stable in an updated projection, dense in a fresh one)
        let chains = |x: &OverwriteProjection| {
            type Seen = (Vec<ModId>, Chain, Vec<PathId>, Option<Vec<ModId>>);
            let mut v: Vec<Seen> = x
                .chain_ids()
                .into_iter()
                .map(|c| {
                    let ctx = x.chain_context(c).map(|k| x.contexts()[k.idx()].mods.clone());
                    (
                        x.chain_mods(c).to_vec(),
                        x.chain(c).clone(),
                        x.chain_paths(c),
                        ctx,
                    )
                })
                .collect();
            v.sort_by(|p, q| p.0.cmp(&q.0));
            v
        };
        assert_eq!(chains(a), chains(b), "{what}: chains");
        let ctx = |x: &OverwriteProjection| {
            x.contexts()
                .iter()
                .map(|c| (c.mods.clone(), c.files, c.chains.len()))
                .collect::<Vec<_>>()
        };
        assert_eq!(ctx(a), ctx(b), "{what}: contexts");
        for m in 0..t.mod_count() {
            let m = ModId::from_idx(m);
            let stacks = |x: &OverwriteProjection| {
                let mut v: Vec<Vec<ModId>> = x
                    .mod_chains(m)
                    .into_iter()
                    .map(|c| x.chain_mods(c).to_vec())
                    .collect();
                v.sort();
                v
            };
            assert_eq!(stacks(a), stacks(b), "{what}: chains of a mod");
        }
        for p in 0..t.path_count() {
            let p = PathId::from_idx(p);
            assert_eq!(a.path_winner(p), b.path_winner(p), "{what}: winner");
            assert_eq!(
                a.path_chain(p).map(|c| a.chain_mods(c).to_vec()),
                b.path_chain(p).map(|c| b.chain_mods(c).to_vec()),
                "{what}: chain of path"
            );
        }
    }

    // @sat:overwrite.tests.incremental
    /// Model test: after random moves and toggles, the incremental update equals a full computation.
    #[test]
    fn incremental_update_equals_full_compute() {
        let n = 60usize;
        let mods: Vec<_> = (0..n)
            .map(|i| {
                let files: Vec<String> = (0..40)
                    .map(|f| format!("d{}/f{}.dds", f % 5, (f * 13 + i * 7) % 120))
                    .collect();
                let refs: Vec<&str> = files.iter().map(String::as_str).collect();
                let m = mk(&format!("M{i}"), &refs);
                if i % 9 == 0 {
                    with_archive(m, &format!("M{i}.bsa"), &["d0/f1.dds", "d1/f2.dds", "x/arch.nif"])
                } else {
                    m
                }
            })
            .collect();
        let t = ProviderTable::build(&mods);
        // canon: an order of mods with enabled flags; priorities = rank if enabled
        let mut order: Vec<usize> = (0..n).collect();
        let mut enabled = vec![true; n];
        let prio = |order: &[usize], enabled: &[bool]| {
            let mut p = Priorities::all_inactive(n);
            for (rank, &m) in order.iter().enumerate() {
                if enabled[m] {
                    p.0[m] = rank as i32;
                }
            }
            p
        };
        let mut cur = OverwriteProjection::compute(&t, prio(&order, &enabled));
        let mut rng: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |k: usize| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % k as u64) as usize
        };
        for step in 0..300 {
            let mut touched = Vec::new();
            match next(3) {
                0 | 1 => {
                    let cnt = 1 + next(3);
                    for _ in 0..cnt {
                        let from = next(n);
                        let m = order.remove(from);
                        order.insert(next(n), m);
                        touched.push(ModId::from_idx(m));
                    }
                }
                _ => {
                    let m = next(n);
                    enabled[m] = !enabled[m];
                    touched.push(ModId::from_idx(m));
                }
            }
            let pr = prio(&order, &enabled);
            let inc = cur.update(&t, pr.clone(), &touched);
            let full = OverwriteProjection::compute(&t, pr);
            same(&inc, &full, &t, &format!("step {step}"));
            cur = inc;
        }
    }

    #[test]
    fn deterministic_across_runs() {
        let mods: Vec<_> = (0..40)
            .map(|i| {
                let files: Vec<String> = (0..60)
                    .map(|f| format!("d{}/f{}.dds", f % 7, (f * 31 + i) % 90))
                    .collect();
                let refs: Vec<&str> = files.iter().map(String::as_str).collect();
                mk(&format!("M{i}"), &refs)
            })
            .collect();
        let t = ProviderTable::build(&mods);
        let pr = Priorities((0..40).collect());
        let a = OverwriteProjection::compute(&t, pr.clone());
        let b = OverwriteProjection::compute(&t, pr);
        assert_eq!(a.totals(), b.totals());
        assert_eq!(a.stats(), b.stats());
        for c in 0..a.chain_count() {
            assert_eq!(
                a.chain_mods(ChainId::from_idx(c)),
                b.chain_mods(ChainId::from_idx(c))
            );
        }
    }
}
