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

use sat_core::order::Key;

use crate::App;

/// One chain of a context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Records: a later plugin overrides an earlier one without having it as a master.
    #[serde(default)]
    pub blind: bool,
    /// Losing copies equal to the winner's (files: when hashes are known; records: always).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equal: Option<u32>,
}

/// A member of a chain with one tooth per path or record of the chain (in the details' order).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CardMember {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<Key>,
    pub enabled: bool,
    /// `w` wins, `l` loses, `s` equal to the winner, `o` the record's own plugin, `-` not there.
    pub teeth: String,
    pub wins: u32,
    pub loses: u32,
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
    /// Cards of the file context of mod `key`.
    pub(crate) fn context_cards(&mut self, key: Key) -> Option<Vec<ChainCard>> {
        let cx = self.context_of(key)?;
        let ow = self.session.overwrite();
        let table = self.session.table().clone();
        let heads = self.heads();
        let decisions = self.session.decisions();
        let groups = crate::context::context_groups(&ow, &heads, cx);
        Some(
            groups
                .iter()
                .enumerate()
                .map(|(i, &g)| {
                    let h = &heads[g as usize];
                    ChainCard {
                        index: i as u32,
                        signature: h.hex(),
                        done: h.done(decisions),
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
                        blind: false,
                        equal: crate::chains::chain_content(&table, &self.content_hashes, &ow, &h.chains)
                            .map(|c| c.equal_files),
                    }
                })
                .collect(),
        )
    }

    /// Details of card `index` of the file context of mod `key`.
    pub(crate) fn context_card(&mut self, key: Key, index: usize, limit: usize) -> Option<ChainCardDetail> {
        let cx = self.context_of(key)?;
        let ow = self.session.overwrite();
        let table = self.session.table().clone();
        let heads = self.heads();
        let g = *crate::context::context_groups(&ow, &heads, cx).get(index)?;
        let h = &heads[g as usize];
        // a tooth per stack: an exact chain's paths go through the chain the same way
        let stacks = crate::context::group_stacks(&ow, h);
        let shown = &stacks[..stacks.len().min(limit)];
        let counts: Vec<u32> = shown.iter().map(|&c| ow.chain(c).files).collect();
        let book = self.session.book();
        let members = h
            .route
            .iter()
            .rev()
            .map(|&m| {
                let teeth: String = shown
                    .iter()
                    .map(|&c| match ow.chain_mods(c).iter().position(|&x| x == m) {
                        Some(0) => 'w',
                        Some(_) => 'l',
                        None => '-',
                    })
                    .collect();
                let (wins, loses) = tally(&teeth, &counts);
                let name = self.session.unit_label(m).to_string();
                let key = book.key_of(table.mod_name(m));
                CardMember {
                    enabled: key.is_some_and(|k| book.enabled(k)),
                    key,
                    name,
                    teeth,
                    wins,
                    loses,
                }
            })
            .collect();
        Some(ChainCardDetail {
            index: index as u32,
            members,
            items: shown
                .iter()
                .map(|&c| crate::context::stack_label(&crate::context::stack_paths(&ow, &table, c)))
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
    use crate::{Query, Reply};

    #[test]
    fn a_file_context_gives_one_card_per_chain_with_teeth_per_path() {
        let mut a = crate::tests::app_of(["A", "B"]);
        let key = a.session.book().key_of("A").unwrap();
        let Reply::ChainCards { cards: Some(cards) } = a.query(Query::ContextCards { key }).unwrap() else {
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
