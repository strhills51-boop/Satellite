//! Metrics: named values of a mod that filters, sticker rules and counters read (ADR 0013 §1). @sat:metrics
//!
//! Owns: the catalog of mod metrics (id, title, group, unit) and their values for one mod from the facts of a
//! scan, the overwrite projection and the canon.
//! Not here: the language that reads them (`@sat:expr`), the scan (`@sat:scan`), the overwrite (`@sat:overwrite`).
//!
//! Groups follow the monolith's counter catalog (`COUNTER_METRIC_CATALOG_GROUPS_V621`). New analyzers add metrics
//! here, not filter code. Besides the fixed list, `ext.<extension>` counts files of one extension (`ext.dds`) and
//! `dir.<top folder>` files under one top folder (`dir.textures`).

use serde::Serialize;

use crate::expr::Value;
use crate::facts::ModMetrics;
use crate::overwrite::ModStat;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Unit {
    Bytes,
    Count,
    Flag,
    Text,
}

/// One metric of the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MetricDef {
    pub id: &'static str,
    pub title: &'static str,
    pub group: &'static str,
    pub unit: Unit,
}

const fn m(id: &'static str, title: &'static str, group: &'static str, unit: Unit) -> MetricDef {
    MetricDef {
        id,
        title,
        group,
        unit,
    }
}

// @sat:metrics.catalog
/// Mod metrics.
pub const MOD_METRICS: &[MetricDef] = &[
    m("name", "Имя мода", "order", Unit::Text),
    m("separator", "Разделитель", "order", Unit::Text),
    m("enabled", "Включён", "order", Unit::Flag),
    m(
        "tags",
        "Теги (найденные и свои, через запятую)",
        "order",
        Unit::Text,
    ),
    m("priority", "Приоритет (позиция в списке)", "order", Unit::Count),
    m("size", "Размер папки", "filesystem", Unit::Bytes),
    m("files", "Файлов", "filesystem", Unit::Count),
    m("dirs", "Папок", "filesystem", Unit::Count),
    m("root_files", "Файлов в корне", "filesystem", Unit::Count),
    m("depth", "Глубина папок", "filesystem", Unit::Count),
    m("archives", "Архивов BSA/BA2", "archives", Unit::Count),
    m("archive_files", "Файлов в архивах", "archives", Unit::Count),
    m("plugins", "Плагинов", "plugins", Unit::Count),
    m("dds", "DDS-текстур", "dds", Unit::Count),
    m("vram", "VRAM текстур", "dds", Unit::Bytes),
    m("nif", "Мешей NIF", "assets", Unit::Count),
    m("pex", "Скриптов PEX", "assets", Unit::Count),
    m("hkx", "Анимаций HKX", "assets", Unit::Count),
    m("dll", "Библиотек DLL", "assets", Unit::Count),
    m("nested_archives", "Вложенных zip/7z/rar", "package", Unit::Count),
    m(
        "source_files",
        "Исходников (psd, blend, psc…)",
        "package",
        Unit::Count,
    ),
    m("wins", "Конфликтных путей выигрывает", "conflicts", Unit::Count),
    m("loses", "Конфликтных путей проигрывает", "conflicts", Unit::Count),
    m("mid", "В середине стека", "conflicts", Unit::Count),
    m("unique", "Уникальных путей", "conflicts", Unit::Count),
    m("overridden", "Не виден в игре (X)", "conflicts", Unit::Flag),
    m("nexus_id", "Id мода на Nexus", "source", Unit::Count),
    m("update", "Есть обновление", "source", Unit::Flag),
];

// @sat:metrics.aliases
/// Other names for metrics, the words people use (`weight > 1 GB`): one table, so saved expressions and rule sets
/// keep working whichever name they were written with.
pub const ALIASES: &[(&str, &str)] = &[("weight", "size"), ("bytes", "size")];

/// The metric id an identifier names (lower case): itself, an alias, or `ext.*` / `dir.*`; `None` if it names
/// nothing.
pub fn resolve(id: &str) -> Option<String> {
    let id = id.to_lowercase();
    if let Some((_, m)) = ALIASES.iter().find(|(a, _)| *a == id) {
        return Some((*m).to_string());
    }
    is_mod_metric(&id).then_some(id)
}

/// Whether `id` names a mod metric (fixed or `ext.*` / `dir.*`).
pub fn is_mod_metric(id: &str) -> bool {
    MOD_METRICS.iter().any(|d| d.id == id)
        || id.strip_prefix("ext.").is_some_and(|e| !e.is_empty())
        || id.strip_prefix("dir.").is_some_and(|e| !e.is_empty())
}

/// What is known about one mod for its metrics.
#[derive(Debug, Clone, Copy)]
pub struct ModView<'a> {
    /// Scan aggregates; `None` when the mod was not scanned or its folder is missing.
    pub metrics: Option<&'a ModMetrics>,
    pub stat: Option<&'a ModStat>,
    pub name: &'a str,
    pub separator: Option<&'a str>,
    /// Tags of the mod joined with `, ` (`tags ~ "skse"`); `None` when not known.
    pub tags: Option<&'a str>,
    pub enabled: bool,
    pub priority: u32,
    /// Where the mod comes from (`@sat:source`); `None` when not known.
    pub source: Option<&'a crate::source::ModSource>,
}

// @sat:metrics.value
impl ModView<'_> {
    /// The value of a metric; `Null` when the mod was not scanned (or the metric needs a projection not given).
    pub fn get(&self, id: &str) -> Value {
        let n = |x: u64| Value::Num(x as f64);
        match id {
            "name" => return Value::Text(self.name.to_string()),
            "separator" => return self.separator.map_or(Value::Null, |s| Value::Text(s.to_string())),
            "enabled" => return Value::Bool(self.enabled),
            "tags" => return self.tags.map_or(Value::Null, |t| Value::Text(t.to_string())),
            "priority" => return n(u64::from(self.priority)),
            "nexus_id" => return self.source.and_then(|s| s.mod_id).map_or(Value::Null, n),
            "update" => return Value::Bool(self.source.is_some_and(|s| s.update_available())),
            _ => {}
        }
        if let Some(s) = self.stat {
            match id {
                "wins" => return n(u64::from(s.win)),
                "loses" => return n(u64::from(s.lose)),
                "mid" => return n(u64::from(s.mid)),
                "unique" => return n(u64::from(s.unique)),
                "overridden" => return Value::Bool(s.fully_overridden),
                _ => {}
            }
        }
        let Some(x) = self.metrics else {
            return Value::Null;
        };
        if let Some(e) = id.strip_prefix("ext.") {
            return n(u64::from(x.by_extension.get(e).copied().unwrap_or(0)));
        }
        if let Some(d) = id.strip_prefix("dir.") {
            return n(u64::from(x.by_top_dir.get(d).copied().unwrap_or(0)));
        }
        match id {
            "size" => n(x.bytes),
            "files" => n(u64::from(x.files)),
            "dirs" => n(u64::from(x.dirs)),
            "root_files" => n(u64::from(x.root_files)),
            "depth" => n(u64::from(x.max_depth)),
            "archives" => n(u64::from(x.archives)),
            "archive_files" => n(u64::from(x.archive_files)),
            "plugins" => n(u64::from(x.plugins)),
            "dds" => n(u64::from(x.dds)),
            "vram" if x.dds_known > 0 || x.dds == 0 => n(x.vram_bytes),
            "nif" => n(u64::from(x.nif)),
            "pex" => n(u64::from(x.pex)),
            "hkx" => n(u64::from(x.hkx)),
            "dll" => n(u64::from(x.dll)),
            "nested_archives" => n(u64::from(x.nested_archives)),
            "source_files" => n(u64::from(x.source_files)),
            _ => Value::Null,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::Expr;
    use crate::facts::ModFacts;

    #[test]
    fn a_filter_over_a_scanned_mod() {
        let mut f = ModFacts {
            name: "Big Textures".into(),
            ..ModFacts::default()
        };
        f.metrics.bytes = 2 * 1024 * 1024 * 1024;
        f.metrics.dds = 900;
        f.metrics.by_extension.insert("dds".into(), 900);
        f.metrics.by_top_dir.insert("textures".into(), 900);
        let stat = ModStat {
            win: 12,
            ..ModStat::default()
        };
        let v = ModView {
            metrics: Some(&f.metrics),
            stat: Some(&stat),
            name: &f.name,
            separator: Some("Textures"),
            tags: Some("Textures, 4K"),
            enabled: true,
            priority: 40,
            source: None,
        };
        let q = |s: &str| Expr::compile(s, &is_mod_metric).unwrap().test(&|id| v.get(id));
        assert!(q(
            "size > 1 GB && ext.dds == dds && dir.textures > 0 && wins > 10"
        ));
        assert!(q(
            "separator ~ \"tex\" && !overridden && priority < 100 && tags ~ \"4k\""
        ));
        assert!(!q("plugins > 0"));
        // not scanned: facts metrics are null, canon metrics still answer
        let blind = ModView { metrics: None, ..v };
        assert!(
            !Expr::compile("size >= 0", &is_mod_metric)
                .unwrap()
                .test(&|id| blind.get(id))
        );
        assert!(
            Expr::compile("enabled", &is_mod_metric)
                .unwrap()
                .test(&|id| blind.get(id))
        );
        assert!(Expr::compile("ext. > 0", &is_mod_metric).is_err());
        // the owner's word resolves to the metric
        let r = |s: &str| Expr::compile_with(s, &resolve).unwrap();
        assert!(r("weight > 1 GB && size > 1gb").test(&|id| v.get(id)));
        assert_eq!(
            r("weight > 0 && size > 0 && dir.textures > 0").metrics(),
            ["dir.textures", "size"]
        );
    }
}
