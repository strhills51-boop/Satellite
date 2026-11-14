//! The built-in conflict kinds beyond files and records (ADR 0018 §5). @sat:conflict.kinds
//!
//! Constraints, archives against loose files, dangerous records, FaceGen against its record.
//!
//! Owns: turning facts already known — the plugin list, the provider table, the record index — into domain
//! reports (`@sat:conflict`), each by the active game pack's rules (`@sat:game`).
//! Not here: record fields and merges (`@sat:conflict.fields`), specs (`@sat:conflict.spec`), the facts themselves.
//!
//! Each kind says what is, never what to do: a constraint has no winner, an archive case names the copy the game
//! really takes, a FaceGen case names the mod whose record wins and the mod whose face does.

use rustc_hash::FxHashMap;

use crate::conflict::{
    Builder, Centrality, DomainInfo, DomainReport, Family, Origin, Semantics, Severity, cell,
};
use crate::game::{GamePack, RecordRules};
use crate::ids::{ModId, PathId};
use crate::overwrite::Priorities;
use crate::plugins::{PluginList, plugin_key};
use crate::records::RecordIndex;
use crate::table::{ProviderTable, Source};

fn info(
    id: &str,
    title: &str,
    about: &str,
    family: Family,
    centrality: Centrality,
    semantics: Semantics,
) -> DomainInfo {
    DomainInfo {
        id: id.into(),
        title: title.into(),
        about: about.into(),
        family,
        centrality,
        semantics,
        origin: Origin::Builtin,
    }
}

// @sat:conflict.kinds.constraints
/// Missing masters, a master loading after its dependent, plugins beyond the index limits.
pub fn constraints(list: &PluginList) -> DomainReport {
    let mut b = Builder::new(info(
        "constraints",
        "Ограничения",
        "Нет мастера, мастер грузится ниже зависимого, плагин сверх лимита индексов. Победителя нет: требование нарушено.",
        Family::Constraint,
        Centrality::None,
        Semantics::Constraint,
    ));
    let pos: FxHashMap<String, usize> = list
        .plugins
        .iter()
        .enumerate()
        .filter(|(_, p)| p.active)
        .map(|(i, p)| (plugin_key(&p.name), i))
        .collect();
    for (i, p) in list.plugins.iter().enumerate() {
        if !p.present || p.blacklisted {
            continue;
        }
        let me = b.participant(&p.name, None, i as i64);
        for m in &p.missing_masters {
            b.contest(
                &format!("{} ← {m}", p.name),
                &[(me, cell::BREAKS)],
                "нет мастера",
                Severity::Error,
                true,
            );
        }
        if !p.active {
            continue;
        }
        for m in &p.masters {
            if let Some(&mi) = pos.get(&plugin_key(m))
                && mi > i
            {
                let mp = b.participant(&list.plugins[mi].name, None, mi as i64);
                b.contest(
                    &format!("{m} после {}", p.name),
                    &[(me, cell::BREAKS), (mp, cell::BREAKS)],
                    "мастер ниже зависимого",
                    Severity::Error,
                    true,
                );
            }
        }
        if let Some(ix) = p.index
            && ix.overflow()
        {
            let kind = match ix {
                crate::plugins::RuntimeIndex::Full(_) => "сверх лимита полных",
                crate::plugins::RuntimeIndex::Light(_) => "сверх лимита лёгких",
            };
            b.contest(
                &format!("{} [{ix}]", p.name),
                &[(me, cell::BREAKS)],
                kind,
                Severity::Error,
                true,
            );
        }
    }
    b.finish()
}

// @sat:conflict.kinds.archives
/// Paths where the game takes another copy than the mod order shows: a loose file below beats an archive above;
/// archives among themselves go by the plugin order; an archive whose plugin does not load does not load.
pub fn archives(
    table: &ProviderTable,
    prio: &Priorities,
    list: &PluginList,
    pack: &GamePack,
) -> DomainReport {
    let mut b = Builder::new(info(
        "archives",
        "Архивы и файлы",
        "Где игра возьмёт не ту копию, что показывает порядок модов: отдельный файл ниже бьёт архив выше, архивы между собой — по порядку плагинов, архив без загруженного плагина не грузится.",
        Family::Collision,
        Centrality::PluginOrder,
        Semantics::LastWins,
    ));
    let Some(rules) = &pack.archives else {
        return b.finish();
    };
    // plugin stem → load position of an active plugin
    let loaded: FxHashMap<String, usize> = list
        .plugins
        .iter()
        .enumerate()
        .filter(|(_, p)| p.active)
        .map(|(i, p)| {
            let n = p.name.to_lowercase();
            (n.rsplit_once('.').map_or(n.clone(), |x| x.0.to_string()), i)
        })
        .collect();
    let archive_pos = |i: usize| -> Option<usize> {
        match table.source(i) {
            Source::Archive(a) if rules.by_plugin => loaded
                .get(&GamePack::archive_stem(&table.archives()[a.0 as usize].file_name))
                .copied(),
            Source::Archive(_) => Some(0),
            Source::Loose => None,
        }
    };
    for p in 0..table.path_count() {
        let p = PathId::from_idx(p);
        let r = table.path_providers(p);
        if r.len() < 2 {
            continue;
        }
        let active: Vec<usize> = r.filter(|&i| prio.is_active(table.owner(i))).collect();
        if active.len() < 2 || !active.iter().any(|&i| table.is_archive(i)) {
            continue;
        }
        // what the mod order shows
        let Some(&shown) = active
            .iter()
            .max_by_key(|&&i| (prio.get(table.owner(i)), !table.is_archive(i)))
        else {
            continue;
        };
        // what the game takes: loose by mod order above every archive, archives by plugin order
        let game = active
            .iter()
            .filter_map(|&i| {
                if table.is_archive(i) {
                    archive_pos(i).map(|pos| (i, (0u8, pos as i64)))
                } else {
                    Some((
                        i,
                        (
                            u8::from(rules.loose_over_archive),
                            i64::from(prio.get(table.owner(i))),
                        ),
                    ))
                }
            })
            .max_by_key(|x| x.1)
            .map(|x| x.0);
        let shown_mod = table.owner(shown);
        if game.map(|g| table.owner(g)) == Some(shown_mod) {
            continue;
        }
        let (kind, sev) = match game {
            None => ("архив не грузится", Severity::Error),
            Some(_) if table.is_archive(shown) && archive_pos(shown).is_none() => {
                ("архив не грузится", Severity::Error)
            }
            Some(g) if !table.is_archive(g) && table.is_archive(shown) => ("файл бьёт архив", Severity::Warn),
            _ => ("архивы по порядку плагинов", Severity::Warn),
        };
        // one cell per mod, mods by priority
        let mut mods: Vec<ModId> = active.iter().map(|&i| table.owner(i)).collect();
        mods.sort_by_key(|&m| (prio.get(m), m));
        mods.dedup();
        let members: Vec<(u32, u8)> = mods
            .iter()
            .map(|&m| {
                let label = table.mod_name(m);
                let id = b.participant(label, Some(label), i64::from(prio.get(m)));
                let c = if game.map(|g| table.owner(g)) == Some(m) {
                    cell::WIN
                } else if active
                    .iter()
                    .filter(|&&i| table.owner(i) == m)
                    .all(|&i| table.is_archive(i) && archive_pos(i).is_none())
                {
                    cell::BREAKS
                } else {
                    cell::LOSE
                };
                (id, c)
            })
            .collect();
        b.contest(&table.path_string(p), &members, kind, sev, true);
    }
    b.finish()
}

fn sig4(s: &str) -> [u8; 4] {
    let mut out = [b' '; 4];
    for (o, c) in out.iter_mut().zip(s.bytes()) {
        *o = c;
    }
    out
}

// @sat:conflict.kinds.dangerous
/// Records whose override breaks the game: a reference or a navmesh deleted instead of disabled; an exclusive
/// record (a navmesh) edited by two plugins or more beyond its own.
pub fn dangerous(ix: &RecordIndex, rules: &RecordRules) -> DomainReport {
    let mut b = Builder::new(info(
        "dangerous",
        "Опасные записи",
        "Ссылки и навмеши, удалённые вместо «отключены» (краш, когда к ним обратятся), и навмеши, которые правят два плагина и больше (не сливаются, ломают поиск пути).",
        Family::Constraint,
        Centrality::PluginOrder,
        Semantics::Exclusive,
    ));
    let watch: Vec<[u8; 4]> = rules.deleted_watch.iter().map(|s| sig4(s)).collect();
    let exclusive: Vec<[u8; 4]> = rules.exclusive.iter().map(|s| sig4(s)).collect();
    let pid = |b: &mut Builder, p: u32| b.participant(&ix.plugins[p as usize], None, i64::from(p));
    let mut deleted: Vec<(u64, crate::records::Provider)> = ix
        .overrides_flagged(rules.deleted_flag)
        .filter(|(_, pr)| watch.contains(&pr.signature))
        .collect();
    deleted.sort_by_key(|x| (x.1.plugin, x.0));
    for (key, pr) in deleted {
        let me = pid(&mut b, pr.plugin);
        b.contest(
            &format!("{} {}", ix.label(key), String::from_utf8_lossy(&pr.signature)),
            &[(me, cell::BREAKS)],
            "удалена вместо «отключена»",
            Severity::Error,
            true,
        );
    }
    for (key, ps) in ix.contested_groups() {
        let Some(last) = ps.last() else { continue };
        if !exclusive.contains(&last.signature) || ps.iter().filter(|p| !p.origin).count() < 2 {
            continue;
        }
        let members: Vec<(u32, u8)> = ps
            .iter()
            .map(|p| {
                let id = pid(&mut b, p.plugin);
                (id, if p.origin { cell::ORIGIN } else { cell::BREAKS })
            })
            .collect();
        b.contest(
            &format!("{} {}", ix.label(key), String::from_utf8_lossy(&last.signature)),
            &members,
            "правят двое и больше",
            Severity::Error,
            true,
        );
    }
    b.finish()
}

// @sat:conflict.kinds.facegen
/// NPCs whose record wins from one mod while their face files win from another: the face does not match the
/// record (the dark-face bug). Only records some plugin overrides; the base game's own faces are not checked.
pub fn facegen(
    ix: &RecordIndex,
    table: &ProviderTable,
    prio: &Priorities,
    list: &PluginList,
    pack: &GamePack,
) -> DomainReport {
    let mut b = Builder::new(info(
        "facegen",
        "Лица NPC",
        "Запись NPC выигрывает из одного мода, а файлы его лица (FaceGen) — из другого: лицо не совпадает с записью (тёмные лица).",
        Family::Interaction,
        Centrality::ModOrder,
        Semantics::LastWins,
    ));
    let Some(fg) = &pack.facegen else {
        return b.finish();
    };
    let record_sig = sig4(&fg.record);
    // the mod a loaded plugin comes from
    let plugin_mod = |name: &str| -> Option<ModId> {
        let p = list.get(name)?;
        p.winner.map(|w| p.versions[w].owner)
    };
    let templates: Vec<(Vec<&str>, usize, usize)> = fg
        .files
        .iter()
        .filter_map(|t| {
            let segs: Vec<&str> = t.split('/').collect();
            let pi = segs.iter().position(|s| *s == "{plugin}")?;
            let fi = segs.iter().position(|s| s.starts_with("{formid}"))?;
            Some((segs, pi, fi))
        })
        .collect();
    // (key) → (face winner mod, record winner mod, files)
    let mut seen_keys: FxHashMap<u64, (ModId, ModId, bool)> = FxHashMap::default();
    for p in 0..table.path_count() {
        let p = PathId::from_idx(p);
        let s = table.path_string(p);
        let parts: Vec<&str> = s.split('/').collect();
        for (segs, pi, fi) in &templates {
            if parts.len() != segs.len()
                || !segs
                    .iter()
                    .zip(&parts)
                    .enumerate()
                    .all(|(k, (t, x))| k == *pi || k == *fi || t.eq_ignore_ascii_case(x))
            {
                continue;
            }
            let plugin = parts[*pi];
            let stem = parts[*fi].split('.').next().unwrap_or("");
            let Ok(object) = u32::from_str_radix(stem, 16) else {
                continue;
            };
            let Some(key) = ix.key_of(plugin, object) else {
                continue;
            };
            let provs = ix.providers(key);
            let Some(last) = provs.last() else { continue };
            if last.signature != record_sig || last.origin {
                continue;
            }
            let Some(rec_mod) = plugin_mod(&ix.plugins[last.plugin as usize]) else {
                continue;
            };
            let Some(face) = table
                .path_providers(p)
                .filter(|&i| prio.is_active(table.owner(i)))
                .max_by_key(|&i| (prio.get(table.owner(i)), !table.is_archive(i)))
            else {
                continue;
            };
            let face_mod = table.owner(face);
            if face_mod == rec_mod {
                continue;
            }
            let rec_has = table.path_providers(p).any(|i| table.owner(i) == rec_mod);
            let e = seen_keys.entry(key).or_insert((face_mod, rec_mod, false));
            e.2 |= rec_has;
        }
    }
    let mut keys: Vec<(u64, (ModId, ModId, bool))> = seen_keys.into_iter().collect();
    keys.sort_by_key(|x| x.0);
    for (key, (face_mod, rec_mod, rec_has)) in keys {
        let rm = b.participant(
            table.mod_name(rec_mod),
            Some(table.mod_name(rec_mod)),
            i64::from(prio.get(rec_mod)),
        );
        let fm = b.participant(
            table.mod_name(face_mod),
            Some(table.mod_name(face_mod)),
            i64::from(prio.get(face_mod)),
        );
        let (kind, sev) = if rec_has {
            ("лицо мода записи проигрывает", Severity::Error)
        } else {
            ("лицо из другого мода", Severity::Warn)
        };
        b.contest(
            &format!("{} {}", ix.label(key), fg.record),
            &[
                (rm, if rec_has { cell::LOSE } else { cell::BREAKS }),
                (fm, cell::WIN),
            ],
            kind,
            sev,
            true,
        );
    }
    b.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::{PluginCanon, PluginVersions};
    use crate::records::{PluginRecords, RecordFact};
    use crate::table::tests::mk;

    fn with_plugin(name: &str, files: &[&str], plugin: &str, masters: &[&str]) -> crate::facts::ModFacts {
        let mut m = mk(name, files);
        m.plugins.push(crate::facts::PluginFacts {
            file_name: plugin.into(),
            header: Some(sat_formats::PluginHeader {
                masters: masters.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            }),
            records: None,
            error: None,
        });
        m
    }

    fn list_of(facts: &[crate::facts::ModFacts], prio: &Priorities, order: &[&str]) -> PluginList {
        let canon = PluginCanon {
            load_order: crate::plugins::base_plugins()
                .into_iter()
                .map(|(n, _)| n)
                .chain(order.iter().map(|s| s.to_string()))
                .collect(),
            checked: order.iter().map(|s| (s.to_string(), true)).collect(),
            blacklist: Vec::new(),
        };
        PluginList::compute(&PluginVersions::build(facts), &canon, prio)
    }

    #[test]
    fn missing_and_late_masters_are_constraints_without_a_winner() {
        let facts = [
            with_plugin("A", &[], "A.esp", &["Skyrim.esm", "Gone.esm"]),
            with_plugin("B", &[], "B.esp", &["Skyrim.esm"]),
            with_plugin("C", &[], "C.esp", &["B.esp"]),
        ];
        let prio = Priorities(vec![0, 1, 2]);
        // C.esp has B.esp (a regular plugin) as a master but loads before it
        let list = list_of(&facts, &prio, &["A.esp", "C.esp", "B.esp"]);
        let r = constraints(&list);
        assert!(
            r.counts.kinds.iter().any(|(k, n)| k == "нет мастера" && *n == 1),
            "{:?}",
            r.counts
        );
        let late = r
            .chains
            .iter()
            .find(|c| c.kind == "мастер ниже зависимого")
            .expect("late master");
        assert_eq!(r.targets[late.targets[0] as usize], "B.esp после C.esp");
        assert!(
            r.chains
                .iter()
                .all(|c| c.unresolved && c.severity == Severity::Error)
        );
        assert!(
            r.chains
                .iter()
                .flat_map(|c| c.cells.iter())
                .all(|&x| x == cell::BREAKS)
        );
    }

    #[test]
    fn a_loose_file_below_beats_an_archive_above_and_an_unloaded_archive_does_not_load() {
        use crate::facts::ArchiveFacts;
        use sat_formats::{ArchiveEntry, ArchiveKind, ArchiveManifest};
        let arch = |name: &str, path: &str| ArchiveFacts {
            file_name: name.into(),
            size: 10,
            manifest: Some(ArchiveManifest {
                kind: ArchiveKind::Bsa { version: 105 },
                entries: vec![ArchiveEntry {
                    path: path.into(),
                    size: 5,
                    vram: 0,
                }],
            }),
            error: None,
        };
        let mut low = mk("Low", &["textures/a.dds"]);
        low.plugins.clear();
        let mut high = with_plugin("High", &[], "High.esp", &["Skyrim.esm"]);
        high.archives.push(arch("High.bsa", "textures/a.dds"));
        high.archives.push(arch("Orphan.bsa", "textures/b.dds"));
        let other = {
            let mut m = mk("Other", &[]);
            m.archives.push(arch("Other.bsa", "textures/b.dds"));
            m
        };
        let facts = [low, other, high];
        let table = ProviderTable::build(&facts);
        let prio = Priorities(vec![0, 1, 2]);
        let list = list_of(&facts, &prio, &["High.esp"]);
        let r = archives(&table, &prio, &list, crate::game::active());
        let a = r.targets.iter().position(|t| t == "textures/a.dds").unwrap() as u32;
        let ca = r.chains.iter().find(|c| c.targets.contains(&a)).unwrap();
        assert_eq!(ca.kind, "файл бьёт архив");
        let low_id = r.participants.iter().position(|p| p.label == "Low").unwrap() as u32;
        let m = ca.stack.iter().position(|&p| p == low_id).unwrap();
        assert_eq!(ca.row(m), b"w", "the loose copy of the lower mod wins");
        // b.dds: High's Orphan.bsa has no plugin, Other.bsa has none either — nothing loads it
        assert!(r.chains.iter().any(|c| c.kind == "архив не грузится"));
    }

    #[test]
    fn deleted_references_and_shared_navmeshes_are_dangerous() {
        let rec = |form_id: u32, s: &[u8; 4], flags: u32| RecordFact {
            form_id,
            signature: *s,
            hash: Some(u64::from(form_id) ^ u64::from(flags)),
            flags,
        };
        let base = PluginRecords {
            masters: vec![],
            records: vec![rec(0x10, b"REFR", 0), rec(0x20, b"NAVM", 0)],
        };
        let a = PluginRecords {
            masters: vec!["Skyrim.esm".into()],
            records: vec![rec(0x10, b"REFR", 0x20), rec(0x20, b"NAVM", 0)],
        };
        let b = PluginRecords {
            masters: vec!["Skyrim.esm".into()],
            records: vec![rec(0x20, b"NAVM", 1)],
        };
        let ix = RecordIndex::build(&[
            ("Skyrim.esm".into(), &base),
            ("A.esp".into(), &a),
            ("B.esp".into(), &b),
        ]);
        let r = dangerous(&ix, crate::game::active().records.as_ref().unwrap());
        let kinds: Vec<&str> = r.chains.iter().map(|c| c.kind.as_str()).collect();
        assert!(kinds.contains(&"удалена вместо «отключена»"), "{kinds:?}");
        assert!(kinds.contains(&"правят двое и больше"), "{kinds:?}");
        let nav = r
            .chains
            .iter()
            .find(|c| c.kind == "правят двое и больше")
            .unwrap();
        assert_eq!(nav.cells, b"oxx");
    }
}
