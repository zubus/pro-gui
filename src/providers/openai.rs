use std::thread;
use std::time::Duration;

use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::json;

use super::http::{self, RequestError};
use crate::models::{ModelOption, RequestSpec, TurnSnapshot, WorkerEvent};

const ASTRA_MODEL: &str = "gpt-6-astra";
const ASTRA_PRO_MODEL: &str = "gpt-6-astra-pro";
const SOL_MODEL: &str = "gpt-5.6-sol";
const SOL_PRO_MODEL: &str = "gpt-5.6-sol-pro";
const GPT_55_PRO_MODEL: &str = "gpt-5.5-pro-2026-04-23";
const DEFAULT_MODEL: &str = ASTRA_PRO_MODEL;
const MAX_REASONING_EFFORT: &str = "max";
const PRO_REASONING_MODE: &str = "pro";
const REASONING_SUMMARY: &str = "detailed";
const DEFAULT_SERVICE_TIER: &str = "default";
const PRIORITY_SERVICE_TIER: &str = "priority";
const BACKGROUND: bool = true;
const STORE: bool = true;
const RESPONSES_URL: &str = "https://api.openai.com/v1/responses";
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(10);
const MAX_BACKOFF_SECONDS: u64 = 20;
pub const MAX_OUTPUT_TOKENS: i64 = 128_000;

pub const REASONING_EFFORT_OPTIONS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

const MODEL_OPTIONS: &[ModelOption] = &[
    ModelOption {
        id: ASTRA_PRO_MODEL,
        label: "GPT-6 Astra Pro",
        detail: "newest model, pro reasoning",
    },
    ModelOption {
        id: ASTRA_MODEL,
        label: "GPT-6 Astra",
        detail: "newest model, max reasoning",
    },
    ModelOption {
        id: SOL_PRO_MODEL,
        label: "GPT-5.6 Sol Pro",
        detail: "previous flagship, pro reasoning",
    },
    ModelOption {
        id: SOL_MODEL,
        label: "GPT-5.6 Sol",
        detail: "previous flagship, max reasoning",
    },
    ModelOption {
        id: GPT_55_PRO_MODEL,
        label: "GPT-5.5 Pro",
        detail: "2026-04-23 snapshot",
    },
    ModelOption {
        id: "gpt-5.4-pro",
        label: "GPT-5.4 Pro",
        detail: "older Pro model",
    },
];

pub fn model_options() -> &'static [ModelOption] {
    MODEL_OPTIONS
}

pub fn normalize_model(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        DEFAULT_MODEL.to_owned()
    } else {
        trimmed.to_owned()
    }
}

pub fn model_display_name(model: &str) -> String {
    MODEL_OPTIONS
        .iter()
        .find(|option| option.id == model)
        .map(|option| option.label.to_owned())
        .unwrap_or_else(|| model.to_owned())
}

pub fn default_reasoning_effort(_model: &str) -> &'static str {
    MAX_REASONING_EFFORT
}

fn api_model_id(model: &str) -> &str {
    match model {
        ASTRA_PRO_MODEL => ASTRA_MODEL,
        SOL_PRO_MODEL => SOL_MODEL,
        other => other,
    }
}

fn reasoning_mode(model: &str) -> Option<&'static str> {
    matches!(model, ASTRA_PRO_MODEL | SOL_PRO_MODEL).then_some(PRO_REASONING_MODE)
}

fn stored_model_id(requested: &str, served: Option<String>) -> Option<String> {
    match served {
        Some(served) if is_snapshot_of(&served, api_model_id(requested)) => {
            Some(requested.to_owned())
        }
        other => other,
    }
}

fn is_snapshot_of(served: &str, model: &str) -> bool {
    served.strip_prefix(model).is_some_and(|rest| {
        rest.is_empty()
            || rest
                .strip_prefix('-')
                .is_some_and(|date| date.starts_with(|ch: char| ch.is_ascii_digit()))
    })
}

pub fn reasoning_summary() -> &'static str {
    REASONING_SUMMARY
}

pub fn request_service_tier(use_priority_processing: bool) -> &'static str {
    if use_priority_processing {
        PRIORITY_SERVICE_TIER
    } else {
        DEFAULT_SERVICE_TIER
    }
}

pub fn use_background_mode() -> bool {
    BACKGROUND
}

pub fn store_responses() -> bool {
    STORE
}

pub fn spawn_new_turn(
    tx: std::sync::mpsc::Sender<WorkerEvent>,
    turn_id: i64,
    spec: RequestSpec,
    api_key: String,
) {
    thread::spawn(move || run_new_turn(tx, turn_id, spec, api_key));
}

pub fn spawn_resume_turn(
    tx: std::sync::mpsc::Sender<WorkerEvent>,
    turn_id: i64,
    response_id: String,
    requested_model: String,
    api_key: String,
) {
    thread::spawn(move || run_poll_only(tx, turn_id, response_id, requested_model, api_key));
}

pub fn spawn_cancel_turn(
    tx: std::sync::mpsc::Sender<WorkerEvent>,
    turn_id: i64,
    response_id: String,
    requested_model: String,
    api_key: String,
) {
    thread::spawn(move || {
        let client = http::build_client();
        http::throttle();
        let result = client
            .post(format!("{RESPONSES_URL}/{response_id}/cancel"))
            .bearer_auth(&api_key)
            .send()
            .map_err(http::classify_transport_error)
            .and_then(http::parse_json::<ApiResponse>);

        match result {
            Ok(api_response) => {
                let _ = tx.send(WorkerEvent::Snapshot(snapshot_from_response(
                    turn_id,
                    api_response,
                    &requested_model,
                )));
            }
            Err(err) => {
                let _ = tx.send(WorkerEvent::Note {
                    turn_id,
                    message: format!("Cancel failed: {}", err.message),
                });
            }
        }
    });
}

fn run_new_turn(
    tx: std::sync::mpsc::Sender<WorkerEvent>,
    turn_id: i64,
    spec: RequestSpec,
    api_key: String,
) {
    let client = http::build_client();
    let Some(api_response) = super::create_with_retry(
        &tx,
        turn_id,
        "OpenAI",
        "Submitting background request...",
        || create_response(&client, &api_key, &spec),
    ) else {
        return;
    };
    let snapshot = snapshot_from_response(turn_id, api_response, &spec.model);
    let response_id = snapshot.response_id.clone();
    let is_terminal = is_terminal_status(&snapshot.status);
    let _ = tx.send(WorkerEvent::Snapshot(snapshot));

    if !is_terminal && let Some(response_id) = response_id {
        if super::is_cancelled(turn_id) {
            spawn_cancel_turn(
                tx.clone(),
                turn_id,
                response_id.clone(),
                spec.model.clone(),
                api_key.clone(),
            );
        }
        run_poll_loop(tx, turn_id, &client, response_id, &api_key, &spec.model);
    }
}

fn run_poll_only(
    tx: std::sync::mpsc::Sender<WorkerEvent>,
    turn_id: i64,
    response_id: String,
    requested_model: String,
    api_key: String,
) {
    let client = http::build_client();
    run_poll_loop(
        tx,
        turn_id,
        &client,
        response_id,
        &api_key,
        &requested_model,
    );
}

fn run_poll_loop(
    tx: std::sync::mpsc::Sender<WorkerEvent>,
    turn_id: i64,
    client: &Client,
    response_id: String,
    api_key: &str,
    requested_model: &str,
) {
    let mut backoff = Duration::from_secs(2);
    let started = std::time::Instant::now();
    let mut last_sent: Option<TurnSnapshot> = None;

    loop {
        thread::sleep(poll_interval(started.elapsed()));

        match retrieve_response(client, api_key, &response_id) {
            Ok(api_response) => {
                backoff = Duration::from_secs(2);
                let snapshot = snapshot_from_response(turn_id, api_response, requested_model);
                let done = is_terminal_status(&snapshot.status);
                if last_sent.as_ref() != Some(&snapshot) {
                    last_sent = Some(snapshot.clone());
                    let _ = tx.send(WorkerEvent::Snapshot(snapshot));
                }
                if done {
                    break;
                }
            }
            Err(err) if err.retryable => {
                let _ = tx.send(WorkerEvent::Note {
                    turn_id,
                    message: format!(
                        "Polling failed: {}. Retrying in {}s.",
                        err.message,
                        backoff.as_secs()
                    ),
                });
                thread::sleep(backoff);
                backoff = Duration::from_secs((backoff.as_secs() * 2).min(MAX_BACKOFF_SECONDS));
            }
            Err(err) => {
                let _ = tx.send(WorkerEvent::Snapshot(TurnSnapshot {
                    turn_id,
                    response_id: Some(response_id.clone()),
                    model: None,
                    status: "failed".to_owned(),
                    thinking_text: String::new(),
                    answer_text: String::new(),
                    error_text: Some(err.message),
                    input_tokens: None,
                    output_tokens: None,
                    reasoning_tokens: None,
                }));
                break;
            }
        }
    }
}

fn poll_interval(age: Duration) -> Duration {
    (POLL_INTERVAL + Duration::from_secs(age.as_secs() / 60)).min(MAX_POLL_INTERVAL)
}

fn request_body(spec: &RequestSpec) -> serde_json::Value {
    let input = if spec.images.is_empty() {
        json!(spec.prompt)
    } else {
        let mut content = vec![json!({ "type": "input_text", "text": spec.prompt })];
        for data_url in &spec.images {
            if data_url.starts_with("data:application/pdf;base64,") {
                content.push(json!({
                    "type": "input_file",
                    "filename": "document.pdf",
                    "file_data": data_url
                }));
            } else {
                content.push(json!({ "type": "input_image", "image_url": data_url }));
            }
        }
        json!([{ "role": "user", "content": content }])
    };

    let mut reasoning = json!({
        "effort": spec.reasoning_effort,
        "summary": REASONING_SUMMARY
    });
    if let Some(mode) = reasoning_mode(&spec.model) {
        reasoning["mode"] = json!(mode);
    }

    let mut body = json!({
        "model": api_model_id(&spec.model),
        "input": input,
        "background": BACKGROUND,
        "store": STORE,
        "service_tier": spec.service_tier,
        "max_output_tokens": clamp_max_output_tokens(spec.max_output_tokens),
        "reasoning": reasoning
    });

    if !spec.instructions.trim().is_empty() {
        body["instructions"] = json!(spec.instructions);
    }

    if let Some(previous_response_id) = &spec.previous_response_id {
        body["previous_response_id"] = json!(previous_response_id);
    }

    body
}

fn create_response(
    client: &Client,
    api_key: &str,
    spec: &RequestSpec,
) -> Result<ApiResponse, RequestError> {
    let body = request_body(spec);
    http::throttle();
    client
        .post(RESPONSES_URL)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .map_err(http::classify_transport_error)
        .and_then(http::parse_json::<ApiResponse>)
}

fn retrieve_response(
    client: &Client,
    api_key: &str,
    response_id: &str,
) -> Result<ApiResponse, RequestError> {
    http::throttle();
    client
        .get(format!("{RESPONSES_URL}/{response_id}"))
        .bearer_auth(api_key)
        .send()
        .map_err(http::classify_transport_error)
        .and_then(http::parse_json::<ApiResponse>)
}

fn snapshot_from_response(
    turn_id: i64,
    response: ApiResponse,
    requested_model: &str,
) -> TurnSnapshot {
    let mut thinking_parts = Vec::new();
    let mut answer_parts = Vec::new();

    for item in response.output {
        match item.kind.as_str() {
            "reasoning" => {
                for summary in item.summary {
                    if !summary.text.is_empty() {
                        thinking_parts.push(summary.text);
                    }
                }
                for content in item.content {
                    if matches!(content.kind.as_deref(), Some("reasoning_text"))
                        && !content.text.is_empty()
                    {
                        thinking_parts.push(content.text);
                    }
                }
            }
            "message" => {
                for content in item.content {
                    if matches!(content.kind.as_deref(), Some("output_text"))
                        && !content.text.is_empty()
                    {
                        answer_parts.push(content.text);
                    }
                }
            }
            _ => {}
        }
    }

    let (input_tokens, output_tokens, reasoning_tokens) = response
        .usage
        .map(|usage| {
            (
                usage.input_tokens,
                usage.output_tokens,
                usage
                    .output_tokens_details
                    .and_then(|details| details.reasoning_tokens),
            )
        })
        .unwrap_or((None, None, None));

    TurnSnapshot {
        turn_id,
        response_id: Some(response.id),
        model: stored_model_id(requested_model, response.model),
        status: response.status.unwrap_or_else(|| "unknown".to_owned()),
        thinking_text: thinking_parts.join("\n\n"),
        answer_text: answer_parts.join(""),
        error_text: response.error.and_then(|error| error.message),
        input_tokens,
        output_tokens,
        reasoning_tokens,
    }
}

fn is_terminal_status(status: &str) -> bool {
    !matches!(status, "queued" | "in_progress")
}

fn clamp_max_output_tokens(value: i64) -> i64 {
    value.clamp(1_024, MAX_OUTPUT_TOKENS)
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    id: String,
    model: Option<String>,
    status: Option<String>,
    #[serde(default)]
    output: Vec<ApiOutputItem>,
    usage: Option<ApiUsage>,
    error: Option<ApiError>,
}

#[derive(Debug, Deserialize)]
struct ApiUsage {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    output_tokens_details: Option<ApiOutputTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct ApiOutputTokensDetails {
    reasoning_tokens: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ApiOutputItem {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    content: Vec<ApiContentItem>,
    #[serde(default)]
    summary: Vec<ApiSummaryItem>,
}

#[derive(Debug, Deserialize)]
struct ApiContentItem {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct ApiSummaryItem {
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    message: Option<String>,
}

#[cfg(test)]
mod tests;
