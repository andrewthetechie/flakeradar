//! The acknowledge decision conversion: a request body becomes the decision it
//! describes. The write itself is pinned by the HTTP tests over the routes.

// Rust guideline compliant 2026-07-21

use flakeradar_core::dto::AcknowledgeIn;
use flakeradar_db::acknowledge::{AcknowledgeDecision, decision};

#[test]
fn an_acknowledged_body_becomes_its_acknowledge() {
    let body = AcknowledgeIn {
        acknowledged: true,
        reason: Some("owned until the fixture is fixed".to_owned()),
        days: Some(7),
    };
    assert_eq!(
        decision(body),
        AcknowledgeDecision::Acknowledge {
            reason: Some("owned until the fixture is fixed".to_owned()),
            days: 7,
        }
    );
}

#[test]
fn an_acknowledged_body_without_a_reason_acknowledges_with_none() {
    let body = AcknowledgeIn {
        acknowledged: true,
        reason: None,
        days: Some(3),
    };
    assert_eq!(
        decision(body),
        AcknowledgeDecision::Acknowledge {
            reason: None,
            days: 3,
        }
    );
}

#[test]
fn an_unacknowledged_body_clears() {
    let body = AcknowledgeIn {
        acknowledged: false,
        reason: Some("stale".to_owned()),
        days: Some(9),
    };
    assert_eq!(decision(body), AcknowledgeDecision::Clear);
}
