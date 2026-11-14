use super::*;

impl ProGuiApp {
    pub(super) fn render_sidebar(&mut self, ctx: &Context) {
        let mut open_settings = false;
        let mut open_keys = false;
        SidePanel::left("sidebar")
            .resizable(false)
            .exact_width(SIDEBAR_WIDTH)
            .frame(
                Frame::default()
                    .fill(SIDEBAR_FILL)
                    .inner_margin(Margin::same(12)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Chats")
                            .strong()
                            .size(20.0)
                            .color(TEXT_STRONG),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{}", self.conversations.len()))
                                .small()
                                .color(TEXT_MUTED),
                        );
                    });
                });
                ui.add_space(10.0);
                ui.add(
                    TextEdit::singleline(&mut self.chat_search)
                        .hint_text("Search chats...")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    let button_width =
                        ((ui.available_width() - ui.spacing().item_spacing.x) / 2.0).max(80.0);
                    if ui
                        .add_sized(
                            [button_width, 30.0],
                            egui::Button::new(RichText::new("+  New chat").strong()),
                        )
                        .on_hover_text("Cmd+N")
                        .clicked()
                    {
                        self.create_conversation(None);
                    }
                    if ui
                        .add_sized([button_width, 30.0], egui::Button::new("+  New folder"))
                        .clicked()
                    {
                        self.open_modal(AppModal::Folder {
                            folder_id: None,
                            draft: String::new(),
                            move_conversation_id: None,
                        });
                    }
                });
                ui.add_space(10.0);

                TopBottomPanel::bottom("sidebar_footer")
                    .frame(Frame::default().inner_margin(Margin {
                        left: 0,
                        right: 0,
                        top: 8,
                        bottom: 0,
                    }))
                    .show_separator_line(true)
                    .show_inside(ui, |ui| {
                        ui.horizontal(|ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("\u{2699}  Settings").color(TEXT_BODY),
                                    )
                                    .frame_when_inactive(false)
                                    .min_size(egui::vec2(0.0, 28.0)),
                                )
                                .on_hover_text("Settings (Cmd+,)")
                                .clicked()
                            {
                                open_settings = true;
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                for (name, key) in [
                                    ("Anthropic", &self.anthropic_api_key),
                                    ("Gemini", &self.gemini_api_key),
                                    ("OpenAI", &self.api_key),
                                ] {
                                    open_keys |=
                                        key_status_dot(ui, name, !key.trim().is_empty()).clicked();
                                }
                                open_keys |= ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new("API keys").small().color(TEXT_MUTED),
                                        )
                                        .frame_when_inactive(false),
                                    )
                                    .on_hover_text("Manage API keys")
                                    .clicked();
                            });
                        });
                    });

                CentralPanel::default()
                    .frame(Frame::default())
                    .show_inside(ui, |ui| {
                        let normalized_query = normalize_search_query(&self.chat_search);
                        Self::refresh_search(
                            &self.db,
                            &mut self.chat_search_matches,
                            &normalized_query,
                        );
                        let matched_ids = self.chat_search_matches.ids.as_ref();
                        if let Some(error) = &self.chat_search_matches.error {
                            ui.label(RichText::new(error).small().color(KEY_MISSING));
                        }
                        let selected_conversation_id = self.selected_conversation_id;
                        let mut actions = Vec::new();

                        ScrollArea::vertical()
                            .id_salt("chat_list_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                if is_search_active(&normalized_query) {
                                    let filtered = self
                                        .conversations
                                        .iter()
                                        .filter(|conversation| {
                                            matches_search(
                                                conversation,
                                                &normalized_query,
                                                matched_ids,
                                            )
                                        })
                                        .collect::<Vec<_>>();

                                    if filtered.is_empty() {
                                        ui.label(
                                            RichText::new("No chats match the current search.")
                                                .color(Color32::from_rgb(142, 152, 162)),
                                        );
                                        return;
                                    }

                                    for conversation in filtered {
                                        Self::render_chat_row(
                                            ui,
                                            conversation,
                                            selected_conversation_id == Some(conversation.id),
                                            &self.folders,
                                            &mut actions,
                                        );
                                        ui.add_space(8.0);
                                    }
                                    return;
                                }

                                for folder in &self.folders {
                                    let folder_chats = self
                                        .conversations
                                        .iter()
                                        .filter(|conversation| {
                                            conversation.folder_id == Some(folder.id)
                                        })
                                        .collect::<Vec<_>>();
                                    ui.add_space(2.0);
                                    let header = egui::CollapsingHeader::new(
                                        RichText::new(format!(
                                            "{}  ({})",
                                            folder.name,
                                            folder_chats.len()
                                        ))
                                        .strong()
                                        .color(Color32::from_rgb(228, 234, 240)),
                                    )
                                    .id_salt(("chat_folder", folder.id))
                                    .default_open(false)
                                    .show(ui, |ui| {
                                        if folder_chats.is_empty() {
                                            ui.label(
                                                RichText::new("Empty folder")
                                                    .italics()
                                                    .color(Color32::from_rgb(142, 152, 162)),
                                            );
                                        }
                                        for conversation in folder_chats {
                                            Self::render_chat_row(
                                                ui,
                                                conversation,
                                                selected_conversation_id == Some(conversation.id),
                                                &self.folders,
                                                &mut actions,
                                            );
                                            ui.add_space(8.0);
                                        }
                                    });
                                    header
                                        .header_response
                                        .on_hover_text("Right-click for folder options")
                                        .context_menu(|ui| {
                                            ui.set_min_width(160.0);
                                            if ui.button("New chat in folder").clicked() {
                                                actions.push(SidebarAction::NewChatInFolder(Some(
                                                    folder.id,
                                                )));
                                            }
                                            if ui.button("Rename folder").clicked() {
                                                actions
                                                    .push(SidebarAction::RenameFolder(folder.id));
                                            }
                                            if ui.button("Delete folder (keeps chats)").clicked() {
                                                actions
                                                    .push(SidebarAction::DeleteFolder(folder.id));
                                            }
                                        });
                                    ui.add_space(4.0);
                                }

                                if !self.folders.is_empty() {
                                    ui.add_space(4.0);
                                    ui.separator();
                                    ui.add_space(6.0);
                                }

                                let known_folder_ids = self
                                    .folders
                                    .iter()
                                    .map(|folder| folder.id)
                                    .collect::<HashSet<i64>>();
                                let ungrouped = self
                                    .conversations
                                    .iter()
                                    .filter(|conversation| {
                                        conversation
                                            .folder_id
                                            .is_none_or(|id| !known_folder_ids.contains(&id))
                                    })
                                    .collect::<Vec<_>>();
                                for conversation in ungrouped {
                                    Self::render_chat_row(
                                        ui,
                                        conversation,
                                        selected_conversation_id == Some(conversation.id),
                                        &self.folders,
                                        &mut actions,
                                    );
                                    ui.add_space(8.0);
                                }
                            });

                        for action in actions {
                            match action {
                                SidebarAction::Select(conversation_id) => {
                                    self.select_conversation(conversation_id);
                                }
                                SidebarAction::MoveToFolder {
                                    conversation_id,
                                    folder_id,
                                } => {
                                    self.move_conversation_to_folder(conversation_id, folder_id);
                                }
                                SidebarAction::NewChatInFolder(folder_id) => {
                                    self.create_conversation(folder_id);
                                }
                                SidebarAction::NewFolderForChat(conversation_id) => {
                                    self.open_modal(AppModal::Folder {
                                        folder_id: None,
                                        draft: String::new(),
                                        move_conversation_id: Some(conversation_id),
                                    });
                                }
                                SidebarAction::RenameChat(conversation_id) => {
                                    let draft = self
                                        .conversations
                                        .iter()
                                        .find(|conversation| conversation.id == conversation_id)
                                        .map(|conversation| conversation.title.clone())
                                        .unwrap_or_default();
                                    self.open_modal(AppModal::RenameChat {
                                        conversation_id,
                                        draft,
                                    });
                                }
                                SidebarAction::DeleteChat(conversation_id) => {
                                    let info = self
                                        .conversations
                                        .iter()
                                        .find(|conversation| conversation.id == conversation_id)
                                        .map(|conversation| {
                                            (conversation.title.clone(), conversation.turn_count)
                                        });
                                    if let Some((title, turn_count)) = info {
                                        self.open_modal(AppModal::ConfirmDeleteChat {
                                            conversation_id,
                                            title,
                                            turn_count,
                                        });
                                    }
                                }
                                SidebarAction::RenameFolder(folder_id) => {
                                    let name = self
                                        .folders
                                        .iter()
                                        .find(|folder| folder.id == folder_id)
                                        .map(|folder| folder.name.clone())
                                        .unwrap_or_default();
                                    self.open_modal(AppModal::Folder {
                                        folder_id: Some(folder_id),
                                        draft: name,
                                        move_conversation_id: None,
                                    });
                                }
                                SidebarAction::DeleteFolder(folder_id) => {
                                    self.delete_folder(folder_id);
                                }
                            }
                        }
                    });
            });

        if open_settings {
            self.show_settings = true;
        }
        if open_keys {
            self.open_api_key_modal();
            self.active_modal = None;
        }
    }

    pub(super) fn render_chat_row(
        ui: &mut egui::Ui,
        conversation: &Conversation,
        selected: bool,
        folders: &[Folder],
        actions: &mut Vec<SidebarAction>,
    ) {
        let conversation_id = conversation.id;

        ui.push_id(conversation_id, |ui| {
            let (rect, response) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 56.0), egui::Sense::click());
            if !ui.is_rect_visible(rect) {
                return;
            }
            let preview = conversation_preview(conversation);

            let fill = if selected {
                Color32::from_rgb(27, 61, 99)
            } else if response.hovered() {
                Color32::from_rgb(29, 34, 41)
            } else {
                Color32::TRANSPARENT
            };
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(8), fill);

            let status = conversation_status(conversation);
            let dot = egui::pos2(rect.right() - 12.0, rect.bottom() - 12.0);
            ui.painter().circle_filled(dot, 4.0, status_color(status));
            ui.painter().circle_stroke(
                dot,
                4.0,
                Stroke::new(1.0_f32, Color32::from_rgb(18, 21, 25)),
            );

            let inner = rect.shrink2(egui::vec2(10.0, 8.0));
            let mut content = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(inner)
                    .layout(Layout::top_down(Align::Min)),
            );
            content.set_clip_rect(inner.intersect(ui.clip_rect()));
            content.spacing_mut().item_spacing.y = 3.0;
            content.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(22.0);
                    ui.label(
                        RichText::new(relative_time(&conversation.updated_at))
                            .small()
                            .color(Color32::from_rgb(150, 163, 175)),
                    );
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(conversation.title.as_str())
                                    .strong()
                                    .color(Color32::from_rgb(228, 234, 240)),
                            )
                            .truncate()
                            .selectable(false),
                        );
                    });
                });
            });
            content.add(
                egui::Label::new(
                    RichText::new(preview)
                        .small()
                        .color(Color32::from_rgb(150, 160, 170)),
                )
                .truncate()
                .selectable(false),
            );

            if response.clicked() {
                actions.push(SidebarAction::Select(conversation_id));
            }
            let response = response.on_hover_text(status_label(status));
            response.context_menu(|ui| {
                Self::chat_row_menu(ui, conversation, folders, actions);
            });

            let kebab_rect = egui::Rect::from_min_size(
                egui::pos2(rect.right() - 26.0, rect.top() + 4.0),
                egui::vec2(22.0, 20.0),
            );
            let kebab_response = ui
                .put(
                    kebab_rect,
                    egui::Button::new(RichText::new("…").size(14.0)).frame_when_inactive(false),
                )
                .on_hover_text("Chat options");
            egui::Popup::menu(&kebab_response).show(|ui| {
                Self::chat_row_menu(ui, conversation, folders, actions);
            });
        });
    }

    pub(super) fn chat_row_menu(
        ui: &mut egui::Ui,
        conversation: &Conversation,
        folders: &[Folder],
        actions: &mut Vec<SidebarAction>,
    ) {
        let conversation_id = conversation.id;
        ui.set_min_width(160.0);
        if ui.button("Rename chat").clicked() {
            actions.push(SidebarAction::RenameChat(conversation_id));
        }
        ui.menu_button("Move to folder", |ui| {
            ui.set_min_width(140.0);
            for folder in folders {
                if conversation.folder_id == Some(folder.id) {
                    continue;
                }
                if ui.button(&folder.name).clicked() {
                    actions.push(SidebarAction::MoveToFolder {
                        conversation_id,
                        folder_id: Some(folder.id),
                    });
                }
            }
            if !folders.is_empty() {
                ui.separator();
            }
            if ui.button("New folder…").clicked() {
                actions.push(SidebarAction::NewFolderForChat(conversation_id));
            }
            if conversation.folder_id.is_some() {
                ui.separator();
                if ui.button("Remove from folder").clicked() {
                    actions.push(SidebarAction::MoveToFolder {
                        conversation_id,
                        folder_id: None,
                    });
                }
            }
        });
        ui.separator();
        if ui
            .button(RichText::new("Delete chat…").color(Color32::from_rgb(255, 151, 143)))
            .clicked()
        {
            actions.push(SidebarAction::DeleteChat(conversation_id));
        }
    }

    pub(super) fn refresh_search(db: &Database, cache: &mut SearchMatches, query: &str) {
        if cache.query != query {
            cache.query = query.to_owned();
            match db.search_conversation_ids(query) {
                Ok(ids) => {
                    cache.ids = ids.map(|ids| ids.into_iter().collect());
                    cache.error = None;
                }
                Err(err) => {
                    cache.ids = None;
                    cache.error = Some(format!("Message search failed: {err}"));
                }
            }
        }
    }
}

pub(super) fn matches_search(
    conversation: &Conversation,
    query: &str,
    matched_ids: Option<&HashSet<i64>>,
) -> bool {
    if query.is_empty() {
        return true;
    }
    conversation.title.to_lowercase().contains(query)
        || matched_ids.is_some_and(|ids| ids.contains(&conversation.id))
}

pub(super) fn conversation_preview(conversation: &Conversation) -> String {
    if conversation.turn_count == 0 {
        return "No messages yet".to_owned();
    }
    truncate_single_line_text(&conversation.preview, 54)
}

pub(super) fn conversation_status(conversation: &Conversation) -> &str {
    &conversation.last_status
}

pub(super) fn status_label(status: &str) -> &'static str {
    match status {
        "completed" => "Finished",
        "queued" => "Queued",
        "in_progress" => "Waiting for response",
        "failed" => "Failed",
        "cancelled" => "Cancelled",
        "incomplete" => "Stopped at the token limit",
        "requires_action" => "Needs a tool result",
        _ => "No turns yet",
    }
}
