//! Conditional stickers: a sticker on every mod an expression holds for (ADR 0013). @sat:sticker-rules
//!
//! Owns: sticker rules of a project (`sticker` + `when`, an expression over mod metrics — `size > 1 GB`) and
//! applying them to one mod's view.
//! Not here: stickers the user puts by hand (`Marks`, `@sat:order.decor`), the expression language (`@sat:expr`),
//! the metrics (`@sat:metrics`).
//!
//! A rule's sticker is computed, never stored on the mod: change the rule or the mod and the sticker follows.

use serde::{Deserialize, Serialize};

use crate::expr::Expr;
use crate::metrics::ModView;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StickerRule {
    pub sticker: String,
    /// An expression (`@sat:expr`) over the mod metrics (`@sat:metrics`).
    pub when: String,
}

/// Rules ready to apply.
#[derive(Debug, Clone, Default)]
pub struct StickerRules {
    rules: Vec<(String, Expr)>,
}

impl StickerRules {
    // @sat:sticker-rules.compile
    /// Compiles every rule; a rule that does not compile is reported and left out (one typo does not take the
    /// others away).
    pub fn compile(rules: &[StickerRule]) -> (Self, Vec<String>) {
        let mut out = Self::default();
        let mut errors = Vec::new();
        for r in rules {
            match Expr::compile_with(&r.when, &crate::metrics::resolve) {
                Ok(e) => out.rules.push((r.sticker.clone(), e)),
                Err(e) => errors.push(format!("sticker {:?}: {e}", r.sticker)),
            }
        }
        (out, errors)
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Stickers the rules give this mod.
    pub fn stickers(&self, view: &ModView<'_>) -> Vec<String> {
        self.rules
            .iter()
            .filter(|(_, e)| e.test(&|id| view.get(id)))
            .map(|(s, _)| s.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::ModMetrics;

    #[test]
    fn rules_put_stickers_and_a_bad_rule_is_reported() {
        let (r, errs) = StickerRules::compile(&[
            StickerRule {
                sticker: "heavy".into(),
                when: "weight > 1 GB".into(),
            },
            StickerRule {
                sticker: "broken".into(),
                when: "size >".into(),
            },
        ]);
        assert_eq!(errs.len(), 1);
        let m = ModMetrics {
            bytes: 2 << 30,
            ..ModMetrics::default()
        };
        let view = |m| ModView {
            metrics: Some(m),
            stat: None,
            name: "A",
            separator: None,
            tags: None,
            enabled: true,
            priority: 0,
        };
        assert_eq!(r.stickers(&view(&m)), ["heavy"]);
        assert!(r.stickers(&view(&ModMetrics::default())).is_empty());
    }
}
