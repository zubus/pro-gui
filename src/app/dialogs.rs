use super::*;

impl ProGuiApp {
    pub(super) fn render_settings_window(&mut self, ctx: &Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        if ctx.input(|input| input.key_pressed(egui::Key::Escape))
            && ctx.memory(|memory| memory.focused().is_none())
        {
            open = false;
        }
        let mut changed: Vec<(&'static str, bool)> = Vec::new();
        let mut paste_changed = false;
        let mut font_changed = false;
        let mut open_keys = false;

        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(480.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_width(480.0);
                ui.spacing_mut().item_spacing.y = 4.0;

                settings_section(ui, "Answers");
                if setting_toggle(
                    ui,
                    &mut self.render_markdown_answers,
                    true,
                    "Markdown and code rendering",
                    "Format answers and thinking as Markdown with code blocks.",
                ) {
                    changed.push(("render_markdown_answers", self.render_markdown_answers));
                }
                if setting_toggle(
                    ui,
                    &mut self.render_latex_blocks,
                    self.render_markdown_answers,
                    "LaTeX math blocks",
                    "Typeset math blocks. Needs Markdown rendering.",
                ) {
                    changed.push(("render_latex_blocks", self.render_latex_blocks));
                }
                if setting_toggle(
                    ui,
                    &mut self.show_request_chips,
                    true,
                    "Show request parameter chips",
                    "Echo the exact parameters each turn was sent with.",
                ) {
                    changed.push(("show_request_chips", self.show_request_chips));
                }

                settings_section(ui, "Sending");
                if setting_toggle(
                    ui,
                    &mut self.enter_sends,
                    true,
                    "Enter sends the prompt",
                    "Shift+Enter inserts a newline. When off, Cmd+Enter sends.",
                ) {
                    changed.push(("enter_sends", self.enter_sends));
                }
                if setting_toggle(
                    ui,
                    &mut self.use_priority_processing,
                    true,
                    "Priority processing by default",
                    "service_tier=priority for new chats: faster, premium pricing. Each message can override it in the composer.",
                ) {
                    changed.push(("use_priority_processing", self.use_priority_processing));
                }
                if setting_toggle(
                    ui,
                    &mut self.auto_resend_failed_answers,
                    true,
                    "Auto resend once on failure",
                    "Send a failed turn again once, when it failed before any answer text arrived.",
                ) {
                    changed.push(("auto_resend_failed_answers", self.auto_resend_failed_answers));
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Paste becomes a block over").color(TEXT_BODY));
                    paste_changed = ui
                        .add(
                            egui::DragValue::new(&mut self.paste_block_kb)
                                .range(1..=4_096)
                                .suffix(" KB")
                                .speed(4),
                        )
                        .changed();
                });
                ui.label(
                    RichText::new(
                        "Larger pastes ride along as an editable block instead of filling the text box.",
                    )
                    .small()
                    .color(TEXT_MUTED),
                );

                settings_section(ui, "Appearance");
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Font size").color(TEXT_BODY));
                    ui.add_space(8.0);
                    font_changed = ui
                        .add(
                            Slider::new(&mut self.font_size, 11.0..=22.0)
                                .step_by(1.0)
                                .fixed_decimals(0)
                                .suffix(" pt"),
                        )
                        .changed();
                });

                settings_section(ui, "API keys");
                ui.horizontal(|ui| {
                    for (name, key) in [
                        ("OpenAI", &self.api_key),
                        ("Gemini", &self.gemini_api_key),
                        ("Anthropic", &self.anthropic_api_key),
                    ] {
                        let stored = !key.trim().is_empty();
                        key_status_dot(ui, name, stored);
                        ui.label(
                            RichText::new(format!(
                                "{name} {}",
                                if stored { "stored" } else { "missing" }
                            ))
                            .color(if stored { TEXT_BODY } else { KEY_MISSING }),
                        );
                        ui.add_space(10.0);
                    }
                });
                ui.add_space(6.0);
                if ui.button("Manage API keys...").clicked() {
                    open_keys = true;
                }
            });

        for (key, value) in changed {
            self.save_bool_setting(key, value);
        }
        if paste_changed {
            let value = self.paste_block_kb.to_string();
            if let Err(err) = self.db.save_setting("paste_block_kb", &value) {
                self.banner = Some(format!("Failed to save setting: {err}"));
            }
        }
        if font_changed {
            self.save_font_size_setting();
        }
        if open_keys {
            self.show_settings = false;
            self.open_api_key_modal();
            self.active_modal = None;
        } else {
            self.show_settings = open;
        }
    }

    pub(super) fn render_active_modal(&mut self, ctx: &Context) {
        let Some(mut modal) = self.active_modal.take() else {
            return;
        };

        let mut open = true;
        let mut save = false;
        let mut cancel = ctx.input(|input| input.key_pressed(egui::Key::Escape))
            && ctx.memory(|memory| memory.focused().is_none());
        let window_title = match &modal {
            AppModal::Folder {
                folder_id: Some(_), ..
            } => "Rename folder",
            AppModal::Folder {
                folder_id: None, ..
            } => "New folder",
            AppModal::RenameChat { .. } => "Rename chat",
            AppModal::ConfirmDeleteChat { .. } => "Delete chat",
            AppModal::SystemPrompt { .. } => "System prompt",
            AppModal::QuickSwitcher { .. } => "Switch chat",
            AppModal::Shortcuts => "Keyboard shortcuts",
            AppModal::ConfirmSecrets { .. } => "Possible secrets in context",
        };
        let line_edit_hint = match &modal {
            AppModal::Folder { .. } => "Folder name...",
            _ => "Chat title...",
        };
        let default_width = match &modal {
            AppModal::SystemPrompt { .. } => 540.0,
            AppModal::QuickSwitcher { .. }
            | AppModal::Shortcuts
            | AppModal::ConfirmSecrets { .. } => 460.0,
            _ => 380.0,
        };
        let (anchor, anchor_offset) = if matches!(modal, AppModal::QuickSwitcher { .. }) {
            (egui::Align2::CENTER_TOP, [0.0, 110.0])
        } else {
            (egui::Align2::CENTER_CENTER, [0.0, 0.0])
        };

        egui::Window::new(window_title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(default_width)
            .anchor(anchor, anchor_offset)
            .show(ctx, |ui| match &mut modal {
                AppModal::Folder { draft, .. } | AppModal::RenameChat { draft, .. } => {
                    let response = ui.add_sized(
                        [340.0, 26.0],
                        TextEdit::singleline(draft).hint_text(line_edit_hint),
                    );
                    if self.modal_needs_focus {
                        response.request_focus();
                        self.modal_needs_focus = false;
                    }
                    save |= response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            save = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                }
                AppModal::ConfirmDeleteChat {
                    title: chat_title,
                    turn_count,
                    ..
                } => {
                    ui.label(format!(
                        "This permanently deletes \"{chat_title}\" and its {turn_count} turn(s)."
                    ));
                    ui.label(
                        RichText::new("This cannot be undone.")
                            .small()
                            .color(Color32::from_rgb(255, 151, 143)),
                    );
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button(
                                RichText::new("Delete").color(Color32::from_rgb(255, 151, 143)),
                            )
                            .clicked()
                        {
                            save = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                }
                AppModal::SystemPrompt { draft, .. } => {
                    ui.label(
                        RichText::new(
                            "Sent as `instructions` with every request in this chat. Leave empty to disable.",
                        )
                        .small()
                        .color(Color32::from_rgb(188, 197, 206)),
                    );
                    ui.add_space(8.0);
                    ScrollArea::vertical()
                        .id_salt("system_prompt_scroll")
                        .max_height(240.0)
                        .show(ui, |ui| {
                            let response = ui.add_sized(
                                [500.0, 200.0],
                                TextEdit::multiline(draft)
                                    .hint_text("You are an expert software engineer..."),
                            );
                            if self.modal_needs_focus {
                                response.request_focus();
                                self.modal_needs_focus = false;
                            }
                        });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            save = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                }
                AppModal::QuickSwitcher { query, selected } => {
                    let response = ui.add_sized(
                        [400.0, 26.0],
                        TextEdit::singleline(query).hint_text("Search chats..."),
                    );
                    if self.modal_needs_focus {
                        response.request_focus();
                        self.modal_needs_focus = false;
                    }
                    if response.changed() {
                        *selected = 0;
                    }

                    Self::refresh_search(&self.db, &mut self.switcher_matches, query);
                    let matched_ids = self.switcher_matches.ids.as_ref();
                    let rows = switcher_rows(
                        &self.conversations,
                        &self.folders,
                        query,
                        matched_ids,
                    );
                    if !rows.is_empty() {
                        if ui.input(|input| input.key_pressed(egui::Key::ArrowDown)) {
                            *selected = (*selected + 1) % rows.len();
                        }
                        if ui.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
                            *selected = if *selected == 0 {
                                rows.len() - 1
                            } else {
                                *selected - 1
                            };
                        }
                        *selected = (*selected).min(rows.len() - 1);
                    }

                    let mut chosen = None;
                    ui.add_space(6.0);
                    ScrollArea::vertical()
                        .id_salt("quick_switcher_scroll")
                        .max_height(340.0)
                        .show(ui, |ui| {
                            if rows.is_empty() {
                                ui.label(
                                    RichText::new("No chats match.")
                                        .italics()
                                        .color(Color32::from_rgb(142, 152, 162)),
                                );
                            }
                            for (index, (conversation_id, label, sub)) in
                                rows.iter().enumerate()
                            {
                                let row = ui.selectable_label(
                                    index == *selected,
                                    format!("{label}\n{sub}"),
                                );
                                if row.clicked() {
                                    chosen = Some(*conversation_id);
                                }
                            }
                        });

                    if ui.input(|input| input.key_pressed(egui::Key::Enter))
                        && let Some((conversation_id, _, _)) = rows.get(*selected)
                    {
                        chosen = Some(*conversation_id);
                    }
                    if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                        cancel = true;
                    }
                    if let Some(conversation_id) = chosen {
                        self.select_conversation(conversation_id);
                        cancel = true;
                    }
                }
                AppModal::ConfirmSecrets { hits, total, .. } => {
                    ui.label(
                        RichText::new(format!(
                            "The packed repository context contains {total} line(s) that look like credentials:"
                        ))
                        .color(Color32::from_rgb(255, 209, 128)),
                    );
                    ui.add_space(8.0);
                    ScrollArea::vertical()
                        .id_salt("secret_hits_scroll")
                        .max_height(220.0)
                        .show(ui, |ui| {
                            for hit in hits.iter() {
                                ui.label(
                                    RichText::new(hit)
                                        .monospace()
                                        .small()
                                        .color(Color32::from_rgb(255, 151, 143)),
                                );
                            }
                            if *total > hits.len() {
                                ui.label(
                                    RichText::new(format!("…and {} more", *total - hits.len()))
                                        .small()
                                        .italics()
                                        .color(Color32::from_rgb(142, 152, 162)),
                                );
                            }
                        });
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(
                            "Sending uploads these values to the selected model's API. Deselect the files, add them to .proguiignore, or send anyway.",
                        )
                        .small()
                        .color(Color32::from_rgb(188, 197, 206)),
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button(
                                RichText::new("Send anyway")
                                    .color(Color32::from_rgb(255, 151, 143)),
                            )
                            .clicked()
                        {
                            save = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                }
                AppModal::Shortcuts => {
                    egui::Grid::new("shortcuts_grid")
                        .num_columns(2)
                        .spacing([24.0, 6.0])
                        .show(ui, |ui| {
                            let shortcut = |ui: &mut egui::Ui, keys: &str, what: &str| {
                                ui.label(
                                    RichText::new(keys)
                                        .monospace()
                                        .color(Color32::from_rgb(188, 217, 240)),
                                );
                                ui.label(
                                    RichText::new(what)
                                        .color(Color32::from_rgb(210, 218, 226)),
                                );
                                ui.end_row();
                            };
                            shortcut(ui, "Cmd+Enter", "Send the prompt");
                            shortcut(
                                ui,
                                "Enter",
                                "Send when \"Enter sends\" is on (Shift+Enter = newline)",
                            );
                            shortcut(ui, "Cmd+N", "New chat");
                            shortcut(ui, "Cmd+K", "Switch chat (fuzzy search)");
                            shortcut(ui, "Cmd+F", "Find in the open chat");
                            shortcut(ui, "Cmd+/", "This help");
                            shortcut(ui, "Cmd+,", "Settings");
                            shortcut(ui, "Cmd+E", "Repository context panel");
                            shortcut(
                                ui,
                                "Cmd+Enter",
                                "Resend the message being edited (Esc cancels)",
                            );
                            shortcut(ui, "Esc", "Unfocus field, then close dialog");
                        });
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(
                            "Tips: right-click a chat row or folder header (or use the … button) for rename/move/delete. \"Edit\" on any sent message rewrites it in place. \"Resend\" reruns from there and drops the replies below, \"Send as new message\" keeps the history. The settings row above the prompt applies per message and inherits from the previous turn. Background runs keep going even if you quit, and reopening resumes polling and notifies on completion.",
                        )
                        .small()
                        .color(Color32::from_rgb(170, 184, 197)),
                    );
                    ui.add_space(10.0);
                    if ui.button("Close").clicked() {
                        cancel = true;
                    }
                }
            });

        if save {
            self.apply_modal(modal);
        } else if open && !cancel {
            self.active_modal = Some(modal);
        }
    }

    pub(super) fn render_api_key_modal(&mut self, ctx: &Context) {
        if !self.show_api_key_modal {
            return;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Escape))
            && ctx.memory(|memory| memory.focused().is_none())
        {
            self.show_api_key_modal = false;
            return;
        }

        let mut open = self.show_api_key_modal;
        egui::Window::new("API Keys")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(480.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(
                        "Stored in the local SQLite database. Each model uses its own API's key.",
                    )
                    .small()
                    .color(Color32::from_rgb(188, 197, 206)),
                );
                for (name, draft, hint) in [
                    (
                        "OpenAI",
                        &mut self.api_key_draft,
                        "Paste your OpenAI API key here...",
                    ),
                    (
                        "Gemini",
                        &mut self.gemini_api_key_draft,
                        "Paste your Gemini API key (from Google AI Studio) here...",
                    ),
                    (
                        "Anthropic",
                        &mut self.anthropic_api_key_draft,
                        "Paste your Anthropic API key here...",
                    ),
                ] {
                    ui.add_space(8.0);
                    ui.label(RichText::new(name).strong());
                    ui.add_sized(
                        [440.0, 48.0],
                        TextEdit::multiline(draft).desired_rows(2).hint_text(hint),
                    );
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        self.save_api_key();
                    }
                    if ui.button("Close").clicked() {
                        self.show_api_key_modal = false;
                    }
                });
            });
        self.show_api_key_modal = open && self.show_api_key_modal;
    }

    pub(super) fn open_modal(&mut self, modal: AppModal) {
        self.active_modal = Some(modal);
        self.modal_needs_focus = true;
        self.show_api_key_modal = false;
    }

    pub(super) fn apply_modal(&mut self, modal: AppModal) {
        match modal {
            AppModal::Folder {
                folder_id,
                draft,
                move_conversation_id,
            } => {
                let name = draft.trim().to_owned();
                if name.is_empty() {
                    self.banner = Some("Folder name cannot be empty.".to_owned());
                    self.active_modal = Some(AppModal::Folder {
                        folder_id,
                        draft,
                        move_conversation_id,
                    });
                    return;
                }
                match folder_id {
                    Some(folder_id) => {
                        if let Err(err) = self.db.rename_folder(folder_id, &name) {
                            self.banner = Some(format!("Failed to rename folder: {err}"));
                            return;
                        }
                        if let Some(folder) = self
                            .folders
                            .iter_mut()
                            .find(|folder| folder.id == folder_id)
                        {
                            folder.name = name;
                        }
                    }
                    None => match self.db.create_folder(&name) {
                        Ok(new_folder_id) => {
                            self.folders.push(Folder {
                                id: new_folder_id,
                                name,
                            });
                            if let Some(conversation_id) = move_conversation_id {
                                self.move_conversation_to_folder(
                                    conversation_id,
                                    Some(new_folder_id),
                                );
                            }
                        }
                        Err(err) => {
                            self.banner = Some(format!("Failed to create folder: {err}"));
                            return;
                        }
                    },
                }
                self.sort_folders();
            }
            AppModal::RenameChat {
                conversation_id,
                draft,
            } => {
                let title = draft.trim().to_owned();
                if title.is_empty() {
                    self.banner = Some("Chat title cannot be empty.".to_owned());
                    self.active_modal = Some(AppModal::RenameChat {
                        conversation_id,
                        draft,
                    });
                    return;
                }
                if let Err(err) = self.db.rename_conversation(conversation_id, &title) {
                    self.banner = Some(format!("Failed to rename chat: {err}"));
                    return;
                }
                if let Some(conversation) = self
                    .conversations
                    .iter_mut()
                    .find(|conversation| conversation.id == conversation_id)
                {
                    conversation.rename(title);
                }
            }
            AppModal::ConfirmDeleteChat {
                conversation_id, ..
            } => {
                self.delete_conversation(conversation_id);
            }
            AppModal::QuickSwitcher { .. } | AppModal::Shortcuts => {}
            AppModal::ConfirmSecrets { .. } => {
                self.allow_secrets_once = true;
                if self.submit_prompt()
                    && let Some((draft, blocks)) = self.parked_composer.take()
                {
                    self.composer = draft;
                    self.composer_blocks = blocks;
                }
            }
            AppModal::SystemPrompt {
                conversation_id,
                draft,
            } => {
                let system_prompt = draft.trim().to_owned();
                if let Err(err) = self
                    .db
                    .update_conversation_system_prompt(conversation_id, &system_prompt)
                {
                    self.banner = Some(format!("Failed to save system prompt: {err}"));
                    return;
                }
                if let Some(conversation) = self
                    .conversations
                    .iter_mut()
                    .find(|conversation| conversation.id == conversation_id)
                {
                    conversation.system_prompt = system_prompt;
                }
            }
        }
    }
}

pub(super) fn switcher_rows(
    conversations: &[Conversation],
    folders: &[Folder],
    query: &str,
    matched_ids: Option<&HashSet<i64>>,
) -> Vec<(i64, String, String)> {
    let needle = normalize_search_query(query);
    let mut scored = conversations
        .iter()
        .enumerate()
        .filter_map(|(index, conversation)| {
            let score = if needle.is_empty() || conversation.title.to_lowercase().contains(&needle)
            {
                0
            } else if matched_ids.is_some_and(|ids| ids.contains(&conversation.id)) {
                1
            } else {
                return None;
            };
            Some((score, index, conversation))
        })
        .collect::<Vec<_>>();
    scored.sort_by_key(|(score, index, _)| (*score, *index));

    scored
        .into_iter()
        .take(12)
        .map(|(_, _, conversation)| {
            let mut sub = relative_time(&conversation.updated_at);
            if let Some(folder) = conversation
                .folder_id
                .and_then(|id| folders.iter().find(|folder| folder.id == id))
            {
                sub.push_str("  |  ");
                sub.push_str(&folder.name);
            }
            (
                conversation.id,
                truncate_single_line_text(&conversation.title, 52),
                sub,
            )
        })
        .collect()
}

pub(super) fn settings_section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).strong().color(TEXT_STRONG));
    ui.add_space(2.0);
}

pub(super) fn setting_toggle(
    ui: &mut egui::Ui,
    value: &mut bool,
    enabled: bool,
    title: &str,
    detail: &str,
) -> bool {
    let changed = ui
        .add_enabled(
            enabled,
            egui::Checkbox::new(value, RichText::new(title).color(TEXT_BODY)),
        )
        .changed();
    ui.horizontal(|ui| {
        ui.add_space(ui.spacing().icon_width + ui.spacing().icon_spacing);
        ui.add(egui::Label::new(RichText::new(detail).small().color(TEXT_MUTED)).wrap());
    });
    ui.add_space(4.0);
    changed
}
