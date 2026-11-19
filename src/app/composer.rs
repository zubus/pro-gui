use std::fmt::Write as _;

use super::*;

impl ProGuiApp {
    pub(super) fn render_composer(&mut self, ctx: &Context) {
        let mut editor = std::mem::take(&mut self.composer_editor);
        let mut send_requested = false;
        let mut composer_focused = false;
        TopBottomPanel::bottom("composer")
            .frame(Frame::default().fill(CANVAS).inner_margin(Margin {
                left: 0,
                right: 0,
                top: 6,
                bottom: 14,
            }))
            .show_separator_line(false)
            .show(ctx, |ui| {
                let full_width = ui.available_width();
                let content_width = column_width(full_width);
                let side_margin = ((full_width - content_width) / 2.0).max(0.0);
                ui.horizontal(|ui| {
                    ui.add_space(side_margin);
                    ui.vertical(|ui| {
                        ui.set_width(content_width);
                        Frame::default()
                            .fill(CARD_FILL)
                            .stroke(Stroke::new(1.0_f32, CARD_STROKE))
                            .corner_radius(egui::CornerRadius::same(14))
                            .inner_margin(Margin::symmetric(14, 12))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                if !self.pending_attachments.is_empty() {
                                    self.render_pending_attachments(ui);
                                    ui.add_space(6.0);
                                }
                                if !self.composer_blocks.is_empty() {
                                    self.render_prompt_blocks(ui);
                                    ui.add_space(6.0);
                                }

                                let hint_text = format!(
                                    "Message {}...  (paste or drop images and PDFs to attach)",
                                    providers::model_display_name(&self.composer_settings.model)
                                );
                                let composer_height = 88.0;
                                let return_key = self.enter_sends.then(|| {
                                    egui::KeyboardShortcut::new(
                                        egui::Modifiers::SHIFT,
                                        egui::Key::Enter,
                                    )
                                });
                                let response = ui
                                    .allocate_ui_with_layout(
                                        egui::vec2(ui.available_width(), composer_height),
                                        Layout::top_down(Align::Min),
                                        |ui| {
                                            render_big_text(
                                                ui,
                                                &mut self.composer,
                                                &mut editor,
                                                "composer_input",
                                                composer_height,
                                                return_key,
                                                &hint_text,
                                            )
                                        },
                                    )
                                    .inner;
                                let composer_has_focus = response.has_focus() || editor.focused;
                                if bigtext::needs_document_editor(&self.composer) {
                                    ui.add_space(4.0);
                                    render_big_text_note(ui, &self.composer, &editor, &response);
                                }
                                ui.add_space(8.0);

                                let mut send_clicked = false;
                                let send_hint = if self.enter_sends {
                                    "Enter to send"
                                } else {
                                    "Cmd+Enter to send"
                                };
                                let show_hint = ui.available_width() >= 1_000.0;
                                ui.horizontal(|ui| {
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        send_clicked = ui
                                            .add(
                                                egui::Button::new(
                                                    RichText::new("Send")
                                                        .strong()
                                                        .color(Color32::WHITE),
                                                )
                                                .fill(ACCENT)
                                                .corner_radius(egui::CornerRadius::same(8))
                                                .min_size(egui::vec2(84.0, 30.0)),
                                            )
                                            .on_hover_text(send_hint)
                                            .clicked();
                                        if show_hint {
                                            ui.label(
                                                RichText::new(send_hint).small().color(TEXT_FAINT),
                                            );
                                        }
                                        ui.add_space(8.0);
                                        ui.with_layout(
                                            Layout::left_to_right(Align::Center)
                                                .with_main_wrap(true),
                                            |ui| self.render_composer_settings(ui),
                                        );
                                    });
                                });

                                let enter_sends = self.enter_sends;
                                let submit_shortcut = composer_has_focus
                                    && ui.input(|input| {
                                        let enter_pressed = input.key_pressed(egui::Key::Enter);
                                        if enter_sends {
                                            enter_pressed && !input.modifiers.shift
                                        } else {
                                            enter_pressed && input.modifiers.command
                                        }
                                    });

                                send_requested = send_clicked || submit_shortcut;
                                composer_focused = composer_has_focus;
                            });
                    });
                });
            });

        self.composer_editor = editor;
        self.composer_focused = composer_focused;
        if send_requested {
            self.submit_prompt();
            self.composer_editor = DocumentEditor::default();
        }
    }

    pub(super) fn render_prompt_blocks(&mut self, ui: &mut egui::Ui) {
        if let Some(action) = render_block_chips(ui, &self.composer_blocks) {
            match action {
                BlockAction::Edit(index) => {
                    self.viewing_block = Some((BlockOwner::Composer, index));
                    self.block_editor = DocumentEditor::default();
                }
                BlockAction::Remove(index) => {
                    self.composer_blocks.remove(index);
                    self.viewing_block = None;
                    self.block_editor = DocumentEditor::default();
                }
            }
        }
    }

    pub(super) fn render_block_viewer(&mut self, ctx: &Context) {
        let Some((owner, index)) = self.viewing_block else {
            return;
        };
        let block = match owner {
            BlockOwner::Composer => self.composer_blocks.get_mut(index),
            BlockOwner::TurnEdit => self
                .editing_turn
                .as_mut()
                .and_then(|edit| edit.blocks.get_mut(index)),
        };
        let Some(block) = block else {
            self.viewing_block = None;
            self.block_editor = DocumentEditor::default();
            return;
        };

        let mut open = true;
        let editor = &mut self.block_editor;
        let title = format!("Block: {}", block.label);
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .default_width(880.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let before = (block.text.len(), editor.revision);
                let response =
                    render_big_text(ui, &mut block.text, editor, "block-viewer", 460.0, None, "");
                if response.changed() || before != (block.text.len(), editor.revision) {
                    block.refresh_lines();
                }
                if bigtext::needs_document_editor(&block.text) {
                    ui.add_space(4.0);
                    render_big_text_note(ui, &block.text, editor, &response);
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(block.text.clone());
                    }
                    ui.label(
                        RichText::new(
                            "Edits are kept; the block is folded into the message when you send.",
                        )
                        .small()
                        .color(Color32::from_rgb(150, 160, 172)),
                    );
                });
            });

        if !open {
            self.viewing_block = None;
            self.block_editor = DocumentEditor::default();
        }
    }

    pub(super) fn set_composer_model(&mut self, model: String) {
        self.composer_settings = composer_settings_for_model(&self.composer_settings, model);
    }

    pub(super) fn render_composer_settings(&mut self, ui: &mut egui::Ui) {
        let selected_label = providers::model_display_name(&self.composer_settings.model);
        let mut open_custom_input = false;
        egui::ComboBox::from_id_salt("composer_model")
            .width(170.0)
            .selected_text(RichText::new(selected_label).monospace().small())
            .show_ui(ui, |ui| {
                ui.set_min_width(260.0);
                for (index, group) in providers::model_groups().into_iter().enumerate() {
                    if index > 0 {
                        ui.separator();
                    }
                    for option in group {
                        let is_selected = self.composer_settings.model == option.id;
                        if ui
                            .selectable_label(
                                is_selected,
                                format!("{}: {}", option.label, option.detail),
                            )
                            .on_hover_text(option.id)
                            .clicked()
                            && !is_selected
                        {
                            self.set_composer_model(option.id.to_owned());
                        }
                    }
                }
                ui.separator();
                if ui.button("Custom model ID...").clicked() {
                    open_custom_input = true;
                }
            })
            .response
            .on_hover_text(format!("API model: {}", self.composer_settings.model));
        if open_custom_input {
            self.custom_model_input = Some(self.composer_settings.model.clone());
            self.custom_model_needs_focus = true;
        }

        if let Some(mut draft) = self.custom_model_input.take() {
            let response = ui.add_sized(
                [170.0, 22.0],
                TextEdit::singleline(&mut draft).hint_text("model-id..."),
            );
            if self.custom_model_needs_focus {
                response.request_focus();
                self.custom_model_needs_focus = false;
            }
            let apply = ui.small_button("Use").clicked()
                || (response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)));
            let dismiss = ui.small_button("Cancel").clicked()
                || ui.input(|input| input.key_pressed(egui::Key::Escape));
            if apply && !dismiss && !draft.trim().is_empty() {
                self.set_composer_model(openai::normalize_model(&draft));
            } else if !dismiss {
                self.custom_model_input = Some(draft);
            }
        }

        ui.add_space(6.0);
        ui.label(RichText::new("effort").small().color(TEXT_MUTED));
        egui::ComboBox::from_id_salt("composer_effort")
            .width(76.0)
            .selected_text(
                RichText::new(self.composer_settings.effort.clone())
                    .monospace()
                    .small(),
            )
            .show_ui(ui, |ui| {
                for effort in providers::effort_options(&self.composer_settings.model) {
                    ui.selectable_value(
                        &mut self.composer_settings.effort,
                        (*effort).to_owned(),
                        *effort,
                    );
                }
            });

        ui.add_space(6.0);
        ui.label(RichText::new("max tokens").small().color(TEXT_MUTED));
        let token_limit = providers::max_output_tokens(&self.composer_settings.model);
        let at_limit = self.composer_settings.max_output_tokens >= token_limit;
        ui.add(
                egui::DragValue::new(&mut self.composer_settings.max_output_tokens)
                    .range(1_024..=token_limit)
                    .speed(1_024)
                    .custom_formatter(|value, _| format_token_input(value as i64))
                    .custom_parser(move |text| parse_token_input(text, token_limit)),
            )
            .on_hover_text(format!(
                "max_output_tokens={} (this model takes up to {}). Type 128k, 1M or max.{}",
                self.composer_settings.max_output_tokens,
                format_token_input(token_limit),
                if token_limit > openai::MAX_OUTPUT_TOKENS {
                    "\nThinking and answer share this limit. Above 65,536 the run uses long decode: the server writes the answer in hops of up to 64K tokens, and a hard prompt can run for a long time."
                } else {
                    ""
                }
            ));
        if ui
            .add_enabled(!at_limit, egui::Button::new("Max").small())
            .on_hover_text("Use this model's whole output limit.")
            .clicked()
        {
            self.composer_settings.max_output_tokens = token_limit;
        }

        ui.add_space(6.0);
        let priority_available = providers::supports_priority(&self.composer_settings.model);
        ui.add_enabled(
            priority_available,
            egui::Checkbox::new(
                &mut self.composer_settings.priority,
                RichText::new("Priority").small(),
            ),
        )
        .on_hover_text("service_tier=priority: faster processing, premium pricing.")
        .on_disabled_hover_text(
            "This model has no Priority Tier, so it always runs at standard tier.",
        );

        ui.add_space(6.0);
        if ui
            .button(RichText::new("Attach").small())
            .on_hover_text("Attach images or PDFs, or paste them with Cmd+V, or drop them here.")
            .clicked()
        {
            self.attach_files_dialog();
        }
        self.render_repo_chip(ui);
    }

    pub(super) fn render_repo_chip(&mut self, ui: &mut egui::Ui) {
        let workspace = self
            .selected_conversation()
            .map(|conversation| conversation.workspace_path.clone())
            .unwrap_or_default();

        if workspace.is_empty() {
            if ui
                .add(
                    egui::Button::new(RichText::new("Context").small())
                        .selected(self.show_context_panel),
                )
                .on_hover_text("Pick a repository and the files packed into each message (Cmd+E).")
                .clicked()
            {
                self.set_context_panel(!self.show_context_panel);
            }
            return;
        }

        let folder_name = std::path::Path::new(&workspace)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| workspace.clone());
        let (files, tokens, all_cached) = self.context_selection_summary().unwrap_or((0, 0, true));
        let over = tokens >= repoctx::CONTEXT_TOKEN_WARN;
        let label = if files == 0 {
            format!("{}: no files", truncate_single_line_text(&folder_name, 20))
        } else {
            format!(
                "{}: {files} files \u{2248}{}{}",
                truncate_single_line_text(&folder_name, 20),
                format_compact_count(tokens as i64),
                if all_cached { "" } else { "+" }
            )
        };
        let mut rich = RichText::new(label).small();
        if over {
            rich = rich.color(Color32::from_rgb(255, 209, 128));
        } else if files > 0 {
            rich = rich.color(Color32::from_rgb(148, 219, 178));
        }

        let mut change_folder = false;
        let mut clear_selection = false;
        let mut detach = false;
        let response = ui
            .add(egui::Button::new(rich).selected(self.show_context_panel))
            .on_hover_text(if over {
                "Large context. Check the selection before sending (Cmd+E)."
            } else {
                "Show or hide the files packed into each message (Cmd+E)."
            });
        if response.clicked() {
            self.set_context_panel(!self.show_context_panel);
        }
        response.context_menu(|ui| {
            ui.set_min_width(160.0);
            if ui.button("Change folder…").clicked() {
                change_folder = true;
            }
            if ui.button("Clear selection").clicked() {
                clear_selection = true;
            }
            if ui.button("Detach repo").clicked() {
                detach = true;
            }
        });
        if change_folder {
            self.pick_workspace_folder();
        }
        if clear_selection && let Some(conversation_id) = self.selected_conversation_id {
            if let Some(conversation) = self.selected_conversation_mut() {
                conversation.context_selection.clear();
            }
            self.save_context_state(conversation_id);
        }
        if detach {
            self.detach_workspace();
        }
    }

    pub(super) fn render_pending_attachments(&mut self, ui: &mut egui::Ui) {
        let mut remove_index = None;
        ui.horizontal_wrapped(|ui| {
            for (index, attachment) in self.pending_attachments.iter().enumerate() {
                Frame::default()
                    .fill(Color32::from_rgb(26, 32, 40))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(51, 59, 69)))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(Margin::same(6))
                    .show(ui, |ui| {
                        if let Some(Some(texture)) = self.pending_textures.get(&attachment.key) {
                            ui.add(egui::Image::new(texture).max_size(egui::vec2(120.0, 72.0)));
                        } else if attachment.mime == "application/pdf" {
                            ui.label(
                                RichText::new("PDF")
                                    .small()
                                    .monospace()
                                    .strong()
                                    .background_color(Color32::from_rgb(138, 42, 42))
                                    .color(Color32::from_rgb(255, 224, 224)),
                            );
                        }
                        ui.label(
                            RichText::new(format!(
                                "{} ({})",
                                truncate_single_line_text(&attachment.file_name, 18),
                                format_bytes(attachment.bytes.len() as i64)
                            ))
                            .small()
                            .color(Color32::from_rgb(188, 197, 206)),
                        );
                        if ui
                            .small_button("×")
                            .on_hover_text("Remove attachment")
                            .clicked()
                        {
                            remove_index = Some(index);
                        }
                    });
            }
        });
        if let Some(index) = remove_index {
            let removed = self.pending_attachments.remove(index);
            self.pending_textures.remove(&removed.key);
        }
    }
    pub(super) fn stash_composer_draft(&mut self) {
        if let Some(current_id) = self.selected_conversation_id {
            if self.composer.trim().is_empty() {
                self.composer_drafts.remove(&current_id);
            } else {
                self.composer_drafts
                    .insert(current_id, std::mem::take(&mut self.composer));
            }
            let blocks = std::mem::take(&mut self.composer_blocks);
            if blocks.is_empty() {
                self.block_drafts.remove(&current_id);
            } else {
                self.block_drafts.insert(current_id, blocks);
            }
            let attachments = std::mem::take(&mut self.pending_attachments);
            if attachments.is_empty() {
                self.attachment_drafts.remove(&current_id);
            } else {
                self.attachment_drafts.insert(current_id, attachments);
            }
        }
        self.composer.clear();
        self.composer_blocks.clear();
        self.pending_attachments.clear();
        self.pending_textures.clear();
        self.viewing_block = None;
    }

    pub(super) fn intercept_large_pastes(&mut self, ctx: &Context) {
        if !self.composer_focused || self.selected_conversation_id.is_none() {
            return;
        }
        let paste_block_bytes = (self.paste_block_kb.max(1) as usize) * 1024;
        let mut captured = Vec::new();
        ctx.input_mut(|input| {
            input.events.retain(|event| match event {
                egui::Event::Paste(text)
                    if text.len() >= paste_block_bytes
                        || bigtext::count_lines(text) >= PASTE_BLOCK_LINES =>
                {
                    captured.push(text.clone());
                    false
                }
                _ => true,
            });
        });

        for text in captured {
            let label = paste_label(&text);
            self.banner = Some(format!(
                "Kept {} of pasted text as a block. It goes into the message on send, without slowing the editor.",
                format_bytes(text.len() as i64)
            ));
            self.composer_blocks.push(PromptBlock::new(label, text));
        }
    }

    pub(super) fn seed_composer_settings(&mut self) {
        let Some(conversation) = self.selected_conversation() else {
            return;
        };

        self.composer_settings = match conversation.turns.last() {
            Some(last_turn) => ComposerSettings {
                model: last_turn.model.clone(),
                effort: last_turn.reasoning_effort.clone(),
                max_output_tokens: providers::clamp_output_tokens(
                    &last_turn.model,
                    last_turn.max_output_tokens,
                ),
                priority: last_turn.service_tier == "priority",
            },
            None => ComposerSettings {
                model: self.selected_model.clone(),
                effort: providers::default_reasoning_effort(&self.selected_model).to_owned(),
                max_output_tokens: providers::max_output_tokens(&self.selected_model),
                priority: self.use_priority_processing,
            },
        };
        self.custom_model_input = None;
    }
}

pub(super) fn composer_settings_for_model(
    current: &ComposerSettings,
    model: String,
) -> ComposerSettings {
    ComposerSettings {
        effort: providers::default_reasoning_effort(&model).to_owned(),
        max_output_tokens: providers::max_output_tokens(&model),
        priority: current.priority,
        model,
    }
}

pub(super) fn format_token_input(value: i64) -> String {
    let trim = |text: String| {
        if text.contains('.') {
            text.trim_end_matches('0').trim_end_matches('.').to_owned()
        } else {
            text
        }
    };
    if value >= 1_000_000 {
        format!("{}M", trim(format!("{:.2}", value as f64 / 1_000_000.0)))
    } else if value >= 10_000 {
        format!("{}k", trim(format!("{:.1}", value as f64 / 1_000.0)))
    } else {
        value.to_string()
    }
}

pub(super) fn parse_token_input(text: &str, limit: i64) -> Option<f64> {
    let text = text
        .trim()
        .to_ascii_lowercase()
        .replace(['_', ',', ' '], "");
    if text == "max" {
        return Some(limit as f64);
    }
    let (number, scale) = if let Some(number) = text.strip_suffix('k') {
        (number, 1_000.0)
    } else if let Some(number) = text.strip_suffix('m') {
        (number, 1_000_000.0)
    } else {
        (text.as_str(), 1.0)
    };
    number
        .parse::<f64>()
        .ok()
        .map(|value| (value * scale).round())
}

pub(super) enum BlockAction {
    Edit(usize),
    Remove(usize),
}

pub(super) fn render_block_chips(ui: &mut egui::Ui, blocks: &[PromptBlock]) -> Option<BlockAction> {
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        for (index, block) in blocks.iter().enumerate() {
            Frame::default()
                .fill(Color32::from_rgb(28, 40, 54))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(44, 70, 98)))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(format!("\u{25a4} {}", block.label))
                            .small()
                            .color(Color32::from_rgb(198, 222, 244)),
                    );
                    ui.label(
                        RichText::new(format!(
                            "{}, {} lines",
                            format_bytes(block.text.len() as i64),
                            format_compact_count(block.lines() as i64)
                        ))
                        .small()
                        .monospace()
                        .color(Color32::from_rgb(150, 176, 200)),
                    );
                    if ui
                        .small_button("Edit")
                        .on_hover_text("Open the block to read and edit it.")
                        .clicked()
                    {
                        action = Some(BlockAction::Edit(index));
                    }
                    if ui
                        .small_button("\u{2715}")
                        .on_hover_text("Drop this block from the message.")
                        .clicked()
                    {
                        action = Some(BlockAction::Remove(index));
                    }
                });
        }
    });
    action
}

pub(super) fn paste_label(text: &str) -> String {
    let first = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
        .trim_start_matches(['#', '/', '*', '-', ' ']);
    if first.is_empty() {
        return "Pasted text".to_owned();
    }
    truncate_single_line_text(first, 44)
}

pub(super) fn render_big_text(
    ui: &mut egui::Ui,
    text: &mut String,
    editor: &mut DocumentEditor,
    salt: &'static str,
    height: f32,
    return_key: Option<egui::KeyboardShortcut>,
    hint: &str,
) -> egui::Response {
    if !bigtext::needs_document_editor(text) {
        let response = ScrollArea::vertical()
            .id_salt(salt)
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut widget = TextEdit::multiline(text)
                    .id_salt((salt, "edit"))
                    .desired_width(f32::INFINITY)
                    .desired_rows(5)
                    .hint_text(hint);
                if let Some(shortcut) = return_key {
                    widget = widget.return_key(shortcut);
                }
                ui.add(widget)
            })
            .inner;
        editor.focused = false;
        editor.history = bigtext::History::default();
        return response;
    }

    editor.return_key = return_key;
    if let Some(offset) = render_find_bar(ui, text, editor, salt) {
        let length = editor.find.query.trim().len();
        editor.reveal(text, offset, length);
    }
    editor.show(ui, text, height, salt)
}

pub(super) fn render_big_text_note(
    ui: &mut egui::Ui,
    text: &str,
    state: &DocumentEditor,
    editor: &egui::Response,
) {
    let mut note = format!(
        "{}, {} lines. Document mode: Cmd+F find, Cmd+A select all, Cmd+Shift+C copy all",
        format_bytes(text.len() as i64),
        format_compact_count(state.line_count() as i64),
    );
    if state.history.can_undo() {
        note.push_str(", Cmd+Z undo");
    }
    if state.history.can_redo() {
        note.push_str(", Cmd+Shift+Z redo");
    }
    let label = ui.label(
        RichText::new(note)
            .small()
            .color(Color32::from_rgb(150, 160, 172)),
    );
    editor.clone().labelled_by(label.id);
}

pub(super) fn render_find_bar(
    ui: &mut egui::Ui,
    text: &mut String,
    editor: &mut DocumentEditor,
    salt: &'static str,
) -> Option<usize> {
    if !editor.find.open {
        return None;
    }
    editor.find.refresh(text, editor.revision);

    let mut jump = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Find").small());
        let response = ui.add(
            TextEdit::singleline(&mut editor.find.query)
                .id_salt((salt, "find"))
                .desired_width(180.0)
                .hint_text("in the whole document"),
        );
        if editor.find.needs_focus {
            response.request_focus();
            editor.find.needs_focus = false;
        }
        if response.changed() {
            editor.find.refresh(text, editor.revision);
            editor.find.current = 0;
            jump = editor.find.matches.first().map(|(start, _)| *start);
        }

        let total = editor.find.matches.len();
        ui.label(
            RichText::new(if total == 0 {
                "no matches".to_owned()
            } else {
                format!(
                    "{} of {}",
                    editor.find.current + 1,
                    format_compact_count(total as i64)
                )
            })
            .small()
            .monospace()
            .color(Color32::from_rgb(150, 176, 200)),
        );
        if ui
            .small_button("\u{25b2}")
            .on_hover_text("Previous match")
            .clicked()
        {
            jump = editor.find.step(false).map(|(start, _)| start);
        }
        if ui
            .small_button("\u{25bc}")
            .on_hover_text("Next match")
            .clicked()
        {
            jump = editor.find.step(true).map(|(start, _)| start);
        }

        ui.separator();
        ui.label(RichText::new("Replace").small());
        ui.add(
            TextEdit::singleline(&mut editor.find.replacement)
                .id_salt((salt, "replace"))
                .desired_width(180.0),
        );
        if ui
            .small_button("Replace all")
            .on_hover_text("One undoable step across the whole document.")
            .clicked()
        {
            let query = editor.find.query.clone();
            let replacement = editor.find.replacement.clone();
            let count = bigtext::replace_all(text, &mut editor.history, &query, &replacement);
            if count > 0 {
                editor.invalidate();
                editor.sync(text);
                editor.revision += 1;
            }
            editor.find.refresh(text, editor.revision);
            jump = Some(0);
        }
        if ui.small_button("Close").clicked() {
            editor.find.open = false;
            editor.find.query.clear();
            editor.find.refresh(text, editor.revision);
        }
    });
    ui.add_space(6.0);
    jump
}

pub(super) fn assemble_blocks(
    blocks: &[PromptBlock],
    typed: &str,
) -> (String, usize, Vec<BlockRange>) {
    let mut prompt = String::new();
    let mut ranges = Vec::with_capacity(blocks.len());

    for (index, block) in blocks.iter().enumerate() {
        let _ = write!(prompt, "## Block {}: {}\n\n", index + 1, block.label);
        let start = prompt.len();
        prompt.push_str(&block.text);
        ranges.push(BlockRange {
            label: block.label.clone(),
            start,
            end: prompt.len(),
        });
        prompt.push_str("\n\n");
    }
    if !prompt.is_empty() {
        prompt.push_str(PACKED_CONTEXT_SEPARATOR);
    }
    let prefix_len = prompt.len();
    prompt.push_str(typed);
    (prompt, prefix_len, ranges)
}

pub(super) fn blocks_of_turn(turn: &Turn, ranges: &[BlockRange]) -> Vec<PromptBlock> {
    let text = turn.prompt_text.as_str();
    let recovered: Vec<PromptBlock> = ranges
        .iter()
        .filter_map(|range| {
            let body = text.get(range.start..range.end)?;
            Some(PromptBlock::new(range.label.clone(), body.to_owned()))
        })
        .collect();
    if !recovered.is_empty() {
        return recovered;
    }

    let (prefix, _) = split_prompt(turn);
    if prefix.is_empty() {
        return Vec::new();
    }
    vec![PromptBlock::new(
        "Context of the original message".to_owned(),
        prefix
            .strip_suffix(PACKED_CONTEXT_SEPARATOR)
            .unwrap_or(prefix)
            .to_owned(),
    )]
}
