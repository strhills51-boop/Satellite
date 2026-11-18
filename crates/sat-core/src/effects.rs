//! Effects of the list: the visual layers of a mod container, each on or off with its settings. @sat:effects
//!
//! Owns: the effects and their settings as the project keeps them — tags (stripes on the contour), badges (overwrite
//! badges), statistics (W / L / M / + counters and the plugin count), counter (the priority number), indicator (the
//! toggle and what the mod's plugins put on it), hover cloud (a mod's tags, plugins and note on hover), colour focus
//! (dim the colours outside the object in focus) — their defaults and their bounds.
//! Not here: drawing them (the window), what they show (`@sat:plugins.mod-mark`, `@sat:app.badges`, the counters).
//!
//! Parity: monolith `EFFECT_IDS`, `createDefaultEffectSettings`, `normalizeEffectSettings` (v438 CP1, v367 CP7,
//! v650 canonicality) and the effects meta block `renderEffectsMetaBlock` (v362). An effect is a layer of the view,
//! not of the build: switching one off hides the layer and changes nothing else.

use serde::{Deserialize, Serialize};

/// Where an effect is drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Area {
    #[default]
    All,
    Mods,
    Separators,
}

/// How overwrite badges are drawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BadgeModel {
    /// A pill per context with the icons of its chains (monolith «классические цепочки»).
    #[default]
    Classic,
    /// One `OW N` summary.
    Summary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tags {
    pub enabled: bool,
    pub area: Area,
    /// Percent, 20…100.
    pub opacity: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Badges {
    pub enabled: bool,
    pub model: BadgeModel,
    /// Contexts a mod container shows before «+N»: 3 (compact), 6 (normal), 12 (free).
    pub density: u8,
    /// Percent, 25…100.
    pub opacity: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Statistics {
    pub enabled: bool,
    /// W / L / M / + of a mod.
    pub conflicts: bool,
    /// The plugin count of a mod.
    pub plugins: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Counter {
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Indicator {
    pub enabled: bool,
    /// Pixels, 8…16.
    pub size: u8,
    /// The toggle takes the colour of the mod's plugins (`@sat:plugins.mod-mark`).
    pub plugin_colours: bool,
    /// The other marks of the mod's plugins stack around the toggle as a ring.
    pub stack: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HoverCloud {
    pub enabled: bool,
    /// Milliseconds before the cloud opens, 100…1500.
    pub delay: u16,
    pub tags: bool,
    pub plugins: bool,
    pub note: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColourFocus {
    pub enabled: bool,
    /// Saturation of what is out of focus, percent 0…65.
    pub dim_saturation: u8,
    /// Brightness of what is out of focus, percent 65…100.
    pub dim_brightness: u8,
}

// @sat:effects.data
/// The effects of the list, as the project keeps them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Effects {
    pub tags: Tags,
    pub badges: Badges,
    pub statistics: Statistics,
    pub counter: Counter,
    pub indicator: Indicator,
    pub hover_cloud: HoverCloud,
    pub colour_focus: ColourFocus,
}

impl Default for Tags {
    fn default() -> Self {
        Self {
            enabled: true,
            area: Area::All,
            opacity: 100,
        }
    }
}

impl Default for Badges {
    fn default() -> Self {
        Self {
            enabled: true,
            model: BadgeModel::Classic,
            density: 6,
            opacity: 100,
        }
    }
}

impl Default for Statistics {
    fn default() -> Self {
        Self {
            enabled: true,
            conflicts: true,
            plugins: true,
        }
    }
}

impl Default for Counter {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl Default for Indicator {
    fn default() -> Self {
        Self {
            enabled: true,
            size: 10,
            plugin_colours: true,
            stack: true,
        }
    }
}

impl Default for HoverCloud {
    fn default() -> Self {
        Self {
            enabled: true,
            delay: 450,
            tags: true,
            plugins: true,
            note: true,
        }
    }
}

impl Default for ColourFocus {
    fn default() -> Self {
        Self {
            enabled: false,
            dim_saturation: 16,
            dim_brightness: 88,
        }
    }
}

impl Effects {
    // @sat:effects.normalize
    /// The same effects with every number inside its bounds (monolith `normalizeEffectSettings`): a project written
    /// by hand or by another build never draws a broken layer.
    pub fn normalized(mut self) -> Self {
        self.tags.opacity = self.tags.opacity.clamp(20, 100);
        self.badges.opacity = self.badges.opacity.clamp(25, 100);
        self.badges.density = match self.badges.density {
            0..=4 => 3,
            5..=8 => 6,
            _ => 12,
        };
        self.indicator.size = self.indicator.size.clamp(8, 16);
        self.hover_cloud.delay = self.hover_cloud.delay.clamp(100, 1500);
        self.colour_focus.dim_saturation = self.colour_focus.dim_saturation.min(65);
        self.colour_focus.dim_brightness = self.colour_focus.dim_brightness.clamp(65, 100);
        self
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_take_defaults_and_numbers_stay_in_bounds() {
        let e: Effects =
            serde_json::from_str(r#"{"indicator":{"size":40},"badges":{"density":7,"opacity":0}}"#).unwrap();
        let e = e.normalized();
        assert!(
            e.indicator.enabled && e.indicator.plugin_colours,
            "fields not written take defaults"
        );
        assert_eq!(e.indicator.size, 16);
        assert_eq!((e.badges.density, e.badges.opacity), (6, 25));
        assert!(!e.colour_focus.enabled);
        assert!(Effects::default().is_default());
    }
}
