//! Conflict domains in the window: the list (tabs), cards and table over any domain (ADR 0018 §2). @sat:app.domains
//!
//! Owns: which domains there are (files and records — the specialized adapters —, then constraints, archives,
//! dangerous records, record fields, FaceGen, the game pack's specs and the user's), computing each when its inputs
//! change, and the generic projections: contexts, chain cards with their teeth, the context table window.
//! Not here: the domains themselves (`@sat:conflict`, `@sat:conflict.kinds`, `@sat:conflict.fields`,
//! `@sat:conflict.spec`), the file and record projections (`@sat:app.cards`, `@sat:app.context-table`,
//! `@sat:app.records`), reading files (the shell hands a reader over).
//!
//! A new domain shows up here by being in the list: the page builds its tabs from it and asks the same queries.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use sat_core::conflict::{Centrality, DomainInfo, DomainReport, Family, Origin, Semantics, Severity, cell};
use sat_core::conflict_spec::DomainSpec;

use crate::App;
use crate::cards::{CardMember, ChainCard, ChainCardDetail};
use crate::context::{ContextTable, TableChain, TableCol, TableRow};

/// Reads a loose file of a mod: (mod name, path inside the mod).
pub type FileReader = Arc<dyn Fn(&str, &str) -> Option<Vec<u8>> + Send + Sync>;

/// Where a domain is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainState {
    /// The shell does not compute its inputs (records not read).
    Off,
    /// Its inputs are being read.
    Pending,
    Ready,
}

/// A domain in the list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainItem {
    pub id: String,
    pub title: String,
    pub about: String,
    pub family: Family,
    pub centrality: Centrality,
    pub semantics: Semantics,
    pub origin: Origin,
    pub state: DomainState,
    /// Targets with a case, and those wanting a decision.
    pub targets: u32,
    pub unresolved: u32,
    pub kinds: Vec<(String, u32)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// A context of a generic domain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainContextItem {
    pub name: String,
    pub hue: u16,
    pub participants: usize,
    pub chains: usize,
    pub targets: u32,
    pub unresolved: u32,
    pub severity: Severity,
    /// Its participants (lower case), so one finds its context.
    pub members: Vec<String>,
}

/// Domains computed from the session (constraints, archives, records-derived, specs), per input version.
#[derive(Debug, Default)]
pub(crate) struct DomainCache {
    version: Option<(u64, u64, usize, u64, u64)>,
    reports: Vec<Arc<DomainReport>>,
    /// Per mod (lower case): opponents and cases, for the same version.
    relations: Option<Arc<std::collections::HashMap<String, sat_core::metrics::ModRelations>>>,
    /// What each kind looks at, per table and specs (`@sat:conflict.kinds.candidates`).
    candidates: Option<(
        Arc<sat_core::table::ProviderTable>,
        u64,
        Arc<sat_core::conflict_kinds::Candidates>,
    )>,
}

/// Inputs the shell hands over.
#[derive(Default)]
pub(crate) struct DomainInputs {
    pub reader: Option<FileReader>,
    pub user_specs: Vec<DomainSpec>,
    pub fields: Option<Arc<sat_core::conflict_fields::FieldFacts>>,
    pub rev: u64,
    /// Bumped with every new record report.
    pub records_rev: u64,
    /// The game's version and folder (`@sat:conflict.kinds.runtime`).
    pub runtime: sat_core::conflict_kinds::GameRuntime,
}

impl std::fmt::Debug for DomainInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DomainInputs")
            .field("reader", &self.reader.is_some())
            .field("user_specs", &self.user_specs.len())
            .field("fields", &self.fields.as_ref().map(|x| x.0.len()))
            .field("rev", &self.rev)
            .finish()
    }
}

fn severity_emoji(s: Severity) -> &'static str {
    match s {
        Severity::Error => "⛔",
        Severity::Warn => "⚠️",
        Severity::Info => "",
    }
}

impl App {
    /// The shell's file reader (for spec domains that rank by a field).
    pub fn set_file_reader(&mut self, reader: FileReader) {
        self.domain_inputs.reader = Some(reader);
        self.domain_inputs.rev += 1;
    }

    /// What the shell found in the game's folder: its version and file names.
    pub fn set_game_runtime(&mut self, rt: sat_core::conflict_kinds::GameRuntime) {
        self.domain_inputs.runtime = rt;
        self.domain_inputs.rev += 1;
    }

    /// The user's own conflict domains (`conflict-domains.json`).
    pub fn set_user_domains(&mut self, specs: Vec<DomainSpec>) {
        self.domain_inputs.user_specs = specs;
        self.domain_inputs.rev += 1;
    }

    /// Fields of the conflict records, read by the shell after the record index (`@sat:scan.records.fields`).
    pub fn set_record_fields(&mut self, facts: sat_core::conflict_fields::FieldFacts) {
        self.domain_inputs.fields = Some(Arc::new(facts));
        self.domain_inputs.rev += 1;
    }

    // @sat:app.domains.compute
    /// The computed domains, recomputed when the plugins, the overwrite, the records or the inputs change.
    pub(crate) fn domain_reports(&mut self) -> Vec<Arc<DomainReport>> {
        let list = self.session.plugins();
        let ow = self.session.overwrite();
        let version = (
            self.session.computed("plugins"),
            self.session.computed("overwrite"),
            self.domain_inputs.records_rev as usize,
            self.domain_inputs.rev,
            u64::from(self.record_state == crate::records::RecordState::Ready),
        );
        if self.domain_cache.version == Some(version) {
            return self.domain_cache.reports.clone();
        }
        let pack = sat_core::game::active();
        let table = self.session.table().clone();
        let prio = ow.priorities();
        let specs: Vec<(&DomainSpec, Origin)> = pack
            .domains
            .iter()
            .map(|s| (s, Origin::Pack))
            .chain(self.domain_inputs.user_specs.iter().map(|s| (s, Origin::User)))
            .collect();
        let cand = match &self.domain_cache.candidates {
            Some((t, rev, c)) if Arc::ptr_eq(t, &table) && *rev == self.domain_inputs.rev => c.clone(),
            _ => {
                let refs: Vec<&DomainSpec> = specs.iter().map(|x| x.0).collect();
                Arc::new(sat_core::conflict_kinds::candidates(&table, pack, &refs))
            }
        };
        let mut out = vec![
            Arc::new(sat_core::conflict_kinds::constraints_with(
                &list,
                (self.record_state == crate::records::RecordState::Ready)
                    .then_some(self.records.index.as_ref()),
                pack,
            )),
            Arc::new(sat_core::conflict_kinds::generations(&cand, &table, prio, pack)),
            Arc::new(sat_core::conflict_kinds::runtime(
                &self.domain_inputs.runtime,
                &table,
                prio,
                pack,
            )),
            Arc::new(sat_core::conflict_kinds::archives_in(
                &cand.archives,
                &table,
                prio,
                &list,
                pack,
            )),
        ];
        if self.record_state == crate::records::RecordState::Ready {
            let ix = &self.records.index;
            if let Some(rules) = &pack.records {
                out.push(Arc::new(sat_core::conflict_kinds::dangerous(ix, rules)));
            }
            if let Some(f) = &self.domain_inputs.fields {
                out.push(Arc::new(sat_core::conflict_fields::classify(ix, f, &[])));
            }
            out.push(Arc::new(sat_core::conflict_kinds::facegen_in(
                &cand.facegen,
                ix,
                &table,
                prio,
                &list,
                pack,
            )));
        }
        let names = table.mod_names().to_vec();
        let reader = self.domain_inputs.reader.clone();
        let read = |i: usize| -> Option<Vec<u8>> {
            if table.is_archive(i) {
                return None;
            }
            let r = reader.as_ref()?;
            r(
                &names[table.owner(i).idx()],
                &table.path_string(table.path_of_provider(i)),
            )
        };
        for (k, (spec, origin)) in specs.iter().enumerate() {
            out.push(Arc::new(sat_core::conflict_spec::evaluate_matched(
                spec,
                *origin,
                &table,
                prio,
                &read,
                &cand.specs[k],
            )));
        }
        self.domain_cache = DomainCache {
            version: Some(version),
            reports: out.clone(),
            relations: None,
            candidates: Some((table.clone(), self.domain_inputs.rev, cand)),
        };
        out
    }

    // @sat:app.domains.relations
    /// Each mod among the others (lower-case name): opponents in overwrite chains, cases in the other domains (a
    /// plugin's case counts for the mod the plugin loads from).
    pub(crate) fn mod_relations(
        &mut self,
    ) -> Arc<std::collections::HashMap<String, sat_core::metrics::ModRelations>> {
        let reports = self.domain_reports();
        if let Some(r) = &self.domain_cache.relations {
            return r.clone();
        }
        let ow = self.session.overwrite();
        let list = self.session.plugins();
        let table = self.session.table().clone();
        let owner_of = |p: &sat_core::conflict::Participant| -> Option<String> {
            p.owner.clone().or_else(|| {
                let pl = list.get(&p.label)?;
                pl.winner
                    .map(|w| table.mod_name(pl.versions[w].owner).to_string())
            })
        };
        let refs: Vec<&DomainReport> = reports.iter().map(|r| r.as_ref()).collect();
        let cases = sat_core::conflict::owner_cases(&refs, owner_of);
        let mut out: std::collections::HashMap<String, sat_core::metrics::ModRelations> = Default::default();
        for (m, n) in ow.opponents().into_iter().enumerate() {
            let name = table.mod_name(sat_core::ids::ModId::from_idx(m)).to_lowercase();
            out.insert(
                name,
                sat_core::metrics::ModRelations {
                    opponents: n,
                    cases: Some(0),
                    unresolved: Some(0),
                },
            );
        }
        for (name, (c, u)) in cases {
            let e = out.entry(name).or_default();
            e.cases = Some(c);
            e.unresolved = Some(u);
        }
        let out = Arc::new(out);
        self.domain_cache.relations = Some(out.clone());
        out
    }

    fn domain(&mut self, id: &str) -> Option<Arc<DomainReport>> {
        self.domain_reports().into_iter().find(|d| d.info.id == id)
    }

    // @sat:app.domains.list
    /// Every domain: files and records first, then the rest in their order.
    pub(crate) fn conflict_domains(&mut self) -> Vec<DomainItem> {
        let ow = self.session.overwrite();
        let chains = ow.chain_ids();
        let files: u32 = chains.iter().map(|&c| ow.chain(c).files).sum();
        let mut out = vec![DomainItem {
            id: "files".into(),
            title: "Файлы".into(),
            about: "Перезапись файлов: один путь у нескольких модов, игра берёт копию мода с большим приоритетом.".into(),
            family: Family::Collision,
            centrality: Centrality::ModOrder,
            semantics: Semantics::LastWins,
            origin: Origin::Builtin,
            state: DomainState::Ready,
            targets: files,
            unresolved: files,
            kinds: Vec::new(),
            notes: Vec::new(),
        }];
        let r = &self.records;
        out.push(DomainItem {
            id: "records".into(),
            title: "Записи плагинов".into(),
            about: "Перезапись записей: одну запись правят несколько плагинов, игра берёт её целиком у последнего по порядку загрузки.".into(),
            family: Family::Collision,
            centrality: Centrality::PluginOrder,
            semantics: Semantics::LastWins,
            origin: Origin::Builtin,
            state: match self.record_state {
                crate::records::RecordState::Off => DomainState::Off,
                crate::records::RecordState::Pending => DomainState::Pending,
                crate::records::RecordState::Ready => DomainState::Ready,
            },
            targets: r.overridden as u32,
            unresolved: r.conflicts as u32,
            kinds: vec![
                ("правка".into(), (r.overridden - r.conflicts) as u32),
                ("конфликт".into(), r.conflicts as u32),
            ],
            notes: Vec::new(),
        });
        let reports = self.domain_reports();
        let records_state = out[1].state;
        for d in &reports {
            out.push(item(d, DomainState::Ready));
        }
        // record-derived domains not computed yet say why
        if records_state != DomainState::Ready {
            for (id, title) in [
                ("dangerous", "Опасные записи"),
                ("fields", "Поля записей"),
                ("facegen", "Лица NPC"),
            ] {
                out.push(DomainItem {
                    state: records_state,
                    ..item(&DomainReport::empty(placeholder(id, title)), records_state)
                });
            }
        } else if self.domain_inputs.fields.is_none() {
            out.push(item(
                &DomainReport::empty(placeholder("fields", "Поля записей")),
                DomainState::Pending,
            ));
        }
        // the order: files, records, then by kind of trouble
        let rank = |id: &str| match id {
            "files" => 0,
            "records" => 1,
            "fields" => 2,
            "constraints" => 3,
            "dangerous" => 4,
            "archives" => 5,
            "facegen" => 6,
            "runtime" => 4,
            "generations" => 7,
            _ => 8,
        };
        out.sort_by_key(|d| rank(&d.id));
        out
    }

    // @sat:app.domains.contexts
    pub(crate) fn domain_contexts(&mut self, id: &str) -> Option<Vec<DomainContextItem>> {
        let d = self.domain(id)?;
        Some(
            d.contexts
                .iter()
                .map(|cx| {
                    let labels: Vec<&str> = cx
                        .participants
                        .iter()
                        .map(|&p| d.participants[p as usize].label.as_str())
                        .collect();
                    let name = match labels.as_slice() {
                        [] => String::new(),
                        [a] => (*a).to_string(),
                        [a, .., b] => format!("{a} -> {b} ({})", labels.len()),
                    };
                    let kind = d.chains[cx.chains[0] as usize].kind.clone();
                    DomainContextItem {
                        name: if kind.is_empty() {
                            name
                        } else {
                            format!("{kind} · {name}")
                        },
                        hue: sat_core::identity::identity(labels.iter().copied()).hue,
                        participants: labels.len(),
                        chains: cx.chains.len(),
                        targets: cx.targets,
                        unresolved: cx.unresolved,
                        severity: cx.severity,
                        members: labels.iter().map(|l| l.to_lowercase()).collect(),
                    }
                })
                .collect(),
        )
    }

    fn chain_card(d: &DomainReport, index: usize, c: &sat_core::conflict::Chain) -> ChainCard {
        let labels: Vec<&str> = c
            .stack
            .iter()
            .map(|&p| d.participants[p as usize].label.as_str())
            .collect();
        let id = sat_core::identity::identity(labels.iter().copied().chain(std::iter::once(c.kind.as_str())));
        let e = severity_emoji(c.severity);
        ChainCard {
            index: index as u32,
            signature: format!("{:016x}", id.signature),
            done: false,
            emoji: if e.is_empty() {
                id.emoji.to_string()
            } else {
                e.to_string()
            },
            hue: id.hue,
            name: d.chain_name(c),
            files: c.targets.len() as u32,
            bytes: 0,
            stack: labels.iter().map(|s| s.to_string()).collect(),
            blind: false,
            equal: None,
        }
    }

    // @sat:app.domains.cards
    pub(crate) fn domain_cards(&mut self, id: &str, context: usize) -> Option<Vec<ChainCard>> {
        let d = self.domain(id)?;
        let cx = d.contexts.get(context)?;
        Some(
            cx.chains
                .iter()
                .enumerate()
                .map(|(i, &c)| Self::chain_card(&d, i, &d.chains[c as usize]))
                .collect(),
        )
    }

    pub(crate) fn domain_card(
        &mut self,
        id: &str,
        context: usize,
        index: usize,
        limit: usize,
    ) -> Option<ChainCardDetail> {
        let d = self.domain(id)?;
        let c = &d.chains[*d.contexts.get(context)?.chains.get(index)? as usize];
        let n = c.targets.len().min(limit);
        let book = self.session.book();
        let members = c
            .stack
            .iter()
            .enumerate()
            .map(|(m, &p)| {
                let teeth: String = c.row(m)[..n].iter().map(|&x| x as char).collect();
                let wins = teeth.bytes().filter(|&b| b == cell::WIN).count() as u32;
                let loses = teeth
                    .bytes()
                    .filter(|&b| matches!(b, cell::LOSE | cell::SAME | cell::TIE | cell::BREAKS))
                    .count() as u32;
                let part = &d.participants[p as usize];
                let key = part.owner.as_deref().and_then(|o| book.key_of(o));
                CardMember {
                    name: part.label.clone(),
                    enabled: key.is_none_or(|k| book.enabled(k)),
                    key,
                    teeth,
                    wins,
                    loses,
                }
            })
            .collect();
        Some(ChainCardDetail {
            index: index as u32,
            members,
            items: c.targets[..n]
                .iter()
                .map(|&t| d.targets[t as usize].clone())
                .collect(),
            items_total: c.targets.len(),
        })
    }

    // @sat:app.domains.table
    /// A window of a generic domain's context table: rows — the context's participants by rank, grouped by their
    /// owner when owners differ from them; columns — targets by chain.
    pub(crate) fn domain_table(
        &mut self,
        id: &str,
        context: usize,
        rows: (usize, usize),
        cols: (usize, usize),
    ) -> Option<ContextTable> {
        let d = self.domain(id)?;
        let cx = d.contexts.get(context)?;
        let items = self.domain_contexts(id)?;
        let meta = &items[context];
        // columns: (chain position, member index by participant, target position in chain)
        let mut all_cols: Vec<(u32, usize)> = Vec::new();
        let mut chain_starts = Vec::new();
        for (ci, &c) in cx.chains.iter().enumerate() {
            chain_starts.push(all_cols.len() as u32);
            for t in 0..d.chains[c as usize].targets.len() {
                all_cols.push((ci as u32, t));
            }
        }
        // rows: participants, grouped by owner when that says more
        let grouped = cx.participants.iter().any(|&p| {
            d.participants[p as usize]
                .owner
                .as_deref()
                .is_some_and(|o| o != d.participants[p as usize].label)
        });
        let mut row_items: Vec<Option<u32>> = Vec::new();
        let mut last_owner: Option<&str> = None;
        let mut parts = cx.participants.clone();
        if grouped {
            parts.sort_by(|&a, &b| {
                let (pa, pb) = (&d.participants[a as usize], &d.participants[b as usize]);
                pa.owner.cmp(&pb.owner).then(pa.rank.cmp(&pb.rank))
            });
        }
        for &p in &parts {
            let o = d.participants[p as usize].owner.as_deref();
            if grouped && o != last_owner {
                row_items.push(None);
                last_owner = o;
            }
            row_items.push(Some(p));
        }
        let col_window = &all_cols[cols.0.min(all_cols.len())..(cols.0 + cols.1).min(all_cols.len())];
        let mut chains_in: Vec<u32> = col_window.iter().map(|c| c.0).collect();
        chains_in.dedup();
        let local = |ci: u32| chains_in.iter().position(|&x| x == ci).unwrap_or(0) as u32;
        let book = self.session.book();
        let mut header_owner: Vec<Option<String>> = Vec::new();
        {
            let mut cur = None;
            for r in &row_items {
                match r {
                    None => cur = None,
                    Some(p) if cur.is_none() => cur = d.participants[*p as usize].owner.clone(),
                    _ => {}
                }
                header_owner.push(cur.clone());
            }
        }
        let rows_out: Vec<TableRow> = row_items
            .iter()
            .enumerate()
            .skip(rows.0)
            .take(rows.1)
            .map(|(ri, r)| match r {
                None => {
                    let owner = row_items[ri + 1..]
                        .iter()
                        .find_map(|x| x.map(|p| d.participants[p as usize].owner.clone()))
                        .flatten()
                        .unwrap_or_default();
                    TableRow {
                        header: true,
                        key: book.key_of(&owner),
                        name: owner,
                        pos: None,
                        separator: None,
                        enabled: true,
                        wins: 0,
                        loses: 0,
                        cells: String::new(),
                    }
                }
                Some(p) => {
                    let part = &d.participants[*p as usize];
                    let member_in = |c: &sat_core::conflict::Chain| c.stack.iter().position(|&x| x == *p);
                    let (mut wins, mut loses) = (0u32, 0u32);
                    for &c in &cx.chains {
                        let ch = &d.chains[c as usize];
                        if let Some(m) = member_in(ch) {
                            for &x in ch.row(m) {
                                match x {
                                    cell::WIN => wins += 1,
                                    cell::LOSE | cell::SAME | cell::TIE | cell::BREAKS => loses += 1,
                                    _ => {}
                                }
                            }
                        }
                    }
                    let cells: String = col_window
                        .iter()
                        .map(|&(ci, t)| {
                            let ch = &d.chains[cx.chains[ci as usize] as usize];
                            member_in(ch).map_or('-', |m| ch.cell(m, t) as char)
                        })
                        .collect();
                    let key = book
                        .key_of(&part.label)
                        .or_else(|| part.owner.as_deref().and_then(|o| book.key_of(o)));
                    TableRow {
                        header: false,
                        key,
                        name: part.label.clone(),
                        pos: Some(part.rank.max(0) as u32),
                        separator: header_owner[ri].clone(),
                        enabled: key.is_none_or(|k| book.enabled(k)),
                        wins,
                        loses,
                        cells,
                    }
                }
            })
            .collect();
        let chains_out: Vec<TableChain> = chains_in
            .iter()
            .map(|&ci| {
                let c = &d.chains[cx.chains[ci as usize] as usize];
                let card = Self::chain_card(&d, ci as usize, c);
                TableChain {
                    index: ci,
                    signature: card.signature,
                    done: false,
                    emoji: card.emoji,
                    hue: card.hue,
                    name: card.name,
                    files: card.files,
                    depth: c.stack.len(),
                }
            })
            .collect();
        Some(ContextTable {
            name: meta.name.clone(),
            hue: meta.hue,
            rows_total: row_items.len(),
            cols_total: all_cols.len(),
            chains: chains_out,
            chains_total: cx.chains.len(),
            chain_starts,
            rows: rows_out,
            cols: col_window
                .iter()
                .map(|&(ci, t)| {
                    let c = &d.chains[cx.chains[ci as usize] as usize];
                    TableCol {
                        path: d.targets[c.targets[t] as usize].clone(),
                        chain: local(ci),
                    }
                })
                .collect(),
        })
    }
}

fn placeholder(id: &str, title: &str) -> DomainInfo {
    DomainInfo {
        id: id.into(),
        title: title.into(),
        about: "Считается из записей плагинов: появится, когда плагины прочитаны.".into(),
        family: Family::Collision,
        centrality: Centrality::PluginOrder,
        semantics: Semantics::LastWins,
        origin: Origin::Builtin,
    }
}

fn item(d: &DomainReport, state: DomainState) -> DomainItem {
    DomainItem {
        id: d.info.id.clone(),
        title: d.info.title.clone(),
        about: d.info.about.clone(),
        family: d.info.family,
        centrality: d.info.centrality,
        semantics: d.info.semantics,
        origin: d.info.origin,
        state,
        targets: d.counts.targets,
        unresolved: d.counts.unresolved,
        kinds: d.counts.kinds.clone(),
        notes: d.notes.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Query, Reply};

    fn files(name: &str, paths: &[&str]) -> sat_core::facts::ModFacts {
        let mut f = sat_core::facts::ModFacts {
            name: name.into(),
            ..Default::default()
        };
        for p in paths {
            let (dir, file) = p.rsplit_once('/').unwrap();
            f.dirs.push(sat_core::facts::LooseDir {
                rel: dir.into(),
                files: vec![sat_core::facts::LooseFile {
                    name: file.into(),
                    size: 1,
                    vram: 0,
                }],
            });
        }
        f
    }

    #[test]
    fn a_pack_spec_domain_shows_in_the_list_and_in_both_projections() {
        let base = "meshes/actors/character/animations/OpenAnimationReplacer";
        let mut a = crate::tests::app_from(vec![
            files(
                "Low",
                &[
                    &format!("{base}/P/A/mt_idle.hkx"),
                    &format!("{base}/P/A/config.json"),
                ],
            ),
            files(
                "High",
                &[
                    &format!("{base}/Q/B/mt_idle.hkx"),
                    &format!("{base}/Q/B/config.json"),
                ],
            ),
        ]);
        a.set_file_reader(Arc::new(|m: &str, _p: &str| {
            Some(format!("{{\"priority\": {}}}", if m == "Low" { 9 } else { 1 }).into_bytes())
        }));
        let Reply::Domains { domains } = a.query(Query::ConflictDomains).unwrap() else {
            panic!()
        };
        let ids: Vec<&str> = domains.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(&ids[..2], ["files", "records"]);
        assert!(ids.contains(&"constraints") && ids.contains(&"archives"));
        let oar = domains.iter().find(|d| d.id == "oar").unwrap();
        assert_eq!((oar.origin, oar.targets, oar.unresolved), (Origin::Pack, 1, 0));
        let records = domains.iter().find(|d| d.id == "records").unwrap();
        assert_eq!(records.state, DomainState::Off);
        let Reply::DomainContexts { contexts: Some(cx) } =
            a.query(Query::DomainContexts { domain: "oar".into() }).unwrap()
        else {
            panic!()
        };
        assert_eq!(cx.len(), 1);
        assert!(cx[0].name.starts_with("по условиям"), "{}", cx[0].name);
        let Reply::ChainCards { cards: Some(cards) } = a
            .query(Query::DomainCards {
                domain: "oar".into(),
                context: 0,
            })
            .unwrap()
        else {
            panic!()
        };
        // the lower mod's submod wins: its priority (9) is higher — the mod order does not decide here
        assert_eq!(cards[0].stack, ["Q/B", "P/A"]);
        let Reply::ChainCard { card: Some(d) } = a
            .query(Query::DomainCard {
                domain: "oar".into(),
                context: 0,
                index: 0,
                limit: 10,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(d.members[1].teeth, "w");
        assert_eq!(d.items, ["character/mt_idle.hkx"]);
        let Reply::ContextTable { table: Some(t) } = a
            .query(Query::DomainTable {
                domain: "oar".into(),
                context: 0,
                row_start: 0,
                row_count: 10,
                col_start: 0,
                col_count: 10,
            })
            .unwrap()
        else {
            panic!()
        };
        // rows grouped by the owning mod: a heading, then the submod
        let rows: Vec<(bool, &str, &str)> = t
            .rows
            .iter()
            .map(|r| (r.header, r.name.as_str(), r.cells.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                (true, "High", ""),
                (false, "Q/B", "l"),
                (true, "Low", ""),
                (false, "P/A", "w")
            ]
        );
        assert_eq!(t.cols[0].path, "character/mt_idle.hkx");
        // counters: both mods take part in one OAR case; neither shares an overwrite chain
        let found = |a: &mut crate::App, e: &str| match a
            .query(Query::Find {
                expr: e.into(),
                limit: 10,
            })
            .unwrap()
        {
            Reply::Found { found } => found.keys.len(),
            _ => panic!(),
        };
        assert_eq!(found(&mut a, "cases == 1"), 2);
        assert_eq!(found(&mut a, "unresolved_cases > 0"), 0);
        assert_eq!(found(&mut a, "opponents > 0"), 0);
    }

    #[test]
    fn the_table_drops_a_mod_below_a_row() {
        let mut a = crate::tests::app_from(vec![
            files("A", &["x/a.dds"]),
            files("B", &["x/a.dds"]),
            files("C", &["x/b.dds"]),
        ]);
        let (ka, kb) = {
            let b = a.session.book();
            (b.key_of("A").unwrap(), b.key_of("B").unwrap())
        };
        a.command(crate::Command::Move {
            keys: vec![ka],
            before: None,
            after: Some(kb),
        })
        .unwrap();
        let b = a.session.book();
        let order: Vec<&str> = (0..b.len()).map(|p| b.name(b.key_at(p))).collect();
        assert_eq!(order, ["S_separator", "B", "A", "C"]);
    }

    #[test]
    fn opponents_and_win_rate_come_from_the_overwrite_chains() {
        let mut a = crate::tests::app_of(["A", "B"]);
        let found = |a: &mut crate::App, e: &str| match a
            .query(Query::Find {
                expr: e.into(),
                limit: 10,
            })
            .unwrap()
        {
            Reply::Found { found } => found
                .keys
                .iter()
                .map(|&k| a.session.book().name(k).to_string())
                .collect::<Vec<_>>(),
            _ => panic!(),
        };
        assert_eq!(found(&mut a, "opponents == 1 && chains == 1"), ["A", "B"]);
        assert_eq!(found(&mut a, "win_rate == 100"), ["B"]);
        assert_eq!(found(&mut a, "conflict_paths == 1"), ["A", "B"]);
    }
}
