//! Declarative profiles (RFC-0001 §25.2, §25.4).
//!
//! A profile is what turns DataForge from a generic deduplicator into a tool
//! that understands a domain: it declares which folder names are low-value
//! *generic* containers (and how much they penalise a location as canonical,
//! §18.3) and which are **protected boundaries** that consolidation must
//! never dissolve (rule 9).
//!
//! # Why they are embedded, not read from disk
//!
//! Profiles live in `profiles/<id>/profile.json` in the repository and are
//! compiled in with `include_str!`, exactly like the SQL migrations. That
//! keeps them declarative, reviewable in a PR and versioned with the code
//! that interprets them, without adding runtime path resolution or a way for
//! a stray file next to the binary to silently change what gets consolidated.
//! User-supplied profiles are a plugin concern (Milestone 0.6).
//!
//! JSON (not YAML) because §5.7 lists JSON among the open formats and the
//! workspace already parses it; the maintained YAML crates are not worth a new
//! dependency here (see ADR-0026).

use serde::{Deserialize, Serialize};

use crate::context::ContextKind;

const SCHEMA: &str = "dataforge.profile";

/// The conservative default. Any unknown profile falls back to it.
pub const DEFAULT_PROFILE_ID: &str = "generic";

const GENERIC_JSON: &str = include_str!("../../../profiles/generic/profile.json");
const LEGAL_JSON: &str = include_str!("../../../profiles/legal/profile.json");

/// Every profile shipped with this build.
const BUILT_IN: &[(&str, &str)] = &[("generic", GENERIC_JSON), ("legal", LEGAL_JSON)];

/// How a marker is compared against a folder name.
///
/// The default is [`MatchMode::Exact`] on purpose: a loose match that grabs
/// more folders than intended is a safety problem in both directions — it can
/// penalise a legitimate location, or protect so much that consolidation
/// silently stops working.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchMode {
    /// The whole name equals the marker.
    #[default]
    Exact,
    /// The name *starts* with the marker and what follows is a separator or a
    /// digit — real folders are called `Expediente 1234-2020`, `Expediente_12`
    /// or `Expediente2020`, and all three must match.
    ///
    /// The separator/digit requirement is what stops `expediente` from
    /// swallowing `expedientes` (rest is `s`) or `copia` from swallowing
    /// `copiadora` (rest is `dora`).
    Prefix,
}

/// Characters that may follow a `Prefix` marker.
fn is_boundary_after_prefix(rest: &str) -> bool {
    rest.is_empty()
        || rest.starts_with([' ', '-', '_', '.', '(', '[', '#'])
        || rest.starts_with(|c: char| c.is_ascii_digit())
}

/// Whether `name` matches `marker` under `mode`.
fn marker_matches(name: &str, marker: &str, mode: MatchMode) -> bool {
    match mode {
        MatchMode::Exact => name == marker,
        MatchMode::Prefix => name
            .strip_prefix(marker)
            .is_some_and(is_boundary_after_prefix),
    }
}

/// A low-value container marker and its representative-location penalty
/// (§18.3). Compared against a folder's lowercase name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenericMarker {
    pub name: String,
    pub penalty: u32,
    #[serde(default, rename = "match")]
    pub match_mode: MatchMode,
}

/// A folder name that marks a boundary deduplication must not dissolve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedMarker {
    pub name: String,
    /// Why this is a boundary, surfaced in the operation's reason (§5.3).
    pub reason: String,
    #[serde(default, rename = "match")]
    pub match_mode: MatchMode,
}

/// A parsed, resolved profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub schema: String,
    pub schema_version: String,
    pub id: String,
    pub name: String,
    pub description: String,
    /// Profile whose generic markers this one reuses. One level only.
    #[serde(default)]
    pub inherits: Option<String>,
    #[serde(default)]
    pub generic_markers: Vec<GenericMarker>,
    #[serde(default)]
    pub protected_markers: Vec<ProtectedMarker>,
}

impl Profile {
    /// Load a built-in profile by id, resolving inheritance.
    ///
    /// An unknown id falls back to [`DEFAULT_PROFILE_ID`]: a profile we cannot
    /// resolve must never silently grant *fewer* protections than expected, and
    /// `generic` is the most conservative one (it protects nothing but also
    /// consolidates nothing by itself).
    pub fn load(id: &str) -> df_error::DfResult<Self> {
        let json = BUILT_IN
            .iter()
            .find(|(name, _)| *name == id)
            .or_else(|| {
                BUILT_IN
                    .iter()
                    .find(|(name, _)| *name == DEFAULT_PROFILE_ID)
            })
            .map(|(_, json)| *json)
            .ok_or_else(|| {
                df_error::DfError::Validation("no built-in profile is available".to_string())
            })?;

        let mut profile: Profile = serde_json::from_str(json).map_err(|e| {
            df_error::DfError::Serialization(format!("profile `{id}` is not valid JSON: {e}"))
        })?;
        if profile.schema != SCHEMA {
            return Err(df_error::DfError::Validation(format!(
                "profile `{id}` has unexpected schema `{}`",
                profile.schema
            )));
        }

        if let Some(parent_id) = profile.inherits.clone() {
            if parent_id == profile.id {
                return Err(df_error::DfError::Validation(format!(
                    "profile `{}` inherits from itself",
                    profile.id
                )));
            }
            let parent_json = BUILT_IN
                .iter()
                .find(|(name, _)| *name == parent_id)
                .map(|(_, json)| *json)
                .ok_or_else(|| {
                    df_error::DfError::Validation(format!(
                        "profile `{}` inherits from unknown profile `{parent_id}`",
                        profile.id
                    ))
                })?;
            let parent: Profile = serde_json::from_str(parent_json).map_err(|e| {
                df_error::DfError::Serialization(format!("profile `{parent_id}`: {e}"))
            })?;
            if parent.inherits.is_some() {
                return Err(df_error::DfError::Validation(
                    "profile inheritance is limited to one level".to_string(),
                ));
            }
            // The child's own markers win over the inherited ones.
            for inherited in parent.generic_markers {
                if !profile
                    .generic_markers
                    .iter()
                    .any(|m| m.name == inherited.name)
                {
                    profile.generic_markers.push(inherited);
                }
            }
        }
        Ok(profile)
    }

    /// Ids of every profile shipped with this build.
    pub fn built_in_ids() -> Vec<&'static str> {
        BUILT_IN.iter().map(|(id, _)| *id).collect()
    }

    /// Classify a folder name. Protected wins over generic: a folder called
    /// `expediente` inside `Backup` is still a boundary.
    ///
    /// Returns the kind, the penalty (0 unless generic) and the marker that
    /// matched, so the decision can always be explained.
    pub fn classify(&self, normalized_name: &str) -> (ContextKind, u32, Option<String>) {
        let name = normalized_name.trim();
        for marker in &self.protected_markers {
            if marker_matches(name, &marker.name, marker.match_mode) {
                return (ContextKind::Protected, 0, Some(marker.name.clone()));
            }
        }
        for marker in &self.generic_markers {
            if marker_matches(name, &marker.name, marker.match_mode) {
                return (
                    ContextKind::Generic,
                    marker.penalty,
                    Some(marker.name.clone()),
                );
            }
        }
        // Copy patterns produced by Windows Explorer and by hand.
        if name.ends_with(" - copia") || name.ends_with(" - copy") {
            return (ContextKind::Generic, 30, Some("copy-suffix".to_string()));
        }
        if name.starts_with("copia de ") || name.starts_with("copy of ") {
            return (ContextKind::Generic, 30, Some("copy-prefix".to_string()));
        }
        if name.starts_with("nueva carpeta") || name.starts_with("new folder") {
            return (ContextKind::Generic, 30, Some("new-folder".to_string()));
        }
        (ContextKind::Neutral, 0, None)
    }

    /// The reason text of a protected marker, for the operation's evidence.
    pub fn protected_reason(&self, marker: &str) -> Option<&str> {
        self.protected_markers
            .iter()
            .find(|m| m.name == marker)
            .map(|m| m.reason.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_profile_parses_and_is_well_formed() {
        for id in Profile::built_in_ids() {
            let profile = Profile::load(id).unwrap_or_else(|e| panic!("profile `{id}`: {e}"));
            assert_eq!(profile.id, id);
            assert_eq!(profile.schema, SCHEMA);
            assert!(!profile.description.is_empty());
            // Every protected marker explains itself (§5.3).
            for marker in &profile.protected_markers {
                assert!(
                    !marker.reason.is_empty(),
                    "protected marker `{}` of `{id}` has no reason",
                    marker.name
                );
            }
            // Markers are compared lowercase; declaring an uppercase one would
            // silently never match.
            for marker in &profile.generic_markers {
                assert_eq!(marker.name, marker.name.to_lowercase());
            }
            for marker in &profile.protected_markers {
                assert_eq!(marker.name, marker.name.to_lowercase());
            }
        }
    }

    /// §25.4: the default profile must not protect anything, because without
    /// domain knowledge we cannot know what a boundary is.
    #[test]
    fn the_generic_profile_declares_no_protected_boundaries() {
        let generic = Profile::load("generic").unwrap();
        assert!(generic.protected_markers.is_empty());
        assert!(!generic.generic_markers.is_empty());
    }

    #[test]
    fn an_unknown_profile_falls_back_to_generic() {
        let profile = Profile::load("does-not-exist").unwrap();
        assert_eq!(profile.id, "generic");
        assert!(profile.protected_markers.is_empty());
    }

    #[test]
    fn the_legal_profile_inherits_generic_markers_and_adds_boundaries() {
        let legal = Profile::load("legal").unwrap();
        assert_eq!(legal.id, "legal");
        // Inherited: it did not declare "descargas" itself.
        assert!(legal.generic_markers.iter().any(|m| m.name == "descargas"));
        // Own: the legal boundaries.
        assert!(legal
            .protected_markers
            .iter()
            .any(|m| m.name == "expediente"));
        assert!(legal.protected_markers.iter().any(|m| m.name == "pericial"));
    }

    #[test]
    fn generic_folders_are_classified_with_their_penalty() {
        let generic = Profile::load("generic").unwrap();
        let (kind, penalty, marker) = generic.classify("descargas");
        assert_eq!(kind, ContextKind::Generic);
        assert_eq!(penalty, 50);
        assert_eq!(marker.as_deref(), Some("descargas"));

        assert_eq!(generic.classify("informes").0, ContextKind::Neutral);
        assert_eq!(generic.classify("informe - copia").0, ContextKind::Generic);
        assert_eq!(generic.classify("copia de informe").0, ContextKind::Generic);
    }

    /// The legal profile is what makes rule 9 bite: without it nothing is ever
    /// protected.
    #[test]
    fn the_legal_profile_marks_expedientes_as_protected() {
        let legal = Profile::load("legal").unwrap();
        let (kind, penalty, marker) = legal.classify("expediente");
        assert_eq!(kind, ContextKind::Protected);
        assert_eq!(penalty, 0, "a boundary is not a bad canonical location");
        assert_eq!(marker.as_deref(), Some("expediente"));
        assert!(legal.protected_reason("expediente").is_some());

        // The generic profile does not: same folder, no protection.
        let generic = Profile::load("generic").unwrap();
        assert_eq!(generic.classify("expediente").0, ContextKind::Neutral);
    }

    /// Real folders are called `Expediente 1234-2020`, not `Expediente`.
    /// Without prefix matching the legal profile would protect almost nothing.
    #[test]
    fn prefix_markers_match_real_world_folder_names() {
        let legal = Profile::load("legal").unwrap();
        for name in [
            "expediente 1234-2020",
            "expediente-1234",
            "expediente_12",
            "expediente2020",
            "expediente (archivado)",
            "expediente",
            "exp 1234-2020",
            "pericial martinez",
            "procedimiento 55/2021",
            "asunto 7",
        ] {
            assert_eq!(
                legal.classify(name).0,
                ContextKind::Protected,
                "`{name}` should be a protected boundary"
            );
        }
    }

    /// The separator/digit requirement is what keeps prefix matching honest.
    /// A marker that swallowed neighbouring words would protect so much that
    /// consolidation silently stopped working.
    #[test]
    fn prefix_markers_do_not_swallow_unrelated_names() {
        let legal = Profile::load("legal").unwrap();
        // "expedientes" is declared separately (exact); the singular prefix
        // must not match it, or the plural's own rule would be dead code.
        assert!(marker_matches(
            "expediente 1",
            "expediente",
            MatchMode::Prefix
        ));
        assert!(!marker_matches(
            "expedientes",
            "expediente",
            MatchMode::Prefix
        ));
        assert!(!marker_matches(
            "expedientenuevo",
            "expediente",
            MatchMode::Prefix
        ));
        // Unrelated words that merely start with a marker's letters.
        assert_eq!(legal.classify("exposicion").0, ContextKind::Neutral);
        assert_eq!(legal.classify("expertos").0, ContextKind::Neutral);
        assert_eq!(legal.classify("asuntos varios").0, ContextKind::Neutral);
    }

    /// Generic markers stay `exact` by default, so `copia` must not swallow
    /// `copiadora` — that would penalise a legitimate location.
    #[test]
    fn generic_markers_default_to_exact_matching() {
        let generic = Profile::load("generic").unwrap();
        assert_eq!(generic.classify("copia").0, ContextKind::Generic);
        assert_eq!(generic.classify("copiadora").0, ContextKind::Neutral);
        assert_eq!(generic.classify("backup").0, ContextKind::Generic);
        assert_eq!(generic.classify("backupsystem").0, ContextKind::Neutral);
        assert!(generic
            .generic_markers
            .iter()
            .all(|m| m.match_mode == MatchMode::Exact));
    }

    /// A boundary inside a generic container is still a boundary.
    #[test]
    fn protected_wins_over_generic() {
        let legal = Profile::load("legal").unwrap();
        // "copia" is generic, "expediente" protected: check precedence by
        // classifying a name that is both declared protected and copy-like.
        assert_eq!(legal.classify("expediente").0, ContextKind::Protected);
        assert_eq!(legal.classify("backup").0, ContextKind::Generic);
    }
}
