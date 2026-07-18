//! Project-level plugin orchestration (RFC-0001 §22, Milestone 0.6).
//!
//! Registrations persist in SQLite as signed, content-addressed envelopes;
//! every run re-verifies signature, hash, manifest and compilability before
//! executing anything read back from storage. Runs are addressed by the
//! SHA-256 of their serialized configuration and their findings are
//! observations for a human — the ABI has no operation to act.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use df_db::plugins as plugin_db;
use df_db::{inventory, repository, Db};
use df_domain::{Actor, PluginRegistrationId, PluginRun, PluginRunCounters};
use df_error::{DfError, DfResult};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::contract::{
    AnalysisRequest, FindingSeverity, PluginManifest, PluginSubject, HOST_ABI_VERSION,
    INPUT_SCHEMA_VERSION, OUTPUT_SCHEMA_ID,
};
use crate::host::{HostLimits, HostPolicy, PluginHost};
use crate::registry::{PluginKey, RegisteredPluginMetadata, SignedPluginPackage};

const SUBJECT_PAGE: u32 = 256;
const FINDING_FLUSH: usize = 256;
const HARD_MAX_SUBJECTS: u64 = 1_000_000;

/// Options of one project-level plugin execution.
#[derive(Debug, Clone)]
pub struct PluginProjectOptions {
    pub limits: HostLimits,
    pub policy: HostPolicy,
    /// Upper bound of subjects offered to each plugin.
    pub max_subjects: u64,
}

impl Default for PluginProjectOptions {
    fn default() -> Self {
        Self {
            limits: HostLimits::default(),
            policy: HostPolicy::default(),
            max_subjects: 10_000,
        }
    }
}

/// Result of one plugin's run.
#[derive(Debug, Clone, Serialize)]
pub struct PluginRunOutcome {
    pub plugin: String,
    pub run_id: String,
    pub status: String,
    pub config_digest: String,
    pub subjects_total: u64,
    pub subjects_analyzed: u64,
    pub subjects_failed: u64,
    pub subject_cap_reached: bool,
    pub findings: u64,
}

impl PluginRunOutcome {
    fn from_run(plugin: &PluginKey, run: &PluginRun) -> Self {
        Self {
            plugin: plugin.to_string(),
            run_id: run.id.to_string(),
            status: run.status.as_str().to_string(),
            config_digest: run.config_digest.clone(),
            subjects_total: run.counters.subjects_total,
            subjects_analyzed: run.counters.subjects_analyzed,
            subjects_failed: run.counters.subjects_failed,
            subject_cap_reached: run.subject_cap_reached,
            findings: run.counters.findings_total,
        }
    }
}

/// Serializable result of executing every registered plugin.
#[derive(Debug, Clone, Serialize)]
pub struct PluginsOutcome {
    pub snapshot_id: String,
    pub runs: Vec<PluginRunOutcome>,
    pub cancelled: bool,
    /// Always true: plugin findings never authorise an operation.
    pub evidence_only: bool,
}

/// Verify and persist one signed package. Verification is the host's:
/// signature, component hash, manifest coherence, ABI compatibility and a
/// full compile + type-check happen before anything is stored.
pub fn register_project_plugin(
    db: &mut Db,
    actor: Actor,
    package: SignedPluginPackage,
    options: &PluginProjectOptions,
) -> DfResult<RegisteredPluginMetadata> {
    let project = repository::load_project(db)?;
    let mut host = PluginHost::new(options.limits.clone(), options.policy.clone())
        .map_err(|error| DfError::Validation(error.to_string()))?;
    let stored = plugin_db::StoredRegistration {
        id: PluginRegistrationId::new(),
        plugin_id: package.manifest.plugin_id.clone(),
        plugin_version: package.manifest.plugin_version.clone(),
        manifest_json: serde_json::to_string(&package.manifest)
            .map_err(|error| DfError::Validation(format!("manifest serialization: {error}")))?,
        component_sha256: package.component_sha256.clone(),
        component: package.component_bytes.clone(),
        publisher_public_key_hex: package.publisher_public_key_hex.clone(),
        signature_hex: package.signature_hex.clone(),
    };
    let key = host
        .register(package)
        .map_err(|error| DfError::Validation(format!("plugin rejected: {error}")))?;
    plugin_db::insert_registration(db, project.id, &stored, actor)?;
    host.registered_plugins()
        .into_iter()
        .find(|metadata| metadata.key == key)
        .ok_or_else(|| DfError::Validation("registered plugin metadata missing".to_string()))
}

/// The exact serialized configuration whose SHA-256 addresses one run.
#[derive(Serialize)]
struct RunConfig<'a> {
    abi_version: &'a str,
    input_schema_version: &'a str,
    output_schema_id: &'a str,
    plugin: String,
    component_sha256: &'a str,
    limits: &'a HostLimits,
    policy: &'a HostPolicy,
    max_subjects: u64,
}

/// Execute every registered plugin over the unique contents of the latest
/// snapshot. Digest-addressed reuse returns sealed runs without executing.
pub fn run_project_plugins(
    db: &mut Db,
    actor: Actor,
    options: &PluginProjectOptions,
    cancel: Option<&AtomicBool>,
) -> DfResult<PluginsOutcome> {
    if options.max_subjects == 0 || options.max_subjects > HARD_MAX_SUBJECTS {
        return Err(DfError::Validation(format!(
            "max_subjects must be between 1 and {HARD_MAX_SUBJECTS}"
        )));
    }
    let project = repository::load_project(db)?;
    let snapshot = inventory::latest_complete_snapshot(db, project.id)?
        .ok_or_else(|| DfError::Validation("the project has no complete snapshot".to_string()))?;
    let registrations = plugin_db::list_registrations(db, project.id)?;
    if registrations.is_empty() {
        return Err(DfError::Validation(
            "no plugins are registered in this project".to_string(),
        ));
    }

    // Re-verify and compile everything read back from storage.
    let mut host = PluginHost::new(options.limits.clone(), options.policy.clone())
        .map_err(|error| DfError::Validation(error.to_string()))?;
    let mut plugins: Vec<(PluginKey, plugin_db::StoredRegistration)> = Vec::new();
    for registration in registrations {
        let manifest: PluginManifest =
            serde_json::from_str(&registration.manifest_json).map_err(|error| {
                DfError::Validation(format!(
                    "stored manifest for registration `{}`: {error}",
                    registration.id
                ))
            })?;
        let key = host
            .register(SignedPluginPackage {
                manifest,
                component_sha256: registration.component_sha256.clone(),
                component_bytes: registration.component.clone(),
                publisher_public_key_hex: registration.publisher_public_key_hex.clone(),
                signature_hex: registration.signature_hex.clone(),
            })
            .map_err(|error| {
                DfError::Conflict(format!(
                    "stored registration `{}` failed re-verification: {error}",
                    registration.id
                ))
            })?;
        plugins.push((key, registration));
    }

    let mut runs = Vec::with_capacity(plugins.len());
    for (key, registration) in &plugins {
        let config_json = serde_json::to_string(&RunConfig {
            abi_version: HOST_ABI_VERSION,
            input_schema_version: INPUT_SCHEMA_VERSION,
            output_schema_id: OUTPUT_SCHEMA_ID,
            plugin: key.to_string(),
            component_sha256: &registration.component_sha256,
            limits: &options.limits,
            policy: &options.policy,
            max_subjects: options.max_subjects,
        })
        .map_err(|error| DfError::Validation(format!("plugin config serialization: {error}")))?;
        let config_digest = hex::encode(Sha256::digest(config_json.as_bytes()));
        let run = plugin_db::start_or_resume_run(
            db,
            &plugin_db::PluginRunSpec {
                project_id: project.id,
                snapshot_id: snapshot.id,
                registration_id: registration.id,
                config_digest,
                config_json,
            },
            actor,
        )?;
        if run.status == df_domain::PluginRunStatus::Completed {
            runs.push(PluginRunOutcome::from_run(key, &run));
            continue;
        }

        plugin_db::reset_run_findings(db, run.id)?;
        let mut counters = PluginRunCounters::default();
        let mut subject_cap_reached = false;
        let mut pending: Vec<plugin_db::FindingInput> = Vec::new();
        let mut cursor: Option<String> = None;
        'subjects: loop {
            let subjects =
                plugin_db::plugin_subjects_after(db, snapshot.id, cursor.as_deref(), SUBJECT_PAGE)?;
            if subjects.is_empty() {
                break;
            }
            for subject in &subjects {
                if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                    return Ok(PluginsOutcome {
                        snapshot_id: snapshot.id.to_string(),
                        runs,
                        cancelled: true,
                        evidence_only: true,
                    });
                }
                if counters.subjects_total == options.max_subjects {
                    // At least this subject remains: the cap cut a real tail.
                    subject_cap_reached = true;
                    break 'subjects;
                }
                counters.subjects_total += 1;
                let request = AnalysisRequest {
                    request_id: format!("{}:{}", run.id, subject.content_id),
                    subject: PluginSubject {
                        // Content-addressed: stable across snapshots and
                        // meaningless to forge.
                        id: subject.sha256.clone(),
                        kind: "CONTENT".to_string(),
                    },
                    metadata: subject_metadata(subject),
                    normalized_text: None,
                };
                match host.analyze(key, &request) {
                    Ok(output) => {
                        counters.subjects_analyzed += 1;
                        // `subject_id` persists exactly as the plugin claimed
                        // it: a finding is that plugin's statement, bound to
                        // its signed identity by the run, never host truth.
                        for finding in output.findings {
                            counters.findings_total += 1;
                            pending.push(plugin_db::FindingInput {
                                subject_id: finding.subject_id,
                                code: finding.code,
                                severity: match finding.severity {
                                    FindingSeverity::Info => "INFO".to_string(),
                                    FindingSeverity::Warning => "WARNING".to_string(),
                                },
                                message: finding.message,
                                suggestions_json: serde_json::to_string(&finding.suggestions)
                                    .map_err(|error| {
                                        DfError::Validation(format!(
                                            "suggestion serialization: {error}"
                                        ))
                                    })?,
                                evidence_json: serde_json::to_string(&finding.evidence).map_err(
                                    |error| {
                                        DfError::Validation(format!(
                                            "evidence serialization: {error}"
                                        ))
                                    },
                                )?,
                            });
                            if pending.len() >= FINDING_FLUSH {
                                plugin_db::record_findings(db, run.id, snapshot.id, &pending)?;
                                pending.clear();
                            }
                        }
                    }
                    // Traps, limit hits and malformed outputs are per-subject
                    // evidence of failure, never a silent gap.
                    Err(_) => counters.subjects_failed += 1,
                }
            }
            cursor = subjects.last().map(|subject| subject.content_id.clone());
        }
        plugin_db::record_findings(db, run.id, snapshot.id, &pending)?;
        let completed = plugin_db::complete_run(db, run.id, counters, subject_cap_reached, actor)?;
        runs.push(PluginRunOutcome::from_run(key, &completed));
    }

    Ok(PluginsOutcome {
        snapshot_id: snapshot.id.to_string(),
        runs,
        cancelled: false,
        evidence_only: true,
    })
}

fn subject_metadata(subject: &plugin_db::PluginSubjectSource) -> BTreeMap<String, String> {
    let mut metadata = BTreeMap::new();
    metadata.insert("relative_path".to_string(), subject.relative_path.clone());
    metadata.insert("size_bytes".to_string(), subject.size_bytes.to_string());
    if let Some(extension) = &subject.extension {
        metadata.insert("extension".to_string(), extension.clone());
    }
    metadata
}
