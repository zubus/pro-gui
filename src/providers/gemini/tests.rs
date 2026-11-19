use std::io::Cursor;
use std::sync::mpsc;

use super::*;

fn spec() -> RequestSpec {
    RequestSpec {
        model: "gemini-4-carbon".to_owned(),
        prompt: "Prove it.".to_owned(),
        images: Vec::new(),
        instructions: String::new(),
        max_output_tokens: 262_144,
        reasoning_effort: "high".to_owned(),
        service_tier: "standard".to_owned(),
        history: Vec::new(),
        previous_response_id: None,
    }
}

const DOCUMENTED_STREAM: &str = r#"event: interaction.created
data: {"interaction":{"id":"v1_xxx","status":"in_progress","object":"interaction","model":"gemini-3-flash-preview"},"event_type":"interaction.created"}

event: step.start
data: {"index":0,"step":{"signature":"","summary":[{"text":"**Evaluating the clues**\n\nI'm considering...","type":"text"}],"type":"thought"},"event_type":"step.start"}

event: step.delta
data: {"index":0,"delta":{"signature":"EpoGCpcGAXLI2nx/...","type":"thought_signature"},"event_type":"step.delta"}

event: step.stop
data: {"index":0,"event_type":"step.stop"}

event: step.start
data: {"index":1,"step":{"content":[{"text":"Based on the clues provided, here","type":"text"}],"type":"model_output"},"event_type":"step.start"}

event: step.delta
data: {"index":1,"delta":{"text":" is the answer to your question...","type":"text"},"event_type":"step.delta"}

event: step.stop
data: {"index":1,"event_type":"step.stop"}

event: interaction.completed
data: {"interaction":{"id":"v1_xxx","status":"completed","usage":{"total_tokens":530,"total_input_tokens":62,"total_output_tokens":171,"total_thought_tokens":297}},"event_type":"interaction.completed"}

event: done
data: [DONE]
"#;

#[test]
fn a_long_run_is_a_stored_background_interaction() {
    let body = request_body(&spec());
    assert_eq!(body["model"], "gemini-4-carbon");
    assert_eq!(body["input"], "Prove it.");
    assert_eq!(body["background"], true);
    assert_eq!(body["store"], true);
    assert_eq!(body["generation_config"]["max_output_tokens"], 262_144);
    assert_eq!(body["generation_config"]["thinking_level"], "high");
    assert_eq!(body["generation_config"]["thinking_summaries"], "auto");
    for absent in [
        "system_instruction",
        "previous_interaction_id",
        "service_tier",
    ] {
        assert!(body.get(absent).is_none(), "{absent} should be omitted");
    }
}

#[test]
fn the_request_carries_chain_instructions_images_pdfs_and_tier() {
    let body = request_body(&RequestSpec {
        images: vec![
            "data:image/png;base64,iVBORw0K".to_owned(),
            "data:application/pdf;base64,JVBERi0xLjQ=".to_owned(),
        ],
        instructions: "Be terse.".to_owned(),
        service_tier: "priority".to_owned(),
        previous_response_id: Some("v1_previous".to_owned()),
        ..spec()
    });
    assert_eq!(
        body["input"],
        json!([
            { "type": "text", "text": "Prove it." },
            { "type": "image", "mime_type": "image/png", "data": "iVBORw0K" },
            { "type": "document", "mime_type": "application/pdf", "data": "JVBERi0xLjQ=" }
        ])
    );
    assert_eq!(body["system_instruction"], "Be terse.");
    assert_eq!(body["previous_interaction_id"], "v1_previous");
    assert_eq!(body["service_tier"], "priority");
}

#[test]
fn carbon_gets_long_decode_too() {
    assert_eq!(max_output_tokens("gemini-4-carbon"), 1_048_576);
    assert_eq!(max_output_tokens(ARGON_MODEL), 1_048_576);
    assert_eq!(max_output_tokens("gemini-3.8-flash"), 65_536);
    let body = request_body(&RequestSpec {
        max_output_tokens: 5_000_000,
        ..spec()
    });
    assert_eq!(body["generation_config"]["max_output_tokens"], 1_048_576);
    let body = request_body(&RequestSpec {
        model: "gemini-3.8-flash".to_owned(),
        max_output_tokens: 262_144,
        ..spec()
    });
    assert_eq!(body["generation_config"]["max_output_tokens"], 65_536);
}

#[test]
fn neon_comes_first_with_long_decode_and_priority() {
    assert_eq!(MODEL_OPTIONS[0].id, NEON_MODEL);
    assert_eq!(MODEL_OPTIONS[0].label, "Gemini 4 Neon");
    assert_eq!(max_output_tokens(NEON_MODEL), LONG_DECODE_MAX_OUTPUT_TOKENS);

    let body = request_body(&RequestSpec {
        model: NEON_MODEL.to_owned(),
        max_output_tokens: 5_000_000,
        reasoning_effort: "xhigh".to_owned(),
        service_tier: "priority".to_owned(),
        ..spec()
    });
    assert_eq!(body["model"], "gemini-4-neon");
    assert_eq!(body["generation_config"]["max_output_tokens"], 1_048_576);
    assert_eq!(body["generation_config"]["thinking_level"], "xhigh");
    assert_eq!(body["service_tier"], "priority");
}

#[test]
fn only_neon_thinks_at_xhigh_and_each_model_starts_at_its_top_level() {
    assert_eq!(
        thinking_levels(NEON_MODEL),
        &["low", "medium", "high", "xhigh"]
    );
    assert_eq!(default_thinking_level(NEON_MODEL), "xhigh");
    for model in [CARBON_MODEL, ARGON_MODEL, "gemini-3.8-flash"] {
        assert_eq!(
            thinking_levels(model),
            &["low", "medium", "high"],
            "{model}"
        );
        assert_eq!(default_thinking_level(model), "high", "{model}");
    }
}

#[test]
fn a_fetched_answer_joins_every_hop_and_skips_the_prompt() {
    let fetched = Fetched::from_value(&json!({
        "id": "v1_abc",
        "status": "completed",
        "steps": [
            { "type": "user_input", "content": [{ "type": "text", "text": "Prove it." }] },
            { "type": "thought", "signature": "x", "summary": [{ "type": "text", "text": "**Plan**" }] },
            { "type": "model_output", "content": [{ "type": "text", "text": "First hop, " }] },
            { "type": "thought", "signature": "y" },
            { "type": "model_output", "content": [{ "type": "text", "text": "second hop." }] }
        ],
        "usage": { "total_input_tokens": 70_000, "total_output_tokens": 90_000, "total_thought_tokens": 40_000 }
    }));
    assert_eq!(fetched.answer, "First hop, second hop.");
    assert_eq!(fetched.thinking, "**Plan**");

    let snapshot = fetched.into_snapshot(7);
    assert_eq!(snapshot.response_id.as_deref(), Some("v1_abc"));
    assert_eq!(snapshot.input_tokens, Some(70_000));
    assert_eq!(snapshot.output_tokens, Some(130_000));
    assert_eq!(snapshot.reasoning_tokens, Some(40_000));
    assert_eq!(snapshot.error_text, None);
}

#[test]
fn the_documented_stream_yields_thoughts_answer_and_usage() {
    let (tx, rx) = mpsc::channel();
    let mut live = LiveRun::new(1, "v1_xxx", "in_progress");
    let end = read_events(Cursor::new(DOCUMENTED_STREAM), &tx, &mut live);

    assert!(matches!(end, StreamEnd::Finished));
    assert_eq!(
        live.thinking_text(),
        "**Evaluating the clues**\n\nI'm considering..."
    );
    assert_eq!(
        live.answer_text(),
        "Based on the clues provided, here is the answer to your question..."
    );
    assert_eq!(live.status, "completed");
    assert_eq!(live.usage.billed_output(), Some(171 + 297));
    assert!(
        rx.try_iter()
            .any(|event| matches!(event, WorkerEvent::Note { .. }))
    );
}

#[test]
fn thought_summary_deltas_extend_the_thought() {
    let (tx, _rx) = mpsc::channel();
    let mut live = LiveRun::new(1, "v1_xxx", "in_progress");
    let stream = concat!(
        "data: {\"event_type\":\"step.start\",\"index\":0,\"step\":{\"type\":\"thought\"}}\n\n",
        "data: {\"event_type\":\"step.delta\",\"index\":0,\"delta\":{\"type\":\"thought_summary\",\"content\":{\"type\":\"text\",\"text\":\"**Weighing**\"}}}\n\n",
        "data: {\"event_type\":\"step.delta\",\"index\":0,\"delta\":{\"type\":\"thought_summary\",\"content\":{\"type\":\"text\",\"text\":\" options\"}}}\n\n",
    );
    let end = read_events(Cursor::new(stream), &tx, &mut live);

    assert!(matches!(end, StreamEnd::Closed));
    assert_eq!(live.thinking_text(), "**Weighing** options");
    assert_eq!(live.activity(), "Thinking...");

    live.apply(&json!({
        "event_type": "step.delta", "index": 0,
        "delta": { "type": "thought_summary", "content": { "type": "text", "text": "**Deciding**" } }
    }));
    assert_eq!(live.thinking_text(), "**Weighing** options\n\n**Deciding**");
}

#[test]
fn sse_fields_fill_in_what_the_payload_leaves_out() {
    let (tx, _rx) = mpsc::channel();
    let mut live = LiveRun::new(1, "v1_xxx", "in_progress");
    let stream = concat!(
        "event: step.start\nid: evt_7\n",
        "data: {\"index\":0,\"step\":{\"type\":\"user_input\",\"content\":[{\"type\":\"text\",\"text\":\"Prove it.\"}]}}\n\n",
        ": keep-alive\n\n",
        "event: step.delta\n",
        "data: {\"index\":0,\"delta\":{\"type\":\"text\",\"text\":\" (echo)\"}}\n\n",
    );
    read_events(Cursor::new(stream), &tx, &mut live);

    assert_eq!(live.events, 2);
    assert_eq!(live.last_event_id.as_deref(), Some("evt_7"));
    assert_eq!(live.answer_text(), "");
    assert_eq!(
        live.activity(),
        "Started, waiting for the first thoughts..."
    );
}

#[test]
fn a_reconnect_resumes_only_when_no_text_event_lacked_an_id() {
    let mut live = LiveRun::new(1, "v1_xxx", "in_progress");
    live.apply(&json!({
        "event_type": "step.delta", "event_id": "evt_1", "index": 0,
        "delta": { "type": "text", "text": "Hello" }
    }));
    assert!(live.resumable);
    assert_eq!(live.last_event_id.as_deref(), Some("evt_1"));

    live.apply(&json!({
        "event_type": "step.delta", "index": 0,
        "delta": { "type": "text", "text": " world" }
    }));
    assert!(!live.resumable);

    live.apply(&json!({ "event_type": "step.delta", "event_id": "evt_3", "index": 0, "delta": { "type": "text", "text": "!" } }));
    live.apply(&json!({ "event_type": "interaction.status_update", "status": "in_progress" }));
    assert!(live.resumable);

    live.restart();
    assert_eq!(live.answer_text(), "");
    assert_eq!(live.last_event_id, None);
}

#[test]
fn a_replay_never_shrinks_the_text_on_screen() {
    let (tx, rx) = mpsc::channel();
    let mut live = LiveRun::new(1, "v1_xxx", "in_progress");
    live.shown = (0, 20);
    live.last_snapshot = None;
    live.apply(&json!({
        "event_type": "step.start", "index": 1,
        "step": { "type": "model_output", "content": [{ "type": "text", "text": "short" }] }
    }));
    live.report(&tx);
    assert!(
        !rx.try_iter()
            .any(|event| matches!(event, WorkerEvent::Snapshot(_)))
    );

    live.apply(&json!({
        "event_type": "step.delta", "index": 1,
        "delta": { "type": "text", "text": " and now longer than before" }
    }));
    live.report(&tx);
    assert!(
        rx.try_iter()
            .any(|event| matches!(event, WorkerEvent::Snapshot(_)))
    );
}

#[test]
fn a_completed_get_wins_and_a_cut_short_run_keeps_the_longer_text() {
    let mut live = LiveRun::new(1, "v1_xxx", "in_progress");
    live.apply(&json!({
        "event_type": "step.start", "index": 1,
        "step": { "type": "model_output", "content": [{ "type": "text", "text": "streamed text, longer" }] }
    }));

    let fetched = |status: &str| {
        Fetched::from_value(&json!({
            "id": "v1_xxx", "status": status,
            "steps": [{ "type": "model_output", "content": [{ "type": "text", "text": "final" }] }]
        }))
    };
    assert_eq!(live.settle(fetched("completed")).answer_text, "final");

    let cancelled = live.settle(fetched("cancelled"));
    assert_eq!(cancelled.status, "cancelled");
    assert_eq!(cancelled.answer_text, "streamed text, longer");
}

#[test]
fn a_restarted_stream_that_then_fails_still_keeps_the_partial_answer() {
    let mut live = LiveRun::new(1, "v1_xxx", "in_progress");
    live.apply(&json!({
        "event_type": "step.start", "index": 0,
        "step": { "type": "thought", "summary": [{ "type": "text", "text": "**Hop 1 plan**" }] }
    }));
    live.apply(&json!({
        "event_type": "step.start", "index": 1,
        "step": { "type": "model_output", "content": [{ "type": "text", "text": "Partial answer from hop 1" }] }
    }));

    live.restart();

    assert_eq!(
        live.failed("stream died".to_owned()).answer_text,
        "Partial answer from hop 1"
    );
    let settled = live.settle(Fetched::from_value(&json!({
        "id": "v1_xxx",
        "status": "failed",
        "errors": [{ "message": "context limit exceeded" }]
    })));
    assert_eq!(settled.status, "failed");
    assert_eq!(settled.thinking_text, "**Hop 1 plan**");
    assert_eq!(settled.answer_text, "Partial answer from hop 1");
}

#[test]
fn thought_deltas_stream_snapshots_and_live_token_counts_immediately() {
    let (tx, rx) = mpsc::channel();
    let mut live = LiveRun::new(1, "v1_xxx", "in_progress");

    live.apply(&json!({
        "event_type": "step.start",
        "index": 0,
        "step": { "type": "thought", "summary": [{ "type": "text", "text": "**Analyzing architecture**\n\nChecking modules and token budgets." }] }
    }));
    live.report(&tx);

    let first = rx
        .try_iter()
        .find_map(|event| match event {
            WorkerEvent::Snapshot(snapshot) => Some(snapshot),
            _ => None,
        })
        .expect("first thought should emit a snapshot immediately");
    assert!(first.thinking_text.contains("**Analyzing architecture**"));
    assert!(
        first.reasoning_tokens.unwrap_or(0) > 0,
        "streaming thoughts should report live token count before step.stop"
    );

    live.apply(&json!({
        "event_type": "step.stop",
        "index": 0,
        "usage": { "total_input_tokens": 1_000, "total_output_tokens": 0, "total_thought_tokens": 4_500 }
    }));
    live.report(&tx);

    let after_stop = rx
        .try_iter()
        .find_map(|event| match event {
            WorkerEvent::Snapshot(snapshot) => Some(snapshot),
            _ => None,
        })
        .expect("step.stop usage update should emit a snapshot");
    assert_eq!(after_stop.reasoning_tokens, Some(4_500));
}

#[test]
fn an_incomplete_run_says_what_to_change() {
    let snapshot =
        Fetched::from_value(&json!({ "id": "v1_xxx", "status": "incomplete" })).into_snapshot(1);
    assert!(snapshot.error_text.unwrap().contains("max_output_tokens"));

    let failed = Fetched::from_value(&json!({
        "id": "v1_xxx", "status": "failed",
        "errors": [{ "code": "internal", "message": "Hop 3 failed." }]
    }))
    .into_snapshot(1);
    assert_eq!(failed.error_text.as_deref(), Some("Hop 3 failed."));
}

#[test]
fn a_run_the_stream_saw_end_is_not_reported_running_again() {
    let (tx, _rx) = mpsc::channel();
    let mut live = LiveRun::new(1, "v1_xxx", "in_progress");
    live.apply(&json!({
        "event_type": "interaction.completed",
        "interaction": { "id": "v1_xxx", "status": "cancelled" }
    }));
    live.report_fetched(
        &tx,
        Fetched::from_value(&json!({ "id": "v1_xxx", "status": "in_progress" })),
    );
    assert_eq!(live.status, "cancelled");
    assert_eq!(
        live.activity(),
        "Run cancelled, fetching its final state..."
    );
}

#[test]
fn stream_errors_switch_to_polling() {
    let (tx, _rx) = mpsc::channel();
    let mut live = LiveRun::new(1, "v1_xxx", "in_progress");
    let stream = "data: {\"event_type\":\"error\",\"error\":{\"code\":\"not_found\",\"message\":\"Result not found.\"}}\n\n";
    match read_events(Cursor::new(stream), &tx, &mut live) {
        StreamEnd::Unavailable(message) => assert_eq!(message, "Result not found."),
        _ => panic!("an error event should end streaming"),
    }
}

#[test]
fn gemini_and_gemma_ids_are_routed_here() {
    assert!(is_gemini_model(NEON_MODEL));
    assert!(is_gemini_model(CARBON_MODEL));
    assert!(is_gemini_model(ARGON_MODEL));
    assert!(is_gemini_model("gemma-4-31b-it"));
    assert!(!is_gemini_model("gpt-6-astra-pro"));
    assert_eq!(
        split_data_url("data:image/jpeg;base64,/9j/"),
        Some(("image/jpeg", "/9j/"))
    );
    assert_eq!(split_data_url("https://example.com/cat.png"), None);
}
