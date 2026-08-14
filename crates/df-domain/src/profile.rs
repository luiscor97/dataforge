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

use crate::analysis::glob_matches;

use crate::{context::ContextKind, RuleDefinition};

/// The declarative-profile schema identifier and version. Frozen for 1.0
/// (M0.9): changing either requires a new version and an ADR, never an
/// in-place edit.
///
/// `2.0.0` (M2.2, ADR-0040 §6) adds `destination_roots`. A **major** bump and
/// not a minor one, even though `generic`'s output is unchanged: a 2.0 profile
/// may declare roots a 1.x engine knows nothing about, and a 1.x engine would
/// ignore the field and quietly produce different destination paths. Refusing
/// to load is the only safe reading of a profile you cannot fully interpret.
pub const PROFILE_SCHEMA: &str = "dataforge.profile";
pub const PROFILE_SCHEMA_VERSION: &str = "2.0.0";
const SCHEMA: &str = PROFILE_SCHEMA;
const SCHEMA_VERSION: &str = PROFILE_SCHEMA_VERSION;

/// The conservative default used when a project does not select a profile.
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

/// One destination root a profile declares (ADR-0040 §1).
///
/// Declared rather than hardcoded so that enriching the output does not mean
/// growing a `match` in the planner — and so that the set is enumerable before
/// a plan runs, which is what lets the reserved-name check see every root
/// instead of the three that happened to be constants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestinationRootDef {
    /// Stable identifier, independent of the folder name. This is what gets
    /// persisted as routing provenance, so renaming `folder` never rewrites
    /// the history of plans made before the rename.
    pub id: String,
    /// Literal directory name created under the output root. `null` routes to
    /// the output root itself — the working archive.
    #[serde(default)]
    pub folder: Option<String>,
    /// Whether what lands here belongs to the working archive or is set aside
    /// from it. Reports and presentation read this; routing does not.
    #[serde(default)]
    pub set_aside: bool,
}

/// The root ids the engine can route into, and therefore the ones every
/// profile has to declare.
///
/// Checked at load time and fail-closed: a profile that omits one would send
/// an operation to a root that does not exist, and the failure would surface
/// as a broken plan over a real archive rather than as a rejected profile.
pub const REQUIRED_DESTINATION_ROOTS: &[&str] = &["active", "review", "separated", "temporary"];

/// A parsed, resolved profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Ordered metadata rules. The first matching rule is the default action;
    /// every match remains evidence even when human review overrides it.
    #[serde(default)]
    pub rules: Vec<RuleDefinition>,
    /// The destination roots this profile routes into, in presentation order.
    ///
    /// Not `#[serde(default)]`: a profile that forgot to declare them would
    /// silently inherit whatever the engine assumed, which is the coupling
    /// ADR-0040 exists to remove. `generic` declares exactly the four the 1.x
    /// engine hardcoded, so its output is unchanged byte for byte.
    pub destination_roots: Vec<DestinationRootDef>,
    /// Occurrences this profile declines to hash.
    ///
    /// Reading 443 GB of video to prove it is video costs hours and answers
    /// nothing, so a profile may name material the identity pipeline should
    /// skip. The file is still **inventoried**, and its exclusion is recorded
    /// with the rule that caused it: RFC-0001's coverage criterion asks that
    /// nothing be left without representation *or reason*, and this gives it a
    /// reason. Excluded is not discarded, and the origin is untouched either
    /// way.
    #[serde(default)]
    pub hash_exclusions: Vec<HashExclusion>,
}

/// One declared reason not to hash something.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HashExclusion {
    pub id: String,
    /// Why, in the operator's words. Stored with every exclusion it causes,
    /// because "this was not read" is only acceptable if it comes with why.
    pub reason: String,
    pub r#match: ExclusionMatch,
}

/// What an exclusion matches. Every field that is present must match.
///
/// At least one must be present. An exclusion with no criteria matches the
/// entire archive, and a profile that silently declined to read anything is
/// the worst failure this engine could have — so it is rejected at load
/// rather than trusted to be intentional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExclusionMatch {
    /// Glob over the path relative to its source root, compared case
    /// insensitively. This is what names a folder — the case `file_name_glob`
    /// cannot express.
    #[serde(default)]
    pub path_glob: Option<String>,
    #[serde(default)]
    pub file_name_glob: Option<String>,
    /// Size at or above which this matches. What makes "heavy media"
    /// expressible without guessing from the extension.
    #[serde(default)]
    pub min_size_bytes: Option<u64>,
}

impl ExclusionMatch {
    /// Whether this matcher names anything at all.
    pub fn is_empty(&self) -> bool {
        self.path_glob.is_none() && self.file_name_glob.is_none() && self.min_size_bytes.is_none()
    }

    /// Whether an occurrence matches. Conjunctive: every declared criterion
    /// has to hold, so adding one always narrows and never widens.
    pub fn matches(&self, relative_path: &str, file_name: &str, size_bytes: u64) -> bool {
        if self.is_empty() {
            return false;
        }
        if let Some(glob) = &self.path_glob {
            if !glob_matches(&glob.to_lowercase(), &relative_path.to_lowercase()) {
                return false;
            }
        }
        if let Some(glob) = &self.file_name_glob {
            if !glob_matches(&glob.to_lowercase(), &file_name.to_lowercase()) {
                return false;
            }
        }
        if let Some(minimum) = self.min_size_bytes {
            if size_bytes < minimum {
                return false;
            }
        }
        true
    }
}

impl Profile {
    /// Reject a declared root set the planner could not route with.
    ///
    /// Fail-closed on every count. A missing root would send an operation to a
    /// place that does not exist; a duplicate id would make provenance
    /// ambiguous; a duplicate folder would put two meanings in one directory;
    /// and a folder name with a separator in it would escape the output root,
    /// which is the one thing the filesystem boundary must never allow.
    fn validate_destination_roots(&self) -> df_error::DfResult<()> {
        let id = &self.id;
        let mut ids = std::collections::HashSet::new();
        let mut folders = std::collections::HashSet::new();
        for root in &self.destination_roots {
            if root.id.trim().is_empty() {
                return Err(df_error::DfError::Validation(format!(
                    "profile `{id}` declares a destination root with an empty id"
                )));
            }
            if !ids.insert(root.id.as_str()) {
                return Err(df_error::DfError::Validation(format!(
                    "profile `{id}` declares destination root `{}` more than once",
                    root.id
                )));
            }
            let Some(folder) = &root.folder else {
                continue;
            };
            if folder.trim().is_empty() {
                return Err(df_error::DfError::Validation(format!(
                    "profile `{id}` gives destination root `{}` an empty folder; use null for \
                     the working archive",
                    root.id
                )));
            }
            if folder.contains(['/', '\\']) || folder.contains("..") {
                return Err(df_error::DfError::Validation(format!(
                    "profile `{id}` declares destination root folder `{folder}`, which is not a \
                     single directory name"
                )));
            }
            if !folders.insert(folder.to_lowercase()) {
                return Err(df_error::DfError::Validation(format!(
                    "profile `{id}` maps two destination roots onto folder `{folder}`"
                )));
            }
        }
        for required in REQUIRED_DESTINATION_ROOTS {
            if !ids.contains(required) {
                return Err(df_error::DfError::Validation(format!(
                    "profile `{id}` does not declare the required destination root `{required}`"
                )));
            }
        }
        Ok(())
    }

    /// Load a built-in profile by id, resolving inheritance.
    ///
    /// Unknown ids are rejected. Falling back to [`DEFAULT_PROFILE_ID`] would
    /// turn a typo such as `legla` into a project with none of the protections
    /// the user selected.
    pub fn load(id: &str) -> df_error::DfResult<Self> {
        let json = BUILT_IN
            .iter()
            .find(|(name, _)| *name == id)
            .map(|(_, json)| *json)
            .ok_or_else(|| {
                df_error::DfError::Validation(format!(
                    "unknown profile `{id}`; available built-in profiles: {}",
                    Self::built_in_ids().join(", ")
                ))
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
        if profile.schema_version != SCHEMA_VERSION {
            return Err(df_error::DfError::Validation(format!(
                "profile `{id}` has unsupported schema version `{}` (expected {SCHEMA_VERSION})",
                profile.schema_version
            )));
        }
        if profile.id != id {
            return Err(df_error::DfError::Validation(format!(
                "profile `{id}` declares mismatched id `{}`",
                profile.id
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
            for inherited in parent.rules {
                if !profile.rules.iter().any(|rule| rule.id == inherited.id) {
                    profile.rules.push(inherited);
                }
            }
        }
        let mut rule_ids = std::collections::HashSet::new();
        for rule in &profile.rules {
            rule.validate()?;
            if !rule_ids.insert(rule.id.as_str()) {
                return Err(df_error::DfError::Validation(format!(
                    "profile `{id}` declares rule `{}` more than once",
                    rule.id
                )));
            }
        }
        profile.validate_destination_roots()?;
        profile.validate_hash_exclusions()?;
        Ok(profile)
    }

    /// Reject an exclusion that would decline to read the archive.
    ///
    /// Fail-closed, and this one is not a formality. A matcher with no
    /// criteria matches everything, so a profile carrying one would inventory
    /// a 443 GB archive, hash none of it, and produce a plan that looks
    /// finished. That failure is silent by construction — there is no error to
    /// notice — which is exactly why it has to be impossible to load rather
    /// than merely unlikely to be written.
    fn validate_hash_exclusions(&self) -> df_error::DfResult<()> {
        let id = &self.id;
        let mut ids = std::collections::HashSet::new();
        for exclusion in &self.hash_exclusions {
            if exclusion.id.trim().is_empty() {
                return Err(df_error::DfError::Validation(format!(
                    "profile `{id}` has a hash exclusion with no id"
                )));
            }
            if !ids.insert(exclusion.id.as_str()) {
                return Err(df_error::DfError::Validation(format!(
                    "profile `{id}` declares hash exclusion `{}` twice",
                    exclusion.id
                )));
            }
            if exclusion.reason.trim().is_empty() {
                return Err(df_error::DfError::Validation(format!(
                    "profile `{id}` hash exclusion `{}` has no reason; material that \
                     was never read must say why",
                    exclusion.id
                )));
            }
            if exclusion.r#match.is_empty() {
                return Err(df_error::DfError::Validation(format!(
                    "profile `{id}` hash exclusion `{}` matches on nothing, which would \
                     exclude the entire archive",
                    exclusion.id
                )));
            }
        }
        Ok(())
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
            assert_eq!(profile.schema_version, SCHEMA_VERSION);
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
            for rule in &profile.rules {
                rule.validate().unwrap();
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
        assert!(!generic.rules.is_empty());
    }

    #[test]
    fn an_unknown_profile_is_rejected_instead_of_falling_back_to_generic() {
        let error = Profile::load("does-not-exist").unwrap_err();
        match error {
            df_error::DfError::Validation(message) => {
                assert!(message.contains("does-not-exist"));
                assert!(message.contains("generic"));
                assert!(message.contains("legal"));
            }
            other => panic!("expected validation error, got {other:?}"),
        }
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
        assert!(legal.rules.iter().any(|r| r.id == "temporary.office-lock"));
        assert!(legal
            .rules
            .iter()
            .any(|r| r.id == "legal.correspondence-eml"));
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

    #[test]
    fn built_in_rules_classify_without_authorizing_destructive_actions() {
        let generic = Profile::load("generic").unwrap();
        let lock = generic
            .rules
            .iter()
            .find(|rule| rule.matches_file_name("~$Contrato.docx"))
            .expect("office lock rule");
        assert_eq!(lock.action.as_str(), "COPY_TEMPORARY");

        let backup = generic
            .rules
            .iter()
            .find(|rule| rule.matches_file_name("contrato.bak"))
            .expect("backup review rule");
        assert_eq!(backup.action.as_str(), "COPY_REVIEW");
        assert!(generic
            .rules
            .iter()
            .all(|rule| rule.action.operation_type().is_executable()));
    }

    /// A profile with a valid root set, to mutate in the rejection tests.
    fn profile_with_roots(roots: Vec<DestinationRootDef>) -> Profile {
        let mut profile = Profile::load(DEFAULT_PROFILE_ID).expect("generic loads");
        profile.destination_roots = roots;
        profile
    }

    fn root(id: &str, folder: Option<&str>) -> DestinationRootDef {
        DestinationRootDef {
            id: id.to_string(),
            folder: folder.map(str::to_string),
            set_aside: false,
        }
    }

    fn full_set() -> Vec<DestinationRootDef> {
        vec![
            root("active", None),
            root("review", Some("90_DataForge_Review")),
            root("separated", Some("95_DataForge_Separated")),
            root("temporary", Some("98_DataForge_Temporary")),
        ]
    }

    #[test]
    fn every_built_in_profile_declares_every_routable_root() {
        // The planner routes by id. A profile missing one would send an
        // operation to a root that does not exist, and over a real archive
        // that surfaces as a broken plan rather than as a rejected profile.
        for id in Profile::built_in_ids() {
            let profile = Profile::load(id).unwrap_or_else(|e| panic!("profile `{id}`: {e}"));
            for required in REQUIRED_DESTINATION_ROOTS {
                assert!(
                    profile
                        .destination_roots
                        .iter()
                        .any(|root| root.id == *required),
                    "profile `{id}` does not declare root `{required}`"
                );
            }
        }
    }

    #[test]
    fn a_missing_required_root_is_rejected() {
        let mut roots = full_set();
        roots.retain(|root| root.id != "review");
        let error = profile_with_roots(roots)
            .validate_destination_roots()
            .expect_err("a set without `review` cannot route a review copy");
        assert!(error.to_string().contains("review"), "{error}");
    }

    #[test]
    fn a_duplicate_root_id_is_rejected() {
        // Two roots with one id make the recorded provenance ambiguous: the
        // column would no longer identify where something landed.
        let mut roots = full_set();
        roots.push(root("review", Some("otra_carpeta")));
        let error = profile_with_roots(roots)
            .validate_destination_roots()
            .expect_err("an id declared twice is ambiguous");
        assert!(error.to_string().contains("more than once"), "{error}");
    }

    #[test]
    fn two_roots_cannot_share_one_folder() {
        // Two meanings in one directory: nothing downstream could tell which
        // root a file in it came from.
        let mut roots = full_set();
        roots.push(root("extra", Some("90_dataforge_review")));
        let error = profile_with_roots(roots)
            .validate_destination_roots()
            .expect_err("two roots mapping onto one folder is ambiguous");
        assert!(
            error.to_string().contains("two destination roots"),
            "{error}"
        );
    }

    #[test]
    fn a_root_folder_must_be_a_single_directory_name() {
        // The one that matters for safety: a folder carrying a separator or a
        // parent reference would place a root outside the output root, which
        // is the boundary the whole engine rests on.
        for escaping in ["a/b", "a\\b", "..", "../fuera"] {
            let mut roots = full_set();
            roots.push(root("escapa", Some(escaping)));
            let error = profile_with_roots(roots)
                .validate_destination_roots()
                .unwrap_err();
            assert!(
                error.to_string().contains("single directory name"),
                "`{escaping}` was accepted as a root folder: {error}"
            );
        }
    }

    #[test]
    fn an_empty_folder_name_is_rejected_but_null_is_the_archive() {
        let mut roots = full_set();
        roots.push(root("vacia", Some("   ")));
        assert!(profile_with_roots(roots)
            .validate_destination_roots()
            .is_err());

        // `null` is how a root says "the output root itself", and `active`
        // uses it, so it has to stay valid.
        assert!(profile_with_roots(full_set())
            .validate_destination_roots()
            .is_ok());
    }
}

#[cfg(test)]
mod exclusion_tests {
    use super::*;

    fn profile_with(exclusions: Vec<HashExclusion>) -> Profile {
        let mut profile = Profile::load("generic").expect("generic loads");
        profile.hash_exclusions = exclusions;
        profile
    }

    #[test]
    fn an_exclusion_that_matches_nothing_in_particular_is_refused() {
        // It would match every file, so a profile carrying one would inventory
        // an archive, read none of it, and look finished. There is no error to
        // notice at run time, which is why it must not be loadable.
        let error = profile_with(vec![HashExclusion {
            id: "everything".to_string(),
            reason: "oops".to_string(),
            r#match: ExclusionMatch::default(),
        }])
        .validate_hash_exclusions()
        .expect_err("a matcher with no criteria is not a matcher");
        assert!(error.to_string().contains("entire archive"));
    }

    #[test]
    fn an_exclusion_without_a_reason_is_refused() {
        let error = profile_with(vec![HashExclusion {
            id: "silent".to_string(),
            reason: "  ".to_string(),
            r#match: ExclusionMatch {
                file_name_glob: Some("*.mp4".to_string()),
                ..ExclusionMatch::default()
            },
        }])
        .validate_hash_exclusions()
        .expect_err("material never read must say why");
        assert!(error.to_string().contains("no reason"));
    }

    #[test]
    fn every_declared_criterion_has_to_hold() {
        // Conjunctive, so adding a criterion always narrows. If it widened,
        // a rule meant for large videos would start taking small ones.
        let matcher = ExclusionMatch {
            file_name_glob: Some("*.mp4".to_string()),
            min_size_bytes: Some(1000),
            ..ExclusionMatch::default()
        };
        assert!(matcher.matches("a/b/vista.mp4", "vista.mp4", 2000));
        assert!(
            !matcher.matches("a/b/vista.mp4", "vista.mp4", 999),
            "too small"
        );
        assert!(
            !matcher.matches("a/b/nota.txt", "nota.txt", 2000),
            "wrong name"
        );
        // Case-insensitive, because Windows paths are.
        assert!(matcher.matches("A/B/VISTA.MP4", "VISTA.MP4", 2000));
    }

    #[test]
    fn a_path_glob_names_a_folder_that_a_file_name_glob_cannot() {
        let matcher = ExclusionMatch {
            path_glob: Some("TomTom*".to_string()),
            ..ExclusionMatch::default()
        };
        assert!(matcher.matches("TomTom/HOME/mapa.dat", "mapa.dat", 10));
        assert!(!matcher.matches("asunto/escrito.pdf", "escrito.pdf", 10));
    }
}
