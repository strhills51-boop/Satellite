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

use crate::App;

/// Heading of one normalized chain.
#[derive(Debug, Clone)]
pub(crate) struct Head {
    pub signature: u64,
    /// Content signatures of its exact chains, sorted.
    pub sources: Vec<u64>,
    pub emoji: String,
    pub hue: u16,
    /// What the chain is about (`@sat:naming.title`), or the user's name for it.
    pub title: String,
    /// The user named, iconed or coloured it (`@sat:naming.manual`).
    pub custom: bool,
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

fn build(ow: &OverwriteProjection, s: &Session, manual: &[sat_core::naming::ManualIdentity]) -> Vec<Head> {
    use sat_core::naming::{ChainFeatures, STRENGTH_DEFAULT};
    let table = s.table();
    let book = s.book();
    let mut heads: Vec<Head> = ow
        .routes()
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
            let labels: Vec<&str> = g.route.iter().map(|&m| s.unit_label(m)).collect();
            let seps: Vec<String> = g
                .route
                .iter()
                .filter_map(|&m| book.key_of(table.mod_name(m)))
                .filter_map(|k| book.separator_of(k))
                .map(|k| book.name(k).trim_end_matches("_separator").to_string())
                .collect();
            let seps: Vec<&str> = seps.iter().map(String::as_str).collect();
            let title = sat_core::naming::title(
                &ChainFeatures {
                    mods: &labels,
                    separators: &seps,
                    paths: &paths,
                    files: g.files,
                },
                STRENGTH_DEFAULT,
            );
            let low = labels[labels.len() - 1];
            let win = labels[0];
            Head {
                signature: h,
                sources,
                emoji: id.emoji.to_string(),
                hue: id.hue,
                name: if title.is_empty() {
                    format!("{low} → {win}")
                } else {
                    format!("{title} · {low} → {win}")
                },
                title,
                custom: false,
                files: g.files,
                bytes: g.bytes,
                route: g.route.clone(),
                chains: g.chains.clone(),
            }
        })
        .collect();
    // equal generated names told apart; then the user's identities, carried over by overlap
    let mut names: Vec<String> = heads.iter().map(|h| h.name.clone()).collect();
    let sigs: Vec<u64> = heads.iter().map(|h| h.signature).collect();
    sat_core::naming::make_unique(&mut names, &sigs);
    for (h, n) in heads.iter_mut().zip(names) {
        h.name = n;
    }
    let srcs: Vec<&[u64]> = heads.iter().map(|h| h.sources.as_slice()).collect();
    let given = sat_core::naming::inherit(&srcs, manual);
    for (h, m) in heads.iter_mut().zip(given) {
        let Some(m) = m.map(|i| &manual[i]) else { continue };
        h.custom = true;
        if let Some(n) = &m.name {
            h.title = n.clone();
            h.name = n.clone();
        }
        if let Some(e) = &m.emoji {
            h.emoji = e.clone();
        }
        if let Some(hue) = m.hue {
            h.hue = hue % 360;
        }
    }
    heads
}

/// Headings with what they were built from: (overwrite computation, member-keys revision, user identities revision).
pub(crate) type Cached = ((u64, u64, u64), Arc<Vec<Head>>);
/// What mods lose as equal copies, with what it was built from: (overwrite computation, hashes revision).
pub(crate) type CachedContent = ((u64, u64), Arc<Vec<sat_core::identical::ModContent>>);

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
        let version = (
            self.session.computed("overwrite"),
            self.session.member_keys_rev(),
            self.chain_identities_rev,
        );
        if let Some((v, h)) = &self.heads
            && *v == version
        {
            return h.clone();
        }
        let h = Arc::new(build(&ow, &self.session, &self.chain_identities));
        self.heads = Some((version, h.clone()));
        h
    }

    // @sat:app.content
    /// Content hashes of the current table's providers (`@sat:scan.content-hash.table`); hashes of another table
    /// are dropped.
    pub fn set_content_hashes(&mut self, hashes: sat_core::identical::ContentHashes) {
        self.content_hashes = Arc::new(hashes.for_table(self.session.table()));
        self.content_rev += 1;
    }

    /// What each mod loses as copies equal to the winner, once per overwrite computation and hashes revision;
    /// `None` without hashes (nothing was compared — not "nothing is equal").
    pub(crate) fn mod_content(&mut self) -> Option<Arc<Vec<sat_core::identical::ModContent>>> {
        if self.content_hashes.0.is_empty() {
            return None;
        }
        let ow = self.session.overwrite();
        let version = (self.session.computed("overwrite"), self.content_rev);
        if let Some((v, c)) = &self.mod_content
            && *v == version
        {
            return Some(c.clone());
        }
        let c = Arc::new(sat_core::identical::mod_content(
            self.session.table(),
            &self.content_hashes,
            ow.priorities(),
        ));
        self.mod_content = Some((version, c.clone()));
        Some(c)
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

/// The content verdict of a normalized chain over its exact chains (`@sat:identical.chain`); `None` without hashes.
pub(crate) fn chain_content(
    table: &sat_core::table::ProviderTable,
    hashes: &sat_core::identical::ContentHashes,
    ow: &OverwriteProjection,
    chains: &[sat_core::ids::ChainId],
) -> Option<sat_core::identical::ChainContent> {
    use sat_core::identical::{ChainContent, Verdict};
    if hashes.0.is_empty() {
        return None;
    }
    let parts: Vec<ChainContent> = chains
        .iter()
        .map(|&c| sat_core::identical::chain_content(table, hashes, ow, c))
        .collect();
    let has = |v: Verdict| parts.iter().any(|p| p.verdict == v);
    let verdict = if has(Verdict::Unknown) && !has(Verdict::Different) && !has(Verdict::Partly) {
        Verdict::Unknown
    } else if parts.iter().all(|p| p.verdict == Verdict::Identical) {
        Verdict::Identical
    } else if parts.iter().all(|p| p.verdict == Verdict::Different) {
        Verdict::Different
    } else {
        Verdict::Partly
    };
    Some(ChainContent {
        verdict,
        equal_files: parts.iter().map(|p| p.equal_files).sum(),
        equal_bytes: parts.iter().map(|p| p.equal_bytes).sum(),
        unknown_paths: parts.iter().map(|p| p.unknown_paths).sum(),
    })
}
