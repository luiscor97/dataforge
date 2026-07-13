//! Minimal audit ledger (RFC-0001 §29).
//!
//! Chain definition:
//!
//! ```text
//! canonical_payload = canonical JSON (recursively sorted keys) of the payload
//! payload_hash      = hex(SHA-256(canonical_payload))
//! envelope          = canonical JSON of {actor, event_type, payload, sequence, timestamp}
//! event_hash        = hex(SHA-256(previous_hash_utf8 ‖ envelope_utf8))
//! ```
//!
//! The envelope covers the event metadata so tampering with the type,
//! timestamp, actor or sequence breaks the chain, not only payload edits.
//! The first event of a project uses [`GENESIS_HASH`] as `previous_hash`.

use chrono::{SecondsFormat, Utc};
use df_domain::{Actor, AuditEvent, EventId, ProjectId, Timestamp};
use df_error::{DfError, DfResult};
use sha2::{Digest, Sha256};

/// `previous_hash` of the first event in a project chain.
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Serialize a JSON value canonically: objects with recursively sorted keys,
/// no insignificant whitespace.
pub fn canonical_json(value: &serde_json::Value) -> String {
    fn write(value: &serde_json::Value, out: &mut String) {
        match value {
            serde_json::Value::Object(map) => {
                out.push('{');
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                for (i, key) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::Value::String((*key).clone()).to_string());
                    out.push(':');
                    write(&map[*key], out);
                }
                out.push('}');
            }
            serde_json::Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write(item, out);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut out = String::new();
    write(value, &mut out);
    out
}

fn sha256_hex(input: &[u8]) -> String {
    hex::encode(Sha256::digest(input))
}

/// Canonical RFC 3339 timestamp with millisecond precision.
///
/// The precision is fixed so that the string that is hashed is exactly the
/// string that is stored and re-verified.
pub fn canonical_timestamp(ts: Timestamp) -> String {
    ts.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn envelope(
    event_type: &str,
    timestamp: &str,
    actor: Actor,
    sequence: u64,
    payload: &serde_json::Value,
) -> String {
    let value = serde_json::json!({
        "actor": actor.as_str(),
        "event_type": event_type,
        "payload": payload,
        "sequence": sequence,
        "timestamp": timestamp,
    });
    canonical_json(&value)
}

/// Build the next event of a project chain.
///
/// `previous_hash` must be the `event_hash` of the last stored event, or
/// [`GENESIS_HASH`] when `sequence == 1`.
pub fn build_event(
    project_id: ProjectId,
    sequence: u64,
    previous_hash: &str,
    event_type: &str,
    payload: &serde_json::Value,
    actor: Actor,
) -> DfResult<AuditEvent> {
    if sequence == 0 {
        return Err(DfError::Validation(
            "event sequence numbers are 1-based".to_string(),
        ));
    }
    if (sequence == 1) != (previous_hash == GENESIS_HASH) {
        return Err(DfError::Validation(
            "sequence 1 must (and only it may) chain from the genesis hash".to_string(),
        ));
    }

    let timestamp = Utc::now();
    let timestamp_str = canonical_timestamp(timestamp);
    let payload_canonical = canonical_json(payload);
    let payload_hash = sha256_hex(payload_canonical.as_bytes());
    let envelope = envelope(event_type, &timestamp_str, actor, sequence, payload);
    let event_hash = sha256_hex(
        [previous_hash.as_bytes(), envelope.as_bytes()]
            .concat()
            .as_slice(),
    );

    // Store the exact timestamp string that was hashed.
    let timestamp: Timestamp = timestamp_str
        .parse()
        .map_err(|e| DfError::Serialization(format!("timestamp round trip failed: {e}")))?;

    Ok(AuditEvent {
        id: EventId::new(),
        project_id,
        sequence,
        timestamp,
        previous_hash: previous_hash.to_string(),
        event_type: event_type.to_string(),
        payload_json: payload_canonical,
        payload_hash,
        actor,
        event_hash,
    })
}

/// Recompute the expected `event_hash` of a stored event.
pub fn expected_event_hash(event: &AuditEvent) -> DfResult<String> {
    let payload: serde_json::Value = serde_json::from_str(&event.payload_json)
        .map_err(|e| DfError::Serialization(format!("stored payload is not JSON: {e}")))?;
    let envelope = envelope(
        &event.event_type,
        &canonical_timestamp(event.timestamp),
        event.actor,
        event.sequence,
        &payload,
    );
    Ok(sha256_hex(
        [event.previous_hash.as_bytes(), envelope.as_bytes()]
            .concat()
            .as_slice(),
    ))
}

/// Verify a full project chain: contiguous 1-based sequence, genesis anchor,
/// payload hashes and link hashes.
pub fn verify_chain(events: &[AuditEvent]) -> DfResult<()> {
    let mut previous_hash = GENESIS_HASH.to_string();
    for (index, event) in events.iter().enumerate() {
        let expected_sequence = (index + 1) as u64;
        if event.sequence != expected_sequence {
            return Err(DfError::LedgerIntegrity(format!(
                "event #{expected_sequence}: stored sequence is {}",
                event.sequence
            )));
        }
        if event.previous_hash != previous_hash {
            return Err(DfError::LedgerIntegrity(format!(
                "event #{expected_sequence}: previous_hash does not match prior event"
            )));
        }
        let payload_canonical = event.payload_json.as_str();
        let expected_payload_hash = sha256_hex(payload_canonical.as_bytes());
        if event.payload_hash != expected_payload_hash {
            return Err(DfError::LedgerIntegrity(format!(
                "event #{expected_sequence}: payload_hash mismatch"
            )));
        }
        let expected = expected_event_hash(event)?;
        if event.event_hash != expected {
            return Err(DfError::LedgerIntegrity(format!(
                "event #{expected_sequence}: event_hash mismatch"
            )));
        }
        previous_hash = event.event_hash.clone();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain_of(n: u64) -> Vec<AuditEvent> {
        let project = ProjectId::new();
        let mut events = Vec::new();
        let mut prev = GENESIS_HASH.to_string();
        for seq in 1..=n {
            let event = build_event(
                project,
                seq,
                &prev,
                "TEST_EVENT",
                &serde_json::json!({ "n": seq, "b": "x", "a": [1, 2] }),
                Actor::Test,
            )
            .expect("build event");
            prev = event.event_hash.clone();
            events.push(event);
        }
        events
    }

    #[test]
    fn canonical_json_sorts_keys_recursively() {
        let value = serde_json::json!({
            "b": 1,
            "a": { "z": true, "m": [ { "k2": 2, "k1": 1 } ] }
        });
        assert_eq!(
            canonical_json(&value),
            r#"{"a":{"m":[{"k1":1,"k2":2}],"z":true},"b":1}"#
        );
    }

    #[test]
    fn valid_chain_verifies() {
        let events = chain_of(5);
        verify_chain(&events).expect("chain must verify");
    }

    #[test]
    fn empty_chain_is_valid() {
        verify_chain(&[]).expect("empty chain is trivially valid");
    }

    #[test]
    fn tampered_payload_breaks_the_chain() {
        let mut events = chain_of(3);
        events[1].payload_json = r#"{"n":999}"#.to_string();
        let err = verify_chain(&events).unwrap_err();
        assert!(matches!(err, DfError::LedgerIntegrity(_)), "{err}");
    }

    #[test]
    fn tampered_metadata_breaks_the_chain() {
        let mut events = chain_of(3);
        events[2].event_type = "FORGED".to_string();
        assert!(verify_chain(&events).is_err());

        let mut events = chain_of(3);
        events[2].actor = Actor::System;
        assert!(verify_chain(&events).is_err());
    }

    #[test]
    fn removing_an_event_breaks_the_chain() {
        let mut events = chain_of(3);
        events.remove(1);
        assert!(verify_chain(&events).is_err());
    }

    #[test]
    fn recomputing_hashes_after_tampering_still_fails_via_link() {
        // An attacker who edits event 2 and recomputes its hashes still
        // breaks the previous_hash link of event 3.
        let mut events = chain_of(3);
        events[1].payload_json = r#"{"forged":true}"#.to_string();
        events[1].payload_hash =
            hex::encode(sha2::Sha256::digest(events[1].payload_json.as_bytes()));
        events[1].event_hash = expected_event_hash(&events[1]).unwrap();
        assert!(verify_chain(&events).is_err());
    }

    #[test]
    fn genesis_rules_are_enforced() {
        let project = ProjectId::new();
        // sequence 1 must use the genesis hash
        assert!(build_event(project, 1, "abc", "T", &serde_json::json!({}), Actor::Test).is_err());
        // later events must not reuse the genesis hash
        assert!(build_event(
            project,
            2,
            GENESIS_HASH,
            "T",
            &serde_json::json!({}),
            Actor::Test
        )
        .is_err());
        // sequence 0 is invalid
        assert!(build_event(
            project,
            0,
            GENESIS_HASH,
            "T",
            &serde_json::json!({}),
            Actor::Test
        )
        .is_err());
    }
}
