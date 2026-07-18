//! Declarative structural rules, anomalies and review decisions (RFC-0001
//! §12.5–§12.7, §25; Milestone 0.2).
//!
//! Rules are intentionally metadata-only: they can classify an occurrence
//! and select one of the safe copy buckets, but can never delete, overwrite
//! or mutate source material. Every match is persisted by `df-db` together
//! with the evidence that produced it.

use serde::{Deserialize, Serialize};

use crate::{OperationType, RiskLevel};

/// Safe default action selected by a declarative rule or human reviewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RuleAction {
    CopyActive,
    CopyReview,
    CopySeparated,
    CopyTemporary,
}

impl RuleAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CopyActive => "COPY_ACTIVE",
            Self::CopyReview => "COPY_REVIEW",
            Self::CopySeparated => "COPY_SEPARATED",
            Self::CopyTemporary => "COPY_TEMPORARY",
        }
    }

    pub fn parse(value: &str) -> df_error::DfResult<Self> {
        match value {
            "COPY_ACTIVE" => Ok(Self::CopyActive),
            "COPY_REVIEW" => Ok(Self::CopyReview),
            "COPY_SEPARATED" => Ok(Self::CopySeparated),
            "COPY_TEMPORARY" => Ok(Self::CopyTemporary),
            other => Err(df_error::DfError::Validation(format!(
                "unknown rule action `{other}`"
            ))),
        }
    }

    pub fn operation_type(self) -> OperationType {
        match self {
            Self::CopyActive => OperationType::CopyActive,
            Self::CopyReview => OperationType::CopyReview,
            Self::CopySeparated => OperationType::CopySeparated,
            Self::CopyTemporary => OperationType::CopyTemporary,
        }
    }
}

/// Metadata predicate supported by the M0.2 rule engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleMatch {
    /// Case-insensitive glob over the file name only. `*` matches zero or
    /// more Unicode scalar values and `?` exactly one. Path separators are
    /// rejected so a rule cannot escape its declared subject.
    pub file_name_glob: String,
}

/// Explainable classification emitted by a rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleClassification {
    pub category: String,
    pub confidence: f64,
}

/// One versioned declarative rule embedded in a profile (§25.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleDefinition {
    pub id: String,
    pub version: u32,
    #[serde(rename = "match")]
    pub match_spec: RuleMatch,
    pub classification: RuleClassification,
    pub action: RuleAction,
    pub risk: RiskLevel,
}

impl RuleDefinition {
    pub fn validate(&self) -> df_error::DfResult<()> {
        if self.id.trim().is_empty() || self.id.len() > 128 {
            return Err(df_error::DfError::Validation(
                "rule id must contain 1..=128 characters".to_string(),
            ));
        }
        if self.version == 0 {
            return Err(df_error::DfError::Validation(format!(
                "rule `{}` has version 0",
                self.id
            )));
        }
        if self.classification.category.trim().is_empty() {
            return Err(df_error::DfError::Validation(format!(
                "rule `{}` has an empty category",
                self.id
            )));
        }
        if !self.classification.confidence.is_finite()
            || !(0.0..=1.0).contains(&self.classification.confidence)
        {
            return Err(df_error::DfError::Validation(format!(
                "rule `{}` confidence must be in [0, 1]",
                self.id
            )));
        }
        let glob = &self.match_spec.file_name_glob;
        if glob.is_empty() || glob.len() > 255 {
            return Err(df_error::DfError::Validation(format!(
                "rule `{}` file_name_glob must contain 1..=255 bytes",
                self.id
            )));
        }
        if glob.contains(['/', '\\']) {
            return Err(df_error::DfError::Validation(format!(
                "rule `{}` file_name_glob must not contain a path separator",
                self.id
            )));
        }
        Ok(())
    }

    /// Evaluate the rule against a display file name. Raw path identity is
    /// preserved separately; a metadata rule never decides content identity.
    pub fn matches_file_name(&self, file_name: &str) -> bool {
        glob_matches(
            &self.match_spec.file_name_glob.to_lowercase(),
            &file_name.to_lowercase(),
        )
    }
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let mut previous = vec![false; value.len() + 1];
    previous[0] = true;

    for token in pattern {
        let mut current = vec![false; value.len() + 1];
        match token {
            '*' => {
                current[0] = previous[0];
                for index in 1..=value.len() {
                    current[index] = previous[index] || current[index - 1];
                }
            }
            '?' => {
                current[1..].copy_from_slice(&previous[..value.len()]);
            }
            literal => {
                for index in 1..=value.len() {
                    current[index] = previous[index - 1] && value[index - 1] == literal;
                }
            }
        }
        previous = current;
    }
    previous[value.len()]
}

/// Structural anomaly vocabulary persisted as evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AnomalyKind {
    SameNameDifferentContent,
    LossyPathIdentity,
    UnreadableEntry,
    ExtremePath,
    PartialTreeUniqueContent,
    EmbeddedTree,
}

impl AnomalyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SameNameDifferentContent => "SAME_NAME_DIFFERENT_CONTENT",
            Self::LossyPathIdentity => "LOSSY_PATH_IDENTITY",
            Self::UnreadableEntry => "UNREADABLE_ENTRY",
            Self::ExtremePath => "EXTREME_PATH",
            Self::PartialTreeUniqueContent => "PARTIAL_TREE_UNIQUE_CONTENT",
            Self::EmbeddedTree => "EMBEDDED_TREE",
        }
    }
}

/// Severity used by the structural diagnostic and review queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AnomalySeverity {
    Info,
    Warning,
    High,
}

impl AnomalySeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warning => "WARNING",
            Self::High => "HIGH",
        }
    }

    pub fn risk(self) -> RiskLevel {
        match self {
            Self::Info => RiskLevel::Low,
            Self::Warning => RiskLevel::Medium,
            Self::High => RiskLevel::High,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(glob: &str) -> RuleDefinition {
        RuleDefinition {
            id: "temporary.test".to_string(),
            version: 1,
            match_spec: RuleMatch {
                file_name_glob: glob.to_string(),
            },
            classification: RuleClassification {
                category: "temporary".to_string(),
                confidence: 1.0,
            },
            action: RuleAction::CopyTemporary,
            risk: RiskLevel::Low,
        }
    }

    #[test]
    fn rule_actions_are_safe_copy_operations() {
        for action in [
            RuleAction::CopyActive,
            RuleAction::CopyReview,
            RuleAction::CopySeparated,
            RuleAction::CopyTemporary,
        ] {
            assert_eq!(RuleAction::parse(action.as_str()).unwrap(), action);
            assert!(action.operation_type().is_executable());
        }
        assert!(RuleAction::parse("DELETE").is_err());
    }

    #[test]
    fn file_name_globs_are_case_insensitive_and_unicode_safe() {
        assert!(rule("~$*").matches_file_name("~$Contrato.DOCX"));
        assert!(rule("*.tmp").matches_file_name("BORRADOR.TMP"));
        assert!(rule("copia-?.txt").matches_file_name("Copia-ñ.TXT"));
        assert!(!rule("*.tmp").matches_file_name("tmp/documento"));
    }

    #[test]
    fn invalid_rules_fail_closed() {
        let mut value = rule("*.tmp");
        value.version = 0;
        assert!(value.validate().is_err());
        value.version = 1;
        value.match_spec.file_name_glob = "folder/*.tmp".to_string();
        assert!(value.validate().is_err());
        value.match_spec.file_name_glob = "*.tmp".to_string();
        value.classification.confidence = 1.1;
        assert!(value.validate().is_err());
    }

    #[test]
    fn anomaly_names_are_stable() {
        assert_eq!(
            AnomalyKind::PartialTreeUniqueContent.as_str(),
            "PARTIAL_TREE_UNIQUE_CONTENT"
        );
        assert_eq!(AnomalySeverity::High.risk(), RiskLevel::High);
    }
}
