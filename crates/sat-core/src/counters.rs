//! The counter constructor: user-made counters on mod cards — a template, cases, a format (ADR 0013). @sat:counters
//!
//! Owns: a counter definition (`{expression}` slots in a text, `{expression:format}` with a format; cases that
//! give the counter a tone when their condition holds), compiling it and rendering it for one mod's view.
//! Not here: the language (`@sat:expr`), the metrics (`@sat:metrics`), where a counter is drawn (the window).
//!
//! `"{wins}/{loses}"`, `"{size:bytes}"`, `"{round(vram / size * 100)}%"`; a case `{when: "loses > wins", tone:
//! "bad"}` — the first case that holds gives the tone (a name the window maps to a colour, or `#rrggbb`).

use serde::{Deserialize, Serialize};

use crate::expr::{Expr, Value};
use crate::metrics::ModView;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Case {
    pub when: String,
    pub tone: String,
}

// @sat:counters.def
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CounterDef {
    pub id: String,
    pub template: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cases: Vec<Case>,
    /// Shown only when this holds (empty: always).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub show_if: String,
}

/// A counter as one mod shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CounterValue {
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Plain,
    Bytes,
    Percent,
}

#[derive(Debug, Clone)]
enum Part {
    Text(String),
    Slot(Expr, Format),
}

#[derive(Debug, Clone)]
struct Compiled {
    id: String,
    parts: Vec<Part>,
    cases: Vec<(Expr, String)>,
    show_if: Option<Expr>,
}

/// Counters ready to render.
#[derive(Debug, Clone, Default)]
pub struct Counters {
    list: Vec<Compiled>,
}

fn compile_expr(src: &str) -> Result<Expr, String> {
    Expr::compile_with(src, &crate::metrics::resolve).map_err(|e| e.to_string())
}

fn parse_template(t: &str) -> Result<Vec<Part>, String> {
    let mut parts = Vec::new();
    let mut rest = t;
    while let Some(i) = rest.find('{') {
        if i > 0 {
            parts.push(Part::Text(rest[..i].to_string()));
        }
        let j = rest[i..]
            .find('}')
            .ok_or_else(|| format!("{t:?}: `{{` without `}}`"))?
            + i;
        let slot = &rest[i + 1..j];
        let (src, fmt) = match slot.rsplit_once(':') {
            Some((s, "bytes")) => (s, Format::Bytes),
            Some((s, "pct")) => (s, Format::Percent),
            _ => (slot, Format::Plain),
        };
        parts.push(Part::Slot(compile_expr(src.trim())?, fmt));
        rest = &rest[j + 1..];
    }
    if !rest.is_empty() {
        parts.push(Part::Text(rest.to_string()));
    }
    Ok(parts)
}

fn num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n:.1}")
    }
}

fn render(v: &Value, f: Format) -> String {
    match (v, f) {
        (Value::Null, _) => "—".into(),
        (Value::Bool(b), _) => if *b { "✓" } else { "✗" }.into(),
        (Value::Text(t), _) => t.clone(),
        (Value::Num(n), Format::Plain) => num(*n),
        (Value::Num(n), Format::Percent) => format!("{}%", num((n * 100.0).round() / 100.0)),
        (Value::Num(n), Format::Bytes) => {
            let units = ["B", "KB", "MB", "GB", "TB"];
            let mut x = *n;
            let mut u = 0;
            while x.abs() >= 1024.0 && u + 1 < units.len() {
                x /= 1024.0;
                u += 1;
            }
            if u == 0 {
                format!("{} B", num(x))
            } else {
                format!("{x:.1} {}", units[u])
            }
        }
    }
}

impl Counters {
    // @sat:counters.compile
    /// Compiles definitions; one that does not compile is reported and left out.
    pub fn compile(defs: &[CounterDef]) -> (Self, Vec<String>) {
        let mut out = Self::default();
        let mut errors = Vec::new();
        for d in defs {
            let r = (|| -> Result<Compiled, String> {
                Ok(Compiled {
                    id: d.id.clone(),
                    parts: parse_template(&d.template)?,
                    cases: d
                        .cases
                        .iter()
                        .map(|c| Ok((compile_expr(&c.when)?, c.tone.clone())))
                        .collect::<Result<_, String>>()?,
                    show_if: (!d.show_if.trim().is_empty())
                        .then(|| compile_expr(&d.show_if))
                        .transpose()?,
                })
            })();
            match r {
                Ok(c) => out.list.push(c),
                Err(e) => errors.push(format!("counter {:?}: {e}", d.id)),
            }
        }
        (out, errors)
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    // @sat:counters.render
    /// The counters of one mod.
    pub fn render(&self, view: &ModView<'_>) -> Vec<CounterValue> {
        let get = |id: &str| view.get(id);
        self.list
            .iter()
            .filter(|c| c.show_if.as_ref().is_none_or(|e| e.test(&get)))
            .map(|c| CounterValue {
                id: c.id.clone(),
                text: c
                    .parts
                    .iter()
                    .map(|p| match p {
                        Part::Text(t) => t.clone(),
                        Part::Slot(e, f) => render(&e.eval(&get), *f),
                    })
                    .collect(),
                tone: c.cases.iter().find(|(e, _)| e.test(&get)).map(|(_, t)| t.clone()),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::ModMetrics;
    use crate::overwrite::ModStat;

    #[test]
    fn templates_formats_and_cases() {
        let def = |id: &str, t: &str| CounterDef {
            id: id.into(),
            template: t.into(),
            cases: vec![Case {
                when: "loses > wins".into(),
                tone: "bad".into(),
            }],
            show_if: String::new(),
        };
        let (c, errs) = Counters::compile(&[
            def("wl", "{wins}/{loses}"),
            def("size", "{size:bytes}"),
            def("bad", "{size >}"),
            CounterDef {
                show_if: "dll > 0".into(),
                ..def("dll", "dll {dll}")
            },
        ]);
        assert_eq!(errs.len(), 1, "{errs:?}");
        let m = ModMetrics {
            bytes: 3 << 29,
            ..ModMetrics::default()
        };
        let s = ModStat {
            win: 2,
            lose: 5,
            ..ModStat::default()
        };
        let v = ModView {
            metrics: Some(&m),
            stat: Some(&s),
            name: "A",
            separator: None,
            tags: None,
            enabled: true,
            priority: 0,
        };
        let out = c.render(&v);
        assert_eq!(out.len(), 2, "the dll counter hides: no DLLs");
        assert_eq!(out[0].text, "2/5");
        assert_eq!(out[0].tone.as_deref(), Some("bad"));
        assert_eq!(out[1].text, "1.5 GB");
    }
}
