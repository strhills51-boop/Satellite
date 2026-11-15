//! Solution front: which unsettled chain of a context to look at next. @sat:front
//!
//! Owns: the monolith's connectivity score and the pick of the first and the next chain (v667,
//! `getConflictPresentationConnectivityScoreV667`, `pickInitialSolutionFrontPresentationV667`,
//! `pickNextSolutionFrontPresentationV667`, `08-conflicts.md` §4.10, S17 152 734).
//! Not here: settling a chain (`@sat:decisions`), the cards (`@sat:app.cards`).
//!
//! - connectivity of a chain = Σ over the other chains (+10 per shared mod, +2 per shared file) + its mods +
//!   min(30, files / 4);
//! - first: among unsettled chains, the most connected → more files → earlier;
//! - next, once something is settled: connectivity + per mod +80 when a settled chain has it, +40 when the last
//!   settled one has it (+8 / +4 per file) — the user keeps working on the mods he just touched.
//!
//! NG's chains of one context share no files (an exact chain owns its paths), so the file terms are zero here.

/// One chain of a context: its member units and size.
#[derive(Debug, Clone)]
pub struct Card<'a> {
    pub mods: &'a [u32],
    pub files: u32,
    pub done: bool,
    /// Put off by the user for now: not a candidate, and not "settled" either (it adds no weight to its mods).
    pub skipped: bool,
}

/// How many chains have each mod (a chain's mods are distinct).
fn holders(cards: &[Card<'_>]) -> rustc_hash::FxHashMap<u32, u64> {
    let mut n: rustc_hash::FxHashMap<u32, u64> = Default::default();
    for c in cards {
        for &m in c.mods {
            *n.entry(m).or_default() += 1;
        }
    }
    n
}

fn connectivity_with(cards: &[Card<'_>], holders: &rustc_hash::FxHashMap<u32, u64>, i: usize) -> u64 {
    let c = &cards[i];
    // every other chain with a mod of this one adds 10 for it
    let links: u64 = c.mods.iter().map(|m| 10 * (holders[m] - 1)).sum();
    links + c.mods.len() as u64 + u64::from(c.files / 4).min(30)
}

/// The connectivity score of chain `i`.
pub fn connectivity(cards: &[Card<'_>], i: usize) -> u64 {
    connectivity_with(cards, &holders(cards), i)
}

/// The chain to look at: the next after `last` (an index of a settled chain) or the first; `None` when every chain is
/// settled. Linear in the chains' mods: a context of thousands of chains answers at once.
pub fn pick(cards: &[Card<'_>], last: Option<usize>) -> Option<usize> {
    let held = holders(cards);
    let mut settled: rustc_hash::FxHashSet<u32> = Default::default();
    for c in cards.iter().filter(|c| c.done) {
        settled.extend(c.mods.iter().copied());
    }
    let last: rustc_hash::FxHashSet<u32> = last
        .filter(|&l| l < cards.len())
        .map(|l| cards[l].mods.iter().copied().collect())
        .unwrap_or_default();
    let score = |i: usize| {
        let mut s = connectivity_with(cards, &held, i);
        if !settled.is_empty() {
            for m in cards[i].mods {
                if settled.contains(m) {
                    s += 80;
                }
                if last.contains(m) {
                    s += 40;
                }
            }
        }
        s
    };
    (0..cards.len())
        .filter(|&i| !cards[i].done && !cards[i].skipped)
        .map(|i| (score(i), cards[i].files, std::cmp::Reverse(i)))
        .max()
        .map(|(_, _, std::cmp::Reverse(i))| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_the_most_connected_then_next_to_what_was_settled() {
        let (a, b, c, d) = ([1, 2], [2, 3], [7, 8], [3, 4]);
        let mut cards = vec![
            Card {
                mods: &a,
                files: 10,
                done: false,
                skipped: false,
            },
            Card {
                mods: &b,
                files: 1,
                done: false,
                skipped: false,
            },
            Card {
                mods: &c,
                files: 400,
                done: false,
                skipped: false,
            },
            Card {
                mods: &d,
                files: 1,
                done: false,
                skipped: false,
            },
        ];
        // b shares a mod with a and with d: 20 + 2
        assert_eq!(connectivity(&cards, 1), 22);
        assert_eq!(connectivity(&cards, 2), 2 + 30);
        assert_eq!(pick(&cards, None), Some(2), "400 files outweigh two links");
        cards[2].done = true;
        assert_eq!(
            pick(&cards, Some(2)),
            Some(1),
            "nothing touches c: the most connected is b"
        );
        cards[1].done = true;
        // a and d each share a mod with settled b (+80, +40 as the last): a 14 + 120, d 12 + 120
        assert_eq!(pick(&cards, Some(1)), Some(0));
        cards[0].done = true;
        assert_eq!(pick(&cards, Some(0)), Some(3));
        cards[3].done = true;
        assert_eq!(pick(&cards, None), None);
        // a card put off is passed by and weighs nothing
        cards[3].done = false;
        cards[3].skipped = true;
        assert_eq!(pick(&cards, None), None);
    }
}
