//! FOMOD installers: `fomod/ModuleConfig.xml` → steps, groups, options; the user's choices → files. @sat:formats.fomod
//!
//! Owns: reading a module config (UTF-8 or UTF-16), the installer's defaults, checking choices against the group
//! rules and resolving them (with condition flags and conditional installs) into the files to install.
//! Not here: archives and extraction (`@sat:mo2.install`), where choices are kept (`@sat:intent`).
//!
//! A choice is named `step/group/option`. Conditions on the game's plugins (`fileDependency`) cannot be known when
//! a mod is planned away from the build: they count as not met (the choice is the user's to make then).

use std::collections::HashMap;

/// One file or folder to copy: `source` relative to the folder that holds `fomod/`, `destination` relative to the
/// mod folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileItem {
    pub source: String,
    pub destination: String,
    pub folder: bool,
    pub priority: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    Flag {
        name: String,
        value: String,
    },
    /// About the game's plugins: unknown here, counts as not met.
    File {
        file: String,
        state: String,
    },
    All(Vec<Condition>),
    Any(Vec<Condition>),
}

impl Condition {
    fn holds(&self, flags: &HashMap<String, String>) -> bool {
        match self {
            Condition::Flag { name, value } => flags.get(name).map_or(value.is_empty(), |v| v == value),
            Condition::File { .. } => false,
            Condition::All(c) => c.iter().all(|x| x.holds(flags)),
            Condition::Any(c) => c.iter().any(|x| x.holds(flags)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    SelectAny,
    SelectAll,
    ExactlyOne,
    AtMostOne,
    AtLeastOne,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionType {
    Required,
    Optional,
    Recommended,
    NotUsable,
    CouldBeUsable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub name: String,
    pub description: String,
    pub files: Vec<FileItem>,
    pub flags: Vec<(String, String)>,
    pub kind: OptionType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub kind: GroupKind,
    pub options: Vec<Choice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub name: String,
    pub visible: Option<Condition>,
    pub groups: Vec<Group>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conditional {
    pub when: Condition,
    pub files: Vec<FileItem>,
}

/// A module config.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Config {
    pub name: String,
    pub required: Vec<FileItem>,
    pub steps: Vec<Step>,
    pub conditional: Vec<Conditional>,
}

/// The id of an option: `step/group/option`.
pub fn choice_id(step: &str, group: &str, option: &str) -> String {
    format!("{step}/{group}/{option}")
}

fn decode(bytes: &[u8]) -> Result<String, String> {
    let utf16 = |le: bool, b: &[u8]| {
        let units: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| {
                if le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16(&units).map_err(|e| e.to_string())
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(true, rest),
        [0xFE, 0xFF, rest @ ..] => utf16(false, rest),
        [0xEF, 0xBB, 0xBF, rest @ ..] => Ok(String::from_utf8_lossy(rest).into_owned()),
        // UTF-16 without a mark: `<` then a zero byte
        [b'<', 0, ..] => utf16(true, bytes),
        _ => Ok(String::from_utf8_lossy(bytes).into_owned()),
    }
}

type Node<'a, 'i> = roxmltree::Node<'a, 'i>;

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.has_tag_name(name))
}

fn children<'a, 'i>(n: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children().filter(move |c| c.has_tag_name(name))
}

fn path(s: &str) -> String {
    s.replace('\\', "/").trim_matches('/').to_string()
}

fn files_of(n: Option<Node>) -> Vec<FileItem> {
    let Some(n) = n else { return Vec::new() };
    n.children()
        .filter(|c| c.has_tag_name("file") || c.has_tag_name("folder"))
        .map(|c| {
            let source = path(c.attribute("source").unwrap_or(""));
            FileItem {
                destination: c.attribute("destination").map_or_else(|| source.clone(), path),
                source,
                folder: c.has_tag_name("folder"),
                priority: c.attribute("priority").and_then(|p| p.parse().ok()).unwrap_or(0),
            }
        })
        .collect()
}

fn condition(n: Node) -> Condition {
    let parts: Vec<Condition> = n
        .children()
        .filter_map(|c| match c.tag_name().name() {
            "flagDependency" => Some(Condition::Flag {
                name: c.attribute("flag").unwrap_or("").to_string(),
                value: c.attribute("value").unwrap_or("").to_string(),
            }),
            "fileDependency" => Some(Condition::File {
                file: c.attribute("file").unwrap_or("").to_string(),
                state: c.attribute("state").unwrap_or("").to_string(),
            }),
            "dependencies" => Some(condition(c)),
            _ => None,
        })
        .collect();
    if n.attribute("operator") == Some("Or") {
        Condition::Any(parts)
    } else {
        Condition::All(parts)
    }
}

fn ordered<T>(mut v: Vec<T>, order: Option<&str>, name: impl Fn(&T) -> &str) -> Vec<T> {
    match order {
        Some("Explicit") => {}
        Some("Descending") => v.sort_by_key(|x| std::cmp::Reverse(name(x).to_lowercase())),
        _ => v.sort_by_key(|x| name(x).to_lowercase()),
    }
    v
}

fn option_type(name: &str) -> OptionType {
    match name {
        "Required" => OptionType::Required,
        "Recommended" => OptionType::Recommended,
        "NotUsable" => OptionType::NotUsable,
        "CouldBeUsable" => OptionType::CouldBeUsable,
        _ => OptionType::Optional,
    }
}

// @sat:formats.fomod.parse
/// Reads `ModuleConfig.xml`.
pub fn parse(bytes: &[u8]) -> Result<Config, String> {
    let text = decode(bytes)?;
    let doc = roxmltree::Document::parse(&text).map_err(|e| format!("ModuleConfig.xml: {e}"))?;
    let root = doc.root_element();
    let mut c = Config {
        name: child(root, "moduleName")
            .and_then(|n| n.text())
            .unwrap_or("")
            .trim()
            .to_string(),
        required: files_of(child(root, "requiredInstallFiles")),
        ..Config::default()
    };
    if let Some(steps) = child(root, "installSteps") {
        let list = children(steps, "installStep")
            .map(|s| {
                let groups = child(s, "optionalFileGroups").map_or_else(Vec::new, |g| {
                    let list = children(g, "group")
                        .map(|grp| {
                            let options = child(grp, "plugins").map_or_else(Vec::new, |p| {
                                let list = children(p, "plugin")
                                    .map(|o| {
                                        let td = child(o, "typeDescriptor");
                                        let kind = td
                                            .and_then(|t| {
                                                child(t, "type").or_else(|| {
                                                    child(t, "dependencyType")
                                                        .and_then(|d| child(d, "defaultType"))
                                                })
                                            })
                                            .and_then(|t| t.attribute("name"))
                                            .map_or(OptionType::Optional, option_type);
                                        Choice {
                                            name: o.attribute("name").unwrap_or("").to_string(),
                                            description: child(o, "description")
                                                .and_then(|d| d.text())
                                                .unwrap_or("")
                                                .trim()
                                                .to_string(),
                                            files: files_of(child(o, "files")),
                                            flags: child(o, "conditionFlags").map_or_else(Vec::new, |f| {
                                                children(f, "flag")
                                                    .map(|x| {
                                                        (
                                                            x.attribute("name").unwrap_or("").to_string(),
                                                            x.text().unwrap_or("").trim().to_string(),
                                                        )
                                                    })
                                                    .collect()
                                            }),
                                            kind,
                                        }
                                    })
                                    .collect();
                                ordered(list, p.attribute("order"), |x: &Choice| &x.name)
                            });
                            Group {
                                name: grp.attribute("name").unwrap_or("").to_string(),
                                kind: match grp.attribute("type").unwrap_or("") {
                                    "SelectAll" => GroupKind::SelectAll,
                                    "SelectExactlyOne" => GroupKind::ExactlyOne,
                                    "SelectAtMostOne" => GroupKind::AtMostOne,
                                    "SelectAtLeastOne" => GroupKind::AtLeastOne,
                                    _ => GroupKind::SelectAny,
                                },
                                options,
                            }
                        })
                        .collect();
                    ordered(list, g.attribute("order"), |x: &Group| &x.name)
                });
                Step {
                    name: s.attribute("name").unwrap_or("").to_string(),
                    visible: child(s, "visible").map(condition),
                    groups,
                }
            })
            .collect();
        c.steps = ordered(list, steps.attribute("order"), |x: &Step| &x.name);
    }
    if let Some(cf) = child(root, "conditionalFileInstalls").and_then(|n| child(n, "patterns")) {
        c.conditional = children(cf, "pattern")
            .map(|p| Conditional {
                when: child(p, "dependencies").map_or(Condition::All(Vec::new()), condition),
                files: files_of(child(p, "files")),
            })
            .collect();
    }
    Ok(c)
}

/// What a resolved install gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// Every choice in effect (given or by default), `step/group/option`.
    pub chosen: Vec<String>,
    /// Files in install order: a later one overwrites an earlier one at the same destination.
    pub files: Vec<FileItem>,
}

impl Config {
    // @sat:formats.fomod.resolve
    /// Resolves choices into files. Groups with no given choice take the installer's defaults (required and
    /// recommended options; one option groups — the first usable); given choices are checked against the rules.
    pub fn resolve(&self, given: &[String]) -> Result<Resolved, String> {
        let given_set: std::collections::HashSet<String> = given.iter().map(|g| g.to_lowercase()).collect();
        let mut flags: HashMap<String, String> = HashMap::new();
        let mut chosen = Vec::new();
        let mut files: Vec<FileItem> = self.required.clone();
        for step in &self.steps {
            if step.visible.as_ref().is_some_and(|v| !v.holds(&flags)) {
                continue;
            }
            for g in &step.groups {
                let id = |o: &Choice| choice_id(&step.name, &g.name, &o.name);
                let mut sel: Vec<&Choice> = g
                    .options
                    .iter()
                    .filter(|o| given_set.contains(&id(o).to_lowercase()))
                    .collect();
                let prefix = format!("{}/{}/", step.name, g.name).to_lowercase();
                let group_given = given_set.iter().any(|x| x.starts_with(&prefix));
                if !group_given {
                    sel = match g.kind {
                        GroupKind::SelectAll => g.options.iter().collect(),
                        _ => g
                            .options
                            .iter()
                            .filter(|o| matches!(o.kind, OptionType::Required | OptionType::Recommended))
                            .collect(),
                    };
                    if matches!(g.kind, GroupKind::ExactlyOne | GroupKind::AtLeastOne) && sel.is_empty() {
                        sel.extend(g.options.iter().find(|o| o.kind != OptionType::NotUsable));
                    }
                    if matches!(g.kind, GroupKind::ExactlyOne | GroupKind::AtMostOne) {
                        sel.truncate(1);
                    }
                }
                // required options are always in
                for o in &g.options {
                    if o.kind == OptionType::Required && !sel.iter().any(|s| s.name == o.name) {
                        sel.push(o);
                    }
                }
                let n = sel.len();
                let ok = match g.kind {
                    GroupKind::ExactlyOne => n == 1,
                    GroupKind::AtMostOne => n <= 1,
                    GroupKind::AtLeastOne => n >= 1,
                    GroupKind::SelectAll => n == g.options.len(),
                    GroupKind::SelectAny => true,
                };
                if !ok {
                    return Err(format!(
                        "{}/{}: {} chosen, the group allows {:?}",
                        step.name, g.name, n, g.kind
                    ));
                }
                if let Some(bad) = sel.iter().find(|o| o.kind == OptionType::NotUsable) {
                    return Err(format!("{}: not usable", id(bad)));
                }
                for o in sel {
                    chosen.push(id(o));
                    files.extend(o.files.iter().cloned());
                    for (k, v) in &o.flags {
                        flags.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        for c in &self.conditional {
            if c.when.holds(&flags) {
                files.extend(c.files.iter().cloned());
            }
        }
        // a choice that names nothing is a typo, not a wish
        let known: std::collections::HashSet<String> = chosen.iter().map(|c| c.to_lowercase()).collect();
        if let Some(unknown) = given.iter().find(|g| !known.contains(&g.to_lowercase())) {
            return Err(format!("{unknown}: no such option in a visible step"));
        }
        files.sort_by_key(|f| f.priority);
        Ok(Resolved { chosen, files })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <moduleName>Test Mod</moduleName>
  <requiredInstallFiles><folder source="00 Core" destination="" /></requiredInstallFiles>
  <installSteps order="Explicit">
    <installStep name="Body">
      <optionalFileGroups order="Explicit">
        <group name="Body type" type="SelectExactlyOne">
          <plugins order="Explicit">
            <plugin name="CBBE"><description>for CBBE</description>
              <files><folder source="10 CBBE" destination="" /></files>
              <conditionFlags><flag name="body">cbbe</flag></conditionFlags>
              <typeDescriptor><type name="Recommended"/></typeDescriptor></plugin>
            <plugin name="3BA">
              <files><folder source="11 3BA" destination="" /></files>
              <conditionFlags><flag name="body">3ba</flag></conditionFlags>
              <typeDescriptor><type name="Optional"/></typeDescriptor></plugin>
          </plugins>
        </group>
      </optionalFileGroups>
    </installStep>
    <installStep name="Physics">
      <visible><flagDependency flag="body" value="3ba"/></visible>
      <optionalFileGroups><group name="SMP" type="SelectAny"><plugins>
        <plugin name="SMP"><files><file source="smp\a.xml" destination="meshes\a.xml"/></files>
          <typeDescriptor><type name="Optional"/></typeDescriptor></plugin>
      </plugins></group></optionalFileGroups>
    </installStep>
  </installSteps>
  <conditionalFileInstalls><patterns>
    <pattern><dependencies operator="And"><flagDependency flag="body" value="cbbe"/></dependencies>
      <files><file source="extra\cbbe.esp" destination="cbbe.esp" priority="5"/></files></pattern>
  </patterns></conditionalFileInstalls>
</config>"#;

    #[test]
    fn defaults_choices_flags_and_conditional_files() {
        let c = parse(XML.as_bytes()).unwrap();
        assert_eq!(c.name, "Test Mod");
        assert_eq!(c.steps.len(), 2);
        // defaults: the recommended body; the physics step stays hidden; the CBBE pattern applies
        let d = c.resolve(&[]).unwrap();
        assert_eq!(d.chosen, ["Body/Body type/CBBE"]);
        let srcs: Vec<&str> = d.files.iter().map(|f| f.source.as_str()).collect();
        assert_eq!(srcs, ["00 Core", "10 CBBE", "extra/cbbe.esp"]);
        // 3BA: the physics step shows, its option can be taken
        let r = c
            .resolve(&["Body/Body type/3BA".into(), "Physics/SMP/SMP".into()])
            .unwrap();
        assert_eq!(r.chosen, ["Body/Body type/3BA", "Physics/SMP/SMP"]);
        assert_eq!(r.files.last().unwrap().destination, "meshes/a.xml");
        // two bodies break "exactly one"; a hidden step's option is refused; a typo is refused
        assert!(
            c.resolve(&["Body/Body type/3BA".into(), "Body/Body type/CBBE".into()])
                .is_err()
        );
        assert!(c.resolve(&["Physics/SMP/SMP".into()]).is_err());
        assert!(c.resolve(&["Body/Body type/UBE".into()]).is_err());
    }

    #[test]
    fn utf16_configs_are_read() {
        let mut b = vec![0xFF, 0xFE];
        for u in XML.encode_utf16() {
            b.extend(u.to_le_bytes());
        }
        assert_eq!(parse(&b).unwrap().name, "Test Mod");
    }
}
