//! The project as sections: one tree split into NDJSON files and joined back. @sat:stll.sections
//!
//! Owns: the rule that cuts the project's JSON tree into sections of the `.stll` file and the inverse.
//! Not here: the file itself (`@sat:stll.file`), what the fields mean (`@sat:project`).
//!
//! The rule is generic, so a new project field becomes a section without code here, and a field this build does
//! not know (kept by `Project::extra`) is written back the same way:
//! - a list → `project/<path>.ndjson`, one element per line;
//! - a map (an object whose keys are data: entity ids, names) → `project/<path>.ndjson`, one `{"k":…,"v":…}`
//!   per line;
//! - a structure (an object whose keys are fields: the root and [`STRUCTURES`]) → its parts, one level down;
//!   its plain values go to `project/<path>/fields.json` (the root's: `project/fields.json`).
//!
//! Why lines: the user's truth is small and valuable, it is compared between versions and shared — a change to
//! one mod is a change to one line.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Top-level fields whose keys are field names, not data: split one level further.
pub const STRUCTURES: &[&str] = &["canon", "decor", "plugins", "tags"];

/// How a section's lines map to its part of the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    /// One element per line.
    List,
    /// One `{"k": key, "v": value}` per line.
    Map,
    /// One object of plain values.
    Fields,
}

/// One section ready to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Entry name in the file.
    pub name: String,
    /// Where it sits in the project tree.
    pub path: Vec<String>,
    pub shape: Shape,
    pub body: Vec<u8>,
    pub lines: usize,
}

/// A section that could not be read back.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{name} line {line}: {message}")]
pub struct SectionError {
    pub name: String,
    pub line: usize,
    pub message: String,
}

/// Field names are identifiers, but an extension may write anything: `/` and `%` would break the entry name.
fn escape(key: &str) -> String {
    key.replace('%', "%25").replace('/', "%2F")
}

fn entry(path: &[String], ext: &str) -> String {
    let parts: Vec<String> = path.iter().map(|p| escape(p)).collect();
    format!("project/{}{ext}", parts.join("/"))
}

// @sat:stll.sections.split
/// Cuts a project tree (an object) into sections, in a stable order.
pub fn split(root: &Map<String, Value>) -> Vec<Section> {
    let mut out = Vec::new();
    walk(&mut Vec::new(), root, &mut out);
    out
}

fn walk(path: &mut Vec<String>, obj: &Map<String, Value>, out: &mut Vec<Section>) {
    let mut fields = Map::new();
    for (k, v) in obj {
        path.push(k.clone());
        match v {
            Value::Array(items) => {
                let mut body = Vec::new();
                for it in items {
                    line(&mut body, it);
                }
                out.push(Section {
                    name: entry(path, ".ndjson"),
                    path: path.clone(),
                    shape: Shape::List,
                    body,
                    lines: items.len(),
                });
            }
            Value::Object(o) if path.len() == 1 && STRUCTURES.contains(&k.as_str()) => walk(path, o, out),
            Value::Object(o) => {
                let mut body = Vec::new();
                for (key, val) in o {
                    let mut m = Map::new();
                    m.insert("k".into(), Value::String(key.clone()));
                    m.insert("v".into(), val.clone());
                    line(&mut body, &Value::Object(m));
                }
                out.push(Section {
                    name: entry(path, ".ndjson"),
                    path: path.clone(),
                    shape: Shape::Map,
                    body,
                    lines: o.len(),
                });
            }
            plain => {
                fields.insert(k.clone(), plain.clone());
            }
        }
        path.pop();
    }
    if !fields.is_empty() || path.is_empty() {
        let mut body = serde_json::to_vec_pretty(&Value::Object(fields)).expect("plain values serialize");
        body.push(b'\n');
        let mut p = path.clone();
        p.push("fields".into());
        out.push(Section {
            name: entry(&p, ".json"),
            path: path.clone(),
            shape: Shape::Fields,
            body,
            lines: 1,
        });
    }
}

fn line(body: &mut Vec<u8>, v: &Value) {
    serde_json::to_writer(&mut *body, v).expect("a JSON value serializes");
    body.push(b'\n');
}

// @sat:stll.sections.join
/// Puts sections back into one tree. Sections may come in any order.
pub fn join<'a>(
    sections: impl IntoIterator<Item = (&'a str, &'a [String], Shape, &'a [u8])>,
) -> Result<Map<String, Value>, SectionError> {
    let mut root = Map::new();
    for (name, path, shape, body) in sections {
        let err = |line: usize, message: String| SectionError {
            name: name.to_string(),
            line,
            message,
        };
        let text = std::str::from_utf8(body).map_err(|e| err(0, e.to_string()))?;
        let value = match shape {
            Shape::Fields => serde_json::from_str::<Value>(text).map_err(|e| err(e.line(), e.to_string()))?,
            Shape::List | Shape::Map => {
                let mut items = Vec::new();
                for (i, l) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
                    items.push(serde_json::from_str::<Value>(l).map_err(|e| err(i + 1, e.to_string()))?);
                }
                if shape == Shape::List {
                    Value::Array(items)
                } else {
                    let mut m = Map::new();
                    for (i, it) in items.into_iter().enumerate() {
                        let Value::Object(mut kv) = it else {
                            return Err(err(i + 1, "expected {\"k\":…,\"v\":…}".into()));
                        };
                        let Some(Value::String(k)) = kv.remove("k") else {
                            return Err(err(i + 1, "no key \"k\"".into()));
                        };
                        m.insert(k, kv.remove("v").unwrap_or(Value::Null));
                    }
                    Value::Object(m)
                }
            }
        };
        // walk to the parent, creating structures on the way
        let mut at = &mut root;
        let (last, parents) = match (shape, path.split_last()) {
            (Shape::Fields, _) => (None, path),
            (_, Some((last, parents))) => (Some(last), parents),
            (_, None) => return Err(err(0, "a list or map section needs a path".into())),
        };
        for p in parents {
            let slot = at.entry(p.clone()).or_insert_with(|| Value::Object(Map::new()));
            let Value::Object(o) = slot else {
                return Err(err(0, format!("{p} is not a structure")));
            };
            at = o;
        }
        match (last, value) {
            (Some(k), v) => {
                at.insert(k.clone(), v);
            }
            (None, Value::Object(fields)) => at.extend(fields),
            (None, _) => return Err(err(1, "fields must be an object".into())),
        }
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_tree_splits_into_lines_and_joins_back() {
        let v = json!({
            "schema": 2,
            "entities": [{"id": "e1", "name": "A"}, {"id": "e2", "name": "B"}],
            "signatures": {"e1": 7, "e2": 8},
            "canon": {"entries": [{"name": "A"}]},
            "decor": {"colors": {"e1": 255}, "groups": [], "note": "x"},
            "future/field": {"a": [1, 2]},
            "flag": true
        });
        let Value::Object(root) = v.clone() else { panic!() };
        let s = split(&root);
        let names: Vec<&str> = s.iter().map(|x| x.name.as_str()).collect();
        assert!(names.contains(&"project/entities.ndjson"));
        assert!(names.contains(&"project/canon/entries.ndjson"));
        assert!(names.contains(&"project/decor/colors.ndjson"));
        assert!(names.contains(&"project/decor/fields.json"));
        assert!(names.contains(&"project/future%2Ffield.ndjson"));
        assert!(names.contains(&"project/fields.json"));
        let ent = s.iter().find(|x| x.name == "project/entities.ndjson").unwrap();
        assert_eq!(
            std::str::from_utf8(&ent.body).unwrap(),
            "{\"id\":\"e1\",\"name\":\"A\"}\n{\"id\":\"e2\",\"name\":\"B\"}\n"
        );
        let sig = s.iter().find(|x| x.name == "project/signatures.ndjson").unwrap();
        assert_eq!(
            std::str::from_utf8(&sig.body).unwrap(),
            "{\"k\":\"e1\",\"v\":7}\n{\"k\":\"e2\",\"v\":8}\n"
        );
        // any order joins back to the same tree
        let back = join(
            s.iter()
                .rev()
                .map(|x| (x.name.as_str(), x.path.as_slice(), x.shape, x.body.as_slice())),
        )
        .unwrap();
        assert_eq!(Value::Object(back), v);
    }

    #[test]
    fn a_bad_line_names_its_section_and_line() {
        let path = vec!["entities".to_string()];
        let e = join([(
            "project/entities.ndjson",
            path.as_slice(),
            Shape::List,
            b"{\"id\":1}\n{oops\n".as_slice(),
        )])
        .unwrap_err();
        assert_eq!((e.name.as_str(), e.line), ("project/entities.ndjson", 2));
    }
}
