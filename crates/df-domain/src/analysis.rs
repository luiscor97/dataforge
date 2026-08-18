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
    /// Optional folder name that must appear somewhere above the file.
    ///
    /// A **name**, never a path: separators are rejected for the same reason
    /// they are above, and the comparison is segment by segment rather than a
    /// substring, so `lib` never matches `libreria` and the answer does not
    /// depend on which separator the platform writes.
    ///
    /// It exists because the file name alone could not express the criterion
    /// that works. On the audited archive the human classification separated
    /// 8.171 files as technical, and the extension is close to useless for
    /// finding them: `.jpg` appears 578 times among those and 2.523 times
    /// among the files that were kept, so a rule on the extension would throw
    /// away the pericial photographs. What separates them is where they live
    /// — inside `locale`, `bin`, `node_modules`, a plugin tree. Matching the
    /// containing folder covers 28,4% of those exclusions and touches one
    /// kept file out of 28.569.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ancestor_folder: Option<String>,
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
        if let Some(folder) = &self.match_spec.ancestor_folder {
            if folder.trim().is_empty() || folder.len() > 255 {
                return Err(df_error::DfError::Validation(format!(
                    "rule `{}` ancestor_folder must contain 1..=255 bytes",
                    self.id
                )));
            }
            if folder.contains(['/', '\\']) {
                return Err(df_error::DfError::Validation(format!(
                    "rule `{}` ancestor_folder is a folder name, not a path",
                    self.id
                )));
            }
        }
        Ok(())
    }

    /// Evaluate the rule against a display file name. Raw path identity is
    /// preserved separately; a metadata rule never decides content identity.
    /// Evaluate the rule against a file name and the relative path it sits
    /// on. Both halves must hold; a rule with no `ancestor_folder` behaves
    /// exactly as it did before this existed.
    pub fn matches(&self, file_name: &str, relative_path: &str) -> bool {
        if !self.matches_file_name(file_name) {
            return false;
        }
        let Some(folder) = &self.match_spec.ancestor_folder else {
            return true;
        };
        // Segment by segment, both separators, and never the last component:
        // the subject is a *containing* folder, and the file's own name is
        // already the business of `file_name_glob`.
        let mut segments: Vec<&str> = relative_path
            .split(['/', '\\'])
            .filter(|s| !s.is_empty())
            .collect();
        segments.pop();
        segments
            .iter()
            .any(|segment| segment.eq_ignore_ascii_case(folder))
    }

    pub fn matches_file_name(&self, file_name: &str) -> bool {
        glob_matches(
            &self.match_spec.file_name_glob.to_lowercase(),
            &file_name.to_lowercase(),
        )
    }
}

/// Shared with `profile`, so an exclusion glob and a rule glob mean exactly
/// the same thing. Two implementations would eventually disagree on a corner
/// and nobody would notice until a file was silently skipped.
pub(crate) fn glob_matches(pattern: &str, value: &str) -> bool {
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
    /// A folder repeating an ancestor's name that holds nothing of its own.
    ///
    /// Detected since M2.x and reported ever since, but never decidable: the
    /// plan invariant refused to place one in the active tree and there was
    /// no item anyone could answer. On the real archive that left a plan
    /// unapprovable over `ESCANER\DOCUMENTOS ESCANER\ESCANER`, a folder
    /// whose owner had said in so many words to leave exactly as it was.
    /// Both were right; only one of them could be written down.
    DragScar,
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
            Self::DragScar => "DRAG_SCAR",
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
                ancestor_folder: None,
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
    fn a_rule_can_name_the_folder_that_contains_the_file() {
        // Why this exists, in one number: the human classification of the
        // audited archive separated 8.171 files as technical, and `.jpg`
        // appears 578 times among them and 2.523 times among the files it
        // kept. A rule on the extension would throw away the pericial
        // photographs. Where the file lives is the signal; what it is called
        // is not.
        let inside = |folder: &str, glob: &str| RuleDefinition {
            id: "separate.software-tree".to_string(),
            version: 1,
            match_spec: RuleMatch {
                file_name_glob: glob.to_string(),
                ancestor_folder: Some(folder.to_string()),
            },
            classification: RuleClassification {
                category: "programa instalado".to_string(),
                confidence: 0.95,
            },
            action: RuleAction::CopySeparated,
            risk: RiskLevel::Low,
        };

        let r = inside("locale", "*");
        assert!(r.matches("es.pak", "programa/locale/es.pak"));
        assert!(r.matches("es.pak", r"programa\locale\es.pak"));
        // The pericial photograph this must never touch.
        assert!(!r.matches("dni dubitada.jpg", "PERICIALES/asunto 12/dni dubitada.jpg"));

        // A name, compared whole. `lib` is not `libreria`, on either
        // separator — a substring test says otherwise and has shipped from
        // this repository more than once.
        let lib = inside("lib", "*");
        assert!(lib.matches("x.dll", "app/lib/x.dll"));
        assert!(!lib.matches("escrito.pdf", "libreria/escrito.pdf"));
        assert!(!lib.matches("escrito.pdf", "LIBRERIA/sub/escrito.pdf"));
        // Case-insensitive, because the filesystem is.
        assert!(lib.matches("x.dll", "app/LIB/x.dll"));

        // The file's own name is never the containing folder: a file called
        // `lib` is not inside one.
        assert!(!lib.matches("lib", "app/lib"));

        // Both halves must hold.
        let only_dll = inside("bin", "*.dll");
        assert!(only_dll.matches("a.dll", "app/bin/a.dll"));
        assert!(!only_dll.matches("contrato.pdf", "app/bin/contrato.pdf"));

        // And a folder name is rejected if it is really a path.
        let mut escaping = inside("bin", "*");
        escaping.match_spec.ancestor_folder = Some("app/bin".to_string());
        assert!(escaping.validate().is_err());
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
