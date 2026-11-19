use std::fmt;

use chrono::Local;

#[derive(Clone, Debug)]
pub struct Folder {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct Conversation {
    pub id: i64,
    pub title: String,
    pub folder_id: Option<i64>,
    pub system_prompt: String,
    pub workspace_path: String,
    pub context_selection: Vec<String>,
    pub updated_at: String,
    pub max_output_tokens: i64,
    pub turn_count: usize,
    pub preview: String,
    pub last_status: String,
    pub turns_loaded: bool,
    pub turns: Vec<Turn>,
}

impl Conversation {
    pub fn rename(&mut self, title: String) {
        self.title = title;
    }

    pub fn append_turn(&mut self, turn: Turn) {
        self.preview = turn_preview(&turn);
        self.last_status = turn.status.clone();
        self.turn_count += 1;
        self.turns.push(turn);
    }

    pub fn refresh_summary(&mut self) {
        self.turn_count = self.turns.len();
        match self.turns.last() {
            Some(turn) => {
                self.preview = turn_preview(turn);
                self.last_status = turn.status.clone();
            }
            None => {
                self.preview.clear();
                self.last_status = "idle".to_owned();
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct PromptBlock {
    pub label: String,
    pub text: String,
    line_count: usize,
}

impl PromptBlock {
    pub fn new(label: String, text: String) -> Self {
        let mut block = Self {
            label,
            text,
            line_count: 0,
        };
        block.refresh_lines();
        block
    }

    pub fn lines(&self) -> usize {
        self.line_count
    }

    pub fn refresh_lines(&mut self) {
        self.line_count = self.text.bytes().filter(|byte| *byte == b'\n').count() + 1;
    }
}

pub fn turn_preview(turn: &Turn) -> String {
    let source = if turn.answer_text.is_empty() {
        &turn.prompt_text
    } else {
        &turn.answer_text
    };
    source.chars().take(200).collect()
}

#[derive(Clone, Debug)]
pub struct AttachmentMeta {
    pub id: i64,
    pub mime: String,
    pub file_name: String,
    pub byte_len: i64,
}

#[derive(Clone, Debug)]
pub struct Turn {
    pub id: i64,
    pub prompt_text: String,
    pub prompt_prefix_len: usize,
    pub attachments: Vec<AttachmentMeta>,
    pub thinking_text: String,
    pub answer_text: String,
    pub model: String,
    pub max_output_tokens: i64,
    pub reasoning_effort: String,
    pub reasoning_summary: String,
    pub service_tier: String,
    pub background: bool,
    pub store: bool,
    pub response_id: Option<String>,
    pub parent_response_id: Option<String>,
    pub status: String,
    pub error_text: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub created_at: String,
    pub request_started_at: String,
    pub updated_at: String,
}

impl Turn {
    pub fn is_pending(&self) -> bool {
        matches!(self.status.as_str(), "queued" | "in_progress")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelOption {
    pub id: &'static str,
    pub label: &'static str,
    pub detail: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryRole {
    User,
    Assistant,
}

impl HistoryRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryMessage {
    pub role: HistoryRole,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct RequestSpec {
    pub model: String,
    pub prompt: String,
    pub images: Vec<String>,
    pub instructions: String,
    pub max_output_tokens: i64,
    pub reasoning_effort: String,
    pub service_tier: String,
    pub history: Vec<HistoryMessage>,
    pub previous_response_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct NewTurn {
    pub conversation_id: i64,
    pub prompt_text: String,
    pub prompt_prefix_len: usize,
    pub model: String,
    pub max_output_tokens: i64,
    pub reasoning_effort: String,
    pub reasoning_summary: String,
    pub service_tier: String,
    pub background: bool,
    pub store: bool,
    pub parent_response_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TurnSnapshot {
    pub turn_id: i64,
    pub response_id: Option<String>,
    pub model: Option<String>,
    pub status: String,
    pub thinking_text: String,
    pub answer_text: String,
    pub error_text: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
}

#[derive(Clone, Debug)]
pub enum WorkerEvent {
    Note { turn_id: i64, message: String },
    Snapshot(TurnSnapshot),
}

#[derive(Clone)]
pub struct AppSettings {
    pub api_key: String,
    pub gemini_api_key: String,
    pub anthropic_api_key: String,
    pub selected_model: String,
    pub render_markdown_answers: bool,
    pub render_latex_blocks: bool,
    pub auto_resend_failed_answers: bool,
    pub use_priority_processing: bool,
    pub enter_sends: bool,
    pub show_request_chips: bool,
    pub font_size: f32,
    pub paste_block_kb: i64,
    pub show_context_panel: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            gemini_api_key: String::new(),
            anthropic_api_key: String::new(),
            selected_model: String::new(),
            render_markdown_answers: true,
            render_latex_blocks: true,
            auto_resend_failed_answers: false,
            use_priority_processing: true,
            enter_sends: false,
            show_request_chips: true,
            font_size: 14.0,
            paste_block_kb: 32,
            show_context_panel: false,
        }
    }
}

impl fmt::Debug for AppSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key = |value: &str| if value.is_empty() { "" } else { "<redacted>" };
        f.debug_struct("AppSettings")
            .field("api_key", &key(&self.api_key))
            .field("gemini_api_key", &key(&self.gemini_api_key))
            .field("anthropic_api_key", &key(&self.anthropic_api_key))
            .field("selected_model", &self.selected_model)
            .field("render_markdown_answers", &self.render_markdown_answers)
            .field("render_latex_blocks", &self.render_latex_blocks)
            .field(
                "auto_resend_failed_answers",
                &self.auto_resend_failed_answers,
            )
            .field("use_priority_processing", &self.use_priority_processing)
            .field("enter_sends", &self.enter_sends)
            .field("show_request_chips", &self.show_request_chips)
            .field("font_size", &self.font_size)
            .field("paste_block_kb", &self.paste_block_kb)
            .field("show_context_panel", &self.show_context_panel)
            .finish()
    }
}

pub fn now_string() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn format_compact_count(value: i64) -> String {
    if value >= 1_000_000 {
        format!("{:.2}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

pub fn title_from_prompt(prompt: &str) -> String {
    let compact = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return "New chat".to_owned();
    }

    let mut title = compact.chars().take(52).collect::<String>();
    if compact.chars().count() > 52 {
        title.push_str("...");
    }
    title
}
