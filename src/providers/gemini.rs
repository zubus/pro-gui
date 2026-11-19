use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::CONTENT_LENGTH;
use serde_json::{Value, json};

use super::http::{self, RequestError};
use super::{
    NOTE_SPACING, ended_snapshot, is_terminal_status, send_note, snapshot_spacing, split_data_url,
};
use crate::models::{ModelOption, RequestSpec, TurnSnapshot, WorkerEvent, format_compact_count};

const CARBON_MODEL: &str = "gemini-4-carbon";
const ARGON_MODEL: &str = "gemini-4-argon";
pub const THINKING_LEVELS: &[&str] = &["low", "medium", "high"];
pub const DEFAULT_THINKING_LEVEL: &str = "high";
pub const THINKING_SUMMARIES: &str = "auto";
pub const LONG_DECODE_MAX_OUTPUT_TOKENS: i64 = 1_048_576;
const MAX_OUTPUT_TOKENS: i64 = 65_536;
const INTERACTIONS_URL: &str = "https://generativelanguage.googleapis.com/v1beta/interactions";
const API_REVISION: &str = "2026-05-20";
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const FETCH_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MAX_EMPTY_STREAMS: u32 = 3;
const MAX_BACKOFF_SECONDS: u64 = 30;

const MODEL_OPTIONS: &[ModelOption] = &[
    ModelOption {
        id: CARBON_MODEL,
        label: "Gemini 4 Carbon",
        detail: "Google, strongest reasoning, long answers up to 1M tokens",
    },
    ModelOption {
        id: ARGON_MODEL,
        label: "Gemini 4 Argon",
        detail: "Google, faster responses, long answers up to 1M tokens",
    },
];

pub fn model_options() -> &'static [ModelOption] {
    MODEL_OPTIONS
}

pub fn is_gemini_model(model: &str) -> bool {
    model.starts_with("gemini-") || model.starts_with("gemma-")
}

pub fn max_output_tokens(model: &str) -> i64 {
    if model.starts_with(CARBON_MODEL) || model.starts_with(ARGON_MODEL) {
        LONG_DECODE_MAX_OUTPUT_TOKENS
    } else {
        MAX_OUTPUT_TOKENS
    }
}

pub fn request_service_tier(use_priority_processing: bool) -> &'static str {
    if use_priority_processing {
        "priority"
    } else {
        "standard"
    }
}

pub fn spawn_new_turn(tx: Sender<WorkerEvent>, turn_id: i64, spec: RequestSpec, api_key: String) {
    thread::spawn(move || run_new_turn(tx, turn_id, spec, api_key));
}

pub fn spawn_resume_turn(
    tx: Sender<WorkerEvent>,
    turn_id: i64,
    interaction_id: String,
    api_key: String,
) {
    thread::spawn(move || {
        let client = http::build_client();
        follow_interaction(
            &tx,
            turn_id,
            &client,
            &interaction_id,
            &api_key,
            "in_progress",
        );
    });
}

pub fn spawn_cancel_turn(
    tx: Sender<WorkerEvent>,
    turn_id: i64,
    interaction_id: String,
    api_key: String,
) {
    thread::spawn(move || {
        let client = http::build_client();
        http::throttle();
        let result = authorized(
            client.post(format!("{INTERACTIONS_URL}/{interaction_id}/cancel")),
            &api_key,
        )
        .header(CONTENT_LENGTH, 0)
        .send()
        .map_err(http::classify_transport_error)
        .and_then(http::parse_json::<Value>);

        let message = match result {
            Ok(_) => "Cancel requested, waiting for the run to stop...".to_owned(),
            Err(err) => format!("Cancel failed: {}", err.message),
        };
        send_note(&tx, turn_id, message);
    });
}

fn run_new_turn(tx: Sender<WorkerEvent>, turn_id: i64, spec: RequestSpec, api_key: String) {
    let client = http::build_client();
    let Some(value) = super::create_with_retry(
        &tx,
        turn_id,
        "Gemini",
        "Submitting background interaction...",
        || create_interaction(&client, &api_key, &spec),
    ) else {
        return;
    };
    let created = Fetched::from_value(&value);
    if is_terminal_status(&created.status) {
        let _ = tx.send(WorkerEvent::Snapshot(created.into_snapshot(turn_id)));
    } else if created.id.is_empty() {
        let _ = tx.send(WorkerEvent::Snapshot(ended_snapshot(
            turn_id,
            "failed",
            "Gemini accepted the request but returned no interaction id to follow.".to_owned(),
        )));
    } else {
        let id = created.id.clone();
        let status = created.status.clone();
        let _ = tx.send(WorkerEvent::Snapshot(created.into_snapshot(turn_id)));
        if super::is_cancelled(turn_id) {
            spawn_cancel_turn(tx.clone(), turn_id, id.clone(), api_key.clone());
        }
        follow_interaction(&tx, turn_id, &client, &id, &api_key, &status);
    }
}

fn follow_interaction(
    tx: &Sender<WorkerEvent>,
    turn_id: i64,
    client: &Client,
    id: &str,
    api_key: &str,
    status: &str,
) {
    let mut live = LiveRun::new(turn_id, id, status);
    live.notify(tx, "Connecting to the run's live updates...".to_owned());
    let mut backoff = Duration::from_secs(2);
    let mut empty_streams = 0;

    while live.polling.is_none() {
        let events_before = live.events;
        let end = stream_events(tx, api_key, &mut live);
        let progressed = live.events > events_before;
        if progressed {
            backoff = Duration::from_secs(2);
            empty_streams = 0;
        }

        let wait = match end {
            StreamEnd::Finished => break,
            StreamEnd::Unavailable(reason) => {
                live.polling = Some(reason);
                continue;
            }
            StreamEnd::Closed if !progressed => {
                empty_streams += 1;
                if empty_streams >= MAX_EMPTY_STREAMS {
                    live.polling = Some("the stream keeps closing without events".to_owned());
                    continue;
                }
                Duration::from_secs(1)
            }
            StreamEnd::Closed => Duration::from_secs(1),
            StreamEnd::Interrupted(reason) => {
                let wait = if progressed {
                    Duration::from_secs(1)
                } else {
                    let wait = backoff;
                    backoff = next_backoff(backoff);
                    wait
                };
                live.notify(
                    tx,
                    format!(
                        "Live updates interrupted ({reason}). Reconnecting in {}s...",
                        wait.as_secs()
                    ),
                );
                wait
            }
        };
        thread::sleep(wait);
    }

    finish(tx, client, api_key, &mut live);
}

enum StreamEnd {
    Finished,
    Closed,
    Interrupted(String),
    Unavailable(String),
}

fn stream_events(tx: &Sender<WorkerEvent>, api_key: &str, live: &mut LiveRun) -> StreamEnd {
    let mut query = vec![("stream", "true".to_owned())];
    if live.resumable
        && let Some(event_id) = &live.last_event_id
    {
        query.push(("last_event_id", event_id.clone()));
    } else {
        live.restart();
    }

    http::throttle();
    let response = authorized(
        http::stream_client().get(format!("{INTERACTIONS_URL}/{}", live.id)),
        api_key,
    )
    .query(&query)
    .header("Accept", "text/event-stream")
    .send();
    let response = match response {
        Ok(response) => response,
        Err(err) => return StreamEnd::Interrupted(http::classify_transport_error(err).message),
    };

    let status = response.status();
    if !status.is_success() {
        let err = http::status_error(status, &response.text().unwrap_or_default());
        return if err.retryable {
            StreamEnd::Interrupted(err.message)
        } else {
            StreamEnd::Unavailable(err.message)
        };
    }

    read_events(BufReader::new(response), tx, live)
}

fn read_events(
    mut reader: impl BufRead,
    tx: &Sender<WorkerEvent>,
    live: &mut LiveRun,
) -> StreamEnd {
    let mut raw = Vec::new();
    let mut data = String::new();
    let mut event_type = String::new();
    let mut event_id = String::new();

    loop {
        raw.clear();
        let at_end = match reader.read_until(b'\n', &mut raw) {
            Ok(read) => read == 0,
            Err(err) => return StreamEnd::Interrupted(format!("read failed: {err}")),
        };
        let line = String::from_utf8_lossy(&raw);
        let line = line.trim_end_matches(['\n', '\r']);
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);

        match field {
            "data" => {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(value);
            }
            "event" => event_type = value.to_owned(),
            "id" => event_id = value.to_owned(),
            _ => {}
        }

        if (line.is_empty() || at_end) && !data.is_empty() {
            let payload = std::mem::take(&mut data);
            if payload.trim() == "[DONE]" {
                return StreamEnd::Finished;
            }
            if let Ok(mut event) = serde_json::from_str::<Value>(&payload) {
                if let Some(object) = event.as_object_mut() {
                    for (key, value) in [("event_type", &event_type), ("event_id", &event_id)] {
                        if !value.is_empty() && !object.contains_key(key) {
                            object.insert(key.to_owned(), json!(value));
                        }
                    }
                }
                live.events += 1;
                match live.apply(&event) {
                    Applied::Continue => live.report(tx),
                    Applied::Completed => return StreamEnd::Finished,
                    Applied::StreamError(message) => return StreamEnd::Unavailable(message),
                }
            }
        }
        if line.is_empty() {
            event_type.clear();
            event_id.clear();
        }

        if at_end {
            return StreamEnd::Closed;
        }
    }
}

fn finish(tx: &Sender<WorkerEvent>, client: &Client, api_key: &str, live: &mut LiveRun) {
    let mut backoff = Duration::from_secs(2);

    loop {
        match retrieve_interaction(client, api_key, &live.id) {
            Ok(value) => {
                backoff = Duration::from_secs(2);
                let fetched = Fetched::from_value(&value);
                if is_terminal_status(&fetched.status) {
                    let _ = tx.send(WorkerEvent::Snapshot(live.settle(fetched)));
                    return;
                }
                live.report_fetched(tx, fetched);
                thread::sleep(POLL_INTERVAL);
            }
            Err(err) if err.retryable => {
                live.notify(
                    tx,
                    format!(
                        "Checking the run failed: {}. Retrying in {}s.",
                        err.message,
                        backoff.as_secs()
                    ),
                );
                thread::sleep(backoff);
                backoff = next_backoff(backoff);
            }
            Err(err) => {
                let _ = tx.send(WorkerEvent::Snapshot(live.failed(err.message)));
                return;
            }
        }
    }
}

fn create_interaction(
    client: &Client,
    api_key: &str,
    spec: &RequestSpec,
) -> Result<Value, RequestError> {
    http::throttle();
    authorized(client.post(INTERACTIONS_URL), api_key)
        .json(&request_body(spec))
        .send()
        .map_err(http::classify_transport_error)
        .and_then(http::parse_json::<Value>)
}

fn retrieve_interaction(client: &Client, api_key: &str, id: &str) -> Result<Value, RequestError> {
    http::throttle();
    authorized(client.get(format!("{INTERACTIONS_URL}/{id}")), api_key)
        .timeout(FETCH_TIMEOUT)
        .send()
        .map_err(http::classify_transport_error)
        .and_then(http::parse_json::<Value>)
}

fn authorized(request: RequestBuilder, api_key: &str) -> RequestBuilder {
    request
        .header("x-goog-api-key", api_key)
        .header("Api-Revision", API_REVISION)
}

fn request_body(spec: &RequestSpec) -> Value {
    let input = if spec.images.is_empty() {
        json!(spec.prompt)
    } else {
        let mut content = vec![json!({ "type": "text", "text": spec.prompt })];
        for (mime_type, data) in spec.images.iter().filter_map(|url| split_data_url(url)) {
            let kind = if mime_type == "application/pdf" {
                "document"
            } else {
                "image"
            };
            content.push(json!({ "type": kind, "mime_type": mime_type, "data": data }));
        }
        json!(content)
    };

    let mut body = json!({
        "model": spec.model,
        "input": input,
        "background": true,
        "store": true,
        "generation_config": {
            "max_output_tokens": spec.max_output_tokens.clamp(1_024, max_output_tokens(&spec.model)),
            "thinking_level": spec.reasoning_effort,
            "thinking_summaries": THINKING_SUMMARIES
        }
    });

    if !spec.instructions.trim().is_empty() {
        body["system_instruction"] = json!(spec.instructions);
    }
    if let Some(previous_interaction_id) = &spec.previous_response_id {
        body["previous_interaction_id"] = json!(previous_interaction_id);
    }
    if spec.service_tier == "priority" {
        body["service_tier"] = json!("priority");
    }
    body
}

struct LiveRun {
    turn_id: i64,
    id: String,
    status: String,
    steps: BTreeMap<u64, LiveStep>,
    usage: Usage,
    last_event_id: Option<String>,
    resumable: bool,
    events: u64,
    shown: (usize, usize),
    shown_usage: Usage,
    thought_len_at_usage: usize,
    preserved: (String, String),
    shown_status: String,
    last_snapshot: Option<Instant>,
    note: String,
    note_at: Option<Instant>,
    polling: Option<String>,
}

struct LiveStep {
    kind: String,
    text: String,
}

enum Applied {
    Continue,
    Completed,
    StreamError(String),
}

impl LiveRun {
    fn new(turn_id: i64, id: &str, status: &str) -> Self {
        Self {
            turn_id,
            id: id.to_owned(),
            status: status.to_owned(),
            steps: BTreeMap::new(),
            usage: Usage::default(),
            last_event_id: None,
            resumable: false,
            events: 0,
            shown: (0, 0),
            shown_usage: Usage::default(),
            thought_len_at_usage: 0,
            preserved: (String::new(), String::new()),
            shown_status: status.to_owned(),
            last_snapshot: None,
            note: String::new(),
            note_at: None,
            polling: None,
        }
    }

    fn restart(&mut self) {
        let thinking = self.thinking_text();
        if thinking.len() > self.preserved.0.len() {
            self.preserved.0 = thinking;
        }
        let answer = self.answer_text();
        if answer.len() > self.preserved.1.len() {
            self.preserved.1 = answer;
        }
        self.steps.clear();
        self.last_event_id = None;
        self.resumable = false;
    }

    fn apply(&mut self, event: &Value) -> Applied {
        let index = event.get("index").and_then(Value::as_u64);
        let mut text_event = false;

        let applied = match event
            .get("event_type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            kind @ ("interaction.created" | "interaction.completed") => {
                if let Some(interaction) = event.get("interaction") {
                    self.absorb(interaction.get("status"), interaction.get("usage"));
                }
                if kind == "interaction.completed" {
                    Applied::Completed
                } else {
                    Applied::Continue
                }
            }
            "interaction.status_update" => {
                self.absorb(event.get("status"), event.get("usage"));
                Applied::Continue
            }
            "step.start" => {
                if let (Some(index), Some(step)) = (index, event.get("step")) {
                    let kind = step.get("type").and_then(Value::as_str).unwrap_or_default();
                    let text = match kind {
                        "thought" => summary_text(step),
                        "model_output" => content_text(step.get("content")),
                        _ => String::new(),
                    };
                    text_event = !text.is_empty();
                    self.steps.insert(
                        index,
                        LiveStep {
                            kind: kind.to_owned(),
                            text,
                        },
                    );
                    self.absorb(None, event.get("usage").or_else(|| step.get("usage")));
                }
                Applied::Continue
            }
            "step.delta" => {
                if let (Some(index), Some(delta)) = (index, event.get("delta")) {
                    let piece = match delta.get("type").and_then(Value::as_str) {
                        Some("text") => Some((
                            "model_output",
                            delta
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned(),
                        )),
                        Some("thought_summary" | "thought") => {
                            let text = delta
                                .get("content")
                                .and_then(|content| content.get("text"))
                                .and_then(Value::as_str)
                                .or_else(|| delta.get("text").and_then(Value::as_str))
                                .map(ToOwned::to_owned)
                                .unwrap_or_else(|| content_text(delta.get("content")));
                            Some(("thought", text))
                        }
                        _ => None,
                    };
                    if let Some((kind, text)) = piece
                        && !text.is_empty()
                    {
                        let step = self.steps.entry(index).or_insert_with(|| LiveStep {
                            kind: kind.to_owned(),
                            text: String::new(),
                        });
                        if step.kind == "thought"
                            && text.starts_with("**")
                            && !step.text.is_empty()
                            && !step.text.ends_with('\n')
                        {
                            step.text.push_str("\n\n");
                        }
                        step.text.push_str(&text);
                        text_event = true;
                    }
                    self.absorb(None, event.get("usage").or_else(|| delta.get("usage")));
                }
                Applied::Continue
            }
            "step.stop" => {
                self.absorb(None, event.get("usage"));
                Applied::Continue
            }
            "error" => Applied::StreamError(
                event
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("the stream reported an error")
                    .to_owned(),
            ),
            _ => Applied::Continue,
        };

        match event.get("event_id").and_then(Value::as_str) {
            Some(event_id) => {
                self.last_event_id = Some(event_id.to_owned());
                self.resumable = true;
            }
            None if text_event => self.resumable = false,
            None => {}
        }
        applied
    }

    fn absorb(&mut self, status: Option<&Value>, usage: Option<&Value>) {
        if let Some(status) = status.and_then(Value::as_str) {
            self.status = status.to_owned();
        }
        if let Some(usage) = usage
            .map(Usage::from_value)
            .filter(|usage| !usage.is_empty())
        {
            if usage.thought.is_some() {
                self.thought_len_at_usage = self.text_lens().0;
            }
            self.usage = usage;
        }
    }

    fn text_lens(&self) -> (usize, usize) {
        self.steps
            .values()
            .fold((0, 0), |(thought, output), step| match step.kind.as_str() {
                "thought" => (thought + step.text.len(), output),
                "model_output" => (thought, output + step.text.len()),
                _ => (thought, output),
            })
    }

    fn thinking_text(&self) -> String {
        self.steps
            .values()
            .filter(|step| step.kind == "thought" && !step.text.is_empty())
            .map(|step| step.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    fn answer_text(&self) -> String {
        self.steps
            .values()
            .filter(|step| step.kind == "model_output")
            .map(|step| step.text.as_str())
            .collect()
    }

    fn pending_status(&self) -> String {
        if is_terminal_status(&self.status) {
            "in_progress".to_owned()
        } else {
            self.status.clone()
        }
    }

    fn should_show(&self, lens: (usize, usize), status: &str) -> bool {
        let caught_up = lens.0 >= self.shown.0 && lens.1 >= self.shown.1;
        let spacing = if lens.1 == self.shown.1 && lens.0 != self.shown.0 {
            Duration::from_millis((200 + lens.0 as u64 / 512).min(15_000))
        } else {
            snapshot_spacing(lens.0 + lens.1)
        };
        let spaced = self.last_snapshot.is_none_or(|at| at.elapsed() >= spacing);
        caught_up
            && ((lens != self.shown && spaced)
                || status != self.shown_status
                || self.usage != self.shown_usage)
    }

    fn report(&mut self, tx: &Sender<WorkerEvent>) {
        let lens = self.text_lens();
        if self.should_show(lens, &self.pending_status()) {
            let snapshot = self.snapshot();
            self.show(tx, snapshot, lens);
        }
        self.send_activity(tx);
    }

    fn report_fetched(&mut self, tx: &Sender<WorkerEvent>, fetched: Fetched) {
        if !is_terminal_status(&self.status) {
            self.status = fetched.status;
        }
        if !fetched.usage.is_empty() {
            self.usage = fetched.usage;
        }
        let lens = (fetched.thinking.len(), fetched.answer.len());
        let status = self.pending_status();
        if self.should_show(lens, &status) {
            let snapshot = TurnSnapshot {
                status,
                thinking_text: fetched.thinking,
                answer_text: fetched.answer,
                ..self.snapshot_without_text()
            };
            self.show(tx, snapshot, lens);
        }
        self.send_activity(tx);
    }

    fn show(&mut self, tx: &Sender<WorkerEvent>, snapshot: TurnSnapshot, lens: (usize, usize)) {
        self.shown = lens;
        self.shown_usage = self.usage;
        self.shown_status = snapshot.status.clone();
        self.last_snapshot = Some(Instant::now());
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
        let doing = if let Some(reason) = &self.polling {
            format!(
                "Checking every {}s (live updates unavailable: {})",
                POLL_INTERVAL.as_secs(),
                reason.trim_end_matches('.')
            )
        } else if is_terminal_status(&self.status) {
            format!(
                "Run {}, fetching its final state",
                self.status.replace('_', " ")
            )
        } else if self.status == "queued" {
            "Queued, waiting for capacity".to_owned()
        } else {
            let latest = self
                .steps
                .values()
                .rev()
                .map(|step| step.kind.as_str())
                .find(|kind| *kind != "user_input");
            match latest {
                None => "Started, waiting for the first thoughts".to_owned(),
                Some("thought") => "Thinking".to_owned(),
                Some("model_output") => match self
                    .steps
                    .values()
                    .filter(|step| step.kind == "model_output")
                    .count()
                {
                    1 => "Writing the answer".to_owned(),
                    part => format!("Writing the answer, part {part}"),
                },
                Some(kind) => format!("Running {}", kind.replace('_', " ")),
            }
        };
        format!("{doing}...{}", self.usage.describe())
    }

    fn snapshot(&self) -> TurnSnapshot {
        TurnSnapshot {
            thinking_text: self.thinking_text(),
            answer_text: self.answer_text(),
            ..self.snapshot_without_text()
        }
    }

    fn live_reasoning_tokens(&self) -> Option<i64> {
        let thought_len = self.text_lens().0.max(self.preserved.0.len());
        let extra_bytes = thought_len.saturating_sub(self.thought_len_at_usage);
        let extra_tokens = if extra_bytes == 0 {
            0
        } else {
            ((extra_bytes as i64 + 3) / 4).max(1)
        };
        match (self.usage.thought, extra_tokens) {
            (None, 0) => None,
            (thought, extra) => Some(thought.unwrap_or(0) + extra),
        }
    }

    fn snapshot_without_text(&self) -> TurnSnapshot {
        let reasoning_tokens = self.live_reasoning_tokens();
        let output_tokens = match (self.usage.output, reasoning_tokens) {
            (None, None) => None,
            (output, thought) => Some(output.unwrap_or(0) + thought.unwrap_or(0)),
        };
        TurnSnapshot {
            turn_id: self.turn_id,
            response_id: Some(self.id.clone()),
            model: None,
            status: self.pending_status(),
            thinking_text: String::new(),
            answer_text: String::new(),
            error_text: None,
            input_tokens: self.usage.input,
            output_tokens,
            reasoning_tokens,
        }
    }

    fn best_text(&self) -> (String, String) {
        let pick = |current: String, preserved: &str| {
            if current.len() >= preserved.len() {
                current
            } else {
                preserved.to_owned()
            }
        };
        (
            pick(self.thinking_text(), &self.preserved.0),
            pick(self.answer_text(), &self.preserved.1),
        )
    }

    fn settle(&self, fetched: Fetched) -> TurnSnapshot {
        let Fetched {
            model,
            status,
            thinking,
            answer,
            usage,
            errors,
            ..
        } = fetched;
        let authoritative = status == "completed";
        let pick = |fetched: String, live: String| {
            if (authoritative && !fetched.is_empty()) || fetched.len() >= live.len() {
                fetched
            } else {
                live
            }
        };
        let (live_thinking, live_answer) = self.best_text();
        let usage = if usage.is_empty() { self.usage } else { usage };

        TurnSnapshot {
            turn_id: self.turn_id,
            response_id: Some(self.id.clone()),
            model: model.filter(|served| is_gemini_model(served)),
            error_text: final_error_text(&status, &errors),
            status,
            thinking_text: pick(thinking, live_thinking),
            answer_text: pick(answer, live_answer),
            input_tokens: usage.input,
            output_tokens: usage.billed_output(),
            reasoning_tokens: usage.thought,
        }
    }

    fn failed(&self, message: String) -> TurnSnapshot {
        let (thinking_text, answer_text) = self.best_text();
        TurnSnapshot {
            status: "failed".to_owned(),
            thinking_text,
            answer_text,
            error_text: Some(message),
            ..self.snapshot_without_text()
        }
    }
}

struct Fetched {
    id: String,
    model: Option<String>,
    status: String,
    thinking: String,
    answer: String,
    usage: Usage,
    errors: Vec<String>,
}

impl Fetched {
    fn from_value(value: &Value) -> Self {
        let mut thoughts = Vec::new();
        let mut answer = String::new();
        for step in value
            .get("steps")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match step.get("type").and_then(Value::as_str) {
                Some("model_output") => answer.push_str(&content_text(step.get("content"))),
                Some("thought") => thoughts.push(summary_text(step)),
                _ => {}
            }
        }
        thoughts.retain(|thought| !thought.is_empty());
        let text_of = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        };

        Self {
            id: text_of("id").unwrap_or_default(),
            model: text_of("model"),
            status: text_of("status").unwrap_or_else(|| "unknown".to_owned()),
            thinking: thoughts.join("\n\n"),
            answer,
            usage: value
                .get("usage")
                .map(Usage::from_value)
                .unwrap_or_default(),
            errors: value
                .get("errors")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|error| error.get("message").and_then(Value::as_str))
                .map(ToOwned::to_owned)
                .collect(),
        }
    }

    fn into_snapshot(self, turn_id: i64) -> TurnSnapshot {
        let error_text = if is_terminal_status(&self.status) {
            final_error_text(&self.status, &self.errors)
        } else {
            None
        };
        TurnSnapshot {
            turn_id,
            response_id: (!self.id.is_empty()).then_some(self.id),
            model: self.model.filter(|served| is_gemini_model(served)),
            status: self.status,
            thinking_text: self.thinking,
            answer_text: self.answer,
            error_text,
            input_tokens: self.usage.input,
            output_tokens: self.usage.billed_output(),
            reasoning_tokens: self.usage.thought,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Usage {
    input: Option<i64>,
    output: Option<i64>,
    thought: Option<i64>,
}

impl Usage {
    fn from_value(value: &Value) -> Self {
        let count = |key: &str| {
            let field = value.get(key)?;
            field.as_i64().or_else(|| field.as_str()?.parse().ok())
        };
        Self {
            input: count("total_input_tokens"),
            output: count("total_output_tokens"),
            thought: count("total_thought_tokens"),
        }
    }

    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn billed_output(&self) -> Option<i64> {
        match (self.output, self.thought) {
            (None, None) => None,
            (output, thought) => Some(output.unwrap_or(0) + thought.unwrap_or(0)),
        }
    }

    fn describe(&self) -> String {
        let counts = [(self.thought, "thought"), (self.output, "output")]
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

fn content_text(content: Option<&Value>) -> String {
    content
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|part| {
            part.get("type")
                .and_then(Value::as_str)
                .is_none_or(|kind| kind == "text")
        })
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect()
}

fn summary_text(step: &Value) -> String {
    step.get("summary")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn final_error_text(status: &str, errors: &[String]) -> Option<String> {
    let mut lines = errors.to_vec();
    match status {
        "incomplete" => lines.push(
            "The run used up max_output_tokens before it finished. If it was still thinking, there is no answer text. Raise max tokens (or lower the thinking level) and send the prompt again."
                .to_owned(),
        ),
        "requires_action" => {
            lines.push("The model asked for a tool call, which this app does not run.".to_owned())
        }
        "failed" if lines.is_empty() => lines.push("Gemini reported the run as failed.".to_owned()),
        _ => {}
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn next_backoff(backoff: Duration) -> Duration {
    Duration::from_secs((backoff.as_secs() * 2).min(MAX_BACKOFF_SECONDS))
}

#[cfg(test)]
mod tests;
