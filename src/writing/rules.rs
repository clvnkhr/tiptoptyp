//! Validated rule switches layered over Harper's bundled curated configuration.
use harper_core::linting::{FlatConfig, LintGroup, StructuredConfig};
use serde::{Deserialize, Deserializer, Serialize};
use std::{collections::BTreeMap, sync::LazyLock};

const QUIET_RULES: [&str; 12] = [
    "LongSentences",
    "Hedging",
    "FillerWords",
    "DiscourseMarkers",
    "NoFrenchSpaces",
    "OxfordComma",
    "NoOxfordComma",
    "Dashes",
    "UseEllipsisCharacter",
    "ExpandTimeShorthands",
    "ExpandMemoryShorthands",
    "ExpandMinimum",
];

// Validation uses only the compiled catalogue, never the spelling dictionary
// or instantiated linters. Resolve it once, outside frame rendering.
static CATALOGUE: LazyLock<FlatConfig> = LazyLock::new(|| {
    StructuredConfig::curated()
        .to_flat_config()
        .expect("Harper's bundled rule catalogue must be valid")
});
pub(super) static DEFAULT_RULE_OVERRIDES: LazyLock<RuleOverrides> =
    LazyLock::new(RuleOverrides::default);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct RuleOverrides(BTreeMap<String, bool>);
impl Default for RuleOverrides {
    fn default() -> Self {
        Self(
            QUIET_RULES
                .into_iter()
                .map(|rule| (rule.into(), false))
                .collect(),
        )
    }
}
impl<'de> Deserialize<'de> for RuleOverrides {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let rules = Self(BTreeMap::deserialize(deserializer)?);
        rules.validate().map_err(serde::de::Error::custom)?;
        Ok(rules)
    }
}
impl RuleOverrides {
    pub(crate) fn validate(&self) -> Result<(), String> {
        for name in self.0.keys() {
            if !CATALOGUE.has_rule(name) {
                return Err(format!(
                    "Unknown Harper rule {name:?}. Use exact rule IDs, for example SpellCheck, RepeatedWords or AnA."
                ));
            }
        }
        let enabled = |rule| {
            self.0
                .get(rule)
                .copied()
                .unwrap_or_else(|| CATALOGUE.is_rule_enabled(rule))
        };
        if enabled("OxfordComma") && enabled("NoOxfordComma") {
            return Err("Harper OxfordComma and NoOxfordComma cannot both be enabled. Choose one, or disable both.".into());
        }
        Ok(())
    }
    pub(super) fn apply(&self, linter: &mut LintGroup) {
        for (rule, enabled) in &self.0 {
            linter.config.set_rule_enabled(rule, *enabled);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quiet_profile_overrides_exactly_twelve_style_rules_and_keeps_grammar() {
        let rules = RuleOverrides::default();
        rules.validate().unwrap();
        assert_eq!(rules.0.len(), 12);
        assert!(
            QUIET_RULES
                .into_iter()
                .all(|rule| rules.0.get(rule) == Some(&false))
        );
        let mut curated = CATALOGUE.clone();
        for (rule, enabled) in rules.0 {
            curated.set_rule_enabled(rule, enabled);
        }
        for rule in [
            "SpellCheck",
            "RepeatedWords",
            "AnA",
            "ThereIsAgreement",
            "WrongApostrophe",
        ] {
            assert!(
                curated.is_rule_enabled(rule),
                "grammar rule {rule} must stay enabled"
            );
        }
        for rule in ["BoringWords", "AvoidContractions", "SpelledNumbers"] {
            assert!(
                !curated.is_rule_enabled(rule),
                "existing opt-in style rule {rule} must stay off"
            );
        }
    }
    #[test]
    fn overrides_validate_rule_ids_boolean_values_and_conflicting_comma_styles() {
        for invalid in [
            r#"{"SpellChek":false}"#,
            r#"{"SpellCheck":"false"}"#,
            r#"{"SpellCheck":null}"#,
            r#"{"SpellCheck":0}"#,
            r#"{"OxfordComma":true,"NoOxfordComma":true}"#,
            r#"{"NoOxfordComma":true}"#,
        ] {
            assert!(
                serde_json::from_str::<RuleOverrides>(invalid).is_err(),
                "{invalid}"
            );
        }
        let error = serde_json::from_str::<RuleOverrides>(r#"{"SpellChek":false}"#)
            .unwrap_err()
            .to_string();
        assert!(error.contains("SpellChek") && error.contains("exact rule IDs"));
        serde_json::from_str::<RuleOverrides>(r#"{"OxfordComma":false,"NoOxfordComma":true}"#)
            .unwrap();
        let custom: RuleOverrides =
            serde_json::from_str(r#"{"NoFrenchSpaces":true,"SpellCheck":true}"#).unwrap();
        assert_eq!(
            serde_json::to_value(&custom).unwrap(),
            serde_json::json!({"NoFrenchSpaces":true,"SpellCheck":true})
        );
    }
}
