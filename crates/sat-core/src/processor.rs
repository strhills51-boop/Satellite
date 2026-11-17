//! Processors: external generators a commit runs, their output collected into a mod (ADR 0021). @sat:processor
//!
//! Owns: a processor's definition in the project (the MO2 tool it runs, what its inputs are, its output mod, what
//! runs before it), the built-in presets that recognise common tools (Pandora, Nemesis, BodySlide, DynDOLOD,
//! xLODGen, TexGen, Synthesis), the fingerprint of the inputs and whether a processor is dirty, and the order the
//! processors run in (dependencies, cycles refused).
//! Not here: reading MO2's tools (`@sat:mo2.executables`), running and collecting (`@sat:mo2.commit`), the plan
//! (`@sat:home.plan.processors`). Internal processors — factories (`@sat:factory`).
//!
//! The monolith's deployment contract (pass S22–S23) is kept in its essence: inputs by selector, a fingerprint of
//! inputs + tool + settings, a dirty processor rebuilt by a commit, a run that went stale while it ran not promoted,
//! the old output kept for the rollback. Its fact routing becomes a recomputed fingerprint: the inputs are known.

use serde::{Deserialize, Serialize};

/// What a processor reads: the mods that hold any of these folders, or plugins at their root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selector {
    /// Folders (game paths, `/`) a mod must hold to be an input: `meshes/actors/character/behaviors`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<String>,
    /// Plugins (`.esp/.esm/.esl` at the mod's root) make a mod an input.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub plugins: bool,
}

impl Selector {
    /// Whether a mod is an input, given whether a folder exists in it and whether it has plugins at its root.
    pub fn selects(&self, has_folder: impl Fn(&str) -> bool, has_plugins: bool) -> bool {
        (self.plugins && has_plugins) || self.folders.iter().any(|f| has_folder(f))
    }
}

// @sat:processor.def
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Processor {
    /// Stable id in the project (`pandora`, `bodyslide`, …).
    pub id: String,
    pub title: String,
    /// The tool by its MO2 title (`[customExecutables]`); the path is this machine's.
    pub executable: String,
    #[serde(default)]
    pub inputs: Selector,
    /// The output mod the generated files are collected into.
    pub output: String,
    /// Processors that run before this one (their outputs are its inputs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Fingerprint of the inputs at the last promoted run; `None` — never built.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built: Option<String>,
}

fn yes() -> bool {
    true
}

/// A tool Satellite recognises by its executable's file name.
#[derive(Debug, Clone, Copy)]
pub struct Preset {
    pub id: &'static str,
    pub title: &'static str,
    /// Lowercased substrings of the binary's file name.
    pub binaries: &'static [&'static str],
    pub folders: &'static [&'static str],
    pub plugins: bool,
    pub output: &'static str,
    pub after: &'static [&'static str],
}

// @sat:processor.presets
/// The common generators of a Skyrim build. Order matters only for display; `after` decides the run order.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "bodyslide",
        title: "BodySlide",
        binaries: &["bodyslide"],
        folders: &["calientetools/bodyslide"],
        plugins: false,
        output: "BodySlide Output",
        after: &[],
    },
    Preset {
        id: "pandora",
        title: "Pandora",
        binaries: &["pandora"],
        folders: &[
            "pandora_engine",
            "nemesis_engine",
            "meshes/actors/character/behaviors",
            "meshes/actors/character/animations",
        ],
        plugins: false,
        output: "Pandora Output",
        after: &[],
    },
    Preset {
        id: "nemesis",
        title: "Nemesis",
        binaries: &["nemesis"],
        folders: &[
            "nemesis_engine",
            "meshes/actors/character/behaviors",
            "meshes/actors/character/animations",
        ],
        plugins: false,
        output: "Nemesis Output",
        after: &[],
    },
    Preset {
        id: "texgen",
        title: "TexGen",
        binaries: &["texgen"],
        folders: &["textures/terrain", "textures/lod"],
        plugins: true,
        output: "TexGen Output",
        after: &[],
    },
    Preset {
        id: "xlodgen",
        title: "xLODGen",
        binaries: &["xlodgen"],
        folders: &["textures/terrain", "meshes/terrain"],
        plugins: true,
        output: "xLODGen Output",
        after: &[],
    },
    Preset {
        id: "dyndolod",
        title: "DynDOLOD",
        binaries: &["dyndolod"],
        folders: &["meshes/lod", "textures/lod", "dyndolod"],
        plugins: true,
        output: "DynDOLOD Output",
        after: &["texgen", "xlodgen"],
    },
    Preset {
        id: "synthesis",
        title: "Synthesis",
        binaries: &["synthesis"],
        folders: &[],
        plugins: true,
        output: "Synthesis Output",
        after: &["bodyslide", "dyndolod"],
    },
];

impl Preset {
    /// The processor this preset makes for a registered tool.
    pub fn processor(&self, executable: &str) -> Processor {
        Processor {
            id: self.id.into(),
            title: self.title.into(),
            executable: executable.into(),
            inputs: Selector {
                folders: self.folders.iter().map(|s| s.to_string()).collect(),
                plugins: self.plugins,
            },
            output: self.output.into(),
            after: self.after.iter().map(|s| s.to_string()).collect(),
            enabled: false,
            built: None,
        }
    }
}

// @sat:processor.detect
/// Processors for the registered tools Satellite recognises: (MO2 title, binary file name) → one per preset, the
/// first matching tool; the ones the project already has are left out. New ones start disabled — the user turns
/// them on.
pub fn detect(tools: &[(String, String)], have: &[Processor]) -> Vec<Processor> {
    let mut out: Vec<Processor> = Vec::new();
    for p in PRESETS {
        if have.iter().any(|h| h.id == p.id) || out.iter().any(|o| o.id == p.id) {
            continue;
        }
        if let Some((title, _)) = tools
            .iter()
            .find(|(_, bin)| p.binaries.iter().any(|b| bin.contains(b)))
        {
            out.push(p.processor(title));
        }
    }
    // the dependencies point only at processors that exist
    let ids: Vec<String> = have.iter().chain(&out).map(|p| p.id.clone()).collect();
    for p in &mut out {
        p.after.retain(|a| ids.contains(a));
    }
    out
}

// @sat:processor.fingerprint
/// Fingerprint of a processor's run: the input mods in order with their content signatures, the tool's identity
/// (size and time of the binary) and the settings that change the output.
pub fn fingerprint(p: &Processor, inputs: &[(String, Option<u64>)], tool: Option<(u64, u64)>) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: &[u8]| {
        for &x in b {
            h ^= u64::from(x);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0100_0000_01b3);
    };
    eat(p.executable.to_lowercase().as_bytes());
    eat(p.output.to_lowercase().as_bytes());
    for f in &p.inputs.folders {
        eat(f.as_bytes());
    }
    eat(&[u8::from(p.inputs.plugins)]);
    if let Some((size, time)) = tool {
        eat(&size.to_le_bytes());
        eat(&time.to_le_bytes());
    }
    for (name, sig) in inputs {
        eat(name.to_lowercase().as_bytes());
        eat(&sig.unwrap_or(0).to_le_bytes());
    }
    format!("{h:016x}")
}

/// Why a processor needs a run, if it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Dirty {
    NeverBuilt,
    InputsChanged,
    OutputMissing,
}

/// Whether a processor is dirty: never built, its inputs changed, or its output mod is not on disk.
pub fn dirty(p: &Processor, fp: &str, output_on_disk: bool) -> Option<Dirty> {
    match &p.built {
        None => Some(Dirty::NeverBuilt),
        Some(b) if b != fp => Some(Dirty::InputsChanged),
        Some(_) if !output_on_disk => Some(Dirty::OutputMissing),
        Some(_) => None,
    }
}

// @sat:processor.status
/// Where a processor stands against the build now (what the window shows).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum State {
    /// Turned off: a commit does not run it.
    Off,
    /// Its tool is not among MO2's registered executables on this machine.
    NoTool,
    /// It is in a cycle of `after`: nothing runs.
    Cycle { with: Vec<String> },
    /// Built from the inputs as they are.
    Clean,
    /// A commit runs it.
    Dirty { why: Dirty },
}

/// A processor's state with what it was assessed from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub id: String,
    #[serde(flatten)]
    pub state: State,
    /// The input mods, in order.
    pub inputs: Vec<String>,
    /// The fingerprint of the inputs now (empty when not assessed).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fingerprint: String,
}

// @sat:processor.order
/// The order to run processors in: each after the ones it names (unknown names ignored). A cycle is refused with
/// the processors in it.
pub fn order(ps: &[Processor]) -> Result<Vec<usize>, Vec<String>> {
    let idx = |id: &str| ps.iter().position(|p| p.id == id);
    let mut state = vec![0u8; ps.len()]; // 0 new, 1 visiting, 2 done
    let mut out = Vec::new();
    fn visit(
        i: usize,
        ps: &[Processor],
        idx: &dyn Fn(&str) -> Option<usize>,
        state: &mut [u8],
        out: &mut Vec<usize>,
        path: &mut Vec<String>,
    ) -> Result<(), Vec<String>> {
        match state[i] {
            2 => return Ok(()),
            1 => {
                path.push(ps[i].id.clone());
                return Err(path.clone());
            }
            _ => {}
        }
        state[i] = 1;
        path.push(ps[i].id.clone());
        for a in &ps[i].after {
            if let Some(j) = idx(a) {
                visit(j, ps, idx, state, out, path)?;
            }
        }
        path.pop();
        state[i] = 2;
        out.push(i);
        Ok(())
    }
    for i in 0..ps.len() {
        visit(i, ps, &idx, &mut state, &mut out, &mut Vec::new())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(t, b)| (t.to_string(), b.to_string())).collect()
    }

    #[test]
    fn presets_recognise_tools_and_run_in_order() {
        let found = detect(
            &tools(&[
                ("BodySlide x64", "bodyslide x64.exe"),
                ("Pandora", "pandora behaviour engine+.exe"),
                ("DynDOLOD", "dyndolodx64.exe"),
                ("Synthesis", "synthesis.exe"),
                ("SSEEdit", "sseedit.exe"),
            ]),
            &[],
        );
        let ids: Vec<&str> = found.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["bodyslide", "pandora", "dyndolod", "synthesis"]);
        assert!(found.iter().all(|p| !p.enabled), "found tools start disabled");
        assert!(
            found[2].after.is_empty(),
            "DynDOLOD's TexGen / xLODGen are not here"
        );
        let ord = order(&found).unwrap();
        let pos = |id: &str| ord.iter().position(|&i| found[i].id == id).unwrap();
        assert!(pos("synthesis") > pos("bodyslide") && pos("synthesis") > pos("dyndolod"));
        // a cycle is refused
        let mut cyc = found.clone();
        cyc[0].after = vec!["synthesis".into()];
        assert!(order(&cyc).is_err());
        // already in the project: not found again
        assert!(detect(&tools(&[("Pandora", "pandora.exe")]), &found).is_empty());
    }

    #[test]
    fn dirty_follows_inputs_tool_and_output() {
        let mut p = PRESETS[1].processor("Pandora");
        let a = fingerprint(&p, &[("A".into(), Some(1))], Some((10, 20)));
        assert_eq!(dirty(&p, &a, false), Some(Dirty::NeverBuilt));
        p.built = Some(a.clone());
        assert_eq!(dirty(&p, &a, true), None);
        assert_eq!(dirty(&p, &a, false), Some(Dirty::OutputMissing));
        let b = fingerprint(&p, &[("A".into(), Some(2))], Some((10, 20)));
        assert_eq!(
            dirty(&p, &b, true),
            Some(Dirty::InputsChanged),
            "an input's content changed"
        );
        let c = fingerprint(&p, &[("A".into(), Some(1))], Some((11, 20)));
        assert_ne!(a, c, "a new version of the tool");
        assert!(p.inputs.selects(|f| f == "nemesis_engine", false));
        assert!(
            !p.inputs.selects(|_| false, true),
            "Pandora does not read plugins"
        );
    }
}
