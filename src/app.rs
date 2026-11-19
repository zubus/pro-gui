use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{Local, NaiveDateTime};
use eframe::egui::{
    self, Align, CentralPanel, Color32, Context, FontFamily, FontId, Frame, Layout, Margin,
    RichText, ScrollArea, SidePanel, Slider, Stroke, TextEdit, TextFormat, TextureHandle,
    TopBottomPanel, Visuals, text::LayoutJob,
};

use crate::bigtext;
use crate::context as repoctx;
use crate::db::Database;
use crate::docedit::DocumentEditor;
use crate::models::{
    AttachmentMeta, BlockRange, Conversation, Folder, HistoryMessage, HistoryRole, NewTurn,
    PromptBlock, RequestSpec, Turn, TurnSnapshot, WorkerEvent, format_compact_count, now_string,
    title_from_prompt,
};
use crate::providers::{self, AttemptEvent, Provider, openai};

mod attachments;
mod composer;
mod context_panel;
mod dialogs;
mod export;
mod header;
mod markdown;
mod sidebar;
#[cfg(test)]
mod tests;
mod theme;
mod transcript;
mod turns;

use attachments::*;
use composer::*;
use export::*;
use markdown::*;
use theme::*;
use turns::*;

const PACKED_CONTEXT_SEPARATOR: &str = "---\n\nUser request:\n\n";

const PASTE_BLOCK_LINES: usize = 500;

const PACKED_CONTEXT_HEADER: &str = "# Repository context: ";

const MAX_ATTACHMENTS_PER_MESSAGE: usize = 8;
const MAX_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;
const PENDING_PERSIST_INTERVAL: Duration = Duration::from_secs(3);
const REPOSITORY_CONTEXT_LABEL: &str = "Repository context";

pub struct ProGuiApp {
    db: Database,
    conversations: Vec<Conversation>,
    folders: Vec<Folder>,
    active_modal: Option<AppModal>,
    modal_needs_focus: bool,
    selected_conversation_id: Option<i64>,
    chat_search: String,
    in_chat_search: String,
    focus_in_chat_search: bool,
    in_chat_search_current: usize,
    scroll_to_in_chat_match: bool,
    render_markdown_answers: bool,
    render_latex_blocks: bool,
    auto_resend_failed_answers: bool,
    use_priority_processing: bool,
    enter_sends: bool,
    show_request_chips: bool,
    font_size: f32,
    paste_block_kb: i64,
    api_key: String,
    api_key_draft: String,
    gemini_api_key: String,
    gemini_api_key_draft: String,
    anthropic_api_key: String,
    anthropic_api_key_draft: String,
    selected_model: String,
    show_api_key_modal: bool,
    show_settings: bool,
    composer: String,
    composer_drafts: HashMap<i64, String>,
    composer_blocks: Vec<PromptBlock>,
    block_drafts: HashMap<i64, Vec<PromptBlock>>,
    attachment_drafts: HashMap<i64, Vec<PendingAttachment>>,
    viewing_block: Option<(BlockOwner, usize)>,
    block_editor: DocumentEditor,
    skip_context_once: bool,
    parked_composer: Option<(String, Vec<PromptBlock>)>,
    context_reused: bool,
    composer_focused: bool,
    composer_settings: ComposerSettings,
    custom_model_input: Option<String>,
    custom_model_needs_focus: bool,
    pending_attachments: Vec<PendingAttachment>,
    pending_textures: HashMap<u64, Option<TextureHandle>>,
    attachment_textures: HashMap<i64, Option<TextureHandle>>,
    next_attachment_key: u64,
    workspace_tree: Option<repoctx::WorkspaceTree>,
    show_context_panel: bool,
    context_filter: String,
    context_exts: BTreeSet<String>,
    context_selected_only: bool,
    context_expanded: BTreeSet<String>,
    context_rows: Option<context_panel::ContextRowsCache>,
    workspace_generation: u64,
    workspace_scan: Option<(String, Receiver<repoctx::WorkspaceTree>)>,
    file_tokens_generation: u64,
    token_check: Option<(u64, Instant)>,
    token_work_pending: bool,
    selection_summary: Option<(u64, (usize, usize, bool))>,
    context_filter_needs_focus: bool,
    file_token_cache: HashMap<String, FileTokens>,
    allow_secrets_once: bool,
    editing_turn: Option<TurnEdit>,
    composer_editor: DocumentEditor,
    chat_search_matches: SearchMatches,
    switcher_matches: SearchMatches,
    in_chat_match_total: usize,
    tx: Sender<AttemptEvent>,
    rx: Receiver<AttemptEvent>,
    runtime: HashMap<i64, TurnRuntime>,
    answer_cache: HashMap<i64, CachedAnswerBlocks>,
    turn_heights: HashMap<i64, (u64, f32)>,
    persisted_at: HashMap<i64, Instant>,
    export_job: Option<Receiver<Result<PathBuf>>>,
    image_tx: Sender<ImageResult>,
    image_rx: Receiver<ImageResult>,
    applied_font_size: f32,
    banner: Option<String>,
    banner_timer: Option<(String, Instant)>,
}

#[derive(Clone, Default)]
struct TurnRuntime {
    note: Option<String>,
    auto_retry_attempts: u8,
}

struct TurnEdit {
    turn_id: i64,
    draft: String,
    blocks: Vec<PromptBlock>,
    needs_focus: bool,
    editor: DocumentEditor,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockOwner {
    Composer,
    TurnEdit,
}

#[derive(Clone, Copy)]
struct FileTokens {
    stamp: FileStamp,
    tokens: usize,
}

type FileStamp = Option<(u64, u128)>;

#[derive(Default)]
struct SearchMatches {
    query: String,
    ids: Option<HashSet<i64>>,
    error: Option<String>,
}

struct CachedAnswerBlocks {
    answer_len: usize,
    thinking_len: usize,
    thinking_tokens: i64,
    thinking_tokens_exact: bool,
    latex_enabled: bool,
    blocks: Vec<AnswerBlock>,
    thinking_blocks: Vec<AnswerBlock>,
}

struct InChatSearchRender<'a> {
    query: &'a str,
    current_match_index: Option<usize>,
    seen_matches: usize,
    scroll_to_current: bool,
    scrolled_to_current: bool,
}

impl<'a> InChatSearchRender<'a> {
    fn new(query: &'a str, current_match_index: Option<usize>, scroll_to_current: bool) -> Self {
        Self {
            query,
            current_match_index,
            seen_matches: 0,
            scroll_to_current,
            scrolled_to_current: false,
        }
    }

    fn is_active(&self) -> bool {
        is_search_active(self.query)
    }
}

enum AnswerBlock {
    Paragraph(String),
    Code { language: String, code: String },
    Math(String),
    Heading { level: usize, text: String },
    EmphasizedTitle(String),
    Bullet(String),
    Quote(String),
    Spacer(f32),
}

enum TurnAction {
    Retry(i64),
    Cancel(i64),
    EditBlock(usize),
    RemoveBlock(usize),
    BeginEdit(i64),
    CancelEdit,
    ResendEdit,
    ResendEditAsNew,
}

#[derive(Clone)]
struct ComposerSettings {
    model: String,
    effort: String,
    max_output_tokens: i64,
    priority: bool,
}

struct PendingAttachment {
    key: u64,
    file_name: String,
    mime: String,
    bytes: Vec<u8>,
}

enum AppModal {
    Folder {
        folder_id: Option<i64>,
        draft: String,
        move_conversation_id: Option<i64>,
    },
    RenameChat {
        conversation_id: i64,
        draft: String,
    },
    ConfirmDeleteChat {
        conversation_id: i64,
        title: String,
        turn_count: usize,
    },
    SystemPrompt {
        conversation_id: i64,
        draft: String,
    },
    QuickSwitcher {
        query: String,
        selected: usize,
    },
    Shortcuts,
    ConfirmSecrets {
        hits: Vec<String>,
        total: usize,
    },
}

enum HeaderAction {
    OpenSystemPrompt,
    OpenShortcuts,
    CopyMarkdown,
    ExportMarkdown,
    ExportJson,
    ExportAllJson,
    BackupDatabase,
}

enum SidebarAction {
    Select(i64),
    MoveToFolder {
        conversation_id: i64,
        folder_id: Option<i64>,
    },
    NewChatInFolder(Option<i64>),
    NewFolderForChat(i64),
    RenameChat(i64),
    DeleteChat(i64),
    RenameFolder(i64),
    DeleteFolder(i64),
}

impl ProGuiApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Result<Self> {
        configure_visuals(&cc.egui_ctx);

        let db_path = database_path();
        if !db_path.exists() {
            let legacy = PathBuf::from("pro_gui.sqlite3");
            if legacy.exists() {
                let _ = std::fs::copy(&legacy, &db_path);
            }
        }
        let db = Database::open(&db_path)?;
        let settings = db.load_app_settings()?;
        let selected_model = openai::normalize_model(&settings.selected_model);
        apply_font_size(&cc.egui_ctx, settings.font_size);
        let folders = db.load_folders()?;
        let mut conversations = db.load_conversation_summaries()?;
        if conversations.is_empty() {
            db.create_conversation("New chat", None)?;
            conversations = db.load_conversation_summaries()?;
        }
        let pending = db.conversations_with_pending_turns()?;
        for conversation in &mut conversations {
            if pending.contains(&conversation.id) {
                conversation.turns = db.load_turns(conversation.id)?;
                conversation.turns_loaded = true;
            }
        }
        let (image_tx, image_rx) = mpsc::channel();
        let (tx, worker_rx) = mpsc::channel::<AttemptEvent>();
        let (ui_tx, rx) = mpsc::channel();
        let repaint = cc.egui_ctx.clone();
        std::thread::spawn(move || {
            for event in worker_rx {
                if ui_tx.send(event).is_err() {
                    break;
                }
                repaint.request_repaint();
            }
        });

        let composer_settings = ComposerSettings {
            model: selected_model.clone(),
            effort: providers::default_reasoning_effort(&selected_model).to_owned(),
            max_output_tokens: providers::max_output_tokens(&selected_model),
            priority: settings.use_priority_processing,
        };

        let mut app = Self {
            db,
            selected_conversation_id: conversations.first().map(|conversation| conversation.id),
            conversations,
            folders,
            active_modal: None,
            modal_needs_focus: false,
            chat_search: String::new(),
            in_chat_search: String::new(),
            focus_in_chat_search: false,
            in_chat_search_current: 0,
            scroll_to_in_chat_match: false,
            render_markdown_answers: settings.render_markdown_answers,
            render_latex_blocks: settings.render_latex_blocks,
            auto_resend_failed_answers: settings.auto_resend_failed_answers,
            use_priority_processing: settings.use_priority_processing,
            enter_sends: settings.enter_sends,
            show_request_chips: settings.show_request_chips,
            font_size: settings.font_size,
            paste_block_kb: settings.paste_block_kb,
            api_key: settings.api_key.clone(),
            api_key_draft: settings.api_key,
            gemini_api_key: settings.gemini_api_key.clone(),
            gemini_api_key_draft: settings.gemini_api_key,
            anthropic_api_key: settings.anthropic_api_key.clone(),
            anthropic_api_key_draft: settings.anthropic_api_key,
            selected_model,
            show_api_key_modal: false,
            composer: String::new(),
            composer_drafts: HashMap::new(),
            composer_blocks: Vec::new(),
            block_drafts: HashMap::new(),
            attachment_drafts: HashMap::new(),
            viewing_block: None,
            block_editor: DocumentEditor::default(),
            skip_context_once: false,
            parked_composer: None,
            context_reused: false,
            composer_focused: false,
            composer_settings,
            custom_model_input: None,
            custom_model_needs_focus: false,
            pending_attachments: Vec::new(),
            pending_textures: HashMap::new(),
            attachment_textures: HashMap::new(),
            next_attachment_key: 0,
            workspace_tree: None,
            show_context_panel: settings.show_context_panel,
            context_filter: String::new(),
            context_exts: BTreeSet::new(),
            context_selected_only: false,
            context_expanded: BTreeSet::new(),
            context_rows: None,
            workspace_generation: 0,
            workspace_scan: None,
            file_tokens_generation: 0,
            token_check: None,
            token_work_pending: false,
            selection_summary: None,
            context_filter_needs_focus: false,
            file_token_cache: HashMap::new(),
            allow_secrets_once: false,
            editing_turn: None,
            composer_editor: DocumentEditor::default(),
            chat_search_matches: SearchMatches::default(),
            switcher_matches: SearchMatches::default(),
            in_chat_match_total: 0,
            tx,
            rx,
            runtime: HashMap::new(),
            answer_cache: HashMap::new(),
            turn_heights: HashMap::new(),
            persisted_at: HashMap::new(),
            export_job: None,
            image_tx,
            image_rx,
            applied_font_size: settings.font_size,
            banner: None,
            banner_timer: None,
            show_settings: false,
        };
        if let Some(conversation_id) = app.selected_conversation_id {
            app.ensure_turns_loaded(conversation_id);
        }
        app.resume_pending_turns();
        app.seed_composer_settings();
        Ok(app)
    }

    fn select_conversation(&mut self, conversation_id: i64) {
        if self.selected_conversation_id == Some(conversation_id) {
            return;
        }
        if let Some(previous) = self.selected_conversation_id {
            self.unload_conversation(previous);
        }
        self.stash_composer_draft();
        self.composer = self
            .composer_drafts
            .remove(&conversation_id)
            .unwrap_or_default();
        self.composer_blocks = self
            .block_drafts
            .remove(&conversation_id)
            .unwrap_or_default();
        self.pending_attachments = self
            .attachment_drafts
            .remove(&conversation_id)
            .unwrap_or_default();
        self.selected_conversation_id = Some(conversation_id);
        self.editing_turn = None;
        self.composer_editor = DocumentEditor::default();
        self.ensure_turns_loaded(conversation_id);
        self.seed_composer_settings();
    }

    fn block_ranges_for(&self, turns: &[Turn]) -> HashMap<i64, Vec<BlockRange>> {
        turns
            .iter()
            .filter(|turn| turn.status == "completed" && turn.prompt_prefix_len > 0)
            .filter_map(|turn| {
                self.db
                    .load_turn_blocks(turn.id)
                    .ok()
                    .map(|ranges| (turn.id, ranges))
            })
            .collect()
    }

    fn unload_conversation(&mut self, conversation_id: i64) {
        let Some(conversation) = self
            .conversations
            .iter_mut()
            .find(|conversation| conversation.id == conversation_id)
        else {
            return;
        };
        if !conversation.turns_loaded || conversation.turns.iter().any(Turn::is_pending) {
            return;
        }
        for turn in &conversation.turns {
            self.answer_cache.remove(&turn.id);
            self.turn_heights.remove(&turn.id);
            self.persisted_at.remove(&turn.id);
            for attachment in &turn.attachments {
                self.attachment_textures.remove(&attachment.id);
            }
        }
        conversation.turns = Vec::new();
        conversation.turns_loaded = false;
    }

    fn ensure_turns_loaded(&mut self, conversation_id: i64) {
        let Some(index) = self
            .conversations
            .iter()
            .position(|conversation| conversation.id == conversation_id)
        else {
            return;
        };
        if self.conversations[index].turns_loaded {
            return;
        }
        match self.db.load_turns(conversation_id) {
            Ok(turns) => {
                let conversation = &mut self.conversations[index];
                conversation.turns = turns;
                conversation.turns_loaded = true;
                conversation.refresh_summary();
            }
            Err(err) => self.banner = Some(format!("Failed to open the chat: {err}")),
        }
    }

    fn selected_conversation_mut(&mut self) -> Option<&mut Conversation> {
        let selected_id = self.selected_conversation_id?;
        self.conversations
            .iter_mut()
            .find(|conversation| conversation.id == selected_id)
    }

    fn selected_conversation(&self) -> Option<&Conversation> {
        let selected_id = self.selected_conversation_id?;
        self.conversations
            .iter()
            .find(|conversation| conversation.id == selected_id)
    }

    fn create_conversation(&mut self, folder_id: Option<i64>) {
        match self.db.create_conversation("New chat", folder_id) {
            Ok(conversation_id) => {
                self.conversations.insert(
                    0,
                    Conversation {
                        id: conversation_id,
                        title: "New chat".to_owned(),
                        folder_id,
                        system_prompt: String::new(),
                        workspace_path: String::new(),
                        context_selection: Vec::new(),
                        updated_at: now_string(),
                        max_output_tokens: openai::MAX_OUTPUT_TOKENS,
                        turn_count: 0,
                        preview: String::new(),
                        last_status: "idle".to_owned(),
                        turns_loaded: true,
                        turns: Vec::new(),
                    },
                );
                if let Some(previous) = self.selected_conversation_id {
                    self.unload_conversation(previous);
                }
                self.stash_composer_draft();
                self.selected_conversation_id = Some(conversation_id);
                self.editing_turn = None;
                self.composer_editor = DocumentEditor::default();
                self.seed_composer_settings();
                self.banner = None;
            }
            Err(err) => {
                self.banner = Some(format!("Failed to create a new chat: {err}"));
            }
        }
    }

    fn move_conversation_to_folder(&mut self, conversation_id: i64, folder_id: Option<i64>) {
        if let Err(err) = self.db.set_conversation_folder(conversation_id, folder_id) {
            self.banner = Some(format!("Failed to move chat: {err}"));
            return;
        }
        if let Some(conversation) = self
            .conversations
            .iter_mut()
            .find(|conversation| conversation.id == conversation_id)
        {
            conversation.folder_id = folder_id;
        }
    }

    fn delete_folder(&mut self, folder_id: i64) {
        if let Err(err) = self.db.delete_folder(folder_id) {
            self.banner = Some(format!("Failed to delete folder: {err}"));
            return;
        }
        self.folders.retain(|folder| folder.id != folder_id);
        for conversation in &mut self.conversations {
            if conversation.folder_id == Some(folder_id) {
                conversation.folder_id = None;
            }
        }
    }

    fn delete_conversation(&mut self, conversation_id: i64) {
        if let Err(err) = self.db.delete_conversation(conversation_id) {
            self.banner = Some(format!("Failed to delete chat: {err}"));
            return;
        }

        if let Some(index) = self
            .conversations
            .iter()
            .position(|conversation| conversation.id == conversation_id)
        {
            let removed = self.conversations.remove(index);
            let editing_turn_id = self.editing_turn.as_ref().map(|edit| edit.turn_id);
            for turn in &removed.turns {
                self.runtime.remove(&turn.id);
                self.answer_cache.remove(&turn.id);
                for attachment in &turn.attachments {
                    self.attachment_textures.remove(&attachment.id);
                }
                if editing_turn_id == Some(turn.id) {
                    self.editing_turn = None;
                }
            }
            self.banner = Some(format!("Deleted chat \"{}\".", removed.title));
        }

        self.composer_drafts.remove(&conversation_id);
        self.block_drafts.remove(&conversation_id);
        self.attachment_drafts.remove(&conversation_id);
        if self.selected_conversation_id == Some(conversation_id) {
            self.composer.clear();
            self.composer_blocks.clear();
            self.pending_attachments.clear();
            self.pending_textures.clear();
            self.selected_conversation_id = None;
            match self
                .conversations
                .first()
                .map(|conversation| conversation.id)
            {
                Some(next_id) => self.select_conversation(next_id),
                None => self.create_conversation(None),
            }
        }
    }

    fn sort_folders(&mut self) {
        self.folders
            .sort_by_key(|folder| folder.name.to_lowercase());
    }

    fn expire_banner(&mut self, ctx: &Context) {
        const BANNER_LIFETIME: Duration = Duration::from_secs(8);
        match self.banner.clone() {
            Some(current) => {
                let restart = self
                    .banner_timer
                    .as_ref()
                    .is_none_or(|(text, _)| *text != current);
                if restart {
                    self.banner_timer = Some((current, Instant::now()));
                    ctx.request_repaint_after(BANNER_LIFETIME);
                } else if let Some((_, shown_at)) = &self.banner_timer {
                    if shown_at.elapsed() >= BANNER_LIFETIME {
                        self.banner = None;
                        self.banner_timer = None;
                    } else {
                        ctx.request_repaint_after(Duration::from_millis(500));
                    }
                }
            }
            None => self.banner_timer = None,
        }
    }

    fn save_bool_setting(&mut self, key: &str, value: bool) {
        if let Err(err) = self
            .db
            .save_setting(key, if value { "true" } else { "false" })
        {
            self.banner = Some(format!("Failed to save setting: {err}"));
        }
    }

    fn save_font_size_setting(&mut self) {
        if let Err(err) = self
            .db
            .save_setting("font_size", &format!("{:.0}", self.font_size))
        {
            self.banner = Some(format!("Failed to save setting: {err}"));
        }
    }

    fn save_api_key(&mut self) {
        let openai_key = self.api_key_draft.trim().to_owned();
        let gemini_key = self.gemini_api_key_draft.trim().to_owned();
        let anthropic_key = self.anthropic_api_key_draft.trim().to_owned();
        for (setting, value) in [
            ("api_key", &openai_key),
            ("gemini_api_key", &gemini_key),
            ("anthropic_api_key", &anthropic_key),
        ] {
            if let Err(err) = self.db.save_setting(setting, value) {
                self.banner = Some(format!("Failed to save API key: {err}"));
                return;
            }
        }

        self.api_key = openai_key;
        self.gemini_api_key = gemini_key;
        self.anthropic_api_key = anthropic_key;
        self.show_api_key_modal = false;
        self.banner = if self.api_key.is_empty()
            && self.gemini_api_key.is_empty()
            && self.anthropic_api_key.is_empty()
        {
            Some("API keys cleared.".to_owned())
        } else {
            Some("API keys saved.".to_owned())
        };
        self.resume_pending_turns();
    }

    fn api_key_for(&self, provider: Provider) -> &str {
        match provider {
            Provider::OpenAi => &self.api_key,
            Provider::Gemini => &self.gemini_api_key,
            Provider::Anthropic => &self.anthropic_api_key,
        }
    }

    fn require_api_key(&mut self, model: &str) -> Option<String> {
        let provider = Provider::of(model);
        let api_key = self.api_key_for(provider).to_owned();
        if !api_key.trim().is_empty() {
            return Some(api_key);
        }
        self.banner = Some(format!("Set your {} API key first.", provider.name()));
        self.open_api_key_modal();
        None
    }

    fn open_api_key_modal(&mut self) {
        self.api_key_draft = self.api_key.clone();
        self.gemini_api_key_draft = self.gemini_api_key.clone();
        self.anthropic_api_key_draft = self.anthropic_api_key.clone();
        self.show_api_key_modal = true;
    }

    fn save_selected_model_setting(&mut self) {
        let normalized = openai::normalize_model(&self.selected_model);
        if normalized != self.selected_model {
            self.selected_model = normalized;
        }

        if let Err(err) = self.db.save_setting("selected_model", &self.selected_model) {
            self.banner = Some(format!("Failed to save selected model: {err}"));
        }
    }

    fn save_conversation_max_output_tokens(&mut self, conversation_id: i64, value: i64) {
        let clamped = value.clamp(1_024, providers::MAX_OUTPUT_TOKENS);
        if let Some(conversation) = self
            .conversations
            .iter_mut()
            .find(|conversation| conversation.id == conversation_id)
        {
            conversation.max_output_tokens = clamped;
        }

        if let Err(err) = self
            .db
            .update_conversation_max_output_tokens(conversation_id, clamped)
        {
            self.banner = Some(format!("Failed to save answer length: {err}"));
        }
    }
}

impl eframe::App for ProGuiApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        if (self.applied_font_size - self.font_size).abs() > f32::EPSILON {
            apply_font_size(ctx, self.font_size);
            self.applied_font_size = self.font_size;
        }
        let big_text_focused = self.composer_editor.focused
            || self.block_editor.focused
            || self
                .editing_turn
                .as_ref()
                .is_some_and(|edit| edit.editor.focused);
        if !big_text_focused
            && ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::F))
        {
            self.focus_in_chat_search = true;
        }
        let modal_open = self.active_modal.is_some() || self.show_api_key_modal;
        if !modal_open
            && ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::N))
        {
            self.create_conversation(None);
        }
        if ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::K)) {
            if matches!(self.active_modal, Some(AppModal::QuickSwitcher { .. })) {
                self.active_modal = None;
            } else if !self.show_api_key_modal {
                self.open_modal(AppModal::QuickSwitcher {
                    query: String::new(),
                    selected: 0,
                });
            }
        }
        if !modal_open
            && ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::Slash))
        {
            self.open_modal(AppModal::Shortcuts);
        }
        if !modal_open
            && ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::Comma))
        {
            self.show_settings = !self.show_settings;
        }
        if !modal_open
            && ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::E))
        {
            let open = !self.show_context_panel;
            self.set_context_panel(open);
            self.context_filter_needs_focus = open;
        }
        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        if !dropped.is_empty() {
            self.add_dropped_files(dropped);
        }
        let paste_released = ctx.input(|input| {
            input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::V,
                        pressed: false,
                        modifiers,
                        ..
                    } if modifiers.command
                )
            })
        });
        if !modal_open && paste_released {
            self.try_attach_clipboard_image(ctx);
        }
        self.intercept_large_pastes(ctx);
        self.process_events();
        self.expire_banner(ctx);
        self.ensure_attachment_textures(ctx);
        self.ensure_selection_token_counts(ctx);
        self.render_sidebar(ctx);
        self.render_context_panel(ctx);
        self.render_header(ctx);
        self.render_composer(ctx);
        self.render_main(ctx);
        self.render_settings_window(ctx);
        self.render_api_key_modal(ctx);
        self.render_active_modal(ctx);
        self.render_block_viewer(ctx);
        if ctx.input(|input| !input.raw.hovered_files.is_empty()) {
            egui::Area::new(egui::Id::new("drop_overlay"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    Frame::default()
                        .fill(Color32::from_rgb(16, 52, 92))
                        .stroke(Stroke::new(1.5_f32, Color32::from_rgb(122, 196, 255)))
                        .corner_radius(egui::CornerRadius::same(12))
                        .inner_margin(Margin::same(24))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("Drop images or PDFs to attach")
                                    .size(20.0)
                                    .color(Color32::from_rgb(226, 237, 245)),
                            );
                        });
                });
        }
        if !self.runtime.is_empty() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
}

fn database_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let base = if cfg!(target_os = "macos") {
            PathBuf::from(&home).join("Library/Application Support/Pro GUI")
        } else {
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(&home).join(".local/share"))
                .join("pro-gui")
        };
        if std::fs::create_dir_all(&base).is_ok() {
            return base.join("pro_gui.sqlite3");
        }
    }
    PathBuf::from("pro_gui.sqlite3")
}

fn relative_time(value: &str) -> String {
    if value.len() >= 16 {
        return value[5..16].to_owned();
    }
    value.to_owned()
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_owned();
    }
    let mut text = value.chars().take(max_chars).collect::<String>();
    text.push_str("...");
    text
}

fn truncate_single_line_text(value: &str, max_chars: usize) -> String {
    let mut text = String::new();

    for (count, ch) in value.chars().enumerate() {
        let ch = match ch {
            '\n' | '\r' => ' ',
            _ => ch,
        };
        if count >= max_chars {
            text.push_str("...");
            return text;
        }
        text.push(ch);
    }

    text
}

fn normalize_search_query(value: &str) -> String {
    value.trim().to_lowercase()
}

fn is_search_active(query: &str) -> bool {
    !query.trim().is_empty()
}
