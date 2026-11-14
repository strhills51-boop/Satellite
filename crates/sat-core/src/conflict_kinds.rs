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
    constraints_with(list, None, crate::game::active())
}

/// [`constraints`], plus — with the record index — light plugins whose new records leave the light range, and
/// plugins of another edition of the game (an older header version).
pub fn constraints_with(list: &PluginList, ix: Option<&RecordIndex>, pack: &GamePack) -> DomainReport {
    let mut b = Builder::new(info(
        "constraints",
        "Ограничения",
        "Нет мастера, мастер грузится ниже зависимого, плагин сверх лимита индексов, лёгкий плагин с записями вне лёгкого диапазона, плагин другой редакции игры. Победителя нет: требование нарушено.",
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
        let header = p.winner.and_then(|w| p.versions[w].header_version);
        if let (Some(min), Some(h)) = (pack.plugins.min_header, header)
            && h < min
            && p.origin != crate::plugins::Origin::Base
        {
            b.contest(
                &format!("{} (HEDR {h})", p.name),
                &[(me, cell::BREAKS)],
                "другая редакция игры",
                Severity::Warn,
                true,
            );
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
    // light plugins: new records must stay in the light range
    if let Some(ix) = ix {
        let light: Vec<(u32, (u32, u32), usize)> = ix
            .plugins
            .iter()
            .enumerate()
            .filter_map(|(i, name)| {
                let pos = list
                    .plugins
                    .iter()
                    .position(|p| p.name.eq_ignore_ascii_case(name))?;
                let p = &list.plugins[pos];
                if p.kind != sat_formats::PluginKind::Esl {
                    return None;
                }
                let header = p.winner.and_then(|w| p.versions[w].header_version);
                Some((i as u32, pack.plugins.light_range(header)?, pos))
            })
            .collect();
        let ids: Vec<u32> = light.iter().map(|x| x.0).collect();
        let mut out: FxHashMap<u32, Vec<u32>> = FxHashMap::default();
        for (plugin, object) in ix.new_objects(&ids) {
            let (_, (low, high), _) = light.iter().find(|x| x.0 == plugin).expect("listed");
            if object < *low || object > *high {
                out.entry(plugin).or_default().push(object);
            }
        }
        let mut found: Vec<(u32, Vec<u32>)> = out.into_iter().collect();
        found.sort_by_key(|x| x.0);
        for (plugin, mut objects) in found {
            objects.sort_unstable();
            let (_, (low, high), pos) = light.iter().find(|x| x.0 == plugin).expect("listed");
            let name = &list.plugins[*pos].name;
            let me = b.participant(name, None, *pos as i64);
            b.contest(
                &format!(
                    "{name}: {} записей вне {low:03X}–{high:03X} (например {:06X})",
                    objects.len(),
                    objects[0]
                ),
                &[(me, cell::BREAKS)],
                "лёгкий плагин вне диапазона",
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
    archives_in(&candidates(table, pack, &[]).archives, table, prio, list, pack)
}

/// [`archives`] over candidate paths (a loose and an archive copy, or two archive copies, of different mods).
pub fn archives_in(
    paths: &[PathId],
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
    for &p in paths {
        let r = table.path_providers(p);
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

/// A generator's output mods and its input mods (with a sample path).
pub type GeneratorMods = (Vec<ModId>, Vec<(ModId, PathId)>);

// @sat:conflict.kinds.candidates
/// What each kind looks at, found in one pass over the table (it depends on the table only, not on the order):
/// archive candidates, face files, and each spec's matches.
#[derive(Debug, Default)]
pub struct Candidates {
    pub archives: Vec<PathId>,
    pub facegen: Vec<(PathId, String, u32)>,
    pub specs: Vec<crate::conflict_spec::Matched>,
    /// Per generator of the pack: mods with its output markers, mods with its inputs (with a sample path).
    pub generators: Vec<GeneratorMods>,
}

pub fn candidates(
    table: &ProviderTable,
    pack: &GamePack,
    specs: &[&crate::conflict_spec::DomainSpec],
) -> Candidates {
    let matchers: Vec<crate::conflict_spec::Matcher> = specs
        .iter()
        .map(|s| crate::conflict_spec::Matcher::new(s))
        .collect();
    let templates: Vec<(Vec<String>, usize, usize)> = pack
        .facegen
        .iter()
        .flat_map(|fg| fg.files.iter())
        .filter_map(|t| {
            let segs: Vec<String> = t.split('/').map(str::to_lowercase).collect();
            let pi = segs.iter().position(|s| s == "{plugin}")?;
            let fi = segs.iter().position(|s| s.starts_with("{formid}"))?;
            Some((segs, pi, fi))
        })
        .collect();
    let gens: Vec<(Vec<regex::Regex>, Vec<regex::Regex>)> = pack
        .generators
        .iter()
        .map(|g| {
            (
                g.markers.iter().map(|m| crate::game::glob(m)).collect(),
                g.inputs.iter().map(|m| crate::game::glob(m)).collect(),
            )
        })
        .collect();
    let mut gen_sets: Vec<(FxHashMap<ModId, ()>, FxHashMap<ModId, PathId>)> =
        vec![Default::default(); gens.len()];
    let mut out = Candidates {
        specs: vec![Vec::new(); specs.len()],
        ..Candidates::default()
    };
    for p in 0..table.path_count() {
        let p = PathId::from_idx(p);
        let r = table.path_providers(p);
        if r.len() >= 2 {
            let first = table.owner(r.start);
            if r.clone().any(|i| table.is_archive(i)) && r.clone().any(|i| table.owner(i) != first) {
                out.archives.push(p);
            }
        }
        let s = table.path_string(p);
        let lower = s.to_lowercase();
        if !templates.is_empty() {
            let parts: Vec<&str> = lower.split('/').collect();
            for (segs, pi, fi) in &templates {
                if parts.len() == segs.len()
                    && segs
                        .iter()
                        .zip(&parts)
                        .enumerate()
                        .all(|(k, (t, x))| k == *pi || k == *fi || t == x)
                {
                    let orig: Vec<&str> = s.split('/').collect();
                    let stem = parts[*fi].split('.').next().unwrap_or("");
                    if let Ok(object) = u32::from_str_radix(stem, 16) {
                        out.facegen.push((p, orig[*pi].to_string(), object));
                    }
                }
            }
        }
        for (k, m) in matchers.iter().enumerate() {
            if let Some(c) = m.captures(&s, &lower) {
                out.specs[k].push((p, c));
            }
        }
        for (k, (markers, inputs)) in gens.iter().enumerate() {
            let is_marker = markers.iter().any(|r| r.is_match(&s));
            let is_input = inputs.iter().any(|r| r.is_match(&s));
            if !is_marker && !is_input {
                continue;
            }
            for i in table.path_providers(p) {
                let m = table.owner(i);
                if is_marker {
                    gen_sets[k].0.insert(m, ());
                }
                if is_input {
                    gen_sets[k].1.entry(m).or_insert(p);
                }
            }
        }
    }
    out.generators = gen_sets
        .into_iter()
        .map(|(o, i)| {
            let mut o: Vec<ModId> = o.into_keys().collect();
            o.sort();
            let mut i: Vec<(ModId, PathId)> = i.into_iter().collect();
            i.sort();
            (o, i)
        })
        .collect();
    out
}

/// What the shell found in the game's folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameRuntime {
    /// The executable's file version.
    pub version: Option<(u16, u16, u16, u16)>,
    /// File names in the game folder (the loader is looked for among them).
    pub files: Vec<String>,
}

// @sat:conflict.kinds.runtime
/// The script extender against the game: a loader for another game version, extender plugins without a loader, no
/// address library for this game version. Only when the game's version is known.
pub fn runtime(rt: &GameRuntime, table: &ProviderTable, prio: &Priorities, pack: &GamePack) -> DomainReport {
    let mut b = Builder::new(info(
        "runtime",
        "Версии (SKSE)",
        "Загрузчик расширителя (SKSE) под другую версию игры, плагины SKSE без загрузчика, нет Address Library под версию игры — плагины SKSE не загрузятся или уронят игру.",
        Family::Constraint,
        Centrality::None,
        Semantics::Constraint,
    ));
    let (Some(rules), Some(v)) = (&pack.runtime, rt.version) else {
        return b.finish();
    };
    let game = format!("{}.{}.{}", v.0, v.1, v.2);
    let gp = b.participant(&format!("игра {game}"), None, -1);
    let loader = regex::Regex::new(&rules.loader).ok();
    let loaders: Vec<(String, String)> = rt
        .files
        .iter()
        .filter_map(|f| {
            let c = loader.as_ref()?.captures(f)?;
            let ver: Vec<&str> = c.iter().skip(1).flatten().map(|m| m.as_str()).collect();
            Some((f.clone(), ver.join(".")))
        })
        .collect();
    let plugins_re = crate::game::glob(&rules.plugins);
    let dash = format!("{}-{}-{}", v.0, v.1, v.2);
    let lib_res: Vec<regex::Regex> = rules
        .address_library
        .iter()
        .map(|t| crate::game::glob(&t.replace("{v}", &dash)))
        .collect();
    let (mut dll_mods, mut has_lib): (Vec<ModId>, bool) = (Vec::new(), false);
    for p in 0..table.path_count() {
        let p = PathId::from_idx(p);
        let s = table.path_string(p);
        let is_dll = plugins_re.is_match(&s);
        let is_lib = lib_res.iter().any(|r| r.is_match(&s));
        if !is_dll && !is_lib {
            continue;
        }
        for i in table.path_providers(p) {
            let m = table.owner(i);
            if !prio.is_active(m) {
                continue;
            }
            if is_lib {
                has_lib = true;
            }
            if is_dll && !dll_mods.contains(&m) {
                dll_mods.push(m);
            }
        }
    }
    dll_mods.sort_by_key(|&m| prio.get(m));
    for (file, ver) in &loaders {
        if *ver != game {
            let lp = b.participant(file, None, 0);
            b.contest(
                &format!("{file} — для {ver}, игра {game}"),
                &[(gp, cell::MEMBER), (lp, cell::BREAKS)],
                "загрузчик не под версию игры",
                Severity::Error,
                true,
            );
        }
    }
    let members = |b: &mut Builder| -> Vec<(u32, u8)> {
        dll_mods
            .iter()
            .map(|&m| {
                let n = table.mod_name(m);
                (b.participant(n, Some(n), i64::from(prio.get(m))), cell::BREAKS)
            })
            .collect()
    };
    if !dll_mods.is_empty() && loaders.is_empty() {
        let ms = members(&mut b);
        b.contest(
            "нет загрузчика SKSE в папке игры",
            &ms,
            "плагины SKSE без загрузчика",
            Severity::Error,
            true,
        );
    }
    if !dll_mods.is_empty() && !has_lib {
        let ms = members(&mut b);
        b.contest(
            &format!("Address Library для {game}"),
            &ms,
            "нет Address Library под версию игры",
            Severity::Error,
            true,
        );
    }
    b.finish()
}

// @sat:conflict.kinds.generations
/// Outputs of the pack's generators (LOD, behaviors, patches) with an input mod after them: the output was built
/// without it, or the input overrides what the output made. Only active mods count.
pub fn generations(
    cands: &Candidates,
    table: &ProviderTable,
    prio: &Priorities,
    pack: &GamePack,
) -> DomainReport {
    let mut b = Builder::new(info(
        "generations",
        "Генерации",
        "Выход генератора (DynDOLOD, xLODGen, Nemesis, Synthesis, Bashed Patch) должен стоять после всех своих входов: мод-вход ниже выхода по порядку — выход собран без него (или вход перекрывает сделанное). Перегенерировать или переставить.",
        Family::Interaction,
        Centrality::ModOrder,
        Semantics::Constraint,
    ));
    for (g, (outputs, inputs)) in pack.generators.iter().zip(&cands.generators) {
        for &o in outputs {
            if !prio.is_active(o) {
                continue;
            }
            for &(m, sample) in inputs {
                if m == o || outputs.contains(&m) || !prio.is_active(m) || prio.get(m) <= prio.get(o) {
                    continue;
                }
                let om = b.participant(table.mod_name(o), Some(table.mod_name(o)), i64::from(prio.get(o)));
                let im = b.participant(table.mod_name(m), Some(table.mod_name(m)), i64::from(prio.get(m)));
                b.contest(
                    &format!(
                        "{}: {} — например {}",
                        g.title,
                        table.mod_name(m),
                        table.path_string(sample)
                    ),
                    &[(om, cell::MEMBER), (im, cell::BREAKS)],
                    &format!("{}: вход после выхода", g.title),
                    Severity::Warn,
                    true,
                );
            }
        }
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
    facegen_in(&candidates(table, pack, &[]).facegen, ix, table, prio, list, pack)
}

/// [`facegen`] over candidate face files: (path, the record's plugin, its object id).
pub fn facegen_in(
    faces: &[(PathId, String, u32)],
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
    // (key) → (face winner mod, record winner mod, files)
    let mut seen_keys: FxHashMap<u64, (ModId, ModId, bool)> = FxHashMap::default();
    for (p, plugin, object) in faces {
        let (p, object) = (*p, *object);
        {
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
                version: 1.71,
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
    fn the_extender_must_fit_the_game_version() {
        let facts = [
            mk("Engine Fixes", &["SKSE/Plugins/EngineFixes.dll"]),
            mk("Other", &["textures/a.dds"]),
        ];
        let table = ProviderTable::build(&facts);
        let prio = Priorities(vec![0, 1]);
        let rt = GameRuntime {
            version: Some((1, 6, 1170, 0)),
            files: vec!["skse64_1_6_640.dll".into(), "SkyrimSE.exe".into()],
        };
        let r = runtime(&rt, &table, &prio, crate::game::active());
        let kinds: Vec<&str> = r.counts.kinds.iter().map(|k| k.0.as_str()).collect();
        assert!(kinds.contains(&"загрузчик не под версию игры"), "{kinds:?}");
        assert!(
            kinds.contains(&"нет Address Library под версию игры"),
            "{kinds:?}"
        );
        // with the address library and the right loader: nothing
        let facts = [
            mk("Engine Fixes", &["SKSE/Plugins/EngineFixes.dll"]),
            mk("Address Library", &["SKSE/Plugins/versionlib-1-6-1170-0.bin"]),
        ];
        let table = ProviderTable::build(&facts);
        let rt = GameRuntime {
            version: Some((1, 6, 1170, 0)),
            files: vec!["skse64_1_6_1170.dll".into()],
        };
        assert_eq!(
            runtime(&rt, &table, &prio, crate::game::active()).counts.targets,
            0
        );
    }

    #[test]
    fn an_input_after_a_generator_s_output_is_a_case() {
        let facts = [
            mk("Old Plugin", &["Old.esp"]),
            mk("DynDOLOD Output", &["DynDOLOD.esp", "meshes/lod/x.nif"]),
            mk("Late Plugin", &["Late.esp"]),
            mk("Late Texture", &["textures/other.dds"]),
        ];
        let table = ProviderTable::build(&facts);
        let prio = Priorities(vec![0, 1, 2, 3]);
        let pack = crate::game::active();
        let c = candidates(&table, pack, &[]);
        let r = generations(&c, &table, &prio, pack);
        assert_eq!(r.counts.targets, 1, "{:?}", r.targets);
        assert!(
            r.targets[0].starts_with("DynDOLOD: Late Plugin"),
            "{}",
            r.targets[0]
        );
        assert_eq!(r.chains[0].cells, b"mx");
    }

    #[test]
    fn a_light_plugin_out_of_range_and_an_old_edition_plugin_break_constraints() {
        let mut light = with_plugin("Light", &[], "Light.esp", &["Skyrim.esm"]);
        let h = light.plugins[0].header.as_mut().unwrap();
        h.light_flag = true;
        h.version = 1.7;
        let mut old = with_plugin("Old", &[], "Old.esp", &["Skyrim.esm"]);
        old.plugins[0].header.as_mut().unwrap().version = 0.94;
        let facts = [light, old];
        let prio = Priorities(vec![0, 1]);
        let list = list_of(&facts, &prio, &["Light.esp", "Old.esp"]);
        let rec = |form_id: u32| RecordFact {
            form_id,
            signature: *b"MISC",
            hash: Some(1),
            flags: 0,
        };
        let lp = PluginRecords {
            masters: vec!["Skyrim.esm".into()],
            records: vec![rec(0x0100_0900), rec(0x0100_0012)],
        };
        let ix = RecordIndex::build(&[("Light.esp".into(), &lp)]);
        let r = constraints_with(&list, Some(&ix), crate::game::active());
        let kinds: Vec<&str> = r.counts.kinds.iter().map(|k| k.0.as_str()).collect();
        assert!(kinds.contains(&"лёгкий плагин вне диапазона"), "{kinds:?}");
        assert!(kinds.contains(&"другая редакция игры"), "{kinds:?}");
        let t = r.targets.iter().find(|t| t.starts_with("Light.esp:")).unwrap();
        assert!(t.contains("1 записей") && t.contains("000012"), "{t}");
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
