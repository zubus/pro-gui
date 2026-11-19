pub mod anthropic;
pub mod gemini;
mod http;
pub mod openai;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::models::{ModelOption, RequestSpec, TurnSnapshot, WorkerEvent};
use http::RequestError;

const MAX_CREATE_ATTEMPTS: u32 = 5;

const NOTE_SPACING: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub struct AttemptEvent {
    pub attempt: u64,
    pub event: WorkerEvent,
}

impl AttemptEvent {
    pub fn turn_id(&self) -> i64 {
        match &self.event {
            WorkerEvent::Note { turn_id, .. } => *turn_id,
            WorkerEvent::Snapshot(snapshot) => snapshot.turn_id,
        }
    }

    pub fn is_current(&self) -> bool {
        current_attempt(self.turn_id()) == Some(self.attempt)
    }
}

pub const MAX_OUTPUT_TOKENS: i64 = gemini::LONG_DECODE_MAX_OUTPUT_TOKENS;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    OpenAi,
    Gemini,
    Anthropic,
}

impl Provider {
    pub fn of(model: &str) -> Self {
        if gemini::is_gemini_model(model) {
            Self::Gemini
        } else if anthropic::is_anthropic_model(model) {
            Self::Anthropic
        } else {
            Self::OpenAi
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::Gemini => "Gemini",
            Self::Anthropic => "Anthropic",
        }
    }

    pub fn effort_param(self) -> &'static str {
        match self {
            Self::OpenAi => "reasoning.effort",
            Self::Gemini => "thinking_level",
            Self::Anthropic => "output_config.effort",
        }
    }

    pub fn summary_param(self) -> &'static str {
        match self {
            Self::OpenAi => "reasoning.summary",
            Self::Gemini => "thinking_summaries",
            Self::Anthropic => "thinking",
        }
    }
}

pub fn model_groups() -> [&'static [ModelOption]; 3] {
    [
        openai::model_options(),
        gemini::model_options(),
        anthropic::model_options(),
    ]
}

pub fn model_display_name(model: &str) -> String {
    for group in model_groups() {
        if let Some(option) = group.iter().find(|option| option.id == model) {
            return option.label.to_owned();
        }
    }
    openai::model_display_name(model)
}

pub fn effort_options(model: &str) -> &'static [&'static str] {
    match Provider::of(model) {
        Provider::OpenAi => openai::REASONING_EFFORT_OPTIONS,
        Provider::Gemini => gemini::thinking_levels(model),
        Provider::Anthropic => anthropic::EFFORT_OPTIONS,
    }
}

pub fn default_reasoning_effort(model: &str) -> &'static str {
    match Provider::of(model) {
        Provider::OpenAi => openai::default_reasoning_effort(model),
        Provider::Gemini => gemini::default_thinking_level(model),
        Provider::Anthropic => anthropic::DEFAULT_EFFORT,
    }
}

pub fn max_output_tokens(model: &str) -> i64 {
    match Provider::of(model) {
        Provider::OpenAi => openai::MAX_OUTPUT_TOKENS,
        Provider::Gemini => gemini::max_output_tokens(model),
        Provider::Anthropic => anthropic::max_output_tokens(model),
    }
}

pub fn clamp_output_tokens(model: &str, value: i64) -> i64 {
    value.clamp(1_024, max_output_tokens(model))
}

pub fn request_service_tier(model: &str, use_priority_processing: bool) -> &'static str {
    match Provider::of(model) {
        Provider::OpenAi => openai::request_service_tier(use_priority_processing),
        Provider::Gemini => gemini::request_service_tier(use_priority_processing),
        Provider::Anthropic => anthropic::request_service_tier(model, use_priority_processing),
    }
}

pub fn supports_priority(model: &str) -> bool {
    match Provider::of(model) {
        Provider::OpenAi | Provider::Gemini => true,
        Provider::Anthropic => anthropic::supports_priority_tier(model),
    }
}

pub fn use_background_mode(model: &str) -> bool {
    match Provider::of(model) {
        Provider::OpenAi => openai::use_background_mode(),
        Provider::Gemini => true,
        Provider::Anthropic => false,
    }
}

pub fn store_responses(model: &str) -> bool {
    match Provider::of(model) {
        Provider::OpenAi => openai::store_responses(),
        Provider::Gemini => true,
        Provider::Anthropic => false,
    }
}

pub fn reasoning_summary(model: &str) -> &'static str {
    match Provider::of(model) {
        Provider::OpenAi => openai::reasoning_summary(),
        Provider::Gemini => gemini::THINKING_SUMMARIES,
        Provider::Anthropic => anthropic::THINKING_SUMMARY,
    }
}

pub fn spawn_new_turn(out: Sender<AttemptEvent>, turn_id: i64, spec: RequestSpec, api_key: String) {
    set_cancelled(turn_id, false);
    let tx = tagged_sender(out, begin_attempt(turn_id));
    match Provider::of(&spec.model) {
        Provider::OpenAi => openai::spawn_new_turn(tx, turn_id, spec, api_key),
        Provider::Gemini => gemini::spawn_new_turn(tx, turn_id, spec, api_key),
        Provider::Anthropic => anthropic::spawn_new_turn(tx, turn_id, spec, api_key),
    }
}

pub fn spawn_resume_turn(
    out: Sender<AttemptEvent>,
    turn_id: i64,
    response_id: String,
    model: String,
    api_key: String,
) {
    let tx = tagged_sender(out, begin_attempt(turn_id));
    match Provider::of(&model) {
        Provider::OpenAi => openai::spawn_resume_turn(tx, turn_id, response_id, model, api_key),
        Provider::Gemini => gemini::spawn_resume_turn(tx, turn_id, response_id, api_key),
        Provider::Anthropic => anthropic::spawn_resume_turn(tx, turn_id, response_id, api_key),
    }
}

pub fn spawn_cancel_turn(
    out: Sender<AttemptEvent>,
    turn_id: i64,
    response_id: Option<String>,
    model: String,
    api_key: String,
) {
    set_cancelled(turn_id, true);
    let Some(attempt) = current_attempt(turn_id) else {
        return;
    };
    let tx = tagged_sender(out, attempt);
    let Some(response_id) = response_id else {
        send_note(
            &tx,
            turn_id,
            "Cancel requested, stopping before the run starts...".to_owned(),
        );
        return;
    };
    match Provider::of(&model) {
        Provider::OpenAi => openai::spawn_cancel_turn(tx, turn_id, response_id, model, api_key),
        Provider::Gemini => gemini::spawn_cancel_turn(tx, turn_id, response_id, api_key),
        Provider::Anthropic => anthropic::spawn_cancel_turn(tx, turn_id, response_id, api_key),
    }
}

fn cancelled_turns() -> &'static Mutex<HashSet<i64>> {
    static CANCELLED: OnceLock<Mutex<HashSet<i64>>> = OnceLock::new();
    CANCELLED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn set_cancelled(turn_id: i64, cancelled: bool) {
    let mut turns = cancelled_turns().lock().expect("cancel lock poisoned");
    if cancelled {
        turns.insert(turn_id);
    } else {
        turns.remove(&turn_id);
    }
}

fn is_cancelled(turn_id: i64) -> bool {
    cancelled_turns()
        .lock()
        .expect("cancel lock poisoned")
        .contains(&turn_id)
}

fn wait_unless_cancelled(turn_id: i64, delay: Duration) -> bool {
    let deadline = Instant::now() + delay;
    loop {
        if is_cancelled(turn_id) {
            return false;
        }
        let now = Instant::now();
        if now >= deadline {
            return true;
        }
        thread::sleep((deadline - now).min(Duration::from_millis(200)));
    }
}

fn send_note(tx: &Sender<WorkerEvent>, turn_id: i64, message: String) {
    let _ = tx.send(WorkerEvent::Note { turn_id, message });
}

fn split_data_url(url: &str) -> Option<(&str, &str)> {
    url.strip_prefix("data:")?.split_once(";base64,")
}

fn snapshot_spacing(text_len: usize) -> Duration {
    Duration::from_millis((1_000 + text_len as u64 / 256).min(15_000))
}

fn is_terminal_status(status: &str) -> bool {
    !matches!(status, "queued" | "in_progress")
}

fn ended_snapshot(turn_id: i64, status: &str, error_text: String) -> TurnSnapshot {
    TurnSnapshot {
        turn_id,
        response_id: None,
        model: None,
        status: status.to_owned(),
        thinking_text: String::new(),
        answer_text: String::new(),
        error_text: Some(error_text),
        input_tokens: None,
        output_tokens: None,
        reasoning_tokens: None,
    }
}

fn create_with_retry<T>(
    tx: &Sender<WorkerEvent>,
    turn_id: i64,
    api_name: &str,
    note: &str,
    mut create: impl FnMut() -> Result<T, RequestError>,
) -> Option<T> {
    let cancelled = || {
        let _ = tx.send(WorkerEvent::Snapshot(ended_snapshot(
            turn_id,
            "cancelled",
            format!("Cancelled before the request reached {api_name}."),
        )));
    };
    let mut backoff = Duration::from_secs(2);
    for attempt in 1..=MAX_CREATE_ATTEMPTS {
        if is_cancelled(turn_id) {
            cancelled();
            return None;
        }
        send_note(tx, turn_id, note.to_owned());
        let err = match create() {
            Ok(value) => return Some(value),
            Err(err) => err,
        };
        let retry = err.retryable && err.safe_to_retry_create;
        if !retry || attempt == MAX_CREATE_ATTEMPTS {
            let message = if retry {
                format!(
                    "{} Gave up after {MAX_CREATE_ATTEMPTS} attempts. Use Send again to retry.",
                    err.message
                )
            } else if err.retryable {
                format!(
                    "{} The request was not retried automatically because it may already have reached {api_name}. Use Send again to retry manually if needed.",
                    err.message
                )
            } else {
                err.message
            };
            let _ = tx.send(WorkerEvent::Snapshot(ended_snapshot(
                turn_id, "failed", message,
            )));
            return None;
        }
        send_note(
            tx,
            turn_id,
            format!(
                "Create failed: {}. Retrying in {}s (attempt {attempt} of {MAX_CREATE_ATTEMPTS}).",
                err.message,
                backoff.as_secs()
            ),
        );
        if !wait_unless_cancelled(turn_id, backoff) {
            cancelled();
            return None;
        }
        backoff *= 2;
    }
    None
}

fn attempts() -> &'static Mutex<HashMap<i64, u64>> {
    static ATTEMPTS: OnceLock<Mutex<HashMap<i64, u64>>> = OnceLock::new();
    ATTEMPTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn begin_attempt(turn_id: i64) -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let attempt = NEXT.fetch_add(1, Ordering::Relaxed);
    attempts()
        .lock()
        .expect("attempt lock poisoned")
        .insert(turn_id, attempt);
    attempt
}

fn current_attempt(turn_id: i64) -> Option<u64> {
    attempts()
        .lock()
        .expect("attempt lock poisoned")
        .get(&turn_id)
        .copied()
}

fn tagged_sender(out: Sender<AttemptEvent>, attempt: u64) -> Sender<WorkerEvent> {
    let (tx, rx) = mpsc::channel::<WorkerEvent>();
    thread::spawn(move || {
        for event in rx {
            if out.send(AttemptEvent { attempt, event }).is_err() {
                break;
            }
        }
    });
    tx
}
