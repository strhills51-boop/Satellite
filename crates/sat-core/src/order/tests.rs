//! Model tests: the patch stream must reproduce the view exactly, the view must equal a from-scratch scan,
//! and undo must restore the canon.

use super::book::Row;
use super::*;
use super::{Nav, Select, Shift};
use crate::canon::{Entry, EntryKind, Modlist};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
}

fn list(n: usize, rng: &mut Rng) -> Modlist {
    let mut entries = vec![Entry {
        name: "DLC: Dawnguard".into(),
        kind: EntryKind::Foreign,
        enabled: true,
    }];
    for i in 0..n {
        let sep = i % 23 == 0 || rng.chance(3);
        entries.push(Entry {
            name: if sep {
                format!("S{i}_separator")
            } else {
                format!("Mod {i}")
            },
            kind: if sep { EntryKind::Separator } else { EntryKind::Mod },
            enabled: !sep && rng.chance(70),
        });
    }
    Modlist::new(entries)
}

/// What a UI keeps per row: content it rendered.
#[derive(Debug, Clone, PartialEq)]
struct Shown(Key, RowKind, String, bool, u8, bool, Option<u32>, u32, [bool; 3]);
fn shown(r: &Row<'_>) -> Shown {
    Shown(
        r.key,
        r.kind,
        r.name.to_string(),
        r.enabled,
        r.depth,
        r.collapsed,
        r.color,
        r.size,
        [r.muted, r.focused, r.selected],
    )
}

/// A UI mirror: applies patches, then refreshes only the rows patches marked as new or changed.
struct Mirror(Vec<Option<Shown>>);
impl Mirror {
    fn new(b: &OrderBook) -> Self {
        Self(b.rows(0, usize::MAX).iter().map(|r| Some(shown(r))).collect())
    }
    fn apply(&mut self, b: &OrderBook, c: &Change) {
        for p in &c.patches {
            match *p {
                Patch::Remove { at, count } => {
                    self.0.drain(at as usize..(at + count) as usize);
                }
                Patch::Insert { at, count } => {
                    let at = at as usize;
                    self.0.splice(at..at, std::iter::repeat_n(None, count as usize));
                }
                Patch::Update { at, count } => {
                    for s in &mut self.0[at as usize..(at + count) as usize] {
                        *s = None;
                    }
                }
                Patch::Reset { len } => self.0 = vec![None; len as usize],
            }
        }
        assert_eq!(self.0.len(), c.len as usize, "row count after patches");
        for i in 0..self.0.len() {
            if self.0[i].is_none() {
                self.0[i] = Some(shown(&b.rows(i, 1)[0]));
            }
        }
    }
    fn assert_eq(&self, b: &OrderBook, ctx: &str) {
        let truth: Vec<Shown> = b.rows(0, usize::MAX).iter().map(shown).collect();
        let mine: Vec<Shown> = self.0.iter().cloned().map(Option::unwrap).collect();
        assert_eq!(mine.len(), truth.len(), "{ctx}: length");
        for (i, (m, t)) in mine.iter().zip(&truth).enumerate() {
            assert_eq!(m, t, "{ctx}: row {i}");
        }
        let naive = b.naive_rows();
        let keys: Vec<Key> = truth.iter().map(|s| s.0).collect();
        assert_eq!(keys, naive, "{ctx}: incremental view differs from a full scan");
    }
}

fn run(seed: u64, n: usize, steps: usize) {
    let mut rng = Rng(seed);
    let start = list(n, &mut rng);
    let mut b = OrderBook::from_modlist(&start);
    let mut m = Mirror::new(&b);
    let mut applied = 0usize;
    for step in 0..steps {
        let len = b.len();
        let pick = |rng: &mut Rng, b: &OrderBook| b.key_at(1 + rng.below(len - 1));
        let roll = rng.below(100);
        if rng.chance(30) {
            let rows = b.row_count() as u32;
            let what = rng.below(9);
            let c = match what {
                0..=3 => {
                    let nav = match rng.below(7) {
                        0 => Nav::Up,
                        1 => Nav::Down,
                        2 => Nav::PageUp(1 + rng.below(20) as u32),
                        3 => Nav::PageDown(1 + rng.below(20) as u32),
                        4 => Nav::Home,
                        5 => Nav::End,
                        _ => Nav::Row(rng.below(rows as usize + 1) as u32),
                    };
                    let sel =
                        [Select::Replace, Select::Extend, Select::Toggle, Select::FocusOnly][rng.below(4)];
                    Some(b.navigate(nav, sel))
                }
                4 => b.toggle_focused(),
                5 => Some(if rng.chance(50) {
                    b.select_all()
                } else {
                    b.clear_selection()
                }),
                6 => {
                    let how = [Shift::Up, Shift::Down, Shift::BlockStart, Shift::BlockEnd][rng.below(4)];
                    b.shift_selection(how).unwrap()
                }
                7 => b.fold_focused(rng.chance(50)).unwrap(),
                _ => b.jump_to(&format!("mod {}", rng.below(10))),
            };
            if let Some(c) = c {
                m.apply(&b, &c);
            }
            b.check();
            m.assert_eq(&b, &format!("seed {seed} step {step} cursor {what}"));
            continue;
        }
        let (what, change) = match roll {
            0..=34 => {
                let mut keys = vec![pick(&mut rng, &b)];
                if rng.chance(40) {
                    let p = b.pos(keys[0]);
                    for d in 1..=rng.below(6) {
                        if p + d < len {
                            keys.push(b.key_at(p + d));
                        }
                    }
                }
                if rng.chance(20) {
                    keys.push(pick(&mut rng, &b));
                }
                let before = (!rng.chance(10)).then(|| b.key_at(rng.below(len)));
                let r = b.move_keys(&keys, before);
                ("move", r.unwrap_or(None))
            }
            35..=49 => {
                let keys: Vec<Key> = (0..1 + rng.below(4)).map(|_| pick(&mut rng, &b)).collect();
                ("enable", b.set_enabled(&keys, None).unwrap())
            }
            50..=57 => {
                let keys = vec![pick(&mut rng, &b)];
                let c = if rng.chance(70) {
                    Some(rng.below(3) as u32)
                } else {
                    None
                };
                ("color", b.set_color(&keys, c).unwrap())
            }
            58..=67 => {
                let k = pick(&mut rng, &b);
                let collapsed = rng.chance(60);
                ("collapse", b.set_collapsed(k, collapsed).unwrap())
            }
            68..=75 => {
                let p = 1 + rng.below(len - 1);
                let size = 2 + rng.below(4);
                let keys: Vec<Key> = (p..(p + size).min(len)).map(|i| b.key_at(i)).collect();
                ("group", b.create_group("G", &keys).ok().map(|x| x.1))
            }
            76..=81 => {
                let gs = b.groups_of_test();
                if gs.is_empty() {
                    ("group-none", None)
                } else {
                    let g = gs[rng.below(gs.len())];
                    if rng.chance(25) {
                        ("dissolve", b.dissolve_group(g).unwrap())
                    } else {
                        let c = rng.chance(50);
                        ("group-collapse", b.set_group_collapsed(g, c).unwrap())
                    }
                }
            }
            82..=84 => {
                let f = match rng.below(5) {
                    4 => Filter {
                        show_muted: true,
                        ..Filter::default()
                    },
                    0 => Filter::default(),
                    1 => Filter {
                        text: format!("mod {}", rng.below(9)),
                        ..Filter::default()
                    },
                    2 => Filter {
                        enabled: Some(rng.chance(50)),
                        ..Filter::default()
                    },
                    _ => Filter {
                        color: Some(rng.below(3) as u32),
                        ..Filter::default()
                    },
                };
                ("filter", Some(b.set_filter(f)))
            }
            85..=90 => ("undo", b.undo()),
            91..=93 => ("redo", b.redo()),
            94..=95 => {
                let n = 1 + rng.below(3);
                let entries: Vec<Entry> = (0..n)
                    .map(|i| {
                        let sep = rng.chance(25);
                        Entry {
                            name: if sep {
                                format!("N{step}.{i}_separator")
                            } else {
                                format!("New {step}.{i}")
                            },
                            kind: if sep { EntryKind::Separator } else { EntryKind::Mod },
                            enabled: !sep,
                        }
                    })
                    .collect();
                let before = (!rng.chance(10)).then(|| b.key_at(rng.below(len)));
                ("insert", b.insert_entries(&entries, before).ok().map(|x| x.1))
            }
            96 if rng.chance(50) => {
                let keys = vec![pick(&mut rng, &b)];
                let m = rng.chance(70);
                ("mute", b.set_muted(&keys, m).unwrap())
            }
            96 => {
                let k = pick(&mut rng, &b);
                let name = format!("Renamed {step}");
                ("rename", b.rename(k, &name).unwrap())
            }
            97 => {
                let keys: Vec<Key> = (0..1 + rng.below(2)).map(|_| pick(&mut rng, &b)).collect();
                ("remove", b.remove_entries(&keys).unwrap_or(None))
            }
            _ => {
                // decoration round trip through names, on a copy (it clears history)
                let d = b.decor();
                let mut copy = b.clone();
                let (_, issues) = copy.apply_decor(&d);
                assert!(issues.is_empty(), "{issues:?}");
                assert_eq!(copy.decor(), d);
                let rows = |x: &OrderBook| x.rows(0, usize::MAX).iter().map(shown).collect::<Vec<_>>();
                assert_eq!(rows(&copy), rows(&b), "decoration round trip changed the view");
                ("decor", None)
            }
        };
        if let Some(c) = change {
            applied += 1;
            m.apply(&b, &c);
        }
        b.check();
        m.assert_eq(&b, &format!("seed {seed} step {step} {what}"));
    }
    assert!(applied > steps / 3, "too few effective operations: {applied}");
    // All the way back: the canon equals the start (view state is not history).
    while let Some(c) = b.undo() {
        m.apply(&b, &c);
    }
    let back = b.to_modlist();
    assert_eq!(back, start, "seed {seed}: undo did not restore the canon");
    m.assert_eq(&b, "after full undo");
}

#[test]
fn model_small() {
    for seed in 1..=12 {
        run(seed * 7919, 120, 1500);
    }
}

#[test]
fn model_large_chunks_split_and_merge() {
    run(424242, 3000, 2500);
}

#[test]
fn move_matches_canon_semantics() {
    let mut rng = Rng(99);
    let start = list(400, &mut rng);
    let mut b = OrderBook::from_modlist(&start);
    let mut canon = start.clone();
    for _ in 0..800 {
        let len = canon.entries.len();
        let names: Vec<String> = (0..1 + rng.below(3))
            .map(|_| canon.entries[1 + rng.below(len - 1)].name.clone())
            .collect();
        let before = canon.entries[rng.below(len)].name.clone();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let want = canon.move_entries(&refs, Some(&before)).unwrap();
        let keys: Vec<Key> = names.iter().map(|n| b.key_of(n).unwrap()).collect();
        let got = b.move_keys(&keys, b.key_of(&before)).unwrap();
        assert_eq!(want, got.is_some(), "changed flag");
        assert_eq!(b.to_modlist(), canon);
    }
}

#[test]
fn group_snaps_drop_target_and_moves_as_one() {
    let names = ["A_separator", "a1", "a2", "a3", "a4", "B_separator", "b1"];
    let list = Modlist::new(
        names
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
    let mut b = OrderBook::from_modlist(&list);
    let k = |b: &OrderBook, n: &str| b.key_of(n).unwrap();
    let (g, c) = b.create_group("Pack", &[k(&b, "a2"), k(&b, "a3")]).unwrap();
    // a3 row disappears, a2 row becomes the group row
    assert_eq!(
        c.patches,
        vec![
            Patch::Update { at: 2, count: 1 },
            Patch::Remove { at: 3, count: 1 }
        ]
    );
    assert_eq!(b.rows(2, 1)[0].kind, RowKind::Group);
    // dropping b1 "inside" the group lands before it
    b.move_keys(&[k(&b, "b1")], Some(k(&b, "a3"))).unwrap();
    let order: Vec<String> = b.to_modlist().entries.into_iter().map(|e| e.name).collect();
    assert_eq!(
        order,
        ["A_separator", "a1", "b1", "a2", "a3", "a4", "B_separator"]
    );
    // moving the group row moves both members
    b.move_keys(&[k(&b, "a2")], None).unwrap();
    let order: Vec<String> = b.to_modlist().entries.into_iter().map(|e| e.name).collect();
    assert_eq!(
        order,
        ["A_separator", "a1", "b1", "a4", "B_separator", "a2", "a3"]
    );
    assert!(b.set_group_collapsed(g, false).unwrap().is_some());
    assert_eq!(b.row_count(), 7);
}

#[test]
fn errors() {
    let mut rng = Rng(5);
    let mut b = OrderBook::from_modlist(&list(30, &mut rng));
    assert!(matches!(b.move_keys(&[0], None), Err(OrderError::Immovable(_))));
    assert_eq!(b.move_keys(&[9999], None), Err(OrderError::UnknownKey(9999)));
    assert!(b.create_group("x", &[2]).is_err());
}

#[test]
fn keyboard_moves_blocks_whole_and_mods_by_row() {
    let names = ["A_separator", "a1", "a2", "B_separator", "b1", "b2", "C_separator", "c1"];
    let list = Modlist::new(
        names
            .iter()
            .map(|n| Entry {
                name: n.to_string(),
                kind: if n.ends_with("_separator") { EntryKind::Separator } else { EntryKind::Mod },
                enabled: true,
            })
            .collect(),
    );
    let mut b = OrderBook::from_modlist(&list);
    let order = |b: &OrderBook| b.to_modlist().entries.into_iter().map(|e| e.name).collect::<Vec<_>>();
    // focus A_separator, Alt+↓: block A goes below block B (not into it)
    b.navigate(Nav::Home, Select::Replace);
    b.shift_selection(Shift::Down).unwrap().unwrap();
    assert_eq!(order(&b), ["B_separator", "b1", "b2", "A_separator", "a1", "a2", "C_separator", "c1"]);
    // Alt+↑ brings it back
    b.shift_selection(Shift::Up).unwrap().unwrap();
    assert_eq!(order(&b)[..3], ["A_separator", "a1", "a2"]);
    // a mod moves by one row, crossing into the next block
    let a2 = b.key_of("a2").unwrap();
    b.navigate(Nav::Row(b.pos(a2) as u32), Select::Replace);
    b.shift_selection(Shift::Down).unwrap().unwrap();
    assert_eq!(order(&b)[..5], ["A_separator", "a1", "B_separator", "a2", "b1"]);
    // the focus follows the entry
    assert_eq!(b.focus(), Some(a2));
    assert!(b.rows(3, 1)[0].focused);
    // Alt+Shift+↓: end of its block
    b.shift_selection(Shift::BlockEnd).unwrap().unwrap();
    assert_eq!(order(&b)[..6], ["A_separator", "a1", "B_separator", "b1", "b2", "a2"]);
}
