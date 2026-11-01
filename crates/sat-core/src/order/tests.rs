//! Model tests: the patch stream must reproduce the view exactly, the view must equal a from-scratch scan, @sat:order.tests
//! and undo must restore the canon.
//!
//! Owns: model tests of the order engine (patch stream = view = full scan; undo restores canon).

use super::book::Row;
use super::*;
use super::{MAIN, Nav, Select, Shift, ViewId};
use crate::canon::{Entry, EntryKind, Modlist};
use crate::engine::Truth;

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

/// A UI mirror of one view: applies that view's patches, then refreshes only the rows they marked.
struct Mirror(ViewId, Vec<Option<Shown>>);
impl Mirror {
    fn new(b: &OrderBook, v: ViewId) -> Self {
        Self(
            v,
            b.rows(v, 0, usize::MAX).iter().map(|r| Some(shown(r))).collect(),
        )
    }
    fn apply(&mut self, b: &OrderBook, c: &Change) {
        let Some(vc) = c.view(self.0) else { return };
        for p in &vc.patches {
            match *p {
                Patch::Remove { at, count } => {
                    self.1.drain(at as usize..(at + count) as usize);
                }
                Patch::Insert { at, count } => {
                    let at = at as usize;
                    self.1.splice(at..at, std::iter::repeat_n(None, count as usize));
                }
                Patch::Update { at, count } => {
                    for s in &mut self.1[at as usize..(at + count) as usize] {
                        *s = None;
                    }
                }
                Patch::Reset { len } => self.1 = vec![None; len as usize],
            }
        }
        assert_eq!(self.1.len(), vc.len as usize, "row count after patches");
        for i in 0..self.1.len() {
            if self.1[i].is_none() {
                self.1[i] = Some(shown(&b.rows(self.0, i, 1)[0]));
            }
        }
    }
    fn assert_eq(&self, b: &OrderBook, ctx: &str) {
        let v = self.0;
        let ctx = format!("{ctx} (view {v})");
        let truth: Vec<Shown> = b.rows(v, 0, usize::MAX).iter().map(shown).collect();
        let mine: Vec<Shown> = self.1.iter().cloned().map(Option::unwrap).collect();
        assert_eq!(mine.len(), truth.len(), "{ctx}: length");
        for (i, (m, t)) in mine.iter().zip(&truth).enumerate() {
            assert_eq!(m, t, "{ctx}: row {i}");
        }
        let naive = b.naive_rows(v);
        let keys: Vec<Key> = truth.iter().map(|s| s.0).collect();
        assert_eq!(keys, naive, "{ctx}: incremental view differs from a full scan");
    }
}

fn run(seed: u64, n: usize, steps: usize) {
    let mut rng = Rng(seed);
    let start = list(n, &mut rng);
    let mut b = OrderBook::from_modlist(&start);
    // three windows over one canon: patches of every operation must keep each mirror exact
    let views = [MAIN, b.open_view().unwrap(), b.open_view().unwrap()];
    let mut ms: Vec<Mirror> = views.iter().map(|&v| Mirror::new(&b, v)).collect();
    let apply = |ms: &mut Vec<Mirror>, b: &OrderBook, c: &Change| {
        for m in ms.iter_mut() {
            m.apply(b, c);
        }
    };
    let check = |ms: &Vec<Mirror>, b: &OrderBook, ctx: &str| {
        for m in ms {
            m.assert_eq(b, ctx);
        }
    };
    let mut applied = 0usize;
    for step in 0..steps {
        let len = b.len();
        let pick = |rng: &mut Rng, b: &OrderBook| b.key_at(1 + rng.below(len - 1));
        let roll = rng.below(100);
        let v = views[rng.below(views.len())];
        if rng.chance(30) {
            let rows = b.row_count(v) as u32;
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
                    Some(b.navigate(v, nav, sel).unwrap())
                }
                4 => b.toggle_focused(v),
                5 => Some(if rng.chance(50) {
                    b.select_all(v).unwrap()
                } else {
                    b.clear_selection(v).unwrap()
                }),
                6 => {
                    let how = [Shift::Up, Shift::Down, Shift::BlockStart, Shift::BlockEnd][rng.below(4)];
                    b.shift_selection(v, how).unwrap()
                }
                7 => b.fold_focused(v, rng.chance(50)).unwrap(),
                _ => b.jump_to(v, &format!("mod {}", rng.below(10))),
            };
            if let Some(c) = c {
                apply(&mut ms, &b, &c);
            }
            b.check();
            check(&ms, &b, &format!("seed {seed} step {step} cursor {what}"));
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
                ("collapse", b.set_collapsed(v, k, collapsed).unwrap())
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
                        ("group-collapse", b.set_group_collapsed(v, g, c).unwrap())
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
                ("filter", Some(b.set_filter(v, f).unwrap()))
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
                // the main view is what the project stores; other views take its collapse state on reload
                let rows = |x: &OrderBook| x.rows(MAIN, 0, usize::MAX).iter().map(shown).collect::<Vec<_>>();
                assert_eq!(
                    rows(&copy),
                    rows(&b),
                    "decoration round trip changed the main view"
                );
                ("decor", None)
            }
        };
        if let Some(c) = change {
            applied += 1;
            // the truth side: canon and decoration operations report what they changed
            let truth: Vec<Truth> = c.truth.iter().map(|t| t.truth).collect();
            match what {
                "move" | "enable" | "insert" | "remove" | "rename" => {
                    assert!(truth.contains(&Truth::Canon), "{what}")
                }
                "color" | "mute" | "group" | "dissolve" => {
                    assert!(truth.contains(&Truth::Decoration), "{what}")
                }
                "collapse" | "group-collapse" | "filter" => {
                    assert!(truth.is_empty(), "{what} is a view operation")
                }
                _ => {}
            }
            apply(&mut ms, &b, &c);
        }
        b.check();
        check(&ms, &b, &format!("seed {seed} step {step} {what}"));
    }
    assert!(applied > steps / 3, "too few effective operations: {applied}");
    // All the way back: the canon equals the start (view state is not history).
    while let Some(c) = b.undo() {
        apply(&mut ms, &b, &c);
    }
    let back = b.to_modlist();
    assert_eq!(back, start, "seed {seed}: undo did not restore the canon");
    check(&ms, &b, "after full undo");
}

/// Seeds per run: few in debug builds (CI), many with `cargo test --release` or `SAT_MODEL_SEEDS=N`.
fn seeds() -> u64 {
    std::env::var("SAT_MODEL_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if cfg!(debug_assertions) { 6 } else { 40 })
}

#[test]
fn model_small() {
    for seed in 1..=seeds() {
        run(seed * 7919, 120, 1500);
    }
}

#[test]
fn model_large_chunks_split_and_merge() {
    run(424242, 3000, if cfg!(debug_assertions) { 1200 } else { 2500 });
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
        c.main().patches,
        vec![
            Patch::Update { at: 2, count: 1 },
            Patch::Remove { at: 3, count: 1 }
        ]
    );
    assert_eq!(b.rows(MAIN, 2, 1)[0].kind, RowKind::Group);
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
    assert!(b.set_group_collapsed(MAIN, g, false).unwrap().is_some());
    assert_eq!(b.row_count(MAIN), 7);
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
    let names = [
        "A_separator",
        "a1",
        "a2",
        "B_separator",
        "b1",
        "b2",
        "C_separator",
        "c1",
    ];
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
    let order = |b: &OrderBook| {
        b.to_modlist()
            .entries
            .into_iter()
            .map(|e| e.name)
            .collect::<Vec<_>>()
    };
    // focus A_separator, Alt+↓: block A goes below block B (not into it)
    b.navigate(MAIN, Nav::Home, Select::Replace).unwrap();
    b.shift_selection(MAIN, Shift::Down).unwrap().unwrap();
    assert_eq!(
        order(&b),
        [
            "B_separator",
            "b1",
            "b2",
            "A_separator",
            "a1",
            "a2",
            "C_separator",
            "c1"
        ]
    );
    // Alt+↑ brings it back
    b.shift_selection(MAIN, Shift::Up).unwrap().unwrap();
    assert_eq!(order(&b)[..3], ["A_separator", "a1", "a2"]);
    // a mod moves by one row, crossing into the next block
    let a2 = b.key_of("a2").unwrap();
    b.navigate(MAIN, Nav::Row(b.pos(a2) as u32), Select::Replace)
        .unwrap();
    b.shift_selection(MAIN, Shift::Down).unwrap().unwrap();
    assert_eq!(order(&b)[..5], ["A_separator", "a1", "B_separator", "a2", "b1"]);
    // the focus follows the entry
    assert_eq!(b.focus(MAIN), Some(a2));
    assert!(b.rows(MAIN, 3, 1)[0].focused);
    // Alt+Shift+↓: end of its block
    b.shift_selection(MAIN, Shift::BlockEnd).unwrap().unwrap();
    assert_eq!(
        order(&b)[..6],
        ["A_separator", "a1", "B_separator", "b1", "b2", "a2"]
    );
}

#[test]
fn focus_entry_reveals_a_collapsed_block_and_patches_stay_exact() {
    let mut rng = Rng(7);
    let mut b = OrderBook::from_modlist(&list(300, &mut rng));
    let v = b.open_view().unwrap();
    let mut m = Mirror::new(&b, v);
    // collapse every separator, then jump to a mod deep inside
    let seps: Vec<Key> = b
        .rows(v, 0, usize::MAX)
        .iter()
        .filter(|r| r.kind == RowKind::Separator)
        .map(|r| r.key)
        .collect();
    for s in &seps {
        if let Ok(Some(c)) = b.set_collapsed(v, *s, true) {
            m.apply(&b, &c);
        }
    }
    let target = (0..300u32)
        .map(|p| b.key_at(p as usize))
        .find(|&k| b.kind(k) == EntryKind::Mod && b.pos(k) > 150)
        .unwrap();
    let c = b.focus_entry(v, target).expect("reachable");
    m.apply(&b, &c);
    m.assert_eq(&b, "after focus_entry");
    assert_eq!(b.focus(v), Some(target));
    assert!(
        b.rows(v, 0, usize::MAX)
            .iter()
            .any(|r| r.key == target && r.focused)
    );
}

#[test]
fn card_style_is_undoable_interned_and_travels_with_the_project() {
    use super::decor::{Accent, CardStyle, Fill, FillKind};
    let mut rng = Rng(11);
    let mut b = OrderBook::from_modlist(&list(80, &mut rng));
    let mut m = Mirror::new(&b, MAIN);
    let mods: Vec<Key> = b
        .rows(MAIN, 0, usize::MAX)
        .iter()
        .filter(|r| r.kind == RowKind::Mod)
        .map(|r| r.key)
        .take(3)
        .collect();
    let style = CardStyle {
        width: Some(320),
        full_width: true,
        fill: Some(Fill {
            kind: FillKind::GradientRight,
            color: 0x334155,
            color2: Some(0x1e293b),
        }),
        accent: Some(Accent {
            color: 0xf59e0b,
            width: 4,
        }),
        ..CardStyle::default()
    };
    let c = b.set_style(&mods, Some(style.clone())).unwrap().expect("changed");
    assert_eq!(c.truth.len(), 1);
    assert_eq!(c.truth[0].truth, crate::engine::Truth::Decoration);
    m.apply(&b, &c);
    m.assert_eq(&b, "after style");
    assert_eq!(b.style(mods[0]), Some(&style));
    assert!(
        b.set_style(&mods, Some(style.clone())).unwrap().is_none(),
        "same style, no change"
    );
    let row = b
        .rows(MAIN, 0, usize::MAX)
        .into_iter()
        .find(|r| r.key == mods[1])
        .unwrap();
    assert_eq!(row.style, Some(&style));
    // through the project and back
    let d = b.decor();
    assert_eq!(d.styles.len(), 3);
    let json = serde_json::to_string(&d).unwrap();
    let back: super::Decor = serde_json::from_str(&json).unwrap();
    let mut b2 = OrderBook::from_modlist(&b.to_modlist());
    let (_, issues) = b2.apply_decor(&back);
    assert!(issues.is_empty());
    assert_eq!(b2.style(mods[2]), Some(&style));
    // undo removes it
    let c = b.undo().expect("undo");
    m.apply(&b, &c);
    m.assert_eq(&b, "after undo");
    assert_eq!(b.style(mods[0]), None);
}

#[test]
fn marks_are_undoable_normalized_and_travel_with_the_project() {
    use super::decor::Marks;
    let mut rng = Rng(12);
    let mut b = OrderBook::from_modlist(&list(40, &mut rng));
    let mut m = Mirror::new(&b, MAIN);
    let mods: Vec<Key> = b
        .rows(MAIN, 0, usize::MAX)
        .iter()
        .filter(|r| r.kind == RowKind::Mod)
        .map(|r| r.key)
        .take(2)
        .collect();
    // add a tag to two mods, a note to one: each mod keeps what it had
    let add = |tag: &'static str| {
        move |mut mk: Marks| {
            mk.tags.push(tag.into());
            mk
        }
    };
    let c = b.patch_marks(&mods, add(" UI ")).unwrap().expect("changed");
    m.apply(&b, &c);
    b.patch_marks(&mods[..1], |mut mk| {
        mk.note = "line 1\r\nline 2".into();
        mk.tags.push("ui".into()); // same tag, other case: kept once
        mk.tags.push("Armor".into());
        mk
    })
    .unwrap();
    assert_eq!(b.marks(mods[0]).unwrap().tags, ["Armor", "UI"]);
    assert_eq!(b.marks(mods[0]).unwrap().note, "line 1\nline 2");
    assert_eq!(b.marks(mods[1]).unwrap().tags, ["UI"]);
    assert_eq!(b.tags_in_use(), [("Armor".to_string(), 1), ("UI".to_string(), 2)]);
    // project round trip
    let d = b.decor();
    let json = serde_json::to_string(&d).unwrap();
    let mut b2 = OrderBook::from_modlist(&b.to_modlist());
    b2.apply_decor(&serde_json::from_str(&json).unwrap());
    assert_eq!(b2.marks(mods[0]), b.marks(mods[0]));
    // undo the note edit, then the tag: nothing left
    b.undo();
    assert_eq!(b.marks(mods[0]).unwrap().tags, ["UI"]);
    b.undo();
    assert!(b.marks(mods[0]).is_none() && b.marks(mods[1]).is_none());
    // a patch that empties the marks removes them
    b.redo();
    b.patch_marks(&mods, |_| Marks::default()).unwrap();
    assert!(b.marks(mods[1]).is_none());
}
