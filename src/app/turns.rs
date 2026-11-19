use std::fmt::Write as _;

use super::*;

impl ProGuiApp {
    pub(super) fn submit_prompt(&mut self) -> bool {
        let model = openai::normalize_model(&self.composer_settings.model);
        let provider = Provider::of(&model);
        let Some(api_key) = self.require_api_key(&model) else {
            return false;
        };

        let prompt = self.composer.trim().to_owned();
        if prompt.is_empty()
            && self.pending_attachments.is_empty()
            && self.composer_blocks.is_empty()
        {
            self.banner = Some("Write a prompt or attach an image/PDF before sending.".to_owned());
            return false;
        }

        let Some(conversation_id) = self.selected_conversation_id else {
            self.banner = Some("Select a conversation first.".to_owned());
            return false;
        };

        let (parent_response_id, foreign, history) = self
            .selected_conversation()
            .map(|conversation| {
                let ranges = self.block_ranges_for(&conversation.turns);
                let parent = chain_parent(&conversation.turns, provider);
                let foreign =
                    foreign_exchanges(&conversation.turns, parent.as_deref(), provider, &ranges);
                let history = if provider == Provider::Anthropic {
                    anthropic_history(&conversation.turns, &ranges)
                } else {
                    Vec::new()
                };
                (parent, foreign, history)
            })
            .unwrap_or_default();

        let chained_parent = parent_response_id
            .clone()
            .filter(|_| provider != Provider::Anthropic);
        let Some((final_prompt, prompt_prefix_len, block_ranges)) =
            self.assemble_prompt(conversation_id, &prompt, chained_parent.as_deref())
        else {
            return false;
        };

        let reasoning_effort = if providers::effort_options(&model)
            .contains(&self.composer_settings.effort.as_str())
        {
            self.composer_settings.effort.clone()
        } else {
            providers::default_reasoning_effort(&model).to_owned()
        };
        let max_output_tokens =
            providers::clamp_output_tokens(&model, self.composer_settings.max_output_tokens);
        let service_tier =
            providers::request_service_tier(&model, self.composer_settings.priority).to_owned();
        let background = providers::use_background_mode(&model);
        let store = providers::store_responses(&model);
        let system_prompt = self
            .selected_conversation()
            .map(|conversation| conversation.system_prompt.clone())
            .unwrap_or_default();

        let new_turn = NewTurn {
            conversation_id,
            prompt_text: final_prompt.clone(),
            prompt_prefix_len,
            model: model.clone(),
            max_output_tokens,
            reasoning_effort,
            reasoning_summary: providers::reasoning_summary(&model).to_owned(),
            service_tier,
            background,
            store,
            parent_response_id: parent_response_id.clone(),
        };
        let pending = &self.pending_attachments;
        let saved = self.db.atomically(|db| {
            let mut turn = db.insert_turn(&new_turn)?;
            db.replace_turn_blocks(turn.id, &block_ranges)?;
            for attachment in pending {
                turn.attachments.push(db.insert_attachment(
                    turn.id,
                    &attachment.mime,
                    &attachment.file_name,
                    &attachment.bytes,
                )?);
            }
            Ok(turn)
        });
        let turn = match saved {
            Ok(turn) => turn,
            Err(err) => {
                self.banner = Some(format!("Failed to save the message: {err}"));
                return false;
            }
        };

        let pending_attachments = std::mem::take(&mut self.pending_attachments);
        self.pending_textures.clear();
        let first_attachment_name = pending_attachments
            .first()
            .map(|attachment| attachment.file_name.clone());
        let image_urls = pending_attachments
            .iter()
            .map(|attachment| attachment_data_url(&attachment.mime, &attachment.bytes))
            .collect::<Vec<_>>();

        self.save_conversation_max_output_tokens(conversation_id, max_output_tokens);
        if self.selected_model != model {
            self.selected_model = model;
            self.save_selected_model_setting();
        }

        let should_rename = self.selected_conversation().is_some_and(|conversation| {
            conversation.turns.is_empty() && conversation.title == "New chat"
        });

        if should_rename {
            let title_source = if prompt.is_empty() {
                first_attachment_name.clone().unwrap_or_default()
            } else {
                prompt.clone()
            };
            let new_title = title_from_prompt(&title_source);
            if let Err(err) = self.db.rename_conversation(conversation_id, &new_title) {
                self.banner = Some(format!("Failed to rename chat: {err}"));
            } else if let Some(conversation) = self.selected_conversation_mut() {
                conversation.rename(new_title);
            }
        }

        if let Some(conversation) = self.selected_conversation_mut() {
            conversation.append_turn(turn.clone());
            conversation.updated_at = now_string();
        }

        self.conversations
            .sort_by(|left, right| right.updated_at.cmp(&left.updated_at));

        self.runtime.insert(
            turn.id,
            TurnRuntime {
                note: Some(format!("Waiting for {}...", provider.name())),
                auto_retry_attempts: 0,
            },
        );
        providers::spawn_new_turn(
            self.tx.clone(),
            turn.id,
            RequestSpec {
                model: turn.model.clone(),
                prompt: request_prompt_for_provider(provider, foreign, final_prompt),
                images: image_urls,
                instructions: system_prompt,
                max_output_tokens: turn.max_output_tokens,
                reasoning_effort: turn.reasoning_effort.clone(),
                service_tier: turn.service_tier.clone(),
                history,
                previous_response_id: parent_response_id,
            },
            api_key,
        );
        self.composer.clear();
        self.composer_blocks.clear();
        self.viewing_block = None;
        self.banner = std::mem::take(&mut self.context_reused).then(|| {
            "The repository context has not changed since this chat last sent it, so it was not sent again.".to_owned()
        });
        true
    }

    pub(super) fn assemble_prompt(
        &mut self,
        conversation_id: i64,
        typed: &str,
        chained_parent: Option<&str>,
    ) -> Option<(String, usize, Vec<BlockRange>)> {
        let mut prefix = String::new();
        self.context_reused = false;

        let context_source = self
            .conversations
            .iter()
            .find(|conversation| conversation.id == conversation_id)
            .and_then(|conversation| {
                if conversation.workspace_path.is_empty()
                    || conversation.context_selection.is_empty()
                {
                    None
                } else {
                    Some((
                        PathBuf::from(&conversation.workspace_path),
                        conversation.context_selection.clone(),
                    ))
                }
            });

        let context_source = context_source.filter(|_| !self.skip_context_once);
        self.skip_context_once = false;
        if let Some((root, context_selection)) = context_source {
            let packed = repoctx::pack_context(&root, &context_selection);
            let already_in_chain = packed.included > 0
                && chained_parent.is_some_and(|parent| {
                    self.conversations
                        .iter()
                        .find(|conversation| conversation.id == conversation_id)
                        .is_some_and(|conversation| {
                            chain_carries(&conversation.turns, parent, &packed.text)
                        })
                });
            if already_in_chain {
                self.context_reused = true;
            } else if !packed.secret_hits.is_empty() && !self.allow_secrets_once {
                let hits = packed
                    .secret_hits
                    .iter()
                    .take(20)
                    .map(|hit| format!("{}:{}  {}", hit.rel_path, hit.line, hit.kind))
                    .collect::<Vec<_>>();
                self.open_modal(AppModal::ConfirmSecrets {
                    hits,
                    total: packed.secret_hits.len(),
                });
                return None;
            }
            if !already_in_chain && !packed.missing.is_empty() {
                self.banner = Some(format!(
                    "{} context file(s) could not be read and were skipped.",
                    packed.missing.len()
                ));
            }
            if !already_in_chain && packed.included > 0 {
                prefix.push_str(&packed.text);
            }
        }
        self.allow_secrets_once = false;

        let mut blocks = Vec::with_capacity(self.composer_blocks.len() + 1);
        if !prefix.is_empty() {
            blocks.push(PromptBlock::new(
                REPOSITORY_CONTEXT_LABEL.to_owned(),
                prefix,
            ));
        }
        blocks.extend(self.composer_blocks.iter().cloned());
        Some(assemble_blocks(&blocks, typed))
    }

    pub(super) fn begin_turn_edit(&mut self, turn_id: i64) {
        let Some(turn) = self
            .conversations
            .iter()
            .flat_map(|conversation| conversation.turns.iter())
            .find(|turn| turn.id == turn_id)
        else {
            return;
        };
        if turn.is_pending() {
            self.banner =
                Some("This message is still running. Cancel it before editing.".to_owned());
            return;
        }
        let ranges = self.db.load_turn_blocks(turn_id).unwrap_or_default();
        let blocks = blocks_of_turn(turn, &ranges);
        self.editing_turn = Some(TurnEdit {
            turn_id,
            draft: split_prompt(turn).1.to_owned(),
            blocks,
            needs_focus: true,
            editor: DocumentEditor::default(),
        });
        self.banner = None;
    }

    pub(super) fn resend_edited_turn(&mut self) {
        let Some(edit) = self.editing_turn.as_ref() else {
            return;
        };
        let turn_id = edit.turn_id;
        let draft = edit.draft.trim().to_owned();
        let blocks = edit.blocks.clone();

        let Some(conversation_index) = self
            .conversations
            .iter()
            .position(|conversation| conversation.turns.iter().any(|turn| turn.id == turn_id))
        else {
            self.editing_turn = None;
            return;
        };
        let conversation_id = self.conversations[conversation_index].id;
        let Some(turn_index) = self.conversations[conversation_index]
            .turns
            .iter()
            .position(|turn| turn.id == turn_id)
        else {
            self.editing_turn = None;
            return;
        };

        let turn = &self.conversations[conversation_index].turns[turn_index];
        if turn.is_pending() {
            self.banner =
                Some("This message is still running. Cancel it before editing.".to_owned());
            return;
        }
        if draft.is_empty() && blocks.is_empty() && turn.attachments.is_empty() {
            self.banner = Some("The message cannot be empty.".to_owned());
            return;
        }
        let model = turn.model.clone();
        if self.require_api_key(&model).is_none() {
            return;
        }
        if self.conversations[conversation_index].turns[turn_index + 1..]
            .iter()
            .any(Turn::is_pending)
        {
            self.banner = Some(
                "A later message is still running. Cancel it before resending this one.".to_owned(),
            );
            return;
        }

        let (final_prompt, prompt_prefix_len, block_ranges) = assemble_blocks(&blocks, &draft);

        let parent_response_id = chain_parent(
            &self.conversations[conversation_index].turns[..turn_index],
            Provider::of(&model),
        );
        let saved = self.db.atomically(|db| {
            db.delete_turns_after(conversation_id, turn_id)?;
            db.update_turn_prompt(
                turn_id,
                &final_prompt,
                prompt_prefix_len,
                parent_response_id.as_deref(),
            )?;
            db.replace_turn_blocks(turn_id, &block_ranges)
        });
        if let Err(err) = saved {
            self.banner = Some(format!("Failed to save the edited message: {err}"));
            return;
        }
        let dropped = self.conversations[conversation_index]
            .turns
            .split_off(turn_index + 1);
        let dropped_count = dropped.len();
        for turn in &dropped {
            self.runtime.remove(&turn.id);
            self.answer_cache.remove(&turn.id);
            for attachment in &turn.attachments {
                self.attachment_textures.remove(&attachment.id);
            }
        }

        let turn = &mut self.conversations[conversation_index].turns[turn_index];
        turn.prompt_text = final_prompt;
        turn.prompt_prefix_len = prompt_prefix_len;
        turn.parent_response_id = parent_response_id;
        self.conversations[conversation_index].refresh_summary();
        self.editing_turn = None;
        self.viewing_block = None;
        self.block_editor = DocumentEditor::default();

        self.retry_turn(turn_id, false);
        self.banner = Some(match dropped_count {
            0 => "Message edited and sent again.".to_owned(),
            1 => "Message edited and sent again. The reply below it was removed.".to_owned(),
            count => format!(
                "Message edited and sent again. The {count} messages below it were removed."
            ),
        });
    }

    pub(super) fn resend_edited_turn_as_new(&mut self) {
        let Some(edit) = self.editing_turn.as_ref() else {
            return;
        };
        let turn_id = edit.turn_id;
        let Some(conversation_id) = self
            .conversations
            .iter()
            .find(|conversation| conversation.turns.iter().any(|turn| turn.id == turn_id))
            .map(|conversation| conversation.id)
        else {
            self.editing_turn = None;
            return;
        };
        if self.selected_conversation_id != Some(conversation_id) {
            let edit = self.editing_turn.take();
            self.select_conversation(conversation_id);
            self.editing_turn = edit;
        }
        let model = openai::normalize_model(&self.composer_settings.model);
        if self.require_api_key(&model).is_none() {
            return;
        }
        let Some(edit) = self.editing_turn.take() else {
            return;
        };
        let turn_has_attachments = self
            .conversations
            .iter()
            .flat_map(|conversation| conversation.turns.iter())
            .any(|turn| turn.id == turn_id && !turn.attachments.is_empty());
        if edit.draft.trim().is_empty()
            && edit.blocks.is_empty()
            && !turn_has_attachments
            && self.pending_attachments.is_empty()
        {
            self.banner = Some("The message cannot be empty.".to_owned());
            self.editing_turn = Some(edit);
            return;
        }

        let pending_before = self.pending_attachments.len();
        if self.pending_attachments.is_empty() {
            let attachments = self
                .conversations
                .iter()
                .flat_map(|conversation| conversation.turns.iter())
                .find(|turn| turn.id == turn_id)
                .map(|turn| turn.attachments.as_slice())
                .unwrap_or_default();
            match load_stored_attachments(&self.db, attachments) {
                Ok(stored) => {
                    for (meta, bytes) in stored {
                        self.add_pending_attachment(meta.file_name, meta.mime, bytes);
                    }
                }
                Err(err) => {
                    self.banner = Some(format!("Could not load an attachment to resend: {err}"));
                    self.editing_turn = Some(edit);
                    return;
                }
            }
        }

        let TurnEdit {
            turn_id,
            draft,
            blocks,
            needs_focus,
            editor,
        } = edit;
        let parked_draft = std::mem::replace(&mut self.composer, draft);
        let parked_blocks = std::mem::replace(&mut self.composer_blocks, blocks);
        self.skip_context_once = !self.composer_blocks.is_empty();

        if self.submit_prompt() {
            self.composer = parked_draft;
            self.composer_blocks = parked_blocks;
        } else if matches!(self.active_modal, Some(AppModal::ConfirmSecrets { .. })) {
            self.parked_composer = Some((parked_draft, parked_blocks));
        } else {
            let draft = std::mem::replace(&mut self.composer, parked_draft);
            let blocks = std::mem::replace(&mut self.composer_blocks, parked_blocks);
            self.skip_context_once = false;
            for removed in self.pending_attachments.drain(pending_before..) {
                self.pending_textures.remove(&removed.key);
            }
            self.editing_turn = Some(TurnEdit {
                turn_id,
                draft,
                blocks,
                needs_focus,
                editor,
            });
        }
    }

    pub(super) fn cancel_turn(&mut self, turn_id: i64) {
        let turn_meta = self
            .conversations
            .iter()
            .flat_map(|conversation| conversation.turns.iter())
            .find(|turn| turn.id == turn_id && turn.is_pending())
            .map(|turn| (turn.response_id.clone(), turn.model.clone()));

        if let Some((response_id, model)) = turn_meta {
            let api_key = if response_id.is_some() {
                let Some(api_key) = self.require_api_key(&model) else {
                    return;
                };
                api_key
            } else {
                String::new()
            };
            providers::spawn_cancel_turn(self.tx.clone(), turn_id, response_id, model, api_key);
            self.runtime.entry(turn_id).or_default().note = Some("Cancel requested...".to_owned());
        }
    }

    pub(super) fn retry_turn(&mut self, turn_id: i64, automatic: bool) {
        let Some((turn_snapshot, system_prompt, foreign, history)) =
            self.conversations.iter().find_map(|conversation| {
                let index = conversation
                    .turns
                    .iter()
                    .position(|turn| turn.id == turn_id)?;
                let turn = &conversation.turns[index];
                let provider = Provider::of(&turn.model);
                let ranges = self.block_ranges_for(&conversation.turns[..index]);
                let foreign = foreign_exchanges(
                    &conversation.turns[..index],
                    turn.parent_response_id.as_deref(),
                    provider,
                    &ranges,
                );
                let history = if provider == Provider::Anthropic {
                    anthropic_history(&conversation.turns[..index], &ranges)
                } else {
                    Vec::new()
                };
                Some((
                    turn.clone(),
                    conversation.system_prompt.clone(),
                    foreign,
                    history,
                ))
            })
        else {
            self.banner = Some("Could not find the failed turn to resend.".to_owned());
            return;
        };
        let Some(api_key) = self.require_api_key(&turn_snapshot.model) else {
            return;
        };

        let image_urls = match load_stored_attachments(&self.db, &turn_snapshot.attachments) {
            Ok(stored) => stored
                .iter()
                .map(|(meta, bytes)| attachment_data_url(&meta.mime, bytes))
                .collect::<Vec<_>>(),
            Err(err) => {
                self.banner = Some(format!("Could not load an attachment to resend: {err}"));
                return;
            }
        };
        let request_service_tier = turn_snapshot.service_tier.clone();

        if let Err(err) = self.db.reset_turn_for_retry(turn_id, &request_service_tier) {
            self.banner = Some(format!("Failed to prepare the resend: {err}"));
            return;
        }

        for conversation in &mut self.conversations {
            if let Some(turn) = conversation
                .turns
                .iter_mut()
                .find(|turn| turn.id == turn_id)
            {
                let retried_at = now_string();
                conversation.updated_at = retried_at.clone();
                turn.response_id = None;
                turn.service_tier = request_service_tier.clone();
                turn.status = "queued".to_owned();
                turn.thinking_text.clear();
                turn.answer_text.clear();
                turn.error_text = None;
                turn.input_tokens = None;
                turn.output_tokens = None;
                turn.reasoning_tokens = None;
                turn.request_started_at = retried_at.clone();
                turn.updated_at = retried_at;
                self.answer_cache.remove(&turn.id);
                break;
            }
        }
        self.conversations
            .sort_by(|left, right| right.updated_at.cmp(&left.updated_at));

        self.runtime.insert(
            turn_id,
            TurnRuntime {
                note: Some(if automatic {
                    "Previous attempt failed. Sending again...".to_owned()
                } else {
                    "Sending again...".to_owned()
                }),
                auto_retry_attempts: u8::from(automatic),
            },
        );

        providers::spawn_new_turn(
            self.tx.clone(),
            turn_snapshot.id,
            {
                let provider = Provider::of(&turn_snapshot.model);
                RequestSpec {
                    model: turn_snapshot.model,
                    prompt: request_prompt_for_provider(
                        provider,
                        foreign,
                        turn_snapshot.prompt_text,
                    ),
                    images: image_urls,
                    instructions: system_prompt,
                    max_output_tokens: turn_snapshot.max_output_tokens,
                    reasoning_effort: turn_snapshot.reasoning_effort,
                    service_tier: request_service_tier,
                    history,
                    previous_response_id: turn_snapshot.parent_response_id,
                }
            },
            api_key,
        );
        self.banner = if automatic {
            Some("The failed answer is being resent automatically.".to_owned())
        } else {
            Some("The failed answer is being resent.".to_owned())
        };
    }

    pub(super) fn resume_pending_turns(&mut self) {
        let mut restart_orphan_turns = Vec::new();
        let mut keyless = Vec::new();

        for conversation in &self.conversations {
            for turn in &conversation.turns {
                if !turn.is_pending() || self.runtime.contains_key(&turn.id) {
                    continue;
                }

                let Some(response_id) = turn.response_id.clone() else {
                    restart_orphan_turns.push(turn.id);
                    continue;
                };
                let provider = Provider::of(&turn.model);
                let api_key = self.api_key_for(provider).to_owned();
                if api_key.trim().is_empty() {
                    if !keyless.contains(&provider) {
                        keyless.push(provider);
                    }
                    continue;
                }
                self.runtime.insert(
                    turn.id,
                    TurnRuntime {
                        note: Some("Resuming background run...".to_owned()),
                        auto_retry_attempts: 0,
                    },
                );
                providers::spawn_resume_turn(
                    self.tx.clone(),
                    turn.id,
                    response_id,
                    turn.model.clone(),
                    api_key,
                );
            }
        }

        let orphan_count = restart_orphan_turns.len();
        for turn_id in restart_orphan_turns {
            self.mark_turn_needs_manual_retry(turn_id);
        }
        let mut notices = Vec::new();
        if orphan_count > 0 {
            notices.push(format!(
                "{orphan_count} pending turn(s) were marked failed after restart because no response ID had been saved yet. Use Send again to retry manually."
            ));
        }
        if !keyless.is_empty() {
            let names = keyless
                .iter()
                .map(|provider| provider.name())
                .collect::<Vec<_>>()
                .join(" and ");
            notices.push(format!("Set your {names} API key to resume pending runs."));
        }
        if !notices.is_empty() {
            self.banner = Some(notices.join(" "));
        }
    }

    pub(super) fn process_events(&mut self) {
        if let Some(result) = self.export_job.as_ref().and_then(|job| job.try_recv().ok()) {
            self.export_job = None;
            self.banner = Some(export_banner(result));
        }
        while let Ok(tagged) = self.rx.try_recv() {
            if !tagged.is_current() {
                continue;
            }
            match tagged.event {
                WorkerEvent::Note { turn_id, message } => {
                    if let Some(runtime) = self.runtime.get_mut(&turn_id) {
                        runtime.note = Some(message);
                    }
                }
                WorkerEvent::Snapshot(snapshot) => self.apply_snapshot(snapshot),
            }
        }
    }

    pub(super) fn apply_snapshot(&mut self, mut snapshot: TurnSnapshot) {
        let refreshed_at = now_string();
        let mut retry_after_update = None;
        let mut turn_found = false;
        for conversation in &mut self.conversations {
            if let Some(turn) = conversation
                .turns
                .iter_mut()
                .find(|turn| turn.id == snapshot.turn_id)
            {
                turn_found = true;
                let was_completed = turn.status == "completed";
                let gained_response_id =
                    turn.response_id.is_none() && snapshot.response_id.is_some();
                let auto_retry_attempts = self
                    .runtime
                    .get(&turn.id)
                    .map(|runtime| runtime.auto_retry_attempts)
                    .unwrap_or(0);
                let should_retry = apply_snapshot_to_turn(
                    turn,
                    &mut snapshot,
                    &refreshed_at,
                    self.auto_resend_failed_answers,
                    auto_retry_attempts,
                );
                conversation.updated_at = refreshed_at.clone();

                let persist = !turn.is_pending()
                    || gained_response_id
                    || self
                        .persisted_at
                        .get(&turn.id)
                        .is_none_or(|at| at.elapsed() >= PENDING_PERSIST_INTERVAL);
                if persist {
                    if let Err(err) = self.db.save_turn_state(turn) {
                        self.banner = Some(format!("Database update failed: {err}"));
                    }
                    if turn.is_pending() {
                        self.persisted_at.insert(turn.id, Instant::now());
                    } else {
                        self.persisted_at.remove(&turn.id);
                    }
                }

                if turn.is_pending() {
                    self.runtime.entry(turn.id).or_default().note = None;
                } else {
                    self.runtime.remove(&turn.id);
                }

                if !was_completed && turn.status == "completed" {
                    notify_completion(&conversation.title, &turn.answer_text);
                }
                if should_retry {
                    retry_after_update = Some(turn.id);
                }
                conversation.refresh_summary();
                break;
            }
        }

        if !turn_found {
            self.runtime.remove(&snapshot.turn_id);
        }

        if let Some(turn_id) = retry_after_update {
            self.retry_turn(turn_id, true);
        }
    }

    pub(super) fn mark_turn_needs_manual_retry(&mut self, turn_id: i64) {
        let message = "The app restarted before the run's response ID was saved. This turn was not resubmitted automatically to avoid creating a duplicate response. Use Send again to retry.".to_owned();
        let snapshot = TurnSnapshot {
            turn_id,
            response_id: None,
            model: None,
            status: "failed".to_owned(),
            thinking_text: String::new(),
            answer_text: String::new(),
            error_text: Some(message.clone()),
            input_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
        };

        if let Err(err) = self.db.update_turn(turn_id, &snapshot) {
            self.banner = Some(format!("Database update failed: {err}"));
            return;
        }

        let refreshed_at = now_string();
        for conversation in &mut self.conversations {
            if let Some(turn) = conversation
                .turns
                .iter_mut()
                .find(|turn| turn.id == turn_id)
            {
                turn.status = "failed".to_owned();
                turn.error_text = Some(message.clone());
                turn.thinking_text.clear();
                turn.answer_text.clear();
                turn.updated_at = refreshed_at.clone();
                conversation.updated_at = refreshed_at.clone();
                self.runtime.remove(&turn.id);
                self.answer_cache.remove(&turn.id);
                break;
            }
        }

        self.conversations
            .sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    }
}

pub(super) fn apply_snapshot_to_turn(
    turn: &mut Turn,
    snapshot: &mut TurnSnapshot,
    refreshed_at: &str,
    auto_resend_failed_answers: bool,
    auto_retry_attempts: u8,
) -> bool {
    let authoritative = snapshot.status == "completed";
    let keep_thinking = (!authoritative || snapshot.thinking_text.is_empty())
        && snapshot.thinking_text.len() < turn.thinking_text.len();
    let keep_answer = (!authoritative || snapshot.answer_text.is_empty())
        && snapshot.answer_text.len() < turn.answer_text.len();

    if let Some(response_id) = snapshot.response_id.clone() {
        turn.response_id = Some(response_id);
    }
    if let Some(model) = snapshot.model.clone() {
        turn.model = model;
    }
    turn.status = snapshot.status.clone();
    if !keep_thinking {
        turn.thinking_text = std::mem::take(&mut snapshot.thinking_text);
    }
    if !keep_answer {
        turn.answer_text = std::mem::take(&mut snapshot.answer_text);
    }
    turn.error_text = snapshot.error_text.clone();
    if snapshot.input_tokens.is_some() {
        turn.input_tokens = snapshot.input_tokens;
    }
    if snapshot.output_tokens.is_some() {
        turn.output_tokens = snapshot.output_tokens;
    }
    if snapshot.reasoning_tokens.is_some() {
        turn.reasoning_tokens = snapshot.reasoning_tokens;
    }
    turn.updated_at = refreshed_at.to_owned();

    turn.status == "failed"
        && turn.answer_text.trim().is_empty()
        && auto_resend_failed_answers
        && auto_retry_attempts == 0
}
pub(super) fn chain_parent(turns: &[Turn], provider: Provider) -> Option<String> {
    turns
        .iter()
        .rev()
        .find(|turn| {
            turn.status == "completed"
                && turn.response_id.is_some()
                && Provider::of(&turn.model) == provider
        })
        .and_then(|turn| turn.response_id.clone())
}

pub(super) fn foreign_exchanges(
    turns: &[Turn],
    parent: Option<&str>,
    provider: Provider,
    ranges: &HashMap<i64, Vec<BlockRange>>,
) -> String {
    let start = parent
        .and_then(|parent| {
            turns
                .iter()
                .position(|turn| turn.response_id.as_deref() == Some(parent))
        })
        .map_or(0, |index| index + 1);

    let mut exchanges = String::new();
    for turn in &turns[start..] {
        if turn.status != "completed" || Provider::of(&turn.model) == provider {
            continue;
        }
        let images = match turn.attachments.len() {
            0 => String::new(),
            count => format!("\n[{count} attached image(s)]"),
        };
        let _ = write!(
            exchanges,
            "<user>\n{}{images}\n</user>\n<assistant model=\"{}\">\n{}\n</assistant>\n\n",
            history_text(turn, ranges),
            turn.model,
            turn.answer_text.trim()
        );
    }
    if exchanges.is_empty() {
        return exchanges;
    }
    format!(
        "Earlier messages in this conversation were answered by another model, so they are not in your history. Here they are for context:\n\n{exchanges}The new message follows.\n\n---\n\n"
    )
}

pub(super) fn request_prompt(foreign_exchanges: String, prompt: String) -> String {
    if foreign_exchanges.is_empty() {
        prompt
    } else {
        foreign_exchanges + &prompt
    }
}

pub(super) fn request_prompt_for_provider(
    provider: Provider,
    foreign_exchanges: String,
    prompt: String,
) -> String {
    if provider == Provider::Anthropic {
        prompt
    } else {
        request_prompt(foreign_exchanges, prompt)
    }
}

pub(super) fn anthropic_history(
    turns: &[Turn],
    ranges: &HashMap<i64, Vec<BlockRange>>,
) -> Vec<HistoryMessage> {
    let mut history = Vec::new();
    for turn in turns.iter().filter(|turn| turn.status == "completed") {
        let images = match turn.attachments.len() {
            0 => String::new(),
            count => format!("\n[{count} attached image(s)]"),
        };
        history.push(HistoryMessage {
            role: HistoryRole::User,
            text: format!("{}{}", history_text(turn, ranges), images),
        });
        history.push(HistoryMessage {
            role: HistoryRole::Assistant,
            text: turn.answer_text.trim().to_owned(),
        });
    }
    history
}

pub(super) fn history_text(turn: &Turn, ranges: &HashMap<i64, Vec<BlockRange>>) -> String {
    let mut text = String::new();
    for range in ranges.get(&turn.id).into_iter().flatten() {
        if range.label == REPOSITORY_CONTEXT_LABEL {
            continue;
        }
        if let Some(body) = turn.prompt_text.get(range.start..range.end) {
            let _ = write!(text, "## {}\n\n{}\n\n", range.label, body.trim_end());
        }
    }
    text.push_str(split_prompt(turn).1.trim());
    text
}

pub(super) fn chain_carries(turns: &[Turn], parent: &str, packed: &str) -> bool {
    let mut next = Some(parent);
    for _ in 0..turns.len() {
        let Some(id) = next else {
            return false;
        };
        let Some(turn) = turns
            .iter()
            .find(|turn| turn.response_id.as_deref() == Some(id))
        else {
            return false;
        };
        if split_prompt(turn).0.contains(packed) {
            return true;
        }
        next = turn.parent_response_id.as_deref();
    }
    false
}

pub(super) fn split_prompt(turn: &Turn) -> (&str, &str) {
    let text = turn.prompt_text.as_str();
    let recorded = turn.prompt_prefix_len;
    if recorded > 0 && recorded <= text.len() && text.is_char_boundary(recorded) {
        return text.split_at(recorded);
    }
    if text.starts_with(PACKED_CONTEXT_HEADER)
        && let Some(index) = text.find(PACKED_CONTEXT_SEPARATOR)
    {
        return text.split_at(index + PACKED_CONTEXT_SEPARATOR.len());
    }
    ("", text)
}

pub(super) fn notify_completion(chat_title: &str, answer_text: &str) {
    let title = "Pro GUI";
    let subtitle = format!("Reply completed in {chat_title}");
    let body = truncate_text(&answer_text.replace('\n', " "), 120);

    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification {} with title {} subtitle {}\nbeep 1",
            apple_script_string(&body),
            apple_script_string(title),
            apple_script_string(&subtitle),
        );
        if let Ok(mut child) = Command::new("osascript").arg("-e").arg(script).spawn() {
            std::thread::spawn(move || child.wait());
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = std::io::Write::write_all(&mut std::io::stderr(), b"\x07");
        let _ = (title, subtitle, body);
    }
}

pub(super) fn apple_script_string(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}
