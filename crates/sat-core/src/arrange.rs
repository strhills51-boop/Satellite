//! The order of a context's chains: load order, with chains joined by bridge mods kept together. @sat:arrange
//!
//! Owns: placing the chains (cards) of one context in a line and the link each one hangs on — the chain placed
//! before it that shares mods with it (the bridge mods).
//! Not here: which chains there are (`@sat:routes`, `@sat:layout`), the cards (`@sat:app.cards`).
//!
//! Owner, 2026-11-15: a chain is the smallest stable view, a context the largest, nothing in between is
//! predictable — so do not merge further, put related chains side by side. A **bridge mod** takes part in several
//! chains; through bridges the chains make the context. Here:
//! - a chain's place in the load order is the monolith's canonical vector (v651): its members' list positions,
//!   ascending, compared as words — the chain that starts earlier comes first;
//! - a walk in depth: start from the earliest chain not yet placed; from the chain on top, go to the earliest chain
//!   not yet placed that shares a mod with it; when none is left, step back. Every chain after the first of its run
//!   is placed right under a chain it is bridged to, and among those the load order decides;
//! - the link of a chain: the chain it was reached from and the mods they share.

/// One chain: its members (distinct) and their list positions.
#[derive(Debug, Clone)]
pub struct Item {
    pub mods: Vec<u32>,
    /// Members' list positions, ascending (the canonical vector).
    pub key: Vec<usize>,
}

/// Where a chain hangs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// Place (in the result) of the chain it was reached from.
    pub to: usize,
    /// Bridge mods: members of both.
    pub mods: Vec<u32>,
}

/// The chains in their order: `(item index, link)`.
pub fn arrange(items: &[Item]) -> Vec<(usize, Option<Link>)> {
    let n = items.len();
    let mut by_key: Vec<usize> = (0..n).collect();
    by_key.sort_by(|&a, &b| items[a].key.cmp(&items[b].key).then(a.cmp(&b)));
    let mut rank = vec![0usize; n];
    for (r, &i) in by_key.iter().enumerate() {
        rank[i] = r;
    }
    // mod → chains with it, by rank
    let mut of_mod: rustc_hash::FxHashMap<u32, Vec<usize>> = Default::default();
    for (i, it) in items.iter().enumerate() {
        for &m in &it.mods {
            of_mod.entry(m).or_default().push(i);
        }
    }
    for v in of_mod.values_mut() {
        v.sort_by_key(|&i| rank[i]);
    }
    let mut placed = vec![usize::MAX; n];
    let mut out: Vec<(usize, Option<Link>)> = Vec::with_capacity(n);
    let mut next_root = 0;
    // (item, its place, cursor per mod list is recomputed: lists are sorted, so the first unplaced is the earliest)
    let mut stack: Vec<usize> = Vec::new();
    while out.len() < n {
        let Some(&top) = stack.last() else {
            while placed[by_key[next_root]] != usize::MAX {
                next_root += 1;
            }
            let r = by_key[next_root];
            placed[r] = out.len();
            out.push((r, None));
            stack.push(r);
            continue;
        };
        let next = items[top]
            .mods
            .iter()
            .filter_map(|m| of_mod[m].iter().copied().find(|&i| placed[i] == usize::MAX))
            .min_by_key(|&i| rank[i]);
        match next {
            Some(i) => {
                let mut shared: Vec<u32> = items[i]
                    .mods
                    .iter()
                    .copied()
                    .filter(|m| items[top].mods.contains(m))
                    .collect();
                shared.sort_unstable();
                placed[i] = out.len();
                out.push((
                    i,
                    Some(Link {
                        to: placed[top],
                        mods: shared,
                    }),
                ));
                stack.push(i);
            }
            None => {
                stack.pop();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn it(mods: &[u32]) -> Item {
        let mut key: Vec<usize> = mods.iter().map(|&m| m as usize).collect();
        key.sort_unstable();
        Item {
            mods: mods.to_vec(),
            key,
        }
    }

    #[test]
    fn bridged_chains_come_together_in_load_order() {
        // mods are their own positions. a = {1,5}, b = {2,9}, c = {5,7}, d = {9,10}
        // load order alone: a, b, c, d; bridges: a–c (5), b–d (9)
        let items = [it(&[1, 5]), it(&[2, 9]), it(&[5, 7]), it(&[9, 10])];
        let order = arrange(&items);
        let idx: Vec<usize> = order.iter().map(|x| x.0).collect();
        assert_eq!(idx, [0, 2, 1, 3], "c right under a, then b with d");
        assert_eq!(order[1].1, Some(Link { to: 0, mods: vec![5] }));
        assert_eq!(
            order[2].1, None,
            "b is not bridged to what came before: a new run"
        );
        assert_eq!(order[3].1, Some(Link { to: 2, mods: vec![9] }));
    }

    #[test]
    fn the_walk_steps_back_to_the_last_chain_with_unplaced_neighbours() {
        // hub h = {1,2,3}; x = {1,8}, y = {3,9}; z = {8,20} hangs on x
        let items = [it(&[1, 2, 3]), it(&[1, 8]), it(&[3, 9]), it(&[8, 20])];
        let order = arrange(&items);
        let idx: Vec<usize> = order.iter().map(|x| x.0).collect();
        assert_eq!(idx, [0, 1, 3, 2]);
        assert_eq!(
            order[3].1,
            Some(Link { to: 0, mods: vec![3] }),
            "y hangs on the hub, not on z"
        );
    }
}
