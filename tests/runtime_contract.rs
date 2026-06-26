//! Baseline contract tests for runtime migration.
//!
//! These tests pin public serialization behavior that must remain stable while
//! the runtime is introduced behind flags.

use datacenter_agent::server::dto::{AgentRequest, IntentResolvedData, StreamFrame};

#[test]
fn agent_request_history_defaults_to_empty() {
    let req: AgentRequest = serde_json::from_value(serde_json::json!({
        "prompt": "hello"
    }))
    .expect("request should deserialize without history");

    assert_eq!(req.prompt, "hello");
    assert!(req.history.is_empty());
    assert_eq!(req.session_id, None);
    assert_eq!(req.option_id, None);
}

#[test]
fn agent_request_accepts_session_and_option_metadata() {
    let req: AgentRequest = serde_json::from_value(serde_json::json!({
        "prompt": "hello",
        "history": [],
        "session_id": "session-1",
        "option_id": "revenue.monthly"
    }))
    .expect("request should deserialize metadata");

    assert_eq!(req.prompt, "hello");
    assert_eq!(req.session_id.as_deref(), Some("session-1"));
    assert_eq!(req.option_id.as_deref(), Some("revenue.monthly"));
}

#[test]
fn stream_frame_serialization_stays_compatible() {
    let cases = [
        (
            StreamFrame::Token {
                data: "hi".to_string(),
            },
            serde_json::json!({"event": "token", "data": "hi"}),
        ),
        (StreamFrame::Done, serde_json::json!({"event": "done"})),
        (
            StreamFrame::Error {
                data: "boom".to_string(),
            },
            serde_json::json!({"event": "error", "data": "boom"}),
        ),
        (StreamFrame::Clear, serde_json::json!({"event": "clear"})),
    ];

    for (frame, expected) in cases {
        let actual = serde_json::to_value(frame).expect("stream frame should serialize");
        assert_eq!(actual, expected);
    }
}

#[test]
fn stream_frame_intent_resolved_serializes_for_frontend() {
    let frame = StreamFrame::IntentResolved {
        data: IntentResolvedData {
            intent: "revenue".to_string(),
            candidate_intents: vec!["revenue".to_string(), "station".to_string()],
        },
    };

    let actual = serde_json::to_value(frame).expect("intent.resolved should serialize");
    assert_eq!(
        actual,
        serde_json::json!({
            "event": "intent.resolved",
            "data": { "intent": "revenue", "candidateIntents": ["revenue", "station"] }
        })
    );
}
