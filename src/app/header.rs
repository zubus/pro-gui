use super::*;

impl ProGuiApp {
    pub(super) fn render_header(&mut self, ctx: &Context) {
        let mut header_action: Option<HeaderAction> = None;
        TopBottomPanel::top("header")
            .frame(Frame::default().fill(CANVAS).inner_margin(Margin {
                left: 20,
                right: 16,
                top: 10,
                bottom: 10,
            }))
            .show(ctx, |ui| {
                let (title, system_prompt_set, usage_totals, subtitle) = self
                    .selected_conversation()
                    .map(|conversation| {
                        let usage = conversation_usage_totals(&conversation.turns);
                        (
                            conversation.title.clone(),
                            !conversation.system_prompt.trim().is_empty(),
                            usage,
                            chat_subtitle(&conversation.turns, usage),
                        )
                    })
                    .unwrap_or_else(|| {
                        ("No chat selected".to_owned(), false, (0, 0, 0), String::new())
                    });

                let in_chat_match_count = self.in_chat_match_total;
                let in_chat_search_active = is_search_active(&self.in_chat_search);
                if !in_chat_search_active || in_chat_match_count == 0 {
                    self.in_chat_search_current = 0;
                    self.scroll_to_in_chat_match = false;
                } else if self.in_chat_search_current >= in_chat_match_count {
                    self.in_chat_search_current = in_chat_match_count.saturating_sub(1);
                    self.scroll_to_in_chat_match = true;
                }

                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 42.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .button("?")
                            .on_hover_text("Keyboard shortcuts (Cmd+/)")
                            .clicked()
                        {
                            header_action = Some(HeaderAction::OpenShortcuts);
                        }
                        ui.menu_button("Export", |ui| {
                            ui.set_min_width(200.0);
                            if ui.button("Copy as Markdown").clicked() {
                                header_action = Some(HeaderAction::CopyMarkdown);
                            }
                            if ui.button("Export .md file").clicked() {
                                header_action = Some(HeaderAction::ExportMarkdown);
                            }
                            if ui.button("Export .json file").clicked() {
                                header_action = Some(HeaderAction::ExportJson);
                            }
                            ui.separator();
                            if ui.button("Export all chats (.json)").clicked() {
                                header_action = Some(HeaderAction::ExportAllJson);
                            }
                            if ui.button("Backup database file").clicked() {
                                header_action = Some(HeaderAction::BackupDatabase);
                            }
                        })
                        .response
                        .on_hover_text("Export this chat, all chats, or the whole database.");
                        let system_prompt_label = if system_prompt_set {
                            "System prompt (on)"
                        } else {
                            "System prompt"
                        };
                        if ui
                            .button(system_prompt_label)
                            .on_hover_text(if system_prompt_set {
                                "This chat has custom instructions. Click to edit."
                            } else {
                                "Per-chat instructions sent with every request."
                            })
                            .clicked()
                        {
                            header_action = Some(HeaderAction::OpenSystemPrompt);
                        }
                        ui.add_space(12.0);

                        let mut clear_clicked = false;
                        let mut next_clicked = false;
                        let mut prev_clicked = false;
                        if in_chat_search_active {
                            clear_clicked = ui.small_button("Clear").clicked();
                            let match_label = if in_chat_match_count == 0 {
                                "0 matches".to_owned()
                            } else {
                                format!(
                                    "{} / {}",
                                    self.in_chat_search_current + 1,
                                    in_chat_match_count
                                )
                            };
                            ui.label(RichText::new(match_label).small().color(
                                if in_chat_match_count == 0 {
                                    KEY_MISSING
                                } else {
                                    Color32::from_rgb(247, 209, 92)
                                },
                            ));
                            next_clicked = ui
                                .add_enabled(
                                    in_chat_match_count > 0,
                                    egui::Button::new("\u{2193}").small(),
                                )
                                .on_hover_text("Next match (Enter)")
                                .clicked();
                            prev_clicked = ui
                                .add_enabled(
                                    in_chat_match_count > 0,
                                    egui::Button::new("\u{2191}").small(),
                                )
                                .on_hover_text("Previous match (Shift+Enter)")
                                .clicked();
                        }
                        let previous_in_chat_search = self.in_chat_search.clone();
                        let search_response = ui.add_sized(
                            [220.0, 26.0],
                            TextEdit::singleline(&mut self.in_chat_search)
                                .id_salt("in_chat_search_input")
                                .hint_text("Find in chat  (Cmd+F)"),
                        );
                        if self.in_chat_search != previous_in_chat_search {
                            self.in_chat_search_current = 0;
                            self.scroll_to_in_chat_match = is_search_active(&self.in_chat_search);
                        }
                        if self.focus_in_chat_search {
                            search_response.request_focus();
                            self.focus_in_chat_search = false;
                        }
                        let search_enter_pressed = search_response.has_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        let shift = ui.input(|input| input.modifiers.shift);
                        search_response.on_hover_text("Search only within the open chat.");
                        if in_chat_search_active && in_chat_match_count > 0 {
                            if prev_clicked || (search_enter_pressed && shift) {
                                self.in_chat_search_current = if self.in_chat_search_current == 0 {
                                    in_chat_match_count - 1
                                } else {
                                    self.in_chat_search_current - 1
                                };
                                self.scroll_to_in_chat_match = true;
                            } else if next_clicked || (search_enter_pressed && !shift) {
                                self.in_chat_search_current =
                                    (self.in_chat_search_current + 1) % in_chat_match_count;
                                self.scroll_to_in_chat_match = true;
                            }
                        }
                        if clear_clicked {
                            self.in_chat_search.clear();
                            self.in_chat_search_current = 0;
                            self.scroll_to_in_chat_match = false;
                        }
                        ui.add_space(16.0);

                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(title.clone())
                                            .font(FontId::proportional(19.0))
                                            .strong()
                                            .color(TEXT_STRONG),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(title);
                                let summary = ui.add(
                                    egui::Label::new(
                                        RichText::new(&subtitle).small().color(TEXT_MUTED),
                                    )
                                    .truncate(),
                                );
                                if usage_totals.0 > 0 || usage_totals.1 > 0 {
                                    summary.on_hover_text(
                                        "Cumulative billed tokens across this chat's turns.\n\
                                          in: total input tokens billed across turns and server-side long-decode hops \
                                          (on Gemini 4 Neon, Carbon and Argon, each hop re-reads the context at the cached \
                                          rate, so billed input across hops can exceed the 1M context window).\n\
                                          out: total billed output tokens (answer + reasoning).\n\
                                          reasoning: thinking tokens (included in out).",
                                    );
                                }
                            });
                        });
                    });
                    },
                );

                if let Some(message) = &self.banner {
                    ui.add_space(6.0);
                    ui.label(RichText::new(message).color(Color32::from_rgb(255, 209, 128)));
                }
            });

        if let Some(action) = header_action {
            self.handle_header_action(ctx, action);
        }
    }

    pub(super) fn handle_header_action(&mut self, ctx: &Context, action: HeaderAction) {
        match action {
            HeaderAction::OpenShortcuts => {
                self.open_modal(AppModal::Shortcuts);
                return;
            }
            HeaderAction::ExportAllJson => {
                let Some(db_path) = self.db.path() else {
                    self.banner = Some("Export failed: the database has no file path.".to_owned());
                    return;
                };
                let (done_tx, done_rx) = mpsc::channel();
                let repaint = ctx.clone();
                std::thread::spawn(move || {
                    let _ = done_tx.send(export_all_chats(&db_path, &export_directory()));
                    repaint.request_repaint();
                });
                self.export_job = Some(done_rx);
                self.banner = Some("Exporting all chats...".to_owned());
                return;
            }
            HeaderAction::BackupDatabase => {
                self.banner = Some(match backup_database_file(&self.db) {
                    Ok(path) => format!("Backup written to {}", path.display()),
                    Err(err) => format!("Backup failed: {err:#}"),
                });
                return;
            }
            _ => {}
        }

        let Some(conversation) = self.selected_conversation() else {
            return;
        };
        match action {
            HeaderAction::OpenSystemPrompt => {
                let modal = AppModal::SystemPrompt {
                    conversation_id: conversation.id,
                    draft: conversation.system_prompt.clone(),
                };
                self.open_modal(modal);
            }
            HeaderAction::CopyMarkdown => {
                let markdown = conversation_markdown(conversation);
                let title = conversation.title.clone();
                ctx.copy_text(markdown);
                self.banner = Some(format!("Copied \"{title}\" as Markdown."));
            }
            HeaderAction::ExportMarkdown => {
                let content = conversation_markdown(conversation);
                let slug = slugify_title(&conversation.title);
                self.banner = Some(export_banner(write_export_file(&slug, "md", &content)));
            }
            HeaderAction::ExportJson => {
                let slug = slugify_title(&conversation.title);
                let written = conversation_json(conversation)
                    .and_then(|content| write_export_file(&slug, "json", &content));
                self.banner = Some(export_banner(written));
            }
            HeaderAction::OpenShortcuts
            | HeaderAction::ExportAllJson
            | HeaderAction::BackupDatabase => {}
        }
    }
}

pub(super) fn chat_subtitle(turns: &[Turn], usage: (i64, i64, i64)) -> String {
    let Some(last) = turns.last() else {
        return "No messages yet".to_owned();
    };
    let mut parts = vec![
        providers::model_display_name(&last.model),
        format!(
            "{} turn{}",
            turns.len(),
            if turns.len() == 1 { "" } else { "s" }
        ),
    ];
    if usage.0 > 0 || usage.1 > 0 || usage.2 > 0 {
        parts.push(format!(
            "\u{3a3} in {}, out {}, reasoning {}",
            format_compact_count(usage.0),
            format_compact_count(usage.1),
            format_compact_count(usage.2),
        ));
    }
    parts.join("  |  ")
}

pub(super) fn conversation_usage_totals(turns: &[Turn]) -> (i64, i64, i64) {
    let mut totals = (0_i64, 0_i64, 0_i64);
    for turn in turns {
        if turn.status != "completed" {
            continue;
        }
        totals.0 += turn.input_tokens.unwrap_or(0);
        totals.1 += turn.output_tokens.unwrap_or(0);
        totals.2 += turn.reasoning_tokens.unwrap_or(0);
    }
    totals
}
