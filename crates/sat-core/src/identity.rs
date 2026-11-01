//! Visual identity of chains and contexts: a stable emoji and hue from content, never from order. @sat:identity
//!
//! Owns: the rule that turns a set of member mods into a signature, an emoji and a hue — what the monolith's
//! classic overwrite badges show per chain (docs/semantic/monolith/12-mod-card.md §2).
//! Not here: the chains themselves (`@sat:overwrite.projection`), chain titles from paths (`@sat:app.mod-detail`).
//!
//! The signature hashes the **folded member keys, sorted**: moving mods or re-running the scan (which renumbers
//! mod ids) keeps the same icon and colour, so the user recognises a chain and its decisions stick to it. A key is
//! the mod's entity id when the project knows it (`@sat:session.member-keys`) — a rename keeps the chain — and
//! the folder name otherwise. (The old Satellite hashed members in load order, so a reorder changed every signature — a defect.)

use serde::{Deserialize, Serialize};

/// Icons for chains: distinct at a glance on a small dark badge.
const EMOJI: &[&str] = &[
    "🦊", "🐺", "🦉", "🐉", "🦌", "🐻", "🦅", "🐍", "🦂", "🐙", "🦋", "🐝", "🐢", "🦈", "🐸", "🦇", "🌲",
    "🌵", "🍄", "🌙", "⭐", "🔥", "💧", "❄️", "⚡", "🌋", "🌊", "🍁", "🌸", "🌾", "🪨", "🧊", "⚔️", "🛡️",
    "🏹", "🪓", "🗡️", "🔮", "📜", "🗝️", "💎", "⚙️", "🧪", "🕯️", "🏰", "⛺", "🧭", "🎯", "🎭", "🎲", "🧩",
    "🪶", "🦴", "🐚", "🪐", "☄️", "🌀", "🧿", "🔔", "⚓", "🪬", "🧸", "🎐", "🪁",
];

/// Identity of a chain or a context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// Content signature (FNV-1a 64 over the sorted folded member names).
    pub signature: u64,
    pub emoji: &'static str,
    /// Hue in degrees, 0..360.
    pub hue: u16,
}

// @sat:identity.of
/// Identity of a set of members given by name (order and case do not matter).
pub fn identity<'a>(members: impl IntoIterator<Item = &'a str>) -> Identity {
    let mut names: Vec<String> = members.into_iter().map(|n| n.trim().to_lowercase()).collect();
    names.sort_unstable();
    names.dedup();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for n in &names {
        for b in n.bytes().chain(std::iter::once(0xff)) {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    // spread the bits before picking, so near-identical sets still look different
    let mix = (h ^ (h >> 29)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    Identity {
        signature: h,
        emoji: EMOJI[(mix % EMOJI.len() as u64) as usize],
        hue: ((mix >> 20) % 360) as u16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_and_case_do_not_matter() {
        let a = identity(["Skyrim Textures", "Better Bridges", "Noble Furniture"]);
        let b = identity(["noble furniture", "Better Bridges", "SKYRIM TEXTURES"]);
        assert_eq!(a, b);
        assert_ne!(
            a.signature,
            identity(["Skyrim Textures", "Better Bridges"]).signature
        );
    }

    #[test]
    fn icons_and_hues_spread() {
        let mut emoji = std::collections::HashSet::new();
        let mut hues = std::collections::HashSet::new();
        for i in 0..200 {
            let a = format!("Mod {i}");
            let b = format!("Mod {}", i + 1);
            let id = identity([a.as_str(), b.as_str()]);
            emoji.insert(id.emoji);
            hues.insert(id.hue / 30);
        }
        assert!(emoji.len() > 40, "{} distinct icons", emoji.len());
        assert_eq!(hues.len(), 12, "every 30° sector is used");
    }
}
