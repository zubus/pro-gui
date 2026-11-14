use std::io::Cursor;
use std::sync::mpsc;

use super::*;
use crate::models::{HistoryMessage, HistoryRole};

fn spec() -> RequestSpec {
    RequestSpec {
        model: FABLE_MODEL.to_owned(),
        prompt: "Prove it.".to_owned(),
        images: Vec::new(),
        instructions: String::new(),
        max_output_tokens: 128_000,
        reasoning_effort: "high".to_owned(),
        service_tier: "auto".to_owned(),
        history: Vec::new(),
        previous_response_id: None,
    }
}

const DOCUMENTED_STREAM: &str = r#"event: message_start
data: {"type": "message_start", "message": {"id": "msg_01...", "type": "message", "role": "assistant", "content": [], "model": "claude-fable-5-2", "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 25, "output_tokens": 1}}}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "I need to find the GCD.\n\n1071 = 2 × 462 + 147"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pkiMOYds..."}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: content_block_start
data: {"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "The greatest common divisor is **21**."}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 1}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence": null}, "usage": {"input_tokens": 25, "output_tokens": 49, "output_tokens_details": {"thinking_tokens": 30}}}

event: message_stop
data: {"type": "message_stop"}
"#;

#[test]
fn the_request_carries_history_effort_thinking_and_tier() {
    let body = request_body(&RequestSpec {
        history: vec![
            HistoryMessage {
                role: HistoryRole::User,
                text: "First?".to_owned(),
            },
            HistoryMessage {
                role: HistoryRole::Assistant,
                text: "One.".to_owned(),
            },
        ],
        instructions: "Be terse.".to_owned(),
        ..spec()
    });

    assert_eq!(body["model"], "claude-fable-5-2");
    assert_eq!(body["max_tokens"], 128_000);
    assert_eq!(body["stream"], true);
    assert_eq!(body["system"], "Be terse.");
    assert_eq!(body["output_config"]["effort"], "high");
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["thinking"]["display"], "summarized");
    assert_eq!(body["service_tier"], "auto");
    assert_eq!(body["fallbacks"], "default");
    assert_eq!(
        body["messages"],
        json!([
            { "role": "user", "content": "First?" },
            { "role": "assistant", "content": "One." },
            { "role": "user", "content": "Prove it." }
        ])
    );
}

#[test]
fn images_and_pdfs_become_base64_content_blocks() {
    let body = request_body(&RequestSpec {
        images: vec![
            "data:image/png;base64,iVBORw0K".to_owned(),
            "data:application/pdf;base64,JVBERi0xLjQ=".to_owned(),
        ],
        ..spec()
    });

    assert_eq!(
        body["messages"][0]["content"],
        json!([
            { "type": "text", "text": "Prove it." },
            {
                "type": "image",
                "source": { "type": "base64", "media_type": "image/png", "data": "iVBORw0K" }
            },
            {
                "type": "document",
                "source": { "type": "base64", "media_type": "application/pdf", "data": "JVBERi0xLjQ=" }
            }
        ])
    );
}

#[test]
fn only_fable_gets_the_full_output_limit() {
    assert_eq!(max_output_tokens(FABLE_MODEL), 128_000);
    assert_eq!(max_output_tokens("claude-opus-4-5"), 64_000);

    let body = request_body(&RequestSpec {
        max_output_tokens: 5_000_000,
        ..spec()
    });
    assert_eq!(body["max_tokens"], 128_000);
}

#[test]
fn priority_maps_to_auto_and_standard_to_standard_only() {
    assert_eq!(request_service_tier(FABLE_MODEL, true), "standard_only");
    assert_eq!(request_service_tier(FABLE_MODEL, false), "standard_only");
    assert_eq!(request_service_tier("claude-opus-4-8", true), "auto");
    assert_eq!(
        request_service_tier("claude-opus-4-8", false),
        "standard_only"
    );
}

#[test]
fn the_documented_stream_yields_thoughts_answer_and_usage() {
    let (tx, rx) = mpsc::channel();
    let mut live = LiveRun::new(1);
    let end = read_events(Cursor::new(DOCUMENTED_STREAM), &tx, &mut live);

    assert!(matches!(end, StreamEnd::Finished));
    assert_eq!(
        live.thinking,
        "I need to find the GCD.\n\n1071 = 2 × 462 + 147"
    );
    assert_eq!(live.answer, "The greatest common divisor is **21**.");
    assert_eq!(live.status, "in_progress");

    let snapshot = live.settle();
    assert_eq!(snapshot.status, "completed");
    assert_eq!(snapshot.response_id.as_deref(), Some("msg_01..."));
    assert_eq!(snapshot.model.as_deref(), Some(FABLE_MODEL));
    assert_eq!(snapshot.input_tokens, Some(25));
    assert_eq!(snapshot.output_tokens, Some(49));
    assert_eq!(snapshot.reasoning_tokens, Some(30));
    assert_eq!(snapshot.error_text, None);
    assert!(
        rx.try_iter()
            .any(|event| matches!(event, WorkerEvent::Note { .. }))
    );
}

#[test]
fn a_token_limit_stop_is_incomplete_and_says_what_to_change() {
    let (tx, _rx) = mpsc::channel();
    let mut live = LiveRun::new(1);
    let stream = concat!(
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude-fable-5-2\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Half an answer\"}}\n\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"},\"usage\":{\"output_tokens\":100}}\n\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    let end = read_events(Cursor::new(stream), &tx, &mut live);

    assert!(matches!(end, StreamEnd::Finished));
    let snapshot = live.settle();
    assert_eq!(snapshot.status, "incomplete");
    assert_eq!(snapshot.answer_text, "Half an answer");
    assert!(
        snapshot
            .error_text
            .as_deref()
            .is_some_and(|text| text.contains("max_output_tokens"))
    );
}

#[test]
fn stream_errors_and_early_closes_keep_the_partial_answer() {
    let (tx, _rx) = mpsc::channel();
    let mut live = LiveRun::new(1);
    let stream = concat!(
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude-fable-5-2\"}}\n\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Working\"}}\n\n",
    );
    let end = read_events(Cursor::new(stream), &tx, &mut live);

    assert!(matches!(end, StreamEnd::Closed));
    let snapshot = live.failed("cut off".to_owned());
    assert_eq!(snapshot.status, "failed");
    assert_eq!(snapshot.thinking_text, "Working");
    assert_eq!(snapshot.error_text.as_deref(), Some("cut off"));
}

#[test]
fn usage_sums_the_cached_input_counts() {
    let mut usage = Usage::default();
    usage.merge(Some(&json!({
        "input_tokens": 100,
        "cache_creation_input_tokens": 20,
        "cache_read_input_tokens": 300,
        "output_tokens": 4,
        "output_tokens_details": { "thinking_tokens": 1 }
    })));
    assert_eq!(usage.input, Some(420));
    assert_eq!(usage.output, Some(4));
    assert_eq!(usage.thinking, Some(1));
}

#[test]
fn every_effort_level_reaches_the_request() {
    for effort in EFFORT_OPTIONS {
        let body = request_body(&RequestSpec {
            reasoning_effort: (*effort).to_owned(),
            ..spec()
        });
        assert_eq!(body["output_config"]["effort"], *effort);
    }
    assert_eq!(EFFORT_OPTIONS, &["low", "medium", "high", "xhigh", "max"]);
}

#[test]
fn a_mid_stream_fallback_drops_the_partial_answer() {
    let stream = r#"event: message_start
data: {"type": "message_start", "message": {"id": "msg_02", "model": "claude-fable-5-2", "usage": {"input_tokens": 10, "output_tokens": 1}}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Partial from Fable"}}

event: content_block_start
data: {"type": "content_block_start", "index": 1, "content_block": {"type": "fallback", "from": {"model": "claude-fable-5-2"}, "to": {"model": "claude-opus-5"}}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 2, "delta": {"type": "text_delta", "text": "Full answer from Opus"}}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 20}}

event: message_stop
data: {"type": "message_stop"}
"#;
    let (tx, _rx) = mpsc::channel();
    let mut live = LiveRun::new(1);
    read_events(Cursor::new(stream), &tx, &mut live);

    assert_eq!(live.answer, "Full answer from Opus");
    let snapshot = live.settle();
    assert_eq!(snapshot.model.as_deref(), Some("claude-opus-5"));
    assert!(live.activity().starts_with("Claude Fable 5.2 declined"));
}
