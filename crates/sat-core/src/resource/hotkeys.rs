//! Key bindings as resources: chord → command in a context, with explicit conflicts. @sat:resource.hotkeys
//!
//! Owns: the chord grammar (`Ctrl+Alt+Shift+Meta+Key`, canonical spelling), the built-in key layout, and the
//! key map built from active hotkey resources (precedence by load source, conflicts reported, never guessed).
//! Not here: what a command does (`@sat:resource.command`), the cursor itself (`@sat:order.cursor.navigate`).
//!
//! A hotkey manifest: `kind = hotkey`, params `chord`, `command` (a command id, or `-` to unbind the chord),
//! `context` (`list` — a list window; `global` — anywhere). It depends on its command, so a binding to a
//! missing or unapproved command is blocked, not silently dead.

use std::collections::BTreeMap;
use std::fmt;

use super::{Entry, Kind, Manifest, ResourceId, Source};

const NAMED: &[&str] = &[
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "PageUp",
    "PageDown",
    "Home",
    "End",
    "Space",
    "Enter",
    "Escape",
    "Tab",
    "Delete",
    "Backspace",
    "Insert",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
];

const CONTEXTS: &[&str] = &["list", "global"];

// @sat:resource.hotkeys.chord
/// A key chord in canonical form.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Chord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
    /// A named key (`ArrowDown`) or one uppercase character (`A`, `1`).
    pub key: String,
}

impl Chord {
    /// Parses `Shift+alt+arrowdown` and the like; modifiers in any order and case.
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut c = Chord {
            ctrl: false,
            alt: false,
            shift: false,
            meta: false,
            key: String::new(),
        };
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let (key, mods) = parts.split_last().ok_or("empty chord")?;
        for m in mods {
            let flag = match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut c.ctrl,
                "alt" | "option" => &mut c.alt,
                "shift" => &mut c.shift,
                "meta" | "cmd" | "win" | "super" => &mut c.meta,
                other => return Err(format!("unknown modifier {other:?}")),
            };
            if std::mem::replace(flag, true) {
                return Err(format!("modifier {m:?} repeated"));
            }
        }
        c.key = if let Some(n) = NAMED.iter().find(|n| n.eq_ignore_ascii_case(key)) {
            (*n).to_string()
        } else {
            let mut ch = key.chars();
            match (ch.next(), ch.next()) {
                (Some(k), None) if !k.is_control() && !k.is_whitespace() => k.to_uppercase().collect(),
                _ => return Err(format!("unknown key {key:?}")),
            }
        };
        Ok(c)
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (self.meta, "Meta"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&self.key)
    }
}

/// Manifest checks specific to hotkeys (called by the registry on register).
pub(super) fn validate(m: &Manifest) -> Result<(), String> {
    let chord = m.params.get("chord").ok_or("hotkey needs `chord`")?;
    Chord::parse(chord)?;
    let ctx = m.params.get("context").map(String::as_str).unwrap_or("list");
    if !CONTEXTS.contains(&ctx) {
        return Err(format!("unknown context {ctx:?}"));
    }
    match m.params.get("command").map(String::as_str) {
        None => Err("hotkey needs `command`".into()),
        Some("-") => Ok(()),
        Some(c) if m.depends.contains(c) => Ok(()),
        Some(c) => Err(format!("hotkey must depend on its command {c}")),
    }
}

/// A binding that lost or tied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyConflict {
    pub context: String,
    pub chord: String,
    /// Hotkey resources of equal precedence claiming the chord; none of them is bound.
    pub claimants: Vec<ResourceId>,
}

// @sat:resource.hotkeys.keymap
/// Active bindings: `(context, chord) → command`.
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    bound: BTreeMap<(String, String), ResourceId>,
    pub conflicts: Vec<HotkeyConflict>,
}

impl Keymap {
    /// Highest source precedence wins a chord (the user's own keymap over a project's over built-in); a tie
    /// at the top is a conflict and leaves the chord unbound until the user resolves it.
    pub fn build<'a>(hotkeys: impl Iterator<Item = &'a Entry>) -> Self {
        let mut claims: BTreeMap<(String, String), Vec<(Source, &'a Entry)>> = BTreeMap::new();
        for e in hotkeys.filter(|e| e.manifest.kind == Kind::Hotkey) {
            let p = &e.manifest.params;
            let Ok(chord) = Chord::parse(&p["chord"]) else {
                continue;
            };
            let ctx = p.get("context").cloned().unwrap_or_else(|| "list".into());
            claims
                .entry((ctx, chord.to_string()))
                .or_default()
                .push((e.source, e));
        }
        let mut map = Keymap::default();
        for (slot, mut cs) in claims {
            let top = cs.iter().map(|(s, _)| s.precedence()).max().expect("non-empty");
            cs.retain(|(s, _)| s.precedence() == top);
            if cs.len() > 1 {
                map.conflicts.push(HotkeyConflict {
                    context: slot.0.clone(),
                    chord: slot.1.clone(),
                    claimants: cs.iter().map(|(_, e)| e.manifest.id.clone()).collect(),
                });
                continue;
            }
            let cmd = &cs[0].1.manifest.params["command"];
            if cmd != "-" {
                map.bound.insert(slot, cmd.clone());
            }
        }
        map
    }

    /// Command for a chord: the context first, then `global`.
    pub fn lookup(&self, context: &str, chord: &Chord) -> Option<&str> {
        let c = chord.to_string();
        self.bound
            .get(&(context.to_string(), c.clone()))
            .or_else(|| self.bound.get(&("global".to_string(), c)))
            .map(String::as_str)
    }

    pub fn bindings(&self) -> impl Iterator<Item = (&str, &str, &str)> {
        self.bound
            .iter()
            .map(|((ctx, ch), cmd)| (ctx.as_str(), ch.as_str(), cmd.as_str()))
    }
}

/// A hotkey manifest binding `chord` to `command` in `context`.
pub fn binding(id: &str, chord: &str, command: &str, context: &str) -> Manifest {
    let m = Manifest::new(id, Kind::Hotkey, &format!("{chord} → {command}"))
        .param("chord", chord)
        .param("command", command)
        .param("context", context);
    if command == "-" {
        m
    } else {
        m.depending(&[command])
    }
}

// @sat:resource.hotkeys.builtins
/// The built-in layout of the list window (ADR 0008): arrows move the focus, Shift extends, Ctrl moves the
/// focus only, Space toggles, Alt moves the selection, Alt+Shift by blocks, ← → fold, Ctrl+A / Escape.
pub fn builtin_hotkeys() -> Vec<Manifest> {
    let mut v = Vec::new();
    for (key, to) in [
        ("ArrowUp", "up"),
        ("ArrowDown", "down"),
        ("PageUp", "page-up"),
        ("PageDown", "page-down"),
        ("Home", "home"),
        ("End", "end"),
    ] {
        for (mods, suffix) in [("", ""), ("Shift+", ".extend"), ("Ctrl+", ".focus")] {
            let chord = format!("{mods}{key}");
            let id = format!("core.key.{}", chord.to_lowercase().replace('+', "-"));
            v.push(binding(&id, &chord, &format!("core.cursor.{to}{suffix}"), "list"));
        }
    }
    for (chord, cmd) in [
        ("Alt+ArrowUp", "core.order.shift-up"),
        ("Alt+ArrowDown", "core.order.shift-down"),
        ("Alt+Shift+ArrowUp", "core.order.shift-block-start"),
        ("Alt+Shift+ArrowDown", "core.order.shift-block-end"),
        ("Space", "core.cursor.toggle"),
        ("Ctrl+A", "core.cursor.select-all"),
        ("Escape", "core.cursor.clear"),
        ("ArrowLeft", "core.cursor.fold"),
        ("ArrowRight", "core.cursor.unfold"),
        ("Enter", "core.order.toggle-enabled"),
    ] {
        let id = format!("core.key.{}", chord.to_lowercase().replace('+', "-"));
        v.push(binding(&id, chord, cmd, "list"));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::super::{Approvals, Capability, Registry, command};
    use super::*;
    use crate::canon::{Entry as CEntry, EntryKind, Modlist};
    use crate::order::{MAIN, OrderBook};

    #[test]
    fn chords_are_canonical() {
        assert_eq!(
            Chord::parse("shift+alt+arrowdown").unwrap().to_string(),
            "Alt+Shift+ArrowDown"
        );
        assert_eq!(Chord::parse("ctrl+a").unwrap().to_string(), "Ctrl+A");
        assert!(Chord::parse("Ctrl+Ctrl+A").is_err());
        assert!(Chord::parse("Hyper+A").is_err());
        assert!(Chord::parse("Ctrl+Banana").is_err());
    }

    #[test]
    fn builtin_layout_is_complete_and_conflict_free() {
        let r = Registry::with_builtins();
        assert_eq!(r.pending().count(), 0);
        assert!(
            r.entries().all(|e| e.decision.is_active()),
            "built-ins are active"
        );
        let km = r.keymap();
        assert!(km.conflicts.is_empty());
        assert_eq!(km.bindings().count(), 28);
        let alt_down = Chord::parse("Alt+ArrowDown").unwrap();
        assert_eq!(km.lookup("list", &alt_down), Some("core.order.shift-down"));
    }

    #[test]
    fn user_keymap_overrides_and_ties_conflict() {
        let mut r = Registry::with_builtins();
        // the user rebinds Enter to "select all" and unbinds Escape
        r.register(
            binding("me.enter", "Enter", "core.cursor.select-all", "list"),
            Source::Machine,
        )
        .unwrap();
        r.register(binding("me.no-esc", "Escape", "-", "list"), Source::Machine)
            .unwrap();
        // two project keymaps fight over F5; a third binds a command that is not approved
        r.register(
            binding("p.one", "F5", "core.cursor.clear", "list"),
            Source::Project,
        )
        .unwrap();
        r.register(
            binding("p.two", "F5", "core.cursor.toggle", "list"),
            Source::Project,
        )
        .unwrap();
        let evil = Manifest::new("p.wipe", Kind::Command, "x")
            .param("op", "toggle-enabled")
            .requiring(&[Capability::ReadCanon, Capability::WriteCanon]);
        r.register(evil, Source::Project).unwrap();
        r.register(binding("p.f6", "F6", "p.wipe", "list"), Source::Project)
            .unwrap();
        r.resolve(&Approvals::default());

        let km = r.keymap();
        let k = |s: &str| Chord::parse(s).unwrap();
        assert_eq!(km.lookup("list", &k("Enter")), Some("core.cursor.select-all"));
        assert_eq!(km.lookup("list", &k("Escape")), None);
        assert_eq!(km.lookup("list", &k("F5")), None);
        assert_eq!(km.conflicts.len(), 1);
        assert_eq!(
            km.conflicts[0].claimants,
            vec!["p.one".to_string(), "p.two".to_string()]
        );
        assert_eq!(
            km.lookup("list", &k("F6")),
            None,
            "binding to an unapproved command is blocked"
        );
        assert!(matches!(
            command::authorize(r.get("p.wipe"), "p.wipe"),
            Err(command::CommandError::Inactive(_))
        ));
    }

    #[test]
    fn keys_drive_the_book() {
        let list = Modlist::new(
            ["A", "B", "C"]
                .iter()
                .map(|n| CEntry {
                    name: n.to_string(),
                    kind: EntryKind::Mod,
                    enabled: true,
                })
                .collect(),
        );
        let mut book = OrderBook::from_modlist(&list);
        let r = Registry::with_builtins();
        let km = r.keymap();
        let press = |book: &mut OrderBook, chord: &str| {
            let id = km.lookup("list", &Chord::parse(chord).unwrap()).unwrap();
            command::authorize(r.get(id), id)
                .unwrap()
                .apply(book, MAIN)
                .unwrap();
        };
        press(&mut book, "ArrowDown"); // focus A
        press(&mut book, "Alt+ArrowDown"); // A below B
        assert_eq!(
            book.to_modlist()
                .entries
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["B", "A", "C"]
        );
        press(&mut book, "Enter"); // disable A
        assert!(!book.to_modlist().entries[1].enabled);
    }
}
