//! Chunked order sequence: the canon order of a list of any size, with O(log n) position queries. @sat:order.seq
//!
//! Keys (dense `u32`) live in chunks of a few hundred. Per chunk the aggregates are kept — entries,
//! separators and, **per view layer**, visible rows — and Fenwick trees over the chunk order answer "position
//! of key", "key at position", "key at visible row of view L", "nearest separator" in O(log chunks + chunk
//! size). One canon order serves up to [`MAX_VIEWS`] views (ADR 0009): a view is a bit per key plus its own
//! counters, the chunks are shared. Removing a key never moves other chunks; empty and small chunks are folded
//! by [`Seq::compact`] at the end of an operation.
//!
//! Owns: the chunked sequence with Fenwick aggregates (positions, rows, blocks).

pub type Key = u32;

const NONE: u32 = u32::MAX;
/// Chunk size after a split.
const TARGET: usize = 256;
/// A chunk longer than this is split.
const MAX: usize = 2 * TARGET;
/// Neighbouring chunks shorter than this together are merged by `compact`.
const MIN: usize = TARGET / 4;

const SEPARATOR: u8 = 2;

/// View layers one sequence can carry.
pub const MAX_VIEWS: usize = 64;

/// A view layer of the sequence.
pub type Layer = usize;

/// Lowest set bit of `i > 0` (`isolate_lowest_one` is newer than the pinned toolchain).
fn lowbit(i: usize) -> usize {
    1 << i.trailing_zeros()
}

#[derive(Debug, Clone, Default)]
struct Fenwick {
    t: Vec<u32>,
}

impl Fenwick {
    fn build(vals: impl ExactSizeIterator<Item = u32>) -> Self {
        let mut t: Vec<u32> = std::iter::once(0).chain(vals).collect();
        let n = t.len();
        for i in 1..n {
            let j = i + lowbit(i);
            if j < n {
                t[j] += t[i];
            }
        }
        Self { t }
    }
    fn add(&mut self, i: usize, delta: i32) {
        let mut i = i + 1;
        while i < self.t.len() {
            self.t[i] = self.t[i].wrapping_add_signed(delta);
            i += lowbit(i);
        }
    }
    /// Sum of `[0, i)`.
    fn prefix(&self, i: usize) -> u32 {
        let (mut i, mut s) = (i, 0);
        while i > 0 {
            s += self.t[i];
            i &= i - 1;
        }
        s
    }
    /// Smallest `i` with `prefix(i + 1) > k`, and `prefix(i)`. Requires `k < total`.
    fn select(&self, mut k: u32) -> (usize, u32) {
        let n = self.t.len() - 1;
        let (mut pos, mut base) = (0usize, 0u32);
        let mut step = n.next_power_of_two();
        while step > 0 {
            let nxt = pos + step;
            if nxt <= n && self.t[nxt] <= k {
                pos = nxt;
                k -= self.t[nxt];
                base += self.t[nxt];
            }
            step >>= 1;
        }
        (pos, base)
    }
}

#[derive(Debug, Clone, Default)]
struct Chunk {
    keys: Vec<Key>,
    seps: u32,
}

/// Ordered sequence of keys with separator flags and per-view visibility.
#[derive(Debug, Clone, Default)]
pub struct Seq {
    chunks: Vec<Chunk>,
    /// Chunk ids in sequence order.
    order: Vec<u32>,
    /// Rank of a chunk id in `order`.
    rank: Vec<u32>,
    /// Chunk id of a key (`NONE` = not in the sequence).
    chunk_of: Vec<u32>,
    flags: Vec<u8>,
    /// Bit `L` = the key is a row of view layer `L`.
    vis: Vec<u64>,
    /// Active layers.
    active: u64,
    /// Per layer: visible rows per chunk id.
    vis_count: Vec<Vec<u32>>,
    f_len: Fenwick,
    f_vis: Vec<Fenwick>,
    f_sep: Fenwick,
    len: usize,
}

impl Seq {
    /// Builds a sequence from keys in order (no view layers yet).
    pub fn from_keys(keys: &[Key], separator: impl Fn(Key) -> bool) -> Self {
        let mut s = Self {
            vis_count: vec![Vec::new(); MAX_VIEWS],
            f_vis: vec![Fenwick::default(); MAX_VIEWS],
            ..Self::default()
        };
        for &k in keys {
            s.ensure_key(k);
            s.flags[k as usize] = if separator(k) { SEPARATOR } else { 0 };
        }
        for part in keys.chunks(TARGET) {
            s.push_chunk(part.to_vec());
        }
        s.len = keys.len();
        s.rebuild();
        s
    }

    fn ensure_key(&mut self, k: Key) {
        let need = k as usize + 1;
        if self.chunk_of.len() < need {
            self.chunk_of.resize(need, NONE);
            self.flags.resize(need, 0);
            self.vis.resize(need, 0);
        }
    }

    fn layers(&self) -> impl Iterator<Item = Layer> + use<> {
        let active = self.active;
        (0..MAX_VIEWS).filter(move |l| active & (1 << l) != 0)
    }

    /// Counts the aggregates of chunk `id` from its keys.
    fn recount(&mut self, id: u32) {
        let keys = &self.chunks[id as usize].keys;
        let seps = keys
            .iter()
            .filter(|&&k| self.flags[k as usize] & SEPARATOR != 0)
            .count() as u32;
        let counts: Vec<(Layer, u32)> = self
            .layers()
            .map(|l| {
                let n = keys
                    .iter()
                    .filter(|&&k| self.vis[k as usize] & (1 << l) != 0)
                    .count() as u32;
                (l, n)
            })
            .collect();
        for (l, n) in counts {
            self.vis_count[l][id as usize] = n;
        }
        self.chunks[id as usize].seps = seps;
    }

    fn push_chunk(&mut self, keys: Vec<Key>) -> u32 {
        let id = self.chunks.len() as u32;
        for &k in &keys {
            self.chunk_of[k as usize] = id;
        }
        self.chunks.push(Chunk { keys, seps: 0 });
        for l in self.layers() {
            self.vis_count[l].push(0);
        }
        self.recount(id);
        self.order.push(id);
        id
    }

    /// Recomputes ranks and Fenwick trees from the chunks (O(chunks × layers)).
    fn rebuild(&mut self) {
        self.rank.resize(self.chunks.len(), NONE);
        for (r, &id) in self.order.iter().enumerate() {
            self.rank[id as usize] = r as u32;
        }
        let ch = &self.chunks;
        let o = &self.order;
        self.f_len = Fenwick::build(o.iter().map(|&id| ch[id as usize].keys.len() as u32));
        self.f_sep = Fenwick::build(o.iter().map(|&id| ch[id as usize].seps));
        for l in self.layers() {
            let vc = &self.vis_count[l];
            self.f_vis[l] = Fenwick::build(o.iter().map(|&id| vc[id as usize]));
        }
    }

    /// Opens a view layer with no visible rows. `None` when all layers are taken.
    pub fn add_layer(&mut self) -> Option<Layer> {
        let l = (0..MAX_VIEWS).find(|l| self.active & (1 << l) == 0)?;
        self.active |= 1 << l;
        self.vis_count[l] = vec![0; self.chunks.len()];
        self.f_vis[l] = Fenwick::build(self.order.iter().map(|_| 0));
        Some(l)
    }

    /// Closes a view layer (O(n): its bits are cleared).
    pub fn drop_layer(&mut self, l: Layer) {
        let bit = !(1u64 << l);
        for v in &mut self.vis {
            *v &= bit;
        }
        self.active &= !(1 << l);
        self.vis_count[l] = Vec::new();
        self.f_vis[l] = Fenwick::default();
    }

    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn visible_len(&self, l: Layer) -> usize {
        self.f_vis[l].prefix(self.order.len()) as usize
    }
    pub fn separator_count(&self) -> usize {
        self.f_sep.prefix(self.order.len()) as usize
    }
    pub fn contains(&self, k: Key) -> bool {
        self.chunk_of.get(k as usize).is_some_and(|&c| c != NONE)
    }
    pub fn is_visible(&self, k: Key, l: Layer) -> bool {
        self.vis.get(k as usize).is_some_and(|v| v & (1 << l) != 0)
    }
    pub fn is_separator(&self, k: Key) -> bool {
        self.flags.get(k as usize).is_some_and(|f| f & SEPARATOR != 0)
    }

    fn slot(&self, k: Key) -> (u32, usize) {
        let id = self.chunk_of[k as usize];
        debug_assert!(id != NONE, "key {k} not in sequence");
        let slot = self.chunks[id as usize]
            .keys
            .iter()
            .position(|&x| x == k)
            .expect("chunk index out of sync");
        (id, slot)
    }

    /// Position of a key in the canon order.
    pub fn pos(&self, k: Key) -> usize {
        let (id, slot) = self.slot(k);
        self.f_len.prefix(self.rank[id as usize] as usize) as usize + slot
    }

    /// Number of rows of view `l` before the key (its row index when visible).
    pub fn rows_before(&self, k: Key, l: Layer) -> usize {
        let (id, slot) = self.slot(k);
        let bit = 1u64 << l;
        let before = self.chunks[id as usize].keys[..slot]
            .iter()
            .filter(|&&x| self.vis[x as usize] & bit != 0)
            .count();
        self.f_vis[l].prefix(self.rank[id as usize] as usize) as usize + before
    }

    /// Row index of a visible key in view `l`.
    pub fn row(&self, k: Key, l: Layer) -> Option<usize> {
        self.is_visible(k, l).then(|| self.rows_before(k, l))
    }

    /// Key at a canon position.
    pub fn at(&self, pos: usize) -> Key {
        let (r, base) = self.f_len.select(pos as u32);
        self.chunks[self.order[r] as usize].keys[pos - base as usize]
    }

    /// Key shown at a row of view `l`.
    pub fn at_row(&self, row: usize, l: Layer) -> Key {
        let (r, base) = self.f_vis[l].select(row as u32);
        let mut left = row - base as usize;
        let bit = 1u64 << l;
        for &k in &self.chunks[self.order[r] as usize].keys {
            if self.vis[k as usize] & bit != 0 {
                if left == 0 {
                    return k;
                }
                left -= 1;
            }
        }
        unreachable!("visible aggregate out of sync")
    }

    /// Separators at positions `< pos`.
    fn seps_before(&self, pos: usize) -> u32 {
        if pos >= self.len {
            return self.f_sep.prefix(self.order.len());
        }
        let (r, base) = self.f_len.select(pos as u32);
        let id = self.order[r] as usize;
        let inside = self.chunks[id].keys[..pos - base as usize]
            .iter()
            .filter(|&&x| self.flags[x as usize] & SEPARATOR != 0)
            .count() as u32;
        self.f_sep.prefix(r) + inside
    }

    /// The `n`-th separator (0-based) and its position.
    pub fn nth_separator(&self, n: u32) -> (Key, usize) {
        let (r, base) = self.f_sep.select(n);
        let id = self.order[r] as usize;
        let start = self.f_len.prefix(r) as usize;
        let mut left = n - base;
        for (i, &k) in self.chunks[id].keys.iter().enumerate() {
            if self.flags[k as usize] & SEPARATOR != 0 {
                if left == 0 {
                    return (k, start + i);
                }
                left -= 1;
            }
        }
        unreachable!("separator aggregate out of sync")
    }

    /// Nearest separator strictly before `pos`.
    pub fn separator_before(&self, pos: usize) -> Option<Key> {
        let n = self.seps_before(pos);
        (n > 0).then(|| self.nth_separator(n - 1).0)
    }

    /// Position of the first separator at or after `pos` (`len` if none): the end of the block containing `pos`.
    pub fn block_end(&self, pos: usize) -> usize {
        let n = self.seps_before(pos);
        if (n as usize) < self.separator_count() {
            self.nth_separator(n).1
        } else {
            self.len
        }
    }

    /// Sets the visibility of a key in view `l`; returns `true` when it changed.
    pub fn set_visible(&mut self, k: Key, l: Layer, v: bool) -> bool {
        let bit = 1u64 << l;
        let cur = &mut self.vis[k as usize];
        if (*cur & bit != 0) == v {
            return false;
        }
        *cur ^= bit;
        let id = self.chunk_of[k as usize];
        if id != NONE {
            let d = if v { 1 } else { -1 };
            let c = &mut self.vis_count[l][id as usize];
            *c = c.wrapping_add_signed(d);
            self.f_vis[l].add(self.rank[id as usize] as usize, d);
        }
        true
    }

    /// Removes a key (it leaves every view). Chunks are not restructured (see [`Seq::compact`]).
    pub fn remove(&mut self, k: Key) {
        let (id, slot) = self.slot(k);
        let f = self.flags[k as usize];
        self.chunks[id as usize].keys.remove(slot);
        let r = self.rank[id as usize] as usize;
        self.f_len.add(r, -1);
        for l in self.layers() {
            if self.vis[k as usize] & (1 << l) != 0 {
                self.vis_count[l][id as usize] -= 1;
                self.f_vis[l].add(r, -1);
            }
        }
        self.vis[k as usize] = 0;
        if f & SEPARATOR != 0 {
            self.chunks[id as usize].seps -= 1;
            self.f_sep.add(r, -1);
        }
        self.chunk_of[k as usize] = NONE;
        self.len -= 1;
    }

    /// Inserts keys (not in the sequence) at `pos`, keeping their order. They enter **hidden** in every view;
    /// the caller sets visibility afterwards (that is how row patches are produced).
    pub fn insert(&mut self, pos: usize, keys: &[Key], separator: impl Fn(Key) -> bool) {
        if keys.is_empty() {
            return;
        }
        for &k in keys {
            self.ensure_key(k);
            debug_assert!(self.chunk_of[k as usize] == NONE, "key {k} inserted twice");
            self.flags[k as usize] = if separator(k) { SEPARATOR } else { 0 };
            self.vis[k as usize] = 0;
        }
        let (id, slot) = if self.order.is_empty() {
            let id = self.push_chunk(Vec::new());
            self.rebuild();
            (id, 0)
        } else if pos >= self.len {
            let id = *self.order.last().unwrap();
            (id, self.chunks[id as usize].keys.len())
        } else {
            let (r, base) = self.f_len.select(pos as u32);
            (self.order[r], pos - base as usize)
        };
        let c = &mut self.chunks[id as usize];
        c.keys.splice(slot..slot, keys.iter().copied());
        let mut seps = 0;
        for &k in keys {
            self.chunk_of[k as usize] = id;
            seps += (self.flags[k as usize] & SEPARATOR != 0) as u32;
        }
        c.seps += seps;
        self.len += keys.len();
        if self.chunks[id as usize].keys.len() > MAX {
            self.split(id);
            self.rebuild();
        } else {
            let r = self.rank[id as usize] as usize;
            self.f_len.add(r, keys.len() as i32);
            self.f_sep.add(r, seps as i32);
        }
    }

    fn split(&mut self, id: u32) {
        let all = std::mem::take(&mut self.chunks[id as usize].keys);
        let mut parts = all.chunks(TARGET);
        let first = parts.next().unwrap().to_vec();
        let rest: Vec<Vec<Key>> = parts.map(<[Key]>::to_vec).collect();
        self.refill(id, first);
        let at = self.order.iter().position(|&x| x == id).unwrap() + 1;
        let mut new_ids = Vec::with_capacity(rest.len());
        for part in rest {
            let nid = self.push_chunk(part);
            self.order.pop();
            new_ids.push(nid);
        }
        self.order.splice(at..at, new_ids);
    }

    fn refill(&mut self, id: u32, keys: Vec<Key>) {
        for &k in &keys {
            self.chunk_of[k as usize] = id;
        }
        self.chunks[id as usize].keys = keys;
        self.recount(id);
    }

    /// Drops empty chunks and merges small neighbours. Positions do not change.
    pub fn compact(&mut self) {
        let mut order = Vec::with_capacity(self.order.len());
        for id in std::mem::take(&mut self.order) {
            if self.chunks[id as usize].keys.is_empty() {
                continue;
            }
            if let Some(&prev) = order.last() {
                let (a, b) = (
                    self.chunks[prev as usize].keys.len(),
                    self.chunks[id as usize].keys.len(),
                );
                if a + b <= MAX && (a < MIN || b < MIN) {
                    let moved = std::mem::take(&mut self.chunks[id as usize].keys);
                    let mut keys = std::mem::take(&mut self.chunks[prev as usize].keys);
                    keys.extend(moved);
                    self.refill(prev, keys);
                    self.recount(id);
                    continue;
                }
            }
            order.push(id);
        }
        // Recycle the arena when it is mostly garbage.
        if self.chunks.len() > 2 * order.len() + 16 {
            let mut old = std::mem::take(&mut self.chunks);
            self.order.clear();
            for l in self.layers() {
                self.vis_count[l].clear();
            }
            for id in order {
                let keys = std::mem::take(&mut old[id as usize].keys);
                self.push_chunk(keys);
            }
        } else {
            self.order = order;
        }
        self.rebuild();
    }

    /// Keys in canon order starting at `pos`.
    pub fn iter_from(&self, pos: usize) -> impl Iterator<Item = Key> + '_ {
        let (r, skip) = if pos >= self.len {
            (self.order.len(), 0)
        } else {
            let (r, base) = self.f_len.select(pos as u32);
            (r, pos - base as usize)
        };
        self.order[r..]
            .iter()
            .flat_map(|&id| self.chunks[id as usize].keys.iter().copied())
            .skip(skip)
    }

    /// Rows of view `l` starting at row `row`.
    pub fn rows_from(&self, row: usize, l: Layer) -> impl Iterator<Item = Key> + '_ {
        let start = if row >= self.visible_len(l) {
            self.len
        } else {
            self.pos(self.at_row(row, l))
        };
        self.iter_from(start).filter(move |&k| self.is_visible(k, l))
    }

    /// Visibility of every key in view `l` is recomputed by `visible(key)` without patches (a reset).
    pub fn reflag_all(&mut self, l: Layer, visible: impl Fn(Key) -> bool) {
        let bit = 1u64 << l;
        for id in 0..self.chunks.len() {
            let mut n = 0;
            for i in 0..self.chunks[id].keys.len() {
                let k = self.chunks[id].keys[i] as usize;
                let v = visible(k as Key);
                self.vis[k] = if v { self.vis[k] | bit } else { self.vis[k] & !bit };
                n += v as u32;
            }
            self.vis_count[l][id] = n;
        }
        self.rebuild();
    }

    /// Checks internal invariants (tests).
    #[cfg(test)]
    pub(crate) fn check(&self) {
        let mut n = 0;
        for (r, &id) in self.order.iter().enumerate() {
            let c = &self.chunks[id as usize];
            assert_eq!(self.rank[id as usize] as usize, r);
            let seps = c.keys.iter().filter(|&&k| self.is_separator(k)).count() as u32;
            assert_eq!(seps, c.seps);
            for l in self.layers() {
                let vis = c.keys.iter().filter(|&&k| self.is_visible(k, l)).count() as u32;
                assert_eq!(vis, self.vis_count[l][id as usize], "layer {l}");
                assert_eq!(self.f_vis[l].prefix(r + 1) - self.f_vis[l].prefix(r), vis);
            }
            for &k in &c.keys {
                assert_eq!(self.chunk_of[k as usize], id);
            }
            assert_eq!(
                self.f_len.prefix(r + 1) - self.f_len.prefix(r),
                c.keys.len() as u32
            );
            n += c.keys.len();
        }
        assert_eq!(n, self.len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fenwick_select() {
        let f = Fenwick::build([3u32, 0, 2, 5].into_iter());
        assert_eq!(f.prefix(4), 10);
        assert_eq!(f.select(0), (0, 0));
        assert_eq!(f.select(2), (0, 0));
        assert_eq!(f.select(3), (2, 3));
        assert_eq!(f.select(5), (3, 5));
        assert_eq!(f.select(9), (3, 5));
    }

    #[test]
    fn positions_rows_separators() {
        // 0=S 1 2 3=S 4 ... 1999
        let keys: Vec<Key> = (0..2000).collect();
        let mut s = Seq::from_keys(&keys, |k| k % 3 == 0);
        let l = s.add_layer().unwrap();
        let m = s.add_layer().unwrap();
        s.reflag_all(l, |k| k % 2 == 0);
        s.reflag_all(m, |_| true);
        s.check();
        assert_eq!(s.row(10, m), Some(10));
        assert_eq!(s.pos(1234), 1234);
        assert_eq!(s.at(777), 777);
        assert_eq!(s.row(10, l), Some(5));
        assert_eq!(s.at_row(5, l), 10);
        assert_eq!(s.separator_before(5), Some(3));
        assert_eq!(s.separator_before(0), None);
        assert_eq!(s.block_end(4), 6);
        assert_eq!(s.block_end(1999), 2000);
        for k in (0..2000).step_by(7) {
            s.remove(k);
        }
        s.check();
        s.insert(3, &[0, 7, 14], |k| k == 7);
        s.check();
        assert_eq!(s.iter_from(3).take(4).collect::<Vec<_>>(), vec![0, 7, 14, 4]);
        s.compact();
        s.check();
        s.drop_layer(m);
        s.check();
        assert_eq!(s.at(4), 7);
        let big: Vec<Key> = (5000..6200).collect();
        s.insert(10, &big, |_| false);
        s.check();
        assert_eq!(s.pos(5000), 10);
        assert_eq!(s.pos(6199), 10 + 1199);
    }
}
