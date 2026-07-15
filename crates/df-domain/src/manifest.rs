//! The immutable execution manifest (RFC-0001 rule 10, §26.4).
//!
//! One [`ManifestEntry`] per approved operation, freezing the *entire*
//! execution contract: what will be read, what content is expected, where it
//! will be written and which operation runs.
//!
//! Why this exists (ADR-0018): the approved plan's hash used to cover only
//! identifiers — occurrence id, content id, destination. The executor then
//! resolved the actual material (source path, fingerprint, expected hashes)
//! through live joins at run time, so editing `content_objects.sha256` after
//! approval changed what was executed while the plan hash stayed valid.
//! Approval bound the paperwork, not the work.
//!
//! This module is pure data: `df-db` persists it, `df-planner` builds and
//! hashes it, `df-executor` executes it and nothing else.

use serde::{Deserialize, Serialize};

use crate::{
    ids::{OperationId, PlanId, SourceRootId},
    plan::OperationType,
};

/// One frozen operation: everything needed to execute it, and nothing that
/// has to be looked up elsewhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub operation_id: OperationId,
    pub plan_id: PlanId,
    pub sequence: u64,
    pub operation_type: OperationType,
    pub idempotency_key: String,

    // --- what will be read ------------------------------------------------
    pub source_root_id: Option<SourceRootId>,
    /// Physical identity of the source root at approval time
    /// (`"volume:index"`), or `None` when the filesystem could not provide
    /// one. `None` means *degraded*, never "no change".
    pub source_root_identity: Option<String>,
    /// The source root's path as recorded at approval; the live row may drift.
    pub source_root_path_snapshot: Option<String>,
    pub source_relative_path_exact: Option<String>,
    pub source_fingerprint: Option<String>,

    // --- what content is expected ----------------------------------------
    pub expected_size_bytes: Option<u64>,
    pub expected_sha256: Option<String>,
    pub expected_blake3: Option<String>,

    // --- where it will be written ----------------------------------------
    pub destination_relative_path: Option<String>,
}

impl ManifestEntry {
    /// Canonical JSON value of this entry.
    ///
    /// Every field that determines what is read, expected, written or done is
    /// present. If a field ever stops being covered here, tampering with it
    /// becomes invisible to the approval hash — which is precisely the bug
    /// this manifest exists to fix, so adding a field to the struct without
    /// adding it here is a security regression.
    pub fn canonical_value(&self) -> serde_json::Value {
        serde_json::json!({
            "operation_id": self.operation_id.to_string(),
            "plan_id": self.plan_id.to_string(),
            "sequence": self.sequence,
            "operation_type": self.operation_type.as_str(),
            "idempotency_key": self.idempotency_key,
            "source_root_id": self.source_root_id.map(|id| id.to_string()),
            "source_root_identity": self.source_root_identity,
            "source_root_path_snapshot": self.source_root_path_snapshot,
            "source_relative_path_exact": self.source_relative_path_exact,
            "source_fingerprint": self.source_fingerprint,
            "expected_size_bytes": self.expected_size_bytes,
            "expected_sha256": self.expected_sha256,
            "expected_blake3": self.expected_blake3,
            "destination_relative_path": self.destination_relative_path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{OperationId, PlanId};

    fn entry() -> ManifestEntry {
        ManifestEntry {
            operation_id: OperationId::new(),
            plan_id: PlanId::new(),
            sequence: 1,
            operation_type: OperationType::CopyActive,
            idempotency_key: "k".repeat(64),
            source_root_id: None,
            source_root_identity: Some("1234:5678".to_string()),
            source_root_path_snapshot: Some("D:/origen".to_string()),
            source_relative_path_exact: Some("a.txt".to_string()),
            source_fingerprint: Some("v1:10:0".to_string()),
            expected_size_bytes: Some(10),
            expected_sha256: Some("a".repeat(64)),
            expected_blake3: Some("b".repeat(64)),
            destination_relative_path: Some("origen/a.txt".to_string()),
        }
    }

    /// Every field that decides what gets read, expected or written must be in
    /// the canonical value; otherwise tampering with it would not move the
    /// approval hash.
    #[test]
    fn the_canonical_value_covers_every_execution_field() {
        let value = entry().canonical_value();
        for field in [
            "operation_id",
            "plan_id",
            "sequence",
            "operation_type",
            "idempotency_key",
            "source_root_id",
            "source_root_identity",
            "source_root_path_snapshot",
            "source_relative_path_exact",
            "source_fingerprint",
            "expected_size_bytes",
            "expected_sha256",
            "expected_blake3",
            "destination_relative_path",
        ] {
            assert!(value.get(field).is_some(), "`{field}` is not covered");
        }
        // The struct and the canonical value must not drift apart.
        let serialized = serde_json::to_value(entry()).unwrap();
        let struct_fields = serialized.as_object().unwrap().len();
        assert_eq!(
            value.as_object().unwrap().len(),
            struct_fields,
            "ManifestEntry gained a field that canonical_value() does not cover"
        );
    }

    #[test]
    fn changing_an_expected_hash_changes_the_canonical_value() {
        let mut tampered = entry();
        let original = tampered.canonical_value();
        tampered.expected_sha256 = Some("c".repeat(64));
        assert_ne!(original, tampered.canonical_value());
    }

    #[test]
    fn changing_the_source_path_changes_the_canonical_value() {
        let mut tampered = entry();
        let original = tampered.canonical_value();
        tampered.source_root_path_snapshot = Some("D:/otro".to_string());
        assert_ne!(original, tampered.canonical_value());
    }
}
