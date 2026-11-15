//! Chain cards of a conflict context (monolith conflict modal, "Карточки"): one card per chain, details on demand. @sat:app.cards
//!
//! Owns: the cards of a context's chains — files (normalized chains, `@sat:app.chains`) and plugin records
//! (`@sat:records.chains`) in one shape — and a card's details: its members with a "teeth" row (one tooth per path or
//! record of the chain: wins, loses, equal to the winner, the record's own plugin, not there) and its paths or records.
//! Not here: the context table (`@sat:app.context-table`, `@sat:app.records.table`), naming and identity of chains.
//!
//! A card's `index` is its chain's place in the context's chain order — the same as the table's chain bands, so a
//! card leads to its columns. Details come in a window (`limit`): a chain of 40 000 paths never goes whole.

use serde::{Deserialize, Serialize};

use sat_core::ids::ModId;
use sat_core::order::Key;

use crate::App;

/// One chain of a context.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChainCard {
    /// Place in the context's chain order (the table's chain band).
    pub index: u32,
    pub signature: String,
    pub done: bool,
    pub emoji: String,
    pub hue: u16,
    pub name: String,
    /// Paths or records.
    pub files: u32,
    pub bytes: u64,
    /// Members, the lowest first, the winner last.
    pub stack: Vec<String>,
    /// Disabled mods that own paths of the chain (`@sat:app.participants`), list order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub idle: Vec<String>,
    /// Exact chains the card holds (the system merged them, or the user did); more than one — it can be split.
    #[serde(default)]
    pub sources: u32,
    /// The user's layout made this card (merged or split by hand, `@sat:layout`): it can go back to the system's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub user: bool,
    /// The card it hangs on in the context's order and the bridge mods they share (`@sat:arrange`); none — it
    /// starts a run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<CardLink>,
    /// Its mods that are in other chains of the context too (bridge mods).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bridges: Vec<String>,
    /// Its mods that are in many chains of the context (common, not bridges: base mods under a quarter or more).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub common: Vec<String>,
    /// Settled, but its paths or winner changed since (`@sat:decisions.stamp`): what changed; settling again
    /// confirms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed: Option<String>,
    /// Not settled, but a chain it took over from was (its members changed, `@sat:lineage`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub was_done: bool,
    /// Chains of the user's merge this card holds that are gone (removed mods).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub lost: u32,
    /// The solution front points here: the unsettled chain to look at next (`@sat:front`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub front: bool,
    /// Records: a later plugin overrides an earlier one without having it as a master.
    #[serde(default)]
    pub blind: bool,
    /// Losing copies equal to the winner's (files: when hashes are known; records: always).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equal: Option<u32>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Where a card hangs: the card before it in the context's order that shares mods with it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CardLink {
    /// Index of that card.
    pub to: u32,
    /// The bridge mods.
    pub mods: Vec<String>,
}

/// A member of a chain with one tooth per path or record of the chain (in the details' order).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CardMember {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<Key>,
    pub enabled: bool,
    /// `w` wins, `l` loses, `u` owns it while disabled, `s` equal to the winner, `o` the record's own plugin, `-`
    /// not there (the monolith's `m`, missing).
    pub teeth: String,
    pub wins: u32,
    pub loses: u32,
    /// The monolith's chain participant (`08-conflicts.md` §3.4): targets of the chain it has, has not, and its
    /// conflict targets outside the chain. Files only; other domains leave them zero.
    #[serde(default)]
    pub owned: u32,
    #[serde(default)]
    pub missing: u32,
    #[serde(default)]
    pub extra: u32,
}

/// What a card shows open.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainCardDetail {
    pub index: u32,
    /// Lowest first, the winner last.
    pub members: Vec<CardMember>,
    /// Stacks (`@sat:conflict.stacks`): what each stack's targets share, the first `limit` stacks; a tooth per
    /// stack.
    pub items: Vec<String>,
    /// Targets in each of `items`.
    #[serde(default)]
    pub counts: Vec<u32>,
    /// Stacks in the chain.
    pub items_total: usize,
    /// Targets in the chain.
    #[serde(default)]
    pub targets_total: usize,
}

/// Wins and losses of a member over its stacks, each counted by its targets.
pub(crate) fn tally(teeth: &str, counts: &[u32]) -> (u32, u32) {
    let (mut w, mut l) = (0, 0);
    for (b, &n) in teeth.bytes().zip(counts) {
        match b {
            b'w' => w += n,
            b'l' | b's' | b't' | b'x' => l += n,
            _ => {}
        }
    }
    (w, l)
}

impl App {
    // @sat:app.cards.files
    /// Cards of the file context of mod `key`; `last` — the signature of the chain settled last, which steers the
    /// solution front.
    pub(crate) fn context_cards(&mut self, key: Key, last: Option<&str>) -> Option<Vec<ChainCard>> {
        let cx = self.context_of(key)?;
        let ow = self.session.overwrite();
        let table = self.session.table().clone();
        let heads = self.heads();
        let decisions = self.session.decisions();
        let book = self.session.book();
        let pos_of = |m: ModId| book.key_of(table.mod_name(m)).map_or(usize::MAX, |k| book.pos(k));
        let co = crate::context::context_order(&ow, &heads, cx, &pos_of);
        let order = &co.order;
        let groups: Vec<u32> = order.iter().map(|x| x.0).collect();
        let paths_of = ow.context_chain_paths(cx);
        // bridge mods: in two or more chains of the context, but not in most of them (those are common)
        let held = &co.held;
        let label = |m: u32| self.session.unit_label(ModId(m)).to_string();
        let mut cards: Vec<ChainCard> = Vec::with_capacity(groups.len());
        let mut units: Vec<Vec<u32>> = Vec::with_capacity(groups.len());
        for (i, &g) in groups.iter().enumerate() {
            let h = &heads[g as usize];
            let stacks = crate::participants::head_stacks(&ow, &table, h, Some(&paths_of));
            let mut idle: Vec<ModId> = stacks.iter().flat_map(|s| s.idle.iter().copied()).collect();
            idle.sort_by_key(|&m| (pos_of(m), m));
            idle.dedup();
            let mut us: Vec<u32> = h.route.iter().chain(&idle).map(|m| m.0).collect();
            us.sort_unstable();
            units.push(us);
            let done = h.done(decisions);
            let changed = if done { drift(&ow, &self.session, h) } else { None };
            cards.push(ChainCard {
                index: i as u32,
                signature: h.hex(),
                done,
                changed,
                was_done: !done && h.followed.iter().any(|&o| decisions.is_done(o)),
                lost: h.lost,
                emoji: h.emoji.clone(),
                hue: h.hue,
                name: h.name.clone(),
                files: h.files,
                bytes: h.bytes,
                stack: h
                    .route
                    .iter()
                    .rev()
                    .map(|&m| self.session.unit_label(m).to_string())
                    .collect(),
                idle: idle
                    .iter()
                    .map(|&m| self.session.unit_label(m).to_string())
                    .collect(),
                sources: h.sources.len() as u32,
                user: h.user,
                link: order[i].1.as_ref().map(|l| CardLink {
                    to: l.to as u32,
                    mods: l.mods.iter().map(|&m| label(m)).collect(),
                }),
                bridges: h
                    .route
                    .iter()
                    .rev()
                    .filter(|m| held[m] > 1 && !co.common.contains(m))
                    .map(|m| label(m.0))
                    .collect(),
                common: h
                    .route
                    .iter()
                    .rev()
                    .filter(|m| co.common.contains(m))
                    .map(|m| label(m.0))
                    .collect(),
                front: false,
                blind: false,
                equal: crate::chains::chain_content(&table, &self.content_hashes, &ow, &h.chains)
                    .map(|c| c.equal_files),
            });
        }
        // @sat:app.cards.front
        let front: Vec<sat_core::front::Card<'_>> = cards
            .iter()
            .zip(&units)
            .map(|(c, u)| sat_core::front::Card {
                mods: u,
                files: c.files,
                done: c.done,
            })
            .collect();
        let last = last.and_then(|l| cards.iter().position(|c| c.signature == l));
        if let Some(f) = sat_core::front::pick(&front, last) {
            cards[f].front = true;
        }
        Some(cards)
    }

    /// Details of card `index` of the file context of mod `key`: members in list order — the active stack and the
    /// disabled owners — with a tooth per owner stack and the monolith's W / L / M / X.
    pub(crate) fn context_card(&mut self, key: Key, index: usize, limit: usize) -> Option<ChainCardDetail> {
        let cx = self.context_of(key)?;
        let ow = self.session.overwrite();
        let table = self.session.table().clone();
        let heads = self.heads();
        let book = self.session.book();
        let pos_of = |m: ModId| book.key_of(table.mod_name(m)).map_or(usize::MAX, |k| book.pos(k));
        let g = *crate::context::context_groups(&ow, &heads, cx, &pos_of).get(index)?;
        let h = &heads[g as usize];
        let stacks = crate::participants::head_stacks(&ow, &table, h, None);
        let shown = &stacks[..stacks.len().min(limit)];
        let counts: Vec<u32> = shown.iter().map(|s| s.files()).collect();
        let pos_of = |m: ModId| book.key_of(table.mod_name(m)).map_or(usize::MAX, |k| book.pos(k));
        let mut all: Vec<ModId> = stacks.iter().flat_map(|s| s.members(&ow)).collect();
        all.sort_by_key(|&m| (pos_of(m), m));
        all.dedup();
        let members = all
            .iter()
            .map(|&m| {
                let teeth: String = shown.iter().map(|s| s.cell(&ow, m)).collect();
                let t = crate::participants::tally(&ow, &stacks, m, h.files);
                let name = self.session.unit_label(m).to_string();
                let key = book.key_of(table.mod_name(m));
                CardMember {
                    enabled: key.is_some_and(|k| book.enabled(k)),
                    key,
                    name,
                    teeth,
                    wins: t.wins,
                    loses: t.loses,
                    owned: t.owned,
                    missing: t.missing,
                    extra: t.extra,
                }
            })
            .collect();
        Some(ChainCardDetail {
            index: index as u32,
            members,
            items: shown
                .iter()
                .map(|s| crate::context::stack_label(&s.path_strings(&table)))
                .collect(),
            counts,
            items_total: stacks.len(),
            targets_total: h.files as usize,
        })
    }

    // @sat:app.cards.records
    /// Cards of record context `context`.
    pub(crate) fn record_cards(&self, context: usize) -> Option<Vec<ChainCard>> {
        let r = &self.records;
        let cx = r.contexts.get(context)?;
        Some(
            cx.chains
                .iter()
                .enumerate()
                .map(|(i, &c)| {
                    let c = &r.chains[c];
                    let id = sat_core::identity::identity(c.stack.iter().map(String::as_str));
                    ChainCard {
                        index: i as u32,
                        signature: format!("{:016x}", id.signature),
                        done: false,
                        emoji: if c.blind {
                            "⚠️".into()
                        } else {
                            id.emoji.to_string()
                        },
                        hue: id.hue,
                        name: format!(
                            "{} · {}",
                            if c.blind { "вслепую" } else { "поверх" },
                            c.stack.join(" -> ")
                        ),
                        files: c.records,
                        bytes: 0,
                        stack: c.stack.clone(),
                        blind: c.blind,
                        equal: Some(c.equal_to_winner),
                        ..Default::default()
                    }
                })
                .collect(),
        )
    }

    /// Details of card `index` of record context `context`: a tooth per stack of records.
    pub(crate) fn record_card(&self, context: usize, index: usize, limit: usize) -> Option<ChainCardDetail> {
        let r = &self.records;
        let c = &r.chains[*r.contexts.get(context)?.chains.get(index)?];
        let shown = &c.stacks[..c.stacks.len().min(limit)];
        let counts: Vec<u32> = shown.iter().map(|s| s.1).collect();
        let members = c
            .stack_ids
            .iter()
            .enumerate()
            .map(|(m, &p)| {
                let teeth: String = shown.iter().map(|s| s.0.as_bytes()[m] as char).collect();
                let (wins, loses) = tally(&teeth, &counts);
                CardMember {
                    name: r.index.plugins[p as usize].clone(),
                    key: None,
                    enabled: true,
                    teeth,
                    wins,
                    loses,
                    ..Default::default()
                }
            })
            .collect();
        let mut at = 0usize;
        let items = c
            .stacks
            .iter()
            .map(|s| {
                let keys = &c.keys[at..at + s.1 as usize];
                at += s.1 as usize;
                record_stack_label(&r.index, keys)
            })
            .take(shown.len())
            .collect();
        Some(ChainCardDetail {
            index: index as u32,
            members,
            items,
            counts,
            items_total: c.stacks.len(),
            targets_total: c.keys.len(),
        })
    }
}

// @sat:app.cards.drift
/// What changed in a settled card since it was settled: paths and winners against each exact chain's stamp.
fn drift(
    ow: &sat_core::overwrite::OverwriteProjection,
    s: &sat_core::session::Session,
    h: &crate::chains::Head,
) -> Option<String> {
    let d = s.decisions();
    let (mut was, mut now, mut paths, mut winners) = (0u32, 0u32, false, Vec::new());
    for &c in &h.chains {
        let sig = crate::chains::exact_signature(ow, s, c);
        let st = crate::chains::exact_stamp(ow, s, c);
        if let Some(x) = d.drift(sig, &st) {
            was += x.paths.0;
            now += x.paths.1;
            paths |= x.paths_changed;
            if let Some(w) = x.winner_was {
                winners.push((w, st.winner));
            }
        }
    }
    let name = |key: &str| {
        (0..s.table().mod_count())
            .map(ModId::from_idx)
            .find(|&m| s.member_key(m) == key)
            .map_or_else(|| key.to_string(), |m| s.unit_label(m).to_string())
    };
    let mut parts = Vec::new();
    if paths {
        parts.push(if was == now {
            format!("пути другие ({now})")
        } else {
            format!("путей было {was}, стало {now}")
        });
    }
    if let Some((a, b)) = winners.first() {
        parts.push(format!("победитель был {}, теперь {}", name(a), name(b)));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// What a stack of records is: the record (one), or its signatures by count (`REFR 4803, CELL 48`).
pub(crate) fn record_stack_label(ix: &sat_core::records::RecordIndex, keys: &[u64]) -> String {
    let sig = |k: u64| {
        ix.providers(k)
            .last()
            .map(|x| String::from_utf8_lossy(&x.signature).into_owned())
            .unwrap_or_default()
    };
    match keys {
        [k] => format!("{} {}", ix.label(*k), sig(*k)),
        _ => {
            let mut by: std::collections::BTreeMap<String, u32> = Default::default();
            for &k in keys.iter().take(5000) {
                *by.entry(sig(k)).or_default() += 1;
            }
            let mut v: Vec<(String, u32)> = by.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let parts: Vec<String> = v.iter().take(4).map(|(s, n)| format!("{s} {n}")).collect();
            format!("{} … {}", ix.label(keys[0]), parts.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Command, Query, Reply};

    fn mk(name: &str, files: &[&str]) -> sat_core::facts::ModFacts {
        sat_core::facts::ModFacts {
            name: name.into(),
            dirs: vec![sat_core::facts::LooseDir {
                rel: "textures".into(),
                files: files
                    .iter()
                    .map(|f| sat_core::facts::LooseFile {
                        name: f.to_string(),
                        size: 1,
                        vram: 0,
                    })
                    .collect(),
            }],
            ..Default::default()
        }
    }

    fn cards(a: &mut crate::App) -> Vec<super::ChainCard> {
        let key = a.session().book().key_of("A").unwrap();
        let Reply::ChainCards { cards: Some(c) } = a.query(Query::ContextCards { key, last: None }).unwrap()
        else {
            panic!()
        };
        c
    }

    fn card_of<'a>(c: &'a [super::ChainCard], stack: &[&str]) -> &'a super::ChainCard {
        c.iter().find(|k| k.stack == stack).unwrap_or_else(|| {
            panic!(
                "no card {stack:?} in {:?}",
                c.iter().map(|k| &k.stack).collect::<Vec<_>>()
            )
        })
    }

    #[test]
    fn settled_work_follows_changed_mods_and_says_what_changed() {
        let facts = |b: &[&'static str], e: &[&'static str], d: &[&'static str]| {
            vec![
                mk("A", &["x.dds", "y.dds", "w.dds"]),
                mk("B", b),
                mk("C", &["y.dds", "z.dds"]),
                mk("D", d),
                mk("E", e),
            ]
        };
        let mut a = crate::tests::app_from(facts(&["x.dds"], &["q.dds"], &["z.dds"]));
        // settle A → B (x)
        let c = cards(&mut a);
        let x = card_of(&c, &["A", "B"]).signature.clone();
        a.command(Command::SetChainDone {
            signature: x.clone(),
            done: true,
        })
        .unwrap();
        // B gets w too: the same members, other paths — still settled, flagged
        a.session
            .set_facts(std::sync::Arc::new(sat_core::table::ProviderTable::build(
                &facts(&["x.dds", "w.dds"], &["q.dds"], &["z.dds"]),
            )));
        let c = cards(&mut a);
        let k = card_of(&c, &["A", "B"]);
        assert!(k.done);
        assert_eq!(k.changed.as_deref(), Some("путей было 1, стало 2"));
        // settling again confirms
        a.command(Command::SetChainDone {
            signature: k.signature.clone(),
            done: true,
        })
        .unwrap();
        assert_eq!(card_of(&cards(&mut a), &["A", "B"]).changed, None);
        // merge y (A → C) with z (C → D)
        let c = cards(&mut a);
        let (y, z) = (
            card_of(&c, &["A", "C"]).signature.clone(),
            card_of(&c, &["C", "D"]).signature.clone(),
        );
        a.command(Command::MergeChains {
            signatures: vec![y, z],
        })
        .unwrap();
        assert!(cards(&mut a).iter().any(|k| k.sources == 2 && k.user));
        // E now has z too: z is a new chain (C, D, E) — followed, the merge holds
        a.session
            .set_facts(std::sync::Arc::new(sat_core::table::ProviderTable::build(
                &facts(&["x.dds", "w.dds"], &["q.dds", "z.dds"], &["z.dds"]),
            )));
        let c = cards(&mut a);
        let m = c
            .iter()
            .find(|k| k.sources == 2)
            .expect("the merge follows the changed chain");
        assert!(m.user && m.lost == 0);
        assert_eq!(a.orphans(), crate::find::Orphans::default());
        // D and E drop z: the chain is gone — the merge says so, the record is an orphan
        a.session
            .set_facts(std::sync::Arc::new(sat_core::table::ProviderTable::build(
                &facts(&["x.dds", "w.dds"], &["q.dds"], &["w.dds"]),
            )));
        let c = cards(&mut a);
        assert_eq!(card_of(&c, &["A", "C"]).lost, 1);
        assert_eq!(a.orphans().layout, 1);
        a.command(Command::PruneOrphans).unwrap();
        assert_eq!(a.orphans(), crate::find::Orphans::default());
        assert!(a.chain_layout().is_empty(), "a merge of one is no merge");
    }

    #[test]
    fn bridged_chains_sit_together_and_the_user_merges_splits_and_resets() {
        // x: B over A, y: C over A, z: D over C — three winners, bridges A (x–y) and C (y–z)
        let mut a = crate::tests::app_from(vec![
            mk("A", &["x.dds", "y.dds"]),
            mk("B", &["x.dds"]),
            mk("C", &["y.dds", "z.dds"]),
            mk("D", &["z.dds"]),
        ]);
        let c = cards(&mut a);
        let stacks: Vec<Vec<String>> = c.iter().map(|k| k.stack.clone()).collect();
        assert_eq!(
            stacks,
            [vec!["A", "B"], vec!["A", "C"], vec!["C", "D"]],
            "load order, bridged ones together"
        );
        assert_eq!(c[0].link, None);
        assert_eq!(
            c[1].link.as_ref().map(|l| (l.to, l.mods.clone())),
            Some((0, vec!["A".to_string()]))
        );
        assert_eq!(
            c[2].link.as_ref().map(|l| (l.to, l.mods.clone())),
            Some((1, vec!["C".to_string()]))
        );
        assert_eq!(c[1].bridges, ["A", "C"], "lowest first, like the stack");
        // merge the first two
        a.command(Command::MergeChains {
            signatures: vec![c[0].signature.clone(), c[1].signature.clone()],
        })
        .unwrap();
        let c = cards(&mut a);
        assert_eq!(c.len(), 2);
        let m = c.iter().find(|k| k.sources == 2).unwrap();
        assert!(m.user && m.files == 2);
        assert_eq!(
            m.stack,
            ["A", "B", "C"],
            "the merged route by priority, winner last"
        );
        // split it: each exact chain alone, still the user's
        a.command(Command::SplitChain {
            signature: m.signature.clone(),
        })
        .unwrap();
        let c = cards(&mut a);
        assert_eq!(c.len(), 3);
        assert_eq!(c.iter().filter(|k| k.user).count(), 2);
        // back to the system's
        let u = c.iter().find(|k| k.user).unwrap().signature.clone();
        a.command(Command::ResetChain { signature: u }).unwrap();
        assert_eq!(
            cards(&mut a).iter().filter(|k| k.user).count(),
            1,
            "one reset, the other split stays"
        );
        assert!(a.chain_layout().groups().len() == 1);
    }

    #[test]
    fn a_file_context_gives_one_card_per_chain_with_teeth_per_path() {
        let mut a = crate::tests::app_of(["A", "B"]);
        let key = a.session.book().key_of("A").unwrap();
        let Reply::ChainCards { cards: Some(cards) } =
            a.query(Query::ContextCards { key, last: None }).unwrap()
        else {
            panic!()
        };
        assert_eq!(cards.len(), 1);
        assert_eq!((cards[0].index, cards[0].files), (0, 1));
        assert_eq!(cards[0].stack, ["A", "B"], "lowest first, the winner last");
        let Reply::ChainCard { card: Some(d) } = a
            .query(Query::ContextCard {
                key,
                index: 0,
                limit: 10,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(d.items, ["textures/a.dds"]);
        assert_eq!(
            d.members
                .iter()
                .map(|m| (m.name.as_str(), m.teeth.as_str(), m.wins, m.loses))
                .collect::<Vec<_>>(),
            [("A", "l", 0, 1), ("B", "w", 1, 0)]
        );
        assert!(a.context_card(key, 1, 10).is_none());
    }
}
