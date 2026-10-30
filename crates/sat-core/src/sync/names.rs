//! Name identity rules, ported 1:1 from the monolith so that matches agree on the same data.
//!
//! - [`normalize_name`] — `normalizeName`: project ↔ folder identity during reconcile.
//! - [`NameKeys`] — `getExternalNameKeys`: raw / normalized / loose keys for donor matching.
//!
//! JavaScript regex semantics are kept: `\b` is ASCII-word based, `\d` is ASCII.

use std::sync::LazyLock;

use regex::Regex;

macro_rules! re {
    ($s:expr) => {
        LazyLock::new(|| Regex::new($s).expect("static regex"))
    };
}

static WS: LazyLock<Regex> = re!(r"\s+");
static PUNCT: LazyLock<Regex> = re!(r"[._-]+");
static SE: LazyLock<Regex> = re!(r"(?-u:\b)se(?-u:\b)");
static AE: LazyLock<Regex> = re!(r"(?-u:\b)ae(?-u:\b)");
static EDITION: LazyLock<Regex> = re!(r"(?-u:\b)(special edition|anniversary edition|sse|se|ae)(?-u:\b)");
static VERSION: LazyLock<Regex> = re!(r"(?-u:\b)(v|ver|version)\s*[0-9]+(?:\s+[0-9]+)*(?-u:\b)");
static NUMBERS: LazyLock<Regex> = re!(r"(?-u:\b)[0-9]+(?:\s+[0-9]+){1,}(?-u:\b)");

/// Monolith `normalizeName`: lowercase, collapse whitespace, `._-` runs become a space.
///
/// Whitespace is collapsed *before* punctuation is replaced, exactly like the original, so `"a - b"`
/// becomes `"a   b"`. Keys built on top of it collapse again.
pub fn normalize_name(s: &str) -> String {
    let lower = s.to_lowercase();
    let collapsed = WS.replace_all(lower.trim(), " ");
    PUNCT.replace_all(&collapsed, " ").trim().to_string()
}

/// Monolith `externalNormalizeNameKey`: `se`/`ae` spelled out as editions.
pub fn norm_key(s: &str) -> String {
    let n = normalize_name(s);
    let n = SE.replace_all(&n, "special edition");
    let n = AE.replace_all(&n, "anniversary edition");
    WS.replace_all(&n, " ").trim().to_string()
}

/// Monolith `externalLooseNameKey`: [`norm_key`] without editions, version tags and number runs.
pub fn loose_key(s: &str) -> String {
    let n = norm_key(s);
    let n = EDITION.replace_all(&n, "");
    let n = VERSION.replace_all(&n, "");
    let n = NUMBERS.replace_all(&n, "");
    WS.replace_all(&n, " ").trim().to_string()
}

/// Monolith `externalRawNameKey`.
pub fn raw_key(s: &str) -> String {
    s.to_lowercase().trim().to_string()
}

/// Distinct non-empty keys in the order raw, norm, loose (monolith `getExternalNameKeys`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameKeys(pub Vec<String>);

impl NameKeys {
    pub fn of(s: &str) -> Self {
        let mut v: Vec<String> = Vec::with_capacity(3);
        for k in [raw_key(s), norm_key(s), loose_key(s)] {
            if !k.is_empty() && !v.contains(&k) {
                v.push(k);
            }
        }
        Self(v)
    }
}

pub const SEPARATOR_SUFFIX: &str = "_separator";

/// `*_separator`, case-insensitive (monolith `isExternalSeparatorFolderName`).
pub fn is_separator_folder(name: &str) -> bool {
    name.trim().to_lowercase().ends_with(SEPARATOR_SUFFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_matches_monolith() {
        assert_eq!(normalize_name("  My.Mod_Name-v2  "), "my mod name v2");
        // whitespace collapsed before punctuation → inner triple space survives, as in JS
        assert_eq!(normalize_name("a - b"), "a   b");
    }

    #[test]
    fn keys() {
        assert_eq!(norm_key("SkyUI SE"), "skyui special edition");
        assert_eq!(norm_key("Base AE fixes"), "base anniversary edition fixes");
        // "se" inside a word is not an edition
        assert_eq!(norm_key("Sexlab"), "sexlab");
        assert_eq!(loose_key("Engine Fixes SE v7 0 20"), "engine fixes");
        assert_eq!(loose_key("Mod 1 2 3 extra"), "mod extra");
        assert_eq!(loose_key("Mod 2"), "mod 2", "a single number is kept");
        assert_eq!(
            NameKeys::of("Engine_Fixes SE").0,
            vec!["engine_fixes se", "engine fixes special edition", "engine fixes"]
        );
        assert_eq!(NameKeys::of("abc").0, vec!["abc"]);
    }

    #[test]
    fn separator_folder() {
        assert!(is_separator_folder("Foo_Separator"));
        assert!(!is_separator_folder("Foo separator"));
    }
}
