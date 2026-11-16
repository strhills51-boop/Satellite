//! The workspace (ADR 0009 «Холст, воркфлоу и камера», ADR 0020 §10): windows on one canvas, workflows, the camera. @sat:space
//!
//! Owns: the workspace as data — windows (a kind, a rect on the canvas, the workflow it belongs to), workflows
//! (named dock groups the bookmarks point at), the camera — and its checks (unique ids, known kinds, existing
//! workflows, sane sizes). The window draws and moves it; the project keeps it.
//! Not here: what a window shows (the panels of the window), views of the order (`@sat:order.book`).

use serde::{Deserialize, Serialize};

/// The kinds of windows a workspace can hold today: the panels of the classic screen.
pub const KINDS: &[&str] = &["mods", "meta", "separators"];

// @sat:space.data
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    #[serde(default)]
    pub windows: Vec<SpaceWindow>,
    #[serde(default)]
    pub workflows: Vec<Workflow>,
    #[serde(default)]
    pub camera: Camera,
}

/// A window on the canvas (canvas pixels).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceWindow {
    pub id: String,
    pub kind: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// The workflow (dock group) it belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
}

/// A named group of windows: a bookmark on top flies the camera to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workflow {
    pub id: String,
    pub name: String,
}

/// The canvas point at the viewport's top-left corner and the zoom in percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Camera {
    pub x: i32,
    pub y: i32,
    pub zoom: u16,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            zoom: 100,
        }
    }
}

/// The smallest window and the zoom range, percent (old Satellite: 0.35–2.25).
pub const MIN_SIDE: i32 = 120;
pub const ZOOM: (u16, u16) = (35, 225);

impl Space {
    // @sat:space.check
    /// What is wrong with a workspace, if anything: duplicate ids, unknown kinds, a workflow nobody declared, a
    /// window too small or not a number, a zoom out of range.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut ids = std::collections::HashSet::new();
        for w in &self.windows {
            if !ids.insert(w.id.as_str()) {
                out.push(format!("window {:?}: the id repeats", w.id));
            }
            if !KINDS.contains(&w.kind.as_str()) {
                out.push(format!("window {:?}: unknown kind {:?}", w.id, w.kind));
            }
            if w.w < MIN_SIDE || w.h < MIN_SIDE {
                out.push(format!("window {:?}: bad rect", w.id));
            }
            if let Some(g) = &w.workflow
                && !self.workflows.iter().any(|f| &f.id == g)
            {
                out.push(format!("window {:?}: no workflow {g:?}", w.id));
            }
        }
        let mut wf = std::collections::HashSet::new();
        for f in &self.workflows {
            if !wf.insert(f.id.as_str()) {
                out.push(format!("workflow {:?}: the id repeats", f.id));
            }
        }
        let c = self.camera;
        if !(ZOOM.0..=ZOOM.1).contains(&c.zoom) {
            out.push("camera out of range".into());
        }
        // a kind of panel exists once: the panels of the screen are moved, not copied
        let mut kinds = std::collections::HashSet::new();
        for w in &self.windows {
            if !kinds.insert(w.kind.as_str()) {
                out.push(format!("kind {:?} twice", w.kind));
            }
        }
        out
    }

    /// The built-in workspace: the classic screen laid out on the canvas as one workflow.
    pub fn classic() -> Self {
        let win = |id: &str, kind: &str, x: i32, w: i32| SpaceWindow {
            id: id.into(),
            kind: kind.into(),
            x,
            y: 0,
            w,
            h: 760,
            workflow: Some("main".into()),
        };
        Self {
            windows: vec![
                win("seps", "separators", 0, 230),
                win("mods", "mods", 238, 900),
                win("meta", "meta", 1146, 380),
            ],
            workflows: vec![Workflow {
                id: "main".into(),
                name: "Сборка".into(),
            }],
            camera: Camera::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_classic_space_is_valid_and_problems_are_found() {
        assert!(Space::classic().problems().is_empty());
        let mut s = Space::classic();
        s.windows[1].kind = "nope".into();
        s.windows[2].workflow = Some("gone".into());
        s.windows[0].w = 10;
        s.windows.push(s.windows[0].clone());
        s.camera.zoom = 900;
        let p = s.problems();
        assert!(p.iter().any(|x| x.contains("unknown kind")));
        assert!(p.iter().any(|x| x.contains("no workflow")));
        assert!(p.iter().any(|x| x.contains("bad rect")));
        assert!(p.iter().any(|x| x.contains("repeats")));
        assert!(p.iter().any(|x| x.contains("camera")));
        // it travels with the project as JSON
        let j = serde_json::to_string(&Space::classic()).unwrap();
        assert_eq!(serde_json::from_str::<Space>(&j).unwrap(), Space::classic());
    }
}
