use std::io::{BufRead, BufReader};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use reqwest::blocking::Response;
use serde_json::{Value, json};

use super::http::{self, RequestError};
use super::{NOTE_SPACING, ended_snapshot, send_note, snapshot_spacing, split_data_url};
use crate::models::{ModelOption, RequestSpec, TurnSnapshot, WorkerEvent, format_compact_count};

const FABLE_MODEL: &str = "claude-fable-5-2";
pub const EFFORT_OPTIONS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
pub const DEFAULT_EFFORT: &str = "max";
const THINKING_MODE: &str = "adaptive";
pub const THINKING_SUMMARY: &str = "adaptive";
pub const MAX_OUTPUT_TOKENS: i64 = 128_000;
const OTHER_MODEL_MAX_OUTPUT_TOKENS: i64 = 64_000;
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2026-10-21";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

const FABLE_LABEL: &str = "Claude Fable 5.2";

const MODEL_OPTIONS: &[ModelOption] = &[ModelOption {
    id: FABLE_MODEL,
    label: FABLE_LABEL,
    detail: "Anthropic, most capable, 1M context, answers up to 128K tokens",
}];

pub fn model_options() -> &'static [ModelOption] {
    MODEL_OPTIONS
}

pub fn is_anthropic_model(model: &str) -> bool {
    model.starts_with("claude-") || model.starts_with("anthropic.")
}

pub fn max_output_tokens(model: &str) -> i64 {
    if model.starts_with(FABLE_MODEL) {
        MAX_OUTPUT_TOKENS
    } else {
        OTHER_MODEL_MAX_OUTPUT_TOKENS
    }
}

pub fn supports_priority_tier(model: &str) -> bool {
    !model.starts_with(FABLE_MODEL)
}

pub fn request_service_tier(model: &str, use_priority_processing: bool) -> &'static str {
    if use_priority_processing && supports_priority_tier(model) {
        "auto"
    } else {
        "standard_only"
    }
}

fn supports_server_fallback(model: &str) -> bool {
    model.starts_with(FABLE_MODEL)
}

pub fn spawn_new_turn(tx: Sender<WorkerEvent>, turn_id: i64, spec: RequestSpec, api_key: String) {
    thread::spawn(move || run_new_turn(tx, turn_id, spec, api_key));
}

pub fn spawn_resume_turn(
    tx: Sender<WorkerEvent>,
    turn_id: i64,
    _response_id: String,
    _api_key: String,
) {
    thread::spawn(move || {
        let _ = tx.send(WorkerEvent::Snapshot(ended_snapshot(
            turn_id,
            "failed",
            "Anthropic does not keep runs on its servers, so this run could not be resumed after the app restarted. Use Send again to retry.".to_owned(),
        )));
    });
}

pub fn spawn_cancel_turn(
    tx: Sender<WorkerEvent>,
    turn_id: i64,
    _response_id: String,
    _api_key: String,
) {
    send_note(
        &tx,
        turn_id,
        "Cancel requested; the run stops at its next update.".to_owned(),
    );
}

fn run_new_turn(tx: Sender<WorkerEvent>, turn_id: i64, spec: RequestSpec, api_key: String) {
    let Some(response) = super::create_with_retry(
        &tx,
        turn_id,
        "Anthropic",
        "Sending the request to Anthropic...",
        || create_message(&api_key, &spec),
    ) else {
        return;
    };
    read_stream(&tx, turn_id, response);
}

fn create_message(api_key: &str, spec: &RequestSpec) -> Result<Response, RequestError> {
    http::throttle();
    let response = http::stream_client()
        .post(MESSAGES_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", API_VERSION)
        .header("anthropic-beta", FALLBACK_BETA)
        .json(&request_body(spec))
        .send()
        .map_err(http::classify_transport_error)?;

    let status = response.status();
    if status.is_success() {
        Ok(response)
    } else {
        Err(http::status_error(
            status,
            &response.text().unwrap_or_default(),
        ))
    }
}

fn request_body(spec: &RequestSpec) -> Value {
    let mut messages: Vec<Value> = spec
        .history
        .iter()
        .filter(|message| !message.text.is_empty())
        .map(|message| {
            json!({
                "role": message.role.as_str(),
                "content": message.text
            })
        })
        .collect();

    let content = if spec.images.is_empty() {
        json!(spec.prompt)
    } else {
        let mut blocks = vec![json!({ "type": "text", "text": spec.prompt })];
        for (mime_type, data) in spec.images.iter().filter_map(|url| split_data_url(url)) {
            if mime_type == "application/pdf" {
                blocks.push(json!({
                    "type": "document",
                    "source": {
                        "type": "base64",
                        "media_type": "application/pdf",
                        "data": data
                    }
                }));
            } else {
                blocks.push(json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": mime_type,
                        "data": data
                    }
                }));
            }
        }
        json!(blocks)
    };
    messages.push(json!({ "role": "user", "content": content }));

    let mut body = json!({
        "model": spec.model,
        "max_tokens": spec.max_output_tokens.clamp(1_024, max_output_tokens(&spec.model)),
        "messages": messages,
        "stream": true,
        "thinking": {
            "type": THINKING_MODE,
            "display": "summarized"
        },
        "output_config": {
            "effort": spec.reasoning_effort
        },
        "service_tier": spec.service_tier
    });

    if !spec.instructions.trim().is_empty() {
        body["system"] = json!(spec.instructions);
    }
    if supports_server_fallback(&spec.model) {
        body["fallbacks"] = json!("default");
    }
    body
}

enum StreamEnd {
    Finished,
    Cancelled,
    Closed,
    StreamError(String),
}

fn read_stream(tx: &Sender<WorkerEvent>, turn_id: i64, response: Response) {
    let mut live = LiveRun::new(turn_id);
    match read_events(BufReader::new(response), tx, &mut live) {
        StreamEnd::Finished => {
            let _ = tx.send(WorkerEvent::Snapshot(live.settle()));
        }
        StreamEnd::Cancelled => {
            let _ = tx.send(WorkerEvent::Snapshot(live.cancelled()));
        }
        StreamEnd::Closed => {
            let _ = tx.send(WorkerEvent::Snapshot(live.failed(
                "The connection ended before Anthropic finished the response. The partial answer was kept; use Send again to retry.".to_owned(),
            )));
        }
        StreamEnd::StreamError(message) => {
            let _ = tx.send(WorkerEvent::Snapshot(live.failed(message)));
        }
    }
}

fn read_events(
    mut reader: impl BufRead,
    tx: &Sender<WorkerEvent>,
    live: &mut LiveRun,
) -> StreamEnd {
    let mut raw = Vec::new();
    let mut data = String::new();

    loop {
        if super::is_cancelled(live.turn_id) {
            return StreamEnd::Cancelled;
        }

        raw.clear();
        let at_end = match reader.read_until(b'\n', &mut raw) {
            Ok(read) => read == 0,
            Err(err) => {
                return StreamEnd::StreamError(format!("Reading the stream failed: {err}"));
            }
        };
        let line = String::from_utf8_lossy(&raw);
        let line = line.trim_end_matches(['\n', '\r']);
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);

        if field == "data" {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value);
        }

        if (line.is_empty() || at_end) && !data.is_empty() {
            let payload = std::mem::take(&mut data);
            if payload.trim() == "[DONE]" {
                return StreamEnd::Finished;
            }
            if let Ok(event) = serde_json::from_str::<Value>(&payload) {
                live.events += 1;
                match live.apply(&event) {
                    Applied::Continue => live.report(tx),
                    Applied::Finished => return StreamEnd::Finished,
                    Applied::Error(message) => return StreamEnd::StreamError(message),
                }
            }
        }

        if at_end {
            return StreamEnd::Closed;
        }
    }
}

enum Applied {
    Continue,
    Finished,
    Error(String),
}

struct LiveRun {
    turn_id: i64,
    id: Option<String>,
    served_model: Option<String>,
    status: String,
    thinking: String,
    answer: String,
    usage: Usage,
    stop_reason: Option<String>,
    stop_details: Option<Value>,
    fallback_note: Option<String>,
    events: u64,
    shown: (usize, usize),
    shown_usage: Usage,
    shown_status: String,
    last_snapshot: Instant,
    note: String,
    note_at: Option<Instant>,
}

impl LiveRun {
    fn new(turn_id: i64) -> Self {
        Self {
            turn_id,
            id: None,
            served_model: None,
            status: "queued".to_owned(),
            thinking: String::new(),
            answer: String::new(),
            usage: Usage::default(),
            stop_reason: None,
            stop_details: None,
            fallback_note: None,
            events: 0,
            shown: (0, 0),
            shown_usage: Usage::default(),
            shown_status: "queued".to_owned(),
            last_snapshot: Instant::now() - Duration::from_secs(60),
            note: String::new(),
            note_at: None,
        }
    }

    fn apply(&mut self, event: &Value) -> Applied {
        match event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "message_start" => {
                if let Some(message) = event.get("message") {
                    self.id = message
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    self.served_model = message
                        .get("model")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    self.usage.merge(message.get("usage"));
                }
                self.status = "in_progress".to_owned();
                Applied::Continue
            }
            "content_block_start" => {
                let block = event.get("content_block");
                if block
                    .and_then(|block| block.get("type"))
                    .and_then(Value::as_str)
                    == Some("fallback")
                {
                    self.thinking.clear();
                    self.answer.clear();
                    self.shown = (0, 0);
                    if let Some(model) = block
                        .and_then(|block| block.get("to"))
                        .and_then(|to| to.get("model"))
                        .and_then(Value::as_str)
                    {
                        self.served_model = Some(model.to_owned());
                    }
                    self.fallback_note = Some(format!(
                        "{} declined, so {} is answering. ",
                        FABLE_LABEL,
                        self.served_model.as_deref().unwrap_or("another model")
                    ));
                }
                Applied::Continue
            }
            "content_block_delta" => {
                if let Some(delta) = event.get("delta") {
                    match delta.get("type").and_then(Value::as_str) {
                        Some("thinking_delta") => {
                            if let Some(text) = delta.get("thinking").and_then(Value::as_str) {
                                self.thinking.push_str(text);
                            }
                        }
                        Some("text_delta") => {
                            if let Some(text) = delta.get("text").and_then(Value::as_str) {
                                self.answer.push_str(text);
                            }
                        }
                        _ => {}
                    }
                }
                Applied::Continue
            }
            "message_delta" => {
                if let Some(delta) = event.get("delta") {
                    if let Some(reason) = delta.get("stop_reason").and_then(Value::as_str) {
                        self.stop_reason = Some(reason.to_owned());
                    }
                    if let Some(details) = delta.get("stop_details") {
                        self.stop_details = Some(details.clone());
                    }
                }
                self.usage.merge(event.get("usage"));
                Applied::Continue
            }
            "message_stop" => Applied::Finished,
            "error" => Applied::Error(
                event
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("the stream reported an error")
                    .to_owned(),
            ),
            _ => Applied::Continue,
        }
    }

    fn report(&mut self, tx: &Sender<WorkerEvent>) {
        let lens = (self.thinking.len(), self.answer.len());
        if self.should_show(lens) {
            let snapshot = self.snapshot();
            self.show(tx, snapshot, lens);
        }
        self.send_activity(tx);
    }

    fn should_show(&self, lens: (usize, usize)) -> bool {
        let caught_up = lens.0 >= self.shown.0 && lens.1 >= self.shown.1;
        let spaced = self.last_snapshot.elapsed() >= snapshot_spacing(lens.0 + lens.1);
        caught_up
            && ((lens != self.shown && spaced)
                || self.status != self.shown_status
                || self.usage != self.shown_usage)
    }

    fn show(&mut self, tx: &Sender<WorkerEvent>, snapshot: TurnSnapshot, lens: (usize, usize)) {
        self.shown = lens;
        self.shown_usage = self.usage;
        self.shown_status = snapshot.status.clone();
        self.last_snapshot = Instant::now();
        let _ = tx.send(WorkerEvent::Snapshot(snapshot));
        self.note.clear();
    }

    fn send_activity(&mut self, tx: &Sender<WorkerEvent>) {
        let activity = self.activity();
        let spaced = self.note_at.is_none_or(|at| at.elapsed() >= NOTE_SPACING);
        if activity != self.note && (self.note.is_empty() || spaced) {
            self.notify(tx, activity);
        }
    }

    fn notify(&mut self, tx: &Sender<WorkerEvent>, message: String) {
        send_note(tx, self.turn_id, message.clone());
        self.note = message;
        self.note_at = Some(Instant::now());
    }

    fn activity(&self) -> String {
        let doing = if !self.answer.is_empty() {
            "Writing the answer"
        } else if !self.thinking.is_empty() {
            "Thinking"
        } else {
            "Waiting for the first token"
        };
        format!(
            "{}{doing}...{}",
            self.fallback_note.as_deref().unwrap_or_default(),
            self.usage.describe()
        )
    }

    fn snapshot(&self) -> TurnSnapshot {
        TurnSnapshot {
            turn_id: self.turn_id,
            response_id: self.id.clone(),
            model: self.served_model.clone(),
            status: "in_progress".to_owned(),
            thinking_text: self.thinking.clone(),
            answer_text: self.answer.clone(),
            error_text: None,
            input_tokens: self.usage.input,
            output_tokens: self.usage.output,
            reasoning_tokens: self.usage.thinking,
        }
    }

    fn settle(&self) -> TurnSnapshot {
        let status = match self.stop_reason.as_deref() {
            Some("end_turn" | "stop_sequence") | None => "completed",
            Some("max_tokens" | "model_context_window_exceeded" | "pause_turn") => "incomplete",
            Some("tool_use") => "requires_action",
            Some("refusal") => "failed",
            Some(_) => "completed",
        };

        TurnSnapshot {
            turn_id: self.turn_id,
            response_id: self.id.clone(),
            model: self.served_model.clone(),
            status: status.to_owned(),
            thinking_text: self.thinking.clone(),
            answer_text: self.answer.clone(),
            error_text: self.final_error_text(status),
            input_tokens: self.usage.input,
            output_tokens: self.usage.output,
            reasoning_tokens: self.usage.thinking,
        }
    }

    fn cancelled(&self) -> TurnSnapshot {
        TurnSnapshot {
            status: "cancelled".to_owned(),
            ..self.snapshot()
        }
    }

    fn failed(&self, message: String) -> TurnSnapshot {
        TurnSnapshot {
            status: "failed".to_owned(),
            error_text: Some(message),
            ..self.snapshot()
        }
    }

    fn final_error_text(&self, status: &str) -> Option<String> {
        if status == "completed" {
            return None;
        }

        let mut lines = Vec::new();
        if let Some(explanation) = self
            .stop_details
            .as_ref()
            .and_then(|details| details.get("explanation"))
            .and_then(Value::as_str)
            && !explanation.is_empty()
        {
            lines.push(explanation.to_owned());
        }
        match self.stop_reason.as_deref() {
            Some("max_tokens") => lines.push(
                "The run used up max_output_tokens (thinking and answer share it). Raise max tokens, or lower the effort, and send the prompt again."
                    .to_owned(),
            ),
            Some("model_context_window_exceeded") => lines.push(
                "The conversation no longer fits Claude's 1M-token context window. Start a new chat or shorten the attached context."
                    .to_owned(),
            ),
            Some("pause_turn") => lines.push(
                "Anthropic paused the turn before it finished. Use Send again to continue."
                    .to_owned(),
            ),
            Some("tool_use") => {
                lines.push("The model asked for a tool call, which this app does not run.".to_owned())
            }
            Some("refusal") if lines.is_empty() => {
                lines.push("Anthropic declined to answer this request.".to_owned())
            }
            _ => {}
        }
        if lines.is_empty() {
            lines.push("Anthropic ended the run without a complete answer.".to_owned());
        }
        Some(lines.join("\n"))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Usage {
    input: Option<i64>,
    output: Option<i64>,
    thinking: Option<i64>,
}

impl Usage {
    fn merge(&mut self, value: Option<&Value>) {
        let Some(value) = value else {
            return;
        };
        let count = |key: &str| value.get(key).and_then(Value::as_i64);

        if let Some(input) = count("input_tokens") {
            let cached = count("cache_creation_input_tokens").unwrap_or(0)
                + count("cache_read_input_tokens").unwrap_or(0);
            self.input = Some(input + cached);
        }
        if let Some(output) = count("output_tokens") {
            self.output = Some(output);
        }
        if let Some(thinking) = value
            .get("output_tokens_details")
            .and_then(|details| details.get("thinking_tokens"))
            .and_then(Value::as_i64)
        {
            self.thinking = Some(thinking);
        }
    }

    fn describe(&self) -> String {
        let counts = [(self.input, "input"), (self.output, "output")]
            .into_iter()
            .filter_map(|(count, label)| {
                count
                    .filter(|count| *count > 0)
                    .map(|count| format!("{} {label}", format_compact_count(count)))
            })
            .collect::<Vec<_>>();
        if counts.is_empty() {
            String::new()
        } else {
            format!(" {} tokens", counts.join(", "))
        }
    }
}

#[cfg(test)]
mod tests;
