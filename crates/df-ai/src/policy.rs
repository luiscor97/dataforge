//! Consent by policy, with a budget (M2.5, ADR-0042).
//!
//! ADR-0034 asks a human to approve **each** disclosure by looking at its
//! [`DisclosureManifest`]. That is right for assisted use, where a person is
//! already sitting there, and it is unusable for an autonomous run: a queue of
//! 5.334 items cannot be approved one prompt at a time, and a run that stops to
//! ask is a run that does not finish.
//!
//! So the human approves a **policy** once — which fields may leave, to which
//! provider, and how much it may cost — and every invocation is audited against
//! it. The per-request token does not go away; it is issued against a policy
//! that already bounded what could be asked.
//!
//! # The three properties that make this safe
//!
//! **Audited before the key or the network is touched.** [`authorize`] is a
//! pure decision over a manifest and a running total; it returns before
//! anything is sent. Auditing afterwards would record a disclosure that had
//! already happened, which is a log, not a control.
//!
//! **Exhausted degrades, it never blocks.** A spent budget yields
//! [`PolicyDecision::Exhausted`], and the caller routes the remaining ambiguous
//! items to `revisar/` and carries on. It does not stop the run, and it does not
//! keep spending.
//!
//! **A field outside the policy is refused, not truncated.** Quietly dropping
//! it would send *something* the human never approved, and the manifest digest
//! would no longer describe what was agreed.
//!
//! The key itself is untouched by any of this: it stays in the OS store, never
//! in SQLite and never in the policy (ADR-0034 §4).

use serde::{Deserialize, Serialize};

use crate::types::{canonical_json, sha256_hex, DisclosureManifest, ProviderDescriptor};

/// Schema identifier of the disclosure policy.
pub const DISCLOSURE_POLICY_SCHEMA_VERSION: &str = "dataforge.ai-disclosure-policy/0.1.0";

/// What a policy allows to be spent before it degrades.
///
/// Every limit is a hard ceiling, and `0` means "not allowed at all" rather
/// than "unlimited" — a budget nobody set must not be a budget without end.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    /// Provider invocations.
    pub calls: u32,
    /// Bytes of disclosed content, measured after redaction — the bytes that
    /// actually leave, not the bytes considered.
    pub disclosed_bytes: u64,
    /// Spend, in cents of the provider's billing currency.
    pub spend_cents: u64,
}

/// What has been spent so far against a policy.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Consumption {
    pub calls: u32,
    pub disclosed_bytes: u64,
    pub spend_cents: u64,
}

impl Consumption {
    /// Record one completed invocation.
    ///
    /// Saturating on purpose: an overflow that wrapped would turn a spent
    /// budget into a fresh one, which is the one arithmetic mistake here that
    /// spends real money.
    pub fn record(&mut self, disclosed_bytes: u64, spend_cents: u64) {
        self.calls = self.calls.saturating_add(1);
        self.disclosed_bytes = self.disclosed_bytes.saturating_add(disclosed_bytes);
        self.spend_cents = self.spend_cents.saturating_add(spend_cents);
    }
}

/// A disclosure policy: approved once, audited against every invocation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DisclosurePolicy {
    pub schema_version: String,
    /// The only provider this policy authorises. Compared exactly: a different
    /// model or endpoint is a different disclosure than the one approved.
    pub provider: ProviderDescriptor,
    /// Field names that may be disclosed. Anything else is refused.
    pub allowed_fields: Vec<String>,
    pub budget: Budget,
}

impl DisclosurePolicy {
    /// Build a policy for a provider, fields and budget.
    pub fn new(provider: ProviderDescriptor, allowed_fields: Vec<String>, budget: Budget) -> Self {
        Self {
            schema_version: DISCLOSURE_POLICY_SCHEMA_VERSION.to_string(),
            provider,
            allowed_fields,
            budget,
        }
    }

    /// SHA-256 over canonical JSON, so the digest is stable across round trips.
    ///
    /// Sealed with each invocation's provenance: "under what terms was this
    /// disclosed?" has a deterministic answer only if the terms can be named.
    pub fn digest(&self) -> String {
        let value = serde_json::to_value(self)
            .expect("DisclosurePolicy has an infallible serde representation");
        sha256_hex(canonical_json(&value).as_bytes())
    }

    /// Whether this policy permits a field name.
    pub fn permits_field(&self, field_name: &str) -> bool {
        self.allowed_fields.iter().any(|name| name == field_name)
    }
}

/// Why an invocation was refused outright.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "kind", content = "detail")]
pub enum RefusalReason {
    /// The manifest names a provider the policy did not approve.
    ProviderNotApproved(String),
    /// The manifest discloses a field the policy did not approve.
    FieldNotApproved(String),
    /// The policy was written against a schema this build cannot interpret.
    UnsupportedSchema(String),
}

/// The decision for one invocation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "decision")]
pub enum PolicyDecision {
    /// Within the policy and within budget. Carries the digest to seal into
    /// the invocation's provenance.
    Allow { policy_digest: String },
    /// Inside the policy but out of budget. The caller routes what is left to
    /// `revisar/` and the run continues — this is a degradation, not a failure.
    Exhausted { policy_digest: String },
    /// Outside the policy. Nothing about this invocation is permitted.
    Refused {
        policy_digest: String,
        reason: RefusalReason,
    },
}

impl PolicyDecision {
    /// Whether the caller may proceed to touch the key and the network.
    pub fn is_allowed(&self) -> bool {
        matches!(self, PolicyDecision::Allow { .. })
    }
}

/// Judge one invocation against a policy and what has been spent.
///
/// Pure and side-effect free: it decides *before* anything is sent, which is
/// the whole point. `estimated_spend_cents` is the caller's estimate for this
/// invocation; the budget is checked against the total it would produce, not
/// against the total already spent, so the last call that would cross the line
/// does not get to cross it.
pub fn authorize(
    policy: &DisclosurePolicy,
    consumed: &Consumption,
    manifest: &DisclosureManifest,
    estimated_spend_cents: u64,
) -> PolicyDecision {
    let policy_digest = policy.digest();

    if policy.schema_version != DISCLOSURE_POLICY_SCHEMA_VERSION {
        return PolicyDecision::Refused {
            policy_digest,
            reason: RefusalReason::UnsupportedSchema(policy.schema_version.clone()),
        };
    }

    // The provider is part of what was approved. A different model or endpoint
    // is a different disclosure, whatever the field list says.
    if manifest.provider != policy.provider {
        return PolicyDecision::Refused {
            policy_digest,
            reason: RefusalReason::ProviderNotApproved(manifest.provider.provider.clone()),
        };
    }

    // Refused, never truncated: dropping a field would send something the
    // human never approved while the digest still claimed otherwise.
    for field in &manifest.fields {
        if !policy.permits_field(&field.field_name) {
            return PolicyDecision::Refused {
                policy_digest,
                reason: RefusalReason::FieldNotApproved(field.field_name.clone()),
            };
        }
    }

    // Bytes that actually leave, after redaction.
    let disclosed: u64 = manifest
        .fields
        .iter()
        .map(|field| field.visible_bytes as u64)
        .sum();

    let over_calls = consumed.calls.saturating_add(1) > policy.budget.calls;
    let over_bytes =
        consumed.disclosed_bytes.saturating_add(disclosed) > policy.budget.disclosed_bytes;
    let over_spend =
        consumed.spend_cents.saturating_add(estimated_spend_cents) > policy.budget.spend_cents;

    if over_calls || over_bytes || over_spend {
        return PolicyDecision::Exhausted { policy_digest };
    }

    PolicyDecision::Allow { policy_digest }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DisclosedField, ProviderKind};

    fn provider() -> ProviderDescriptor {
        ProviderDescriptor {
            kind: ProviderKind::Cloud,
            provider: "acme".to_string(),
            model: "m-1".to_string(),
            endpoint: "https://example.invalid/v1".to_string(),
        }
    }

    fn field(name: &str, visible_bytes: usize) -> DisclosedField {
        DisclosedField {
            evidence_id: "ev-1".to_string(),
            field_name: name.to_string(),
            visible_text: "x".repeat(visible_bytes),
            visible_bytes,
            source_sha256: "0".repeat(64),
            visible_sha256: "1".repeat(64),
            redactions: Vec::new(),
        }
    }

    fn manifest(fields: Vec<DisclosedField>) -> DisclosureManifest {
        let visible: usize = fields.iter().map(|f| f.visible_bytes).sum();
        DisclosureManifest {
            schema_version: crate::types::DISCLOSURE_SCHEMA_VERSION.to_string(),
            request_id: "req-1".to_string(),
            purpose: crate::types::AssistancePurpose::Explain,
            provider: provider(),
            prompt_version: crate::types::PROMPT_VERSION.to_string(),
            fields,
            visible_content_bytes: visible,
            transport_bytes: visible,
            transport_sha256: "2".repeat(64),
        }
    }

    fn policy() -> DisclosurePolicy {
        DisclosurePolicy::new(
            provider(),
            vec!["title".to_string(), "excerpt".to_string()],
            Budget {
                calls: 2,
                disclosed_bytes: 100,
                spend_cents: 50,
            },
        )
    }

    #[test]
    fn an_invocation_within_the_policy_is_allowed() {
        let decision = authorize(
            &policy(),
            &Consumption::default(),
            &manifest(vec![field("title", 10)]),
            5,
        );
        assert!(decision.is_allowed(), "{decision:?}");
    }

    #[test]
    fn a_field_outside_the_policy_is_refused_not_truncated() {
        // Dropping it would send something the human never approved while the
        // manifest digest still claimed to describe what was agreed.
        let decision = authorize(
            &policy(),
            &Consumption::default(),
            &manifest(vec![field("title", 10), field("full_body", 10)]),
            5,
        );
        assert_eq!(
            decision,
            PolicyDecision::Refused {
                policy_digest: policy().digest(),
                reason: RefusalReason::FieldNotApproved("full_body".to_string()),
            }
        );
    }

    #[test]
    fn a_different_provider_is_refused_even_with_approved_fields() {
        let mut other = manifest(vec![field("title", 10)]);
        other.provider.model = "m-2".to_string();
        let decision = authorize(&policy(), &Consumption::default(), &other, 5);
        assert!(
            matches!(
                decision,
                PolicyDecision::Refused {
                    reason: RefusalReason::ProviderNotApproved(_),
                    ..
                }
            ),
            "{decision:?}"
        );
    }

    #[test]
    fn the_call_that_would_cross_the_line_does_not_cross_it() {
        // Checked against the total this invocation *would* produce, not the
        // total already spent. The difference is one call's worth of spend.
        let mut consumed = Consumption::default();
        consumed.record(0, 48);
        let decision = authorize(&policy(), &consumed, &manifest(vec![field("title", 1)]), 5);
        assert!(
            matches!(decision, PolicyDecision::Exhausted { .. }),
            "48 + 5 exceeds 50 and must not be allowed: {decision:?}"
        );
    }

    #[test]
    fn each_limit_exhausts_on_its_own() {
        for (consumed, spend, label) in [
            (
                Consumption {
                    calls: 2,
                    ..Consumption::default()
                },
                0,
                "calls",
            ),
            (
                Consumption {
                    disclosed_bytes: 100,
                    ..Consumption::default()
                },
                0,
                "bytes",
            ),
            (
                Consumption {
                    spend_cents: 50,
                    ..Consumption::default()
                },
                1,
                "spend",
            ),
        ] {
            let decision = authorize(
                &policy(),
                &consumed,
                &manifest(vec![field("title", 1)]),
                spend,
            );
            assert!(
                matches!(decision, PolicyDecision::Exhausted { .. }),
                "{label} should exhaust on its own: {decision:?}"
            );
        }
    }

    #[test]
    fn exhausted_is_a_degradation_and_not_a_refusal() {
        // The distinction the run depends on: exhausted means "send the rest
        // to revisar/ and carry on", refused means "this was never allowed".
        let consumed = Consumption {
            calls: 2,
            ..Consumption::default()
        };
        let decision = authorize(&policy(), &consumed, &manifest(vec![field("title", 1)]), 0);
        assert!(matches!(decision, PolicyDecision::Exhausted { .. }));
        assert!(!decision.is_allowed());
    }

    #[test]
    fn a_zero_budget_allows_nothing() {
        // `0` has to mean "not allowed", never "unlimited": a budget nobody
        // set must not be a budget without end.
        let zero = DisclosurePolicy::new(provider(), vec!["title".to_string()], Budget::default());
        let decision = authorize(
            &zero,
            &Consumption::default(),
            &manifest(vec![field("title", 1)]),
            0,
        );
        assert!(
            matches!(decision, PolicyDecision::Exhausted { .. }),
            "{decision:?}"
        );
    }

    #[test]
    fn an_unreadable_schema_is_refused() {
        let mut future = policy();
        future.schema_version = "dataforge.ai-disclosure-policy/9.0.0".to_string();
        let decision = authorize(
            &future,
            &Consumption::default(),
            &manifest(vec![field("title", 1)]),
            0,
        );
        assert!(
            matches!(
                decision,
                PolicyDecision::Refused {
                    reason: RefusalReason::UnsupportedSchema(_),
                    ..
                }
            ),
            "{decision:?}"
        );
    }

    #[test]
    fn the_digest_is_stable_across_a_round_trip() {
        let original = policy();
        let round_tripped: DisclosurePolicy =
            serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
        assert_eq!(original.digest(), round_tripped.digest());
    }

    #[test]
    fn changing_the_budget_changes_the_digest() {
        // Otherwise a policy could be widened after approval while its
        // provenance still pointed at the terms the human agreed to.
        let original = policy();
        let mut widened = original.clone();
        widened.budget.spend_cents = 5_000;
        assert_ne!(original.digest(), widened.digest());
    }

    #[test]
    fn consumption_saturates_instead_of_wrapping() {
        // An overflow that wrapped would turn a spent budget into a fresh one.
        let mut consumed = Consumption {
            calls: u32::MAX,
            disclosed_bytes: u64::MAX,
            spend_cents: u64::MAX,
        };
        consumed.record(10, 10);
        assert_eq!(consumed.calls, u32::MAX);
        assert_eq!(consumed.disclosed_bytes, u64::MAX);
        assert_eq!(consumed.spend_cents, u64::MAX);
    }
}
