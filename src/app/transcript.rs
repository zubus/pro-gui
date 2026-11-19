use super::*;

impl ProGuiApp {
    #[expect(clippy::too_many_arguments)]
    pub(super) fn render_turn(
        ui: &mut egui::Ui,
        turn: &Turn,
        runtime: Option<&TurnRuntime>,
        render_markdown_answers: bool,
        render_latex_blocks: bool,
        show_request_chips: bool,
        is_last_turn: bool,
        turns_below: usize,
        editing: &mut Option<TurnEdit>,
        in_chat_search: &mut InChatSearchRender<'_>,
        answer_cache: &mut HashMap<i64, CachedAnswerBlocks>,
        attachment_textures: &HashMap<i64, Option<TextureHandle>>,
    ) -> Option<TurnAction> {
        let is_editing = editing.as_ref().is_some_and(|edit| edit.turn_id == turn.id);
        let column = ui.available_width();
        let user_width = if is_editing {
            column
        } else {
            (column * 0.8).clamp(280.0_f32.min(column), 880.0)
        };
        let mut action = None;

        ui.horizontal(|ui| {
            ui.add_space((column - user_width).max(0.0));
            ui.vertical(|ui| {
                ui.set_width(user_width);
                Frame::default()
                    .fill(Color32::from_rgb(19, 50, 84))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(32, 74, 120)))
                    .corner_radius(egui::CornerRadius::same(14))
                    .inner_margin(Margin::symmetric(16, 12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("You")
                                    .strong()
                                    .color(Color32::from_rgb(222, 237, 252)),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(relative_time(&turn.created_at))
                                        .small()
                                        .color(Color32::from_rgb(150, 182, 212)),
                                )
                                .on_hover_text(&turn.created_at);
                                if !is_editing
                                    && !turn.is_pending()
                                    && ui
                                        .small_button("Edit")
                                        .on_hover_text(
                                            "Edit this message and send the prompt again.",
                                        )
                                        .clicked()
                                {
                                    action = Some(TurnAction::BeginEdit(turn.id));
                                }
                            });
                        });
                        ui.add_space(8.0);
                        if !turn.attachments.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                for attachment in &turn.attachments {
                                    if let Some(Some(texture)) =
                                        attachment_textures.get(&attachment.id)
                                    {
                                        ui.add(
                                            egui::Image::new(texture)
                                                .max_size(egui::vec2(220.0, 150.0)),
                                        );
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
                                            attachment.file_name,
                                            format_bytes(attachment.byte_len)
                                        ))
                                        .small()
                                        .color(Color32::from_rgb(168, 198, 226)),
                                    );
                                }
                            });
                            ui.add_space(6.0);
                        }
                        match editing.as_mut().filter(|edit| edit.turn_id == turn.id) {
                            Some(edit) => {
                                if let Some(edit_action) = render_turn_editor(ui, edit, turns_below)
                                {
                                    action = Some(edit_action);
                                }
                            }
                            None => render_user_prompt(ui, turn, in_chat_search),
                        }
                    });
            });
        });

        ui.add_space(10.0);

        {
            {
                Frame::default()
                    .fill(CARD_FILL)
                    .stroke(Stroke::new(1.0_f32, CARD_STROKE))
                    .corner_radius(egui::CornerRadius::same(14))
                    .inner_margin(Margin::symmetric(16, 14))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(providers::model_display_name(&turn.model))
                                    .strong()
                                    .color(TEXT_STRONG),
                            )
                            .on_hover_text(&turn.model);
                            status_pill(ui, &turn.status);
                            if turn.status == "completed" {
                                if let Some(duration) =
                                    duration_between(&turn.request_started_at, &turn.updated_at)
                                {
                                    ui.label(
                                        RichText::new(format!(
                                            "completed in {}",
                                            format_human_duration(duration)
                                        ))
                                        .small()
                                        .color(Color32::from_rgb(134, 225, 174)),
                                    );
                                }
                            } else if turn.is_pending()
                                && let Some(elapsed) = elapsed_since(&turn.request_started_at) {
                                    ui.label(
                                        RichText::new(format!(
                                            "Elapsed {}",
                                            format_elapsed(elapsed)
                                        ))
                                        .small()
                                        .color(Color32::from_rgb(117, 188, 202)),
                                    );
                                }
                            if let Some(response_id) = &turn.response_id
                                && ui
                                    .add(
                                        egui::Label::new(
                                            RichText::new(shorten_id(response_id))
                                                .small()
                                                .family(FontFamily::Monospace)
                                                .color(TEXT_FAINT),
                                        )
                                        .sense(egui::Sense::click()),
                                    )
                                    .on_hover_text(format!("{response_id}\nClick to copy"))
                                    .clicked()
                                {
                                    ui.ctx().copy_text(response_id.clone());
                                }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if turn.is_pending()
                                    && ui
                                        .small_button("Cancel run")
                                        .on_hover_text("Ask the API to stop this background run.")
                                        .clicked()
                                {
                                    action = Some(TurnAction::Cancel(turn.id));
                                }
                                if !turn.answer_text.is_empty()
                                    && ui.small_button("Copy answer").clicked()
                                {
                                    ui.ctx().copy_text(turn.answer_text.clone());
                                }
                            });
                        });

                        let has_usage = turn.input_tokens.is_some()
                            || turn.output_tokens.is_some()
                            || turn.reasoning_tokens.is_some();
                        if show_request_chips || has_usage {
                            ui.add_space(8.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
                                if show_request_chips {
                                    let provider = Provider::of(&turn.model);
                                    render_setting_chip(ui, "model", &turn.model);
                                    render_setting_chip(
                                        ui,
                                        provider.effort_param(),
                                        &turn.reasoning_effort,
                                    );
                                    render_setting_chip(
                                        ui,
                                        provider.summary_param(),
                                        &turn.reasoning_summary,
                                    );
                                    render_setting_chip(ui, "service_tier", &turn.service_tier);
                                    render_setting_chip(
                                        ui,
                                        "background",
                                        if turn.background { "true" } else { "false" },
                                    );
                                    render_setting_chip(
                                        ui,
                                        "max_output_tokens",
                                        &turn.max_output_tokens.to_string(),
                                    );
                                    render_setting_chip(
                                        ui,
                                        "store",
                                        if turn.store { "true" } else { "false" },
                                    );
                                }
                                if let Some(value) = turn.input_tokens {
                                    render_usage_chip(ui, "tokens.in", value);
                                }
                                if let Some(value) = turn.output_tokens {
                                    render_usage_chip(ui, "tokens.out", value);
                                }
                                if let Some(value) = turn.reasoning_tokens {
                                    render_usage_chip(ui, "tokens.reasoning", value);
                                }
                            });
                        }

                        if let Some(runtime_note) =
                            runtime.and_then(|runtime| runtime.note.as_deref())
                        {
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(14.0));
                                ui.label(
                                    RichText::new(runtime_note)
                                        .color(Color32::from_rgb(255, 208, 114)),
                                );
                            });
                        }

                        if turn.is_pending() && !turn.thinking_text.is_empty() {
                            ui.add_space(8.0);
                            Frame::default()
                                .fill(Color32::from_rgb(20, 30, 40))
                                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(38, 56, 74)))
                                .corner_radius(egui::CornerRadius::same(8))
                                .inner_margin(Margin::symmetric(10, 8))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new("Latest thought")
                                            .small()
                                            .color(Color32::from_rgb(132, 168, 196)),
                                    );
                                    let (preview, _) =
                                        truncate_multiline(latest_thought(&turn.thinking_text), 600, 8);
                                    if render_markdown_answers {
                                        let blocks =
                                            parse_answer_blocks(&preview, render_latex_blocks);
                                        let mut no_search =
                                            InChatSearchRender::new("", None, false);
                                        render_answer_blocks(ui, &blocks, &mut no_search);
                                    } else {
                                        ui.label(
                                            RichText::new(preview)
                                                .italics()
                                                .color(Color32::from_rgb(196, 210, 222)),
                                        );
                                    }
                                });
                        }

                        if let Some(error) = &turn.error_text {
                            ui.add_space(8.0);
                            render_highlighted_label(
                                ui,
                                error,
                                0.0,
                                FontFamily::Proportional,
                                Color32::from_rgb(255, 126, 107),
                                in_chat_search,
                            );
                        }

                        ui.add_space(10.0);
                        if turn.answer_text.is_empty() {
                            let placeholder = if !turn.is_pending() {
                                "No answer text was returned."
                            } else if turn.thinking_text.is_empty() {
                                "Waiting for the first output from the background run..."
                            } else {
                                "No answer text yet. The reasoning so far is under Thinking."
                            };
                            ui.label(
                                RichText::new(placeholder)
                                    .italics()
                                    .color(Color32::from_rgb(147, 157, 166)),
                            );
                        } else {
                            render_answer_content(
                                ui,
                                turn,
                                render_markdown_answers,
                                render_latex_blocks,
                                in_chat_search,
                                answer_cache,
                            );
                        }

                        ui.add_space(12.0);
                        let cached_thinking_tokens =
                            ensure_cached_blocks(turn, render_latex_blocks, answer_cache)
                                .thinking_tokens;
                        let thinking_has_search_match =
                            bigtext::contains(&turn.thinking_text, in_chat_search.query);
                        let streaming_thoughts = turn.is_pending()
                            && !turn.thinking_text.is_empty()
                            && turn.answer_text.is_empty();
                        egui::CollapsingHeader::new(thinking_header_label(
                            turn,
                            cached_thinking_tokens,
                        ))
                        .id_salt(format!("thinking-{}", turn.id))
                        .default_open(false)
                        .open(if thinking_has_search_match || streaming_thoughts {
                            Some(true)
                        } else {
                            None
                        })
                        .show(ui, |ui| {
                            if turn.thinking_text.is_empty() {
                                ui.label(
                                    RichText::new("No reasoning summary has been returned yet.")
                                        .italics()
                                        .color(Color32::from_rgb(138, 146, 156)),
                                );
                            } else {
                                ui.horizontal(|ui| {
                                    ui.with_layout(
                                        Layout::right_to_left(Align::Center),
                                        |ui| {
                                            if ui.small_button("Copy thinking").clicked() {
                                                ui.ctx().copy_text(turn.thinking_text.clone());
                                            }
                                        },
                                    );
                                });
                                ui.add_space(6.0);
                                render_thinking_content(
                                    ui,
                                    turn,
                                    render_markdown_answers,
                                    render_latex_blocks,
                                    in_chat_search,
                                    answer_cache,
                                );
                            }
                        });

                        if turn.status == "failed" || (is_last_turn && !turn.is_pending()) {
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                if turn.status == "failed" {
                                    if ui.button("Send again").clicked() {
                                        action = Some(TurnAction::Retry(turn.id));
                                    }
                                } else if is_last_turn
                                    && !turn.is_pending()
                                    && ui
                                        .button("Regenerate")
                                        .on_hover_text(
                                            "Reset this turn and run it again with the same prompt and settings.",
                                        )
                                        .clicked()
                                {
                                    action = Some(TurnAction::Retry(turn.id));
                                }
                                if is_last_turn
                                    && !turn.is_pending()
                                    && !is_editing
                                    && ui
                                        .button("Edit & resend")
                                        .on_hover_text(
                                            "Edit the prompt of this message and run it again.",
                                        )
                                        .clicked()
                                {
                                    action = Some(TurnAction::BeginEdit(turn.id));
                                }
                            });
                        }

                    });
            }
        }

        action
    }

    pub(super) fn render_main(&mut self, ctx: &Context) {
        let in_chat_match_count = self.in_chat_match_total;
        let selected_conversation_id = self.selected_conversation_id;
        let render_markdown_answers = self.render_markdown_answers;
        let render_latex_blocks = self.render_latex_blocks;
        let show_request_chips = self.show_request_chips;
        let in_chat_search = self.in_chat_search.clone();
        let in_chat_search_current = self.in_chat_search_current;
        let scroll_to_in_chat_match = self.scroll_to_in_chat_match;
        let is_editing_turn = self.editing_turn.is_some();
        let conversations = &self.conversations;
        let runtime = &self.runtime;
        let attachment_textures = &self.attachment_textures;
        let answer_cache = &mut self.answer_cache;
        let turn_heights = &mut self.turn_heights;
        let font_size = self.font_size;
        let editing = &mut self.editing_turn;
        let mut turn_action = None;
        let mut scrolled_to_in_chat_match = false;
        let mut painted_matches = 0;

        CentralPanel::default()
            .frame(Frame::default().fill(CANVAS).inner_margin(Margin::same(0)))
            .show(ctx, |ui| {
                let Some(selected_id) = selected_conversation_id else {
                    ui.label("No chat selected.");
                    return;
                };
                let Some(conversation) = conversations
                    .iter()
                    .find(|conversation| conversation.id == selected_id)
                else {
                    ui.label("No chat selected.");
                    return;
                };
                let current_match_index = (is_search_active(&in_chat_search)
                    && in_chat_match_count > 0)
                    .then(|| in_chat_search_current.min(in_chat_match_count - 1));
                let mut search_render = InChatSearchRender::new(
                    &in_chat_search,
                    current_match_index,
                    scroll_to_in_chat_match,
                );

                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(!search_render.is_active() && !is_editing_turn)
                    .show(ui, |ui| {
                        ui.add_space(18.0);
                        let full_width = ui.available_width();
                        let content_width = column_width(full_width);
                        let side_margin = ((full_width - content_width) / 2.0).max(0.0);
                        ui.horizontal(|ui| {
                        ui.add_space(side_margin);
                        ui.vertical(|ui| {
                        ui.set_width(content_width);

                        if conversation.turns.is_empty() {
                            Frame::default()
                                .fill(Color32::from_rgb(17, 31, 49))
                                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(30, 57, 86)))
                                .corner_radius(egui::CornerRadius::same(16))
                                .inner_margin(Margin::same(20))
                                .show(ui, |ui| {
                                    ui.heading(
                                        RichText::new("Start a long-running background chat")
                                            .color(Color32::from_rgb(226, 237, 245)),
                                    );
                                    ui.add_space(8.0);
                                    ui.label(
                                        RichText::new(
                                            "Messages stay in SQLite, each turn is linked to the previous answer on the server (previous_response_id on OpenAI, previous_interaction_id on Gemini), and the thinking text stays collapsible under each assistant reply. Background runs keep going even if you quit. Reopening the app picks them up again and notifies when they finish.",
                                        )
                                        .color(Color32::from_rgb(170, 184, 197)),
                                    );
                                });
                            ui.add_space(16.0);
                        }

                        let last_turn_id = conversation.turns.last().map(|turn| turn.id);
                        let turn_count = conversation.turns.len();
                        for (index, turn) in conversation.turns.iter().enumerate() {
                            let is_last_turn = last_turn_id == Some(turn.id);
                            let layout_key = turn_layout_key(
                                turn,
                                content_width,
                                font_size,
                                (render_markdown_answers, render_latex_blocks, show_request_chips),
                                is_last_turn,
                            );
                            let skippable = !turn.is_pending()
                                && !search_render.is_active()
                                && editing.is_none();
                            let hidden_height = turn_heights
                                .get(&turn.id)
                                .filter(|(key, _)| skippable && *key == layout_key)
                                .map(|(_, height)| *height)
                                .filter(|height| {
                                    !ui.is_rect_visible(egui::Rect::from_min_size(
                                        ui.cursor().min,
                                        egui::vec2(content_width, *height),
                                    ))
                                });
                            if let Some(height) = hidden_height {
                                ui.allocate_space(egui::vec2(content_width, height));
                            } else {
                                let rendered = ui.scope(|ui| {
                                    Self::render_turn(
                                        ui,
                                        turn,
                                        runtime.get(&turn.id),
                                        render_markdown_answers,
                                        render_latex_blocks,
                                        show_request_chips,
                                        is_last_turn,
                                        turn_count - index - 1,
                                        editing,
                                        &mut search_render,
                                        answer_cache,
                                        attachment_textures,
                                    )
                                });
                                turn_heights
                                    .insert(turn.id, (layout_key, rendered.response.rect.height()));
                                if let Some(new_action) = rendered.inner {
                                    turn_action.get_or_insert(new_action);
                                }
                            }
                            ui.add_space(18.0);
                        }

                        ui.add_space(12.0);
                        });
                        });
                    });
                scrolled_to_in_chat_match = search_render.scrolled_to_current;
                painted_matches = search_render.seen_matches;
            });

        self.in_chat_match_total = painted_matches;
        if scrolled_to_in_chat_match || !is_search_active(&self.in_chat_search) {
            self.scroll_to_in_chat_match = false;
        }

        if let Some(action) = turn_action {
            match action {
                TurnAction::Retry(turn_id) => self.retry_turn(turn_id, false),
                TurnAction::Cancel(turn_id) => self.cancel_turn(turn_id),
                TurnAction::BeginEdit(turn_id) => self.begin_turn_edit(turn_id),
                TurnAction::CancelEdit => {
                    self.editing_turn = None;
                    self.viewing_block = None;
                    self.block_editor = DocumentEditor::default();
                }
                TurnAction::EditBlock(index) => {
                    self.viewing_block = Some((BlockOwner::TurnEdit, index));
                    self.block_editor = DocumentEditor::default();
                }
                TurnAction::RemoveBlock(index) => {
                    if let Some(edit) = self.editing_turn.as_mut()
                        && index < edit.blocks.len()
                    {
                        edit.blocks.remove(index);
                    }
                    self.viewing_block = None;
                    self.block_editor = DocumentEditor::default();
                }
                TurnAction::ResendEdit => self.resend_edited_turn(),
                TurnAction::ResendEditAsNew => self.resend_edited_turn_as_new(),
            }
        }
    }
}

pub(super) fn render_user_prompt(
    ui: &mut egui::Ui,
    turn: &Turn,
    search: &mut InChatSearchRender<'_>,
) {
    let (prefix, typed) = split_prompt(turn);
    if !prefix.is_empty() && !render_search_hits(ui, prefix, "attached context", search) {
        let lines_id = egui::Id::new((
            "prefix_lines",
            turn.id,
            turn.prompt_text.len(),
            turn.prompt_prefix_len,
        ));
        let prefix_lines = ui.ctx().data_mut(|data| {
            *data.get_temp_mut_or_insert_with(lines_id, || bigtext::count_lines(prefix))
        });
        ui.horizontal_wrapped(|ui| {
            Frame::default()
                .fill(Color32::from_rgb(28, 40, 54))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(44, 70, 98)))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("Attached context")
                            .small()
                            .strong()
                            .color(Color32::from_rgb(198, 222, 244)),
                    );
                    ui.label(
                        RichText::new(format!(
                            "{}, {} lines",
                            format_bytes(prefix.len() as i64),
                            format_compact_count(prefix_lines as i64)
                        ))
                        .small()
                        .color(Color32::from_rgb(150, 182, 212)),
                    );
                    if ui
                        .small_button("Copy")
                        .on_hover_text("Copy the attached context.")
                        .clicked()
                    {
                        ui.ctx().copy_text(prefix.to_owned());
                    }
                });
        });
    }
    if typed.trim().is_empty() {
        return;
    }
    if !prefix.is_empty() {
        ui.add_space(8.0);
    }

    if search.is_active()
        && bigtext::needs_document_editor(typed)
        && render_search_hits(ui, typed, "this message", search)
    {
        return;
    } else if bigtext::contains(typed, search.query) {
        render_highlighted_label(
            ui,
            typed,
            0.0,
            FontFamily::Proportional,
            Color32::from_rgb(223, 229, 235),
            search,
        );
        return;
    }

    let (preview, truncated) = truncate_multiline(typed, 1400, 18);
    render_highlighted_label(
        ui,
        &preview,
        0.0,
        FontFamily::Proportional,
        Color32::from_rgb(223, 229, 235),
        search,
    );

    if truncated {
        ui.add_space(10.0);
        egui::CollapsingHeader::new("Expand full prompt")
            .id_salt(format!("prompt-{}", turn.id))
            .default_open(false)
            .show(ui, |ui| {
                render_highlighted_label(
                    ui,
                    typed,
                    0.0,
                    FontFamily::Proportional,
                    Color32::from_rgb(223, 229, 235),
                    search,
                );
            });
    }
}

pub(super) fn render_turn_editor(
    ui: &mut egui::Ui,
    edit: &mut TurnEdit,
    turns_below: usize,
) -> Option<TurnAction> {
    let mut action = None;
    let TurnEdit {
        draft,
        blocks,
        needs_focus,
        editor,
        ..
    } = edit;

    if !blocks.is_empty() {
        if let Some(clicked) = render_block_chips(ui, blocks) {
            action = Some(match clicked {
                BlockAction::Edit(index) => TurnAction::EditBlock(index),
                BlockAction::Remove(index) => TurnAction::RemoveBlock(index),
            });
        }
        ui.add_space(8.0);
    }

    let response = render_big_text(
        ui,
        draft,
        editor,
        "turn-editor",
        340.0,
        None,
        "Edit this message...",
    );
    if *needs_focus {
        response.request_focus();
        editor.request_focus();
        *needs_focus = false;
    }
    if bigtext::needs_document_editor(draft) {
        ui.add_space(4.0);
        render_big_text_note(ui, draft, editor, &response);
    }

    if response.has_focus() || editor.focused {
        let (send, cancel) = ui.input(|input| {
            (
                input.key_pressed(egui::Key::Enter) && input.modifiers.command,
                input.key_pressed(egui::Key::Escape),
            )
        });
        if send {
            action = Some(TurnAction::ResendEdit);
        } else if cancel {
            action = Some(TurnAction::CancelEdit);
        }
    }

    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("Resend")
            .on_hover_text("Replace this message and run it again from here.")
            .clicked()
        {
            action = Some(TurnAction::ResendEdit);
        }
        if ui
            .button("Send as new message")
            .on_hover_text(
                "Keep the whole history and send the edited prompt at the end of the chat.",
            )
            .clicked()
        {
            action = Some(TurnAction::ResendEditAsNew);
        }
        if ui.button("Cancel").clicked() {
            action = Some(TurnAction::CancelEdit);
        }
        ui.label(
            RichText::new("Cmd+Enter resends, Esc cancels")
                .small()
                .color(Color32::from_rgb(150, 180, 210)),
        );
    });

    if turns_below > 0 {
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!(
                "Resending here removes the {turns_below} message{} below it. \"Send as new message\" keeps them.",
                if turns_below == 1 { "" } else { "s" }
            ))
            .small()
            .color(Color32::from_rgb(255, 209, 128)),
        );
    }

    action
}

pub(super) fn render_search_hits(
    ui: &mut egui::Ui,
    text: &str,
    label: &str,
    search: &mut InChatSearchRender<'_>,
) -> bool {
    if !search.is_active() {
        return false;
    }
    let matches = bigtext::find_all(text, search.query);
    if matches.is_empty() {
        return false;
    }

    let active = search
        .current_match_index
        .and_then(|global| global.checked_sub(search.seen_matches))
        .and_then(|local| matches.get(local).copied());
    search.seen_matches += matches.len();

    let response = Frame::default()
        .fill(Color32::from_rgb(28, 40, 54))
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(44, 70, 98)))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.label(
                RichText::new(format!(
                    "{} match{} in {label}, {}",
                    format_compact_count(matches.len() as i64),
                    if matches.len() == 1 { "" } else { "es" },
                    format_bytes(text.len() as i64),
                ))
                .small()
                .color(Color32::from_rgb(198, 222, 244)),
            );

            let Some((start, end)) = active else {
                return;
            };
            ui.add_space(4.0);

            let from = text[..start]
                .char_indices()
                .rev()
                .take(100)
                .last()
                .map(|(index, _)| index)
                .unwrap_or(0);
            let to = text[end..]
                .char_indices()
                .nth(100)
                .map(|(index, _)| end + index)
                .unwrap_or(text.len());

            let plain = TextFormat {
                font_id: FontId::new(13.0, FontFamily::Monospace),
                color: Color32::from_rgb(205, 215, 226),
                ..Default::default()
            };
            let mut job = LayoutJob::default();
            job.wrap.max_width = ui.available_width();
            if from > 0 {
                job.append("\u{2026}", 0.0, plain.clone());
            }
            job.append(&text[from..start], 0.0, plain.clone());
            job.append(
                &text[start..end],
                0.0,
                search_highlight_format(&plain, true),
            );
            job.append(&text[end..to], 0.0, plain.clone());
            if to < text.len() {
                job.append("\u{2026}", 0.0, plain);
            }
            ui.label(job);
        })
        .response;

    if active.is_some() && search.scroll_to_current {
        response.scroll_to_me(Some(Align::Center));
        search.scrolled_to_current = true;
    }
    true
}

pub(super) fn thinking_header_label(turn: &Turn, cached_thinking_tokens: i64) -> String {
    let tokens = turn
        .reasoning_tokens
        .unwrap_or(0)
        .max(cached_thinking_tokens);
    if tokens > 0 || turn.reasoning_tokens.is_some() {
        format!("Thinking ({} tokens)", format_compact_count(tokens))
    } else {
        "Thinking".to_owned()
    }
}

pub(super) fn latest_thought(thinking: &str) -> &str {
    let thinking = thinking.trim_end();
    let mut tail_start = thinking.len().saturating_sub(16 * 1024);
    while !thinking.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let tail = &thinking[tail_start..];
    let start = tail
        .match_indices("**")
        .map(|(index, _)| index)
        .filter(|&index| index == 0 || tail[..index].ends_with('\n'))
        .last()
        .or_else(|| tail.rfind("\n\n").map(|index| index + 2))
        .unwrap_or(0);
    &tail[start..]
}

pub(super) fn ensure_cached_blocks<'a>(
    turn: &Turn,
    latex_enabled: bool,
    answer_cache: &'a mut HashMap<i64, CachedAnswerBlocks>,
) -> &'a CachedAnswerBlocks {
    let entry = answer_cache
        .entry(turn.id)
        .or_insert_with(|| CachedAnswerBlocks {
            answer_len: usize::MAX,
            thinking_len: usize::MAX,
            thinking_tokens: 0,
            thinking_tokens_exact: false,
            latex_enabled,
            blocks: Vec::new(),
            thinking_blocks: Vec::new(),
        });
    let latex_changed = entry.latex_enabled != latex_enabled;
    entry.latex_enabled = latex_enabled;

    if latex_changed || entry.answer_len != turn.answer_text.len() {
        entry.answer_len = turn.answer_text.len();
        entry.blocks = parse_answer_blocks(&turn.answer_text, latex_enabled);
    }
    let thinking_changed = entry.thinking_len != turn.thinking_text.len();
    if latex_changed || thinking_changed {
        entry.thinking_len = turn.thinking_text.len();
        entry.thinking_blocks = parse_answer_blocks(&turn.thinking_text, latex_enabled);
    }
    if thinking_changed || (!entry.thinking_tokens_exact && !turn.is_pending()) {
        entry.thinking_tokens_exact = !turn.is_pending();
        entry.thinking_tokens = if turn.thinking_text.is_empty() {
            0
        } else if entry.thinking_tokens_exact {
            repoctx::count_tokens(&turn.thinking_text) as i64
        } else {
            repoctx::approx_tokens_for_bytes(turn.thinking_text.len() as u64) as i64
        };
    }
    entry
}

pub(super) fn render_answer_content(
    ui: &mut egui::Ui,
    turn: &Turn,
    markdown_enabled: bool,
    latex_enabled: bool,
    search: &mut InChatSearchRender<'_>,
    answer_cache: &mut HashMap<i64, CachedAnswerBlocks>,
) {
    if search.is_active()
        && bigtext::needs_document_editor(&turn.answer_text)
        && render_search_hits(ui, &turn.answer_text, "this answer", search)
    {
        ui.add_space(6.0);
        let (preview, _) = truncate_multiline(&turn.answer_text, 4_000, 40);
        ui.label(RichText::new(preview).color(Color32::from_rgb(180, 190, 200)));
        return;
    }

    if !markdown_enabled {
        render_highlighted_label(
            ui,
            &turn.answer_text,
            0.0,
            FontFamily::Proportional,
            Color32::from_rgb(223, 229, 235),
            search,
        );
        return;
    }

    let cached = ensure_cached_blocks(turn, latex_enabled, answer_cache);
    render_answer_blocks(ui, &cached.blocks, search);
}

pub(super) fn render_thinking_content(
    ui: &mut egui::Ui,
    turn: &Turn,
    markdown_enabled: bool,
    latex_enabled: bool,
    search: &mut InChatSearchRender<'_>,
    answer_cache: &mut HashMap<i64, CachedAnswerBlocks>,
) {
    if search.is_active()
        && bigtext::needs_document_editor(&turn.thinking_text)
        && render_search_hits(ui, &turn.thinking_text, "this reasoning", search)
    {
        ui.add_space(6.0);
        let (preview, _) = truncate_multiline(&turn.thinking_text, 4_000, 40);
        ui.label(RichText::new(preview).color(Color32::from_rgb(180, 190, 200)));
        return;
    }

    if !markdown_enabled {
        render_highlighted_label(
            ui,
            &turn.thinking_text,
            0.0,
            FontFamily::Proportional,
            Color32::from_rgb(223, 229, 235),
            search,
        );
        return;
    }

    let cached = ensure_cached_blocks(turn, latex_enabled, answer_cache);
    render_answer_blocks(ui, &cached.thinking_blocks, search);
}

pub(super) fn truncate_multiline(
    value: &str,
    max_chars: usize,
    max_lines: usize,
) -> (String, bool) {
    let byte_budget = max_chars.saturating_mul(4).saturating_add(1);
    let (value, over_budget) = if value.len() > byte_budget {
        let mut end = byte_budget;
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        (&value[..end], true)
    } else {
        (value, false)
    };

    let mut preview = String::new();
    let mut truncated = over_budget;

    for (index, line) in value.lines().enumerate() {
        if index >= max_lines {
            truncated = true;
            break;
        }

        if !preview.is_empty() {
            preview.push('\n');
        }
        preview.push_str(line);
    }

    let char_count = preview.chars().count();
    if char_count > max_chars {
        preview = preview.chars().take(max_chars).collect::<String>();
        truncated = true;
    }

    if !truncated && value.lines().count() <= max_lines && value.chars().count() <= max_chars {
        return (value.to_owned(), false);
    }

    if !preview.ends_with("...") {
        preview.push_str("\n\n...");
    }
    (preview, true)
}

pub(super) fn elapsed_since(value: &str) -> Option<Duration> {
    let created_at = parse_timestamp(value)?;
    let now = Local::now().naive_local();
    let elapsed = now.signed_duration_since(created_at);
    if elapsed.num_seconds() < 0 {
        return None;
    }
    Some(Duration::from_secs(elapsed.num_seconds() as u64))
}

pub(super) fn duration_between(start: &str, end: &str) -> Option<Duration> {
    let start_time = parse_timestamp(start)?;
    let end_time = parse_timestamp(end)?;
    let elapsed = end_time.signed_duration_since(start_time);
    if elapsed.num_seconds() < 0 {
        return None;
    }
    Some(Duration::from_secs(elapsed.num_seconds() as u64))
}

pub(super) fn parse_timestamp(value: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S").ok()
}

pub(super) fn format_human_duration(duration: Duration) -> String {
    let total = duration.as_secs();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;

    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

pub(super) fn format_elapsed(elapsed: Duration) -> String {
    let total = elapsed.as_secs();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

pub(super) fn shorten_id(value: &str) -> String {
    if value.chars().count() <= 18 {
        return value.to_owned();
    }

    let start = value.chars().take(8).collect::<String>();
    let end = value
        .chars()
        .rev()
        .take(6)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{start}...{end}")
}

pub(super) fn turn_layout_key(
    turn: &Turn,
    width: f32,
    font_size: f32,
    settings: (bool, bool, bool),
    is_last_turn: bool,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    width.to_bits().hash(&mut hasher);
    font_size.to_bits().hash(&mut hasher);
    settings.hash(&mut hasher);
    is_last_turn.hash(&mut hasher);
    turn.status.hash(&mut hasher);
    turn.updated_at.hash(&mut hasher);
    turn.answer_text.len().hash(&mut hasher);
    turn.thinking_text.len().hash(&mut hasher);
    turn.prompt_text.len().hash(&mut hasher);
    turn.error_text.hash(&mut hasher);
    hasher.finish()
}
