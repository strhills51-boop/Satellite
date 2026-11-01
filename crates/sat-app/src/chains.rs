//! What the window shows of a normalized chain: icon, colour, generated name, settled or not. @sat:app.chains
//!
//! Owns: the headings of the normalized chains (`@sat:routes`) of the current projection, rebuilt only when the
//! overwrite projection is recomputed;
//! a mod's view of them (its files and place in each); "settled" of a normalized chain from its exact chains.
//! Not here: grouping exact chains (`@sat:routes`), decisions (`@sat:decisions`).
//!
//! Decisions stay on exact chains (content signatures that survive reorders and re-scans). A normalized chain is
//! settled when every exact chain in it is (monolith v502); settling it settles them all. Its own signature is the
//! hash of the sorted exact signatures, so it changes when an exact chain joins or leaves — the user looks again.

use std::sync::Arc;

use sat_core::ids::{ChainId, ModId};
use sat_core::overwrite::OverwriteProjection;
use sat_core::session::Session;

use crate::{App, chain_title};

/// Heading of one normalized chain.
#[derive(Debug, Clone)]
pub(crate) struct Head {
    pub signature: u64,
    /// Content signatures of its exact chains, sorted.
    pub sources: Vec<u64>,
    pub emoji: &'static str,
    pub hue: u16,
    pub title: String,
    /// `title · lowest → winner` (monolith chain names).
    pub name: String,
    pub files: u32,
    pub bytes: u64,
    /// Super-route, winner first.
    pub route: Vec<ModId>,
    pub chains: Vec<ChainId>,
}

impl Head {
    pub fn done(&self, d: &sat_core::decisions::ChainDecisions) -> bool {
        !self.sources.is_empty() && self.sources.iter().all(|&s| d.is_done(s))
    }
    pub fn hex(&self) -> String {
        format!("{:016x}", self.signature)
    }
}

/// Content signature of an exact chain: over the members' stable keys (`@sat:session.member-keys`).
pub(crate) fn exact_signature(ow: &OverwriteProjection, s: &Session, c: ChainId) -> u64 {
    sat_core::identity::identity(ow.chain_mods(c).iter().map(|&x| s.member_key(x))).signature
}

fn build(ow: &OverwriteProjection, s: &Session) -> Vec<Head> {
    let table = s.table();
    ow.routes()
        .groups()
        .iter()
        .map(|g| {
            let mut sources: Vec<u64> = g.chains.iter().map(|&c| exact_signature(ow, s, c)).collect();
            sources.sort_unstable();
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for s in &sources {
                for b in s.to_le_bytes() {
                    h ^= u64::from(b);
                    h = h.wrapping_mul(0x0100_0000_01b3);
                }
            }
            let id = sat_core::identity::identity(g.route.iter().map(|&x| s.member_key(x)));
            let mut paths: Vec<String> = Vec::new();
            for &c in &g.chains {
                if paths.len() >= 64 {
                    break;
                }
                paths.extend(
                    ow.chain_paths(c)
                        .iter()
                        .take(64 - paths.len())
                        .map(|&p| table.path_string(p)),
                );
            }
            let title = chain_title(&paths);
            let low = table.mod_name(g.route[g.route.len() - 1]);
            let win = table.mod_name(g.route[0]);
            Head {
                signature: h,
                sources,
                emoji: id.emoji,
                hue: id.hue,
                name: if title.is_empty() {
                    format!("{low} → {win}")
                } else {
                    format!("{title} · {low} → {win}")
                },
                title,
                files: g.files,
                bytes: g.bytes,
                route: g.route.clone(),
                chains: g.chains.clone(),
            }
        })
        .collect()
}

/// Headings with what they were built from: (overwrite computation, member-keys revision).
pub(crate) type Cached = ((u64, u64), Arc<Vec<Head>>);

/// A mod's part in one normalized chain.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Part {
    pub group: u32,
    /// Files of the mod's own exact chains in the group.
    pub files: u32,
    /// Place in the super-route (0 = the winner).
    pub place: usize,
}

impl App {
    // @sat:app.chains.heads
    /// Headings of the normalized chains, built once per overwrite computation: cursor moves, colours and
    /// decisions do not rebuild them (settled-or-not is read at use).
    pub(crate) fn heads(&mut self) -> Arc<Vec<Head>> {
        let ow = self.session.overwrite();
        let version = (self.session.computed("overwrite"), self.session.member_keys_rev());
        if let Some((v, h)) = &self.heads
            && *v == version
        {
            return h.clone();
        }
        let h = Arc::new(build(&ow, &self.session));
        self.heads = Some((version, h.clone()));
        h
    }
}

/// Normalized chains of mod `m`, larger first (by the mod's own files in them).
pub(crate) fn parts(ow: &OverwriteProjection, heads: &[Head], m: ModId) -> Vec<Part> {
    let routes = ow.routes();
    let mut out: Vec<Part> = Vec::new();
    for c in ow.mod_chains(m) {
        let Some(g) = routes.of_chain(c) else { continue };
        let files = ow.chain(c).files;
        match out.iter_mut().find(|p| p.group == g) {
            Some(p) => p.files += files,
            None => out.push(Part {
                group: g,
                files,
                place: heads[g as usize].route.iter().position(|&x| x == m).unwrap_or(0),
            }),
        }
    }
    out.sort_by_key(|p| (std::cmp::Reverse(p.files), p.group));
    out
}
