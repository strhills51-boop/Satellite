//! Commands as resources: built-in operations a key, menu or palette can trigger. @sat:resource.command
//!
//! Owns: the table of built-in commands (`core.*`), the `op` parameter vocabulary of a command manifest, and the
//! dispatch of a command to its truth owner with a capability check.
//! Not here: the operations themselves (`@sat:order.cursor.navigate`, `@sat:order.cursor.shift`, …), keys
//! (`@sat:resource.hotkeys`), how changes reach the graph (`@sat:session.command`).
//!
//! A command manifest says *which* built-in operation it is (`op`) and its arguments; it never carries code.
//! An extension can therefore add a command only by combining existing operations (presets, aliases), and it
//! needs the capability the operation needs: moving or toggling mods requires `write-canon`.

use super::{Capability, Entry, Kind, Manifest};
use crate::order::{self, Nav, OrderBook, OrderError, Select, Shift, ViewId};

/// A parsed command operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Navigate(Nav, Select),
    Shift(Shift),
    ToggleFocused,
    SelectAll,
    ClearSelection,
    /// ← / → on the focused row.
    Fold {
        expand: bool,
    },
    /// Enable / disable the selection (toggle).
    ToggleEnabled,
}

impl Action {
    // @sat:resource.command.parse
    /// Reads `op` and its arguments from manifest params.
    pub fn parse(m: &Manifest) -> Result<Self, String> {
        let get = |k: &str| m.params.get(k).map(String::as_str);
        let select = || match get("select").unwrap_or("replace") {
            "replace" => Ok(Select::Replace),
            "extend" => Ok(Select::Extend),
            "toggle" => Ok(Select::Toggle),
            "focus-only" => Ok(Select::FocusOnly),
            s => Err(format!("unknown select {s:?}")),
        };
        let page = || {
            get("page")
                .unwrap_or("20")
                .parse::<u32>()
                .map_err(|e| e.to_string())
        };
        Ok(match get("op").ok_or("missing op")? {
            "navigate" => {
                let nav = match get("to").ok_or("navigate needs `to`")? {
                    "up" => Nav::Up,
                    "down" => Nav::Down,
                    "page-up" => Nav::PageUp(page()?),
                    "page-down" => Nav::PageDown(page()?),
                    "home" => Nav::Home,
                    "end" => Nav::End,
                    s => return Err(format!("unknown navigation {s:?}")),
                };
                Action::Navigate(nav, select()?)
            }
            "shift" => Action::Shift(match get("to").ok_or("shift needs `to`")? {
                "up" => Shift::Up,
                "down" => Shift::Down,
                "block-start" => Shift::BlockStart,
                "block-end" => Shift::BlockEnd,
                s => return Err(format!("unknown shift {s:?}")),
            }),
            "toggle-focused" => Action::ToggleFocused,
            "select-all" => Action::SelectAll,
            "clear-selection" => Action::ClearSelection,
            "fold" => Action::Fold { expand: false },
            "unfold" => Action::Fold { expand: true },
            "toggle-enabled" => Action::ToggleEnabled,
            s => return Err(format!("unknown op {s:?}")),
        })
    }

    /// What the operation needs beyond reading the canon.
    pub fn needs(&self) -> &'static [Capability] {
        match self {
            Action::Shift(_) | Action::ToggleEnabled => &[Capability::ReadCanon, Capability::WriteCanon],
            _ => &[Capability::ReadCanon],
        }
    }

    /// Runs on the book in view `v`.
    pub fn apply(&self, book: &mut OrderBook, v: ViewId) -> Result<Option<order::Change>, OrderError> {
        Ok(match *self {
            Action::Navigate(nav, sel) => Some(book.navigate(v, nav, sel)?),
            Action::Shift(how) => book.shift_selection(v, how)?,
            Action::ToggleFocused => book.toggle_focused(v),
            Action::SelectAll => Some(book.select_all(v)?),
            Action::ClearSelection => Some(book.clear_selection(v)?),
            Action::Fold { expand } => book.fold_focused(v, expand)?,
            Action::ToggleEnabled => {
                let keys = book.selection(v);
                book.set_enabled(&keys, None)?
            }
        })
    }
}

/// Why a command did not run.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("unknown command {0}")]
    Unknown(String),
    #[error("{0} is not a command")]
    NotACommand(String),
    #[error("command {0} is not active")]
    Inactive(String),
    #[error("command {0} lacks capability {1:?}")]
    Denied(String, Capability),
    #[error("command {0}: {1}")]
    Bad(String, String),
    #[error(transparent)]
    Order(#[from] OrderError),
}

// @sat:resource.command.check
/// Resolves a command resource to an action it is allowed to run (runtime check of granted capabilities).
pub fn authorize(e: Option<&Entry>, id: &str) -> Result<Action, CommandError> {
    let e = e.ok_or_else(|| CommandError::Unknown(id.into()))?;
    if e.manifest.kind != Kind::Command {
        return Err(CommandError::NotACommand(id.into()));
    }
    if !e.decision.is_active() {
        return Err(CommandError::Inactive(id.into()));
    }
    let action = Action::parse(&e.manifest).map_err(|w| CommandError::Bad(id.into(), w))?;
    if let Some(&c) = action.needs().iter().find(|c| !e.allows(**c)) {
        return Err(CommandError::Denied(id.into(), c));
    }
    Ok(action)
}

fn cmd(id: &str, title: &str, params: &[(&str, &str)]) -> Manifest {
    let mut m = Manifest::new(id, Kind::Command, title);
    for (k, v) in params {
        m = m.param(k, v);
    }
    let a = Action::parse(&m).expect("built-in command parses");
    m.requiring(a.needs())
}

// @sat:resource.command.builtins
/// Built-in commands: the keyboard cursor of ADR 0008 as resources.
pub fn builtin_commands() -> Vec<Manifest> {
    let mut v = Vec::new();
    for (to, name) in [
        ("up", "up"),
        ("down", "down"),
        ("page-up", "page up"),
        ("page-down", "page down"),
        ("home", "home"),
        ("end", "end"),
    ] {
        for (sel, suffix, what) in [
            ("replace", "", "Move focus"),
            ("extend", ".extend", "Extend selection"),
            ("focus-only", ".focus", "Move focus only"),
        ] {
            v.push(cmd(
                &format!("core.cursor.{to}{suffix}"),
                &format!("{what} {name}"),
                &[("op", "navigate"), ("to", to), ("select", sel)],
            ));
        }
    }
    for (to, title) in [
        ("up", "Move selection up"),
        ("down", "Move selection down"),
        ("block-start", "Move selection to block start"),
        ("block-end", "Move selection to block end"),
    ] {
        v.push(cmd(
            &format!("core.order.shift-{to}"),
            title,
            &[("op", "shift"), ("to", to)],
        ));
    }
    v.push(cmd(
        "core.cursor.toggle",
        "Toggle focused in selection",
        &[("op", "toggle-focused")],
    ));
    v.push(cmd(
        "core.cursor.select-all",
        "Select all",
        &[("op", "select-all")],
    ));
    v.push(cmd(
        "core.cursor.clear",
        "Clear selection",
        &[("op", "clear-selection")],
    ));
    v.push(cmd(
        "core.cursor.fold",
        "Collapse / go to separator",
        &[("op", "fold")],
    ));
    v.push(cmd("core.cursor.unfold", "Expand", &[("op", "unfold")]));
    v.push(cmd(
        "core.order.toggle-enabled",
        "Enable / disable selection",
        &[("op", "toggle-enabled")],
    ));
    v
}
