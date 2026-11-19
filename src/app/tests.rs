use super::context_panel::*;
use super::header::*;
use super::transcript::*;
use super::*;

fn turn_with(prompt_text: &str, prompt_prefix_len: usize) -> Turn {
    Turn {
        id: 1,
        prompt_text: prompt_text.to_owned(),
        prompt_prefix_len,
        attachments: Vec::new(),
        thinking_text: String::new(),
        answer_text: String::new(),
        model: String::new(),
        max_output_tokens: 1_024,
        reasoning_effort: String::new(),
        reasoning_summary: String::new(),
        service_tier: String::new(),
        background: false,
        store: false,
        response_id: None,
        parent_response_id: None,
        status: "completed".to_owned(),
        error_text: None,
        input_tokens: None,
        output_tokens: None,
        reasoning_tokens: None,
        created_at: String::new(),
        request_started_at: String::new(),
        updated_at: String::new(),
    }
}

fn answered(model: &str, response_id: &str, prompt: &str, answer: &str) -> Turn {
    Turn {
        model: model.to_owned(),
        response_id: Some(response_id.to_owned()),
        answer_text: answer.to_owned(),
        ..turn_with(prompt, 0)
    }
}

#[test]
fn each_api_continues_its_own_chain() {
    let turns = [
        answered("gpt-6-astra-pro", "resp_1", "First?", "One."),
        answered("gemini-4-argon", "v1_2", "Second?", "Two."),
    ];
    assert_eq!(
        chain_parent(&turns, Provider::OpenAi).as_deref(),
        Some("resp_1")
    );
    assert_eq!(
        chain_parent(&turns, Provider::Gemini).as_deref(),
        Some("v1_2")
    );
    assert_eq!(chain_parent(&turns[..1], Provider::Gemini), None);
}

#[test]
fn switching_api_carries_the_exchanges_its_chain_never_saw() {
    let turns = [
        answered("gpt-6-astra-pro", "resp_1", "First?", "One."),
        answered("gemini-4-argon", "v1_2", "Second?", "Two."),
        answered("gpt-6-astra-pro", "resp_3", "Third?", "Three."),
    ];
    let foreign = foreign_exchanges(&turns, Some("v1_2"), Provider::Gemini, &HashMap::new());
    assert!(foreign.contains("Third?") && foreign.contains("Three."));
    assert!(!foreign.contains("First?") && !foreign.contains("Two."));

    let foreign = foreign_exchanges(&turns[..1], None, Provider::Gemini, &HashMap::new());
    assert!(foreign.contains("<assistant model=\"gpt-6-astra-pro\">\nOne.\n</assistant>"));
    assert!(request_prompt(foreign, "Next?".to_owned()).ends_with("---\n\nNext?"));

    assert_eq!(
        foreign_exchanges(
            &turns[..1],
            Some("resp_1"),
            Provider::OpenAi,
            &HashMap::new()
        ),
        ""
    );
    assert_eq!(request_prompt(String::new(), "Hi".to_owned()), "Hi");
}

#[test]
fn anthropic_uses_the_full_completed_transcript_as_history() {
    let mut pending = answered("gpt-6-astra-pro", "resp_3", "Third?", "Three.");
    pending.status = "in_progress".to_owned();
    let turns = [
        answered("gpt-6-astra-pro", "resp_1", "First?", "One."),
        answered("gemini-4-argon", "v1_2", "Second?", "Two."),
        pending,
    ];

    assert_eq!(
        anthropic_history(&turns, &HashMap::new()),
        vec![
            HistoryMessage {
                role: HistoryRole::User,
                text: "First?".to_owned(),
            },
            HistoryMessage {
                role: HistoryRole::Assistant,
                text: "One.".to_owned(),
            },
            HistoryMessage {
                role: HistoryRole::User,
                text: "Second?".to_owned(),
            },
            HistoryMessage {
                role: HistoryRole::Assistant,
                text: "Two.".to_owned(),
            },
        ]
    );
}

#[test]
fn anthropic_uses_the_prompt_without_foreign_text() {
    assert_eq!(
        request_prompt_for_provider(
            Provider::Anthropic,
            "Earlier answers".to_owned(),
            "Next?".to_owned()
        ),
        "Next?"
    );
    assert!(
        request_prompt_for_provider(
            Provider::OpenAi,
            "Earlier answers".to_owned(),
            "Next?".to_owned()
        )
        .contains("Earlier answers")
    );
}

#[test]
fn the_latest_thought_is_the_last_titled_section() {
    let thinking =
        "**Planning**\n\nFirst I map it.\n\n**Checking the **edge** cases**\n\nNow the tails.";
    assert_eq!(
        latest_thought(thinking),
        "**Checking the **edge** cases**\n\nNow the tails."
    );
    assert_eq!(latest_thought("one\n\ntwo"), "two");
}

fn paint_turn(turn: &Turn, query: &str, current: Option<usize>) -> (usize, bool) {
    let ctx = Context::default();
    let _ = ctx.run(Default::default(), |_| {});

    let mut answer_cache = HashMap::new();
    let textures = HashMap::new();
    let mut editing = None;
    let mut seen = 0;
    let mut scrolled = false;

    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(900.0, 700.0),
        )),
        ..Default::default()
    };
    let _ = ctx.run(raw, |ctx| {
        CentralPanel::default().show(ctx, |ui| {
            let mut search = InChatSearchRender::new(query, current, current.is_some());
            ProGuiApp::render_turn(
                ui,
                turn,
                None,
                true,
                true,
                false,
                true,
                0,
                &mut editing,
                &mut search,
                &mut answer_cache,
                &textures,
            );
            seen = search.seen_matches;
            scrolled = search.scrolled_to_current;
        });
    });
    (seen, scrolled)
}

#[test]
fn every_counted_match_can_be_navigated_to() {
    let mut turn = turn_with("ask about the needle in the prompt", 0);
    turn.answer_text = "## Heading with a needle\n\n\
         Some prose mentioning a needle again.\n\n\
         ```rust\nlet needle = 1; // needle\n```\n\n\
         **needle** in bold, and > a quoted needle\n\n\
         - a needle in a list\n"
        .to_owned();
    turn.thinking_text = "reasoning that also says needle once".to_owned();

    let (total, _) = paint_turn(&turn, "needle", None);
    assert!(total > 3, "expected several painted matches, got {total}");

    for index in 0..total {
        let (seen, scrolled) = paint_turn(&turn, "needle", Some(index));
        assert_eq!(seen, total, "the painted total moved between frames");
        assert!(
            scrolled,
            "match {} of {total} was never reachable, Next would stop here",
            index + 1
        );
    }
}

#[test]
fn counting_raw_text_overshoots_what_markdown_paints() {
    let mut turn = turn_with("prompt", 0);
    turn.answer_text = "## First heading\n\n## Second heading\n\nplain body\n".to_owned();

    let raw = bigtext::find_all(&turn.answer_text, "##").len();
    let (painted, _) = paint_turn(&turn, "##", None);

    assert!(raw >= 2, "the fixture should contain heading markers");
    assert!(
        painted < raw,
        "markdown syntax is painted after all ({painted} vs {raw}); this test \
         no longer demonstrates why the total must come from rendering"
    );
}

#[test]
fn searching_a_huge_message_stays_cheap() {
    let prefix = format!(
        "# Repository context: demo\n\n{}{PACKED_CONTEXT_SEPARATOR}",
        "fn needle() {} // filler filler filler\n".repeat(300_000)
    );
    assert!(
        prefix.len() > 10_000_000,
        "the fixture must be genuinely large"
    );

    let turn = turn_with(&format!("{prefix}explain the needle"), prefix.len());
    assert!(bigtext::needs_document_editor(split_prompt(&turn).0));

    let started = Instant::now();
    let (seen, _) = paint_turn(&turn, "needle", Some(0));
    let elapsed = started.elapsed();

    assert_eq!(
        seen,
        bigtext::find_all(&turn.prompt_text, "needle").len(),
        "the summary must account for every match in the message"
    );
    assert!(
        elapsed < Duration::from_millis(120),
        "searching a {} MB message took {elapsed:?}; it is being laid out, not summarised",
        turn.prompt_text.len() / 1_048_576
    );
}

#[test]
fn a_sent_message_keeps_every_block_editable() {
    let blocks = vec![
        PromptBlock::new("notes".to_owned(), "alpha\nbeta".to_owned()),
        PromptBlock::new("code".to_owned(), "fn main() {}".to_owned()),
    ];
    let (prompt, prefix, ranges) = assemble_blocks(&blocks, "Review this.");
    let turn = turn_with(&prompt, prefix);

    assert_eq!(split_prompt(&turn).1, "Review this.");
    let recovered = blocks_of_turn(&turn, &ranges);
    assert_eq!(recovered.len(), 2);
    assert_eq!(recovered[0].label, "notes");
    assert_eq!(recovered[0].text, "alpha\nbeta");
    assert_eq!(recovered[1].text, "fn main() {}");

    let mut edited = recovered;
    edited[0].text = "ALPHA\nbeta\ngamma".to_owned();
    let (next, next_prefix, next_ranges) = assemble_blocks(&edited, "Review this again.");
    let next_turn = turn_with(&next, next_prefix);
    let round_tripped = blocks_of_turn(&next_turn, &next_ranges);

    assert_eq!(round_tripped[0].text, "ALPHA\nbeta\ngamma");
    assert_eq!(round_tripped[1].text, "fn main() {}");
    assert_eq!(split_prompt(&next_turn).1, "Review this again.");
}

#[test]
fn messages_sent_before_blocks_existed_are_still_fully_editable() {
    let assembled = format!("# Repository context: demo\n\nfiles{PACKED_CONTEXT_SEPARATOR}");
    let turn = turn_with(&format!("{assembled}Explain."), 0);
    let recovered = blocks_of_turn(&turn, &[]);
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].text, "# Repository context: demo\n\nfiles");
    assert_eq!(split_prompt(&turn).1, "Explain.");

    let plain = turn_with("just text", 0);
    assert!(blocks_of_turn(&plain, &[]).is_empty());
    assert_eq!(split_prompt(&plain).1, "just text");
}

#[test]
fn a_recorded_prefix_splits_exactly_where_the_app_assembled_it() {
    let assembled = "## Pasted block 1: notes\n\nhuge\n\n---\n\nUser request:\n\n";
    let turn = turn_with(&format!("{assembled}Explain this."), assembled.len());
    assert_eq!(split_prompt(&turn), (assembled, "Explain this."));
}

#[test]
fn turns_stored_before_the_prefix_existed_fall_back_to_the_marker() {
    let packed = repoctx::pack_context(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")),
        &["Cargo.toml".to_owned()],
    );
    let stored = format!(
        "{}{PACKED_CONTEXT_SEPARATOR}Explain this file.",
        packed.text
    );
    let turn = turn_with(&stored, 0);
    assert_eq!(split_prompt(&turn).1, "Explain this file.");
}

#[test]
fn plain_prompts_are_never_split() {
    let typed = "Rewrite the diff.\n\n---\n\nUser request:\n\nkeep this line";
    assert_eq!(split_prompt(&turn_with(typed, 0)), ("", typed));
    assert_eq!(split_prompt(&turn_with("", 0)), ("", ""));
    assert_eq!(split_prompt(&turn_with("abc", 99)), ("", "abc"));
}

#[test]
fn search_ranges_are_case_insensitive_and_non_overlapping() {
    assert_eq!(
        search_match_ranges("Hello WORLD hello", "hello"),
        vec![(0, 5), (12, 17)]
    );
    assert_eq!(search_match_ranges("aaaa", "aa"), vec![(0, 2), (2, 4)]);
    assert_eq!(search_match_ranges("abc", "  "), Vec::new());
    assert_eq!(search_match_ranges("ab", "abcd"), Vec::new());
    let text = "cafétería café";
    for (start, end) in search_match_ranges(text, "café") {
        assert!(text.is_char_boundary(start) && text.is_char_boundary(end));
        assert_eq!(&text[start..end], "café");
    }
}

#[test]
fn multiline_previews_are_bounded_but_keep_short_text_whole() {
    let short = "one\ntwo\nthree";
    assert_eq!(
        truncate_multiline(short, 1400, 18),
        (short.to_owned(), false)
    );

    let (preview, truncated) = truncate_multiline(&"x".repeat(2_000_000), 1400, 18);
    assert!(truncated);
    assert!(
        preview.len() < 2_000,
        "preview was not bounded: {}",
        preview.len()
    );

    let many_lines = (0..40)
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let (preview, truncated) = truncate_multiline(&many_lines, 1400, 18);
    assert!(truncated);
    assert_eq!(preview.lines().filter(|l| !l.is_empty()).count(), 19);
}

#[test]
fn auto_resend_only_fires_when_no_answer_text_was_received() {
    let failed_snapshot = |answer: &str| TurnSnapshot {
        turn_id: 1,
        response_id: Some("v1_argon".to_owned()),
        model: Some("gemini-4-argon".to_owned()),
        status: "failed".to_owned(),
        thinking_text: "**Thinking**".to_owned(),
        answer_text: answer.to_owned(),
        error_text: Some("context limit exceeded".to_owned()),
        input_tokens: Some(800_000),
        output_tokens: Some(200_000),
        reasoning_tokens: Some(50_000),
    };

    let mut turn = turn_with("huge prompt", 0);
    turn.status = "in_progress".to_owned();
    let mut snap = failed_snapshot("Partial answer from Argon before failing.");
    assert!(!apply_snapshot_to_turn(
        &mut turn, &mut snap, "now", true, 0
    ));
    assert_eq!(
        turn.answer_text,
        "Partial answer from Argon before failing."
    );

    let mut turn = turn_with("huge prompt", 0);
    turn.status = "in_progress".to_owned();
    turn.thinking_text = "**Earlier thoughts**".to_owned();
    turn.answer_text = "Streamed partial answer already on screen.".to_owned();
    let mut empty_fail = TurnSnapshot {
        thinking_text: String::new(),
        answer_text: String::new(),
        ..failed_snapshot("")
    };
    assert!(!apply_snapshot_to_turn(
        &mut turn,
        &mut empty_fail,
        "now",
        true,
        0
    ));
    assert_eq!(
        turn.answer_text,
        "Streamed partial answer already on screen."
    );
    assert_eq!(turn.thinking_text, "**Earlier thoughts**");

    let mut turn = turn_with("huge prompt", 0);
    turn.status = "in_progress".to_owned();
    let mut no_answer = failed_snapshot("   ");
    assert!(apply_snapshot_to_turn(
        &mut turn,
        &mut no_answer,
        "now",
        true,
        0
    ));
    assert!(!apply_snapshot_to_turn(
        &mut turn,
        &mut no_answer,
        "now",
        true,
        1
    ));
    assert!(!apply_snapshot_to_turn(
        &mut turn,
        &mut no_answer,
        "now",
        false,
        0
    ));
}

#[test]
fn thinking_block_renders_markdown_and_shows_realtime_token_header() {
    let mut turn = turn_with("prompt", 0);
    assert_eq!(thinking_header_label(&turn, 0), "Thinking");

    turn.thinking_text = "**Step 1**\n\nAnalyzing the request.".to_owned();
    assert_eq!(thinking_header_label(&turn, 420), "Thinking (420 tokens)");

    turn.reasoning_tokens = Some(106_700);
    assert_eq!(
        thinking_header_label(&turn, 420),
        "Thinking (106.7k tokens)"
    );
    turn.reasoning_tokens = Some(1_250_000);
    assert_eq!(thinking_header_label(&turn, 420), "Thinking (1.25M tokens)");

    let mut turn = turn_with("prompt", 0);
    turn.thinking_text =
        "## Reasoning heading\n\n```rust\nlet x = 42;\n```\n\nplain body\n".to_owned();
    let raw = bigtext::find_all(&turn.thinking_text, "##").len();
    let (painted, _) = paint_turn(&turn, "##", None);
    assert!(raw >= 1);
    assert_eq!(
        painted, 0,
        "Thinking block painted raw markdown syntax (`##`) instead of rendering markdown blocks"
    );
}

#[test]
fn usage_totals_ignore_failed_and_uncompleted_turns() {
    let mut ok = turn_with("first", 0);
    ok.status = "completed".to_owned();
    ok.input_tokens = Some(918_465);
    ok.output_tokens = Some(8_820);
    ok.reasoning_tokens = Some(3_954);

    let mut failed = turn_with("second", 0);
    failed.status = "failed".to_owned();
    failed.input_tokens = Some(5_917_422);
    failed.output_tokens = Some(125_352);
    failed.reasoning_tokens = Some(106_717);

    let mut cancelled = turn_with("third", 0);
    cancelled.status = "cancelled".to_owned();
    cancelled.input_tokens = Some(500_000);

    assert_eq!(
        conversation_usage_totals(&[ok, failed.clone(), cancelled]),
        (918_465, 8_820, 3_954)
    );
    assert_eq!(conversation_usage_totals(&[failed]), (0, 0, 0));
}

#[test]
fn pdf_paths_are_accepted_as_attachments() {
    assert_eq!(
        attachment_mime_for_path(std::path::Path::new("report.pdf")),
        Some("application/pdf")
    );
    assert_eq!(
        attachment_mime_for_path(std::path::Path::new("REPORT.PDF")),
        Some("application/pdf")
    );
    assert_eq!(
        attachment_mime_for_path(std::path::Path::new("photo.png")),
        Some("image/png")
    );
    assert_eq!(
        attachment_mime_for_path(std::path::Path::new("archive.zip")),
        None
    );
}

#[test]
fn switching_model_takes_the_new_models_whole_output_limit() {
    let gpt = ComposerSettings {
        model: "gpt-6-astra-pro".to_owned(),
        effort: "max".to_owned(),
        max_output_tokens: 128_000,
        priority: true,
    };

    let carbon = composer_settings_for_model(&gpt, "gemini-4-carbon".to_owned());
    assert_eq!(carbon.max_output_tokens, 1_048_576);
    assert_eq!(carbon.effort, "high");
    assert!(carbon.priority);

    let back = composer_settings_for_model(&carbon, "gpt-6-astra-pro".to_owned());
    assert_eq!(back.max_output_tokens, 128_000);

    let claude = composer_settings_for_model(&carbon, "claude-fable-5-2".to_owned());
    assert_eq!(claude.max_output_tokens, 128_000);
}

#[test]
fn token_limits_read_and_print_compactly() {
    assert_eq!(format_token_input(1_048_576), "1.05M");
    assert_eq!(format_token_input(1_000_000), "1M");
    assert_eq!(format_token_input(128_000), "128k");
    assert_eq!(format_token_input(65_536), "65.5k");
    assert_eq!(format_token_input(4_096), "4096");

    let limit = 1_048_576;
    assert_eq!(parse_token_input("128k", limit), Some(128_000.0));
    assert_eq!(parse_token_input(" 1M ", limit), Some(1_000_000.0));
    assert_eq!(parse_token_input("1.05m", limit), Some(1_050_000.0));
    assert_eq!(parse_token_input("1,048,576", limit), Some(1_048_576.0));
    assert_eq!(parse_token_input("MAX", limit), Some(1_048_576.0));
    assert_eq!(parse_token_input("lots", limit), None);
}

#[test]
fn transcript_and_composer_share_one_column() {
    assert_eq!(column_width(1_600.0), CONTENT_MAX_WIDTH);
    assert_eq!(column_width(960.0), 912.0);
    assert_eq!(column_width(200.0), 300.0);
}

#[test]
fn an_empty_chat_says_so_under_its_title() {
    assert_eq!(chat_subtitle(&[], (0, 0, 0)), "No messages yet");
}

#[test]
fn the_context_tree_collapses_filters_and_selects_by_folder() {
    let tree = repoctx::scan_workspace(std::path::Path::new(env!("CARGO_MANIFEST_DIR")));
    let selection: HashSet<String> = ["src/main.rs".to_owned()].into_iter().collect();
    let no_extensions = BTreeSet::new();
    let cache = HashMap::new();
    let view = |query: &str| ContextView {
        tree: &tree,
        filter: repoctx::PathFilter::parse(query),
        extensions: &no_extensions,
        selected_only: false,
        selection: &selection,
        token_cache: &cache,
    };
    let row_names = |rows: &[ContextRow]| {
        rows.iter()
            .map(|row| match row {
                ContextRow::Dir { path, .. } => path.clone(),
                ContextRow::File { index, .. } => tree.files[*index].rel_path.clone(),
            })
            .collect::<Vec<_>>()
    };

    let unfiltered = view("");
    let mut rows = Vec::new();
    build_context_rows(
        &unfiltered,
        &tree.tree,
        "",
        0,
        &BTreeSet::new(),
        true,
        &mut rows,
    );
    let names = row_names(&rows);
    assert!(names.contains(&"src/".to_owned()));
    assert!(!names.contains(&"src/main.rs".to_owned()));
    let Some(ContextRow::Dir { stats, .. }) = rows
        .iter()
        .find(|row| matches!(row, ContextRow::Dir { path, .. } if path == "src/"))
    else {
        panic!("src/ row missing");
    };
    assert_eq!(stats.selected, 1);
    assert!(stats.files > 5);

    let filtered = view("main.rs");
    let mut rows = Vec::new();
    let totals = build_context_rows(
        &filtered,
        &tree.tree,
        "",
        0,
        &BTreeSet::new(),
        true,
        &mut rows,
    );
    assert_eq!(row_names(&rows), ["src/", "src/main.rs"]);
    assert_eq!(filtered.selectable_in("src/", &totals), ["src/main.rs"]);
}

#[test]
fn a_lowered_limit_does_not_survive_a_model_switch() {
    let argon = ComposerSettings {
        model: "gemini-4-argon".to_owned(),
        effort: "low".to_owned(),
        max_output_tokens: 32_768,
        priority: false,
    };
    let carbon = composer_settings_for_model(&argon, "gemini-4-carbon".to_owned());
    assert_eq!(carbon.max_output_tokens, 1_048_576);
}

#[test]
fn only_paths_inside_home_are_shortened() {
    assert_eq!(
        relative_to_home("/Users/al/code/app", "/Users/al"),
        "~/code/app"
    );
    assert_eq!(relative_to_home("/Users/al", "/Users/al"), "~");
    assert_eq!(
        relative_to_home("/Users/alice/code", "/Users/al"),
        "/Users/alice/code"
    );
}

#[test]
fn exporting_all_chats_streams_valid_json_with_every_turn() {
    let dir = std::env::temp_dir().join(format!("pro-gui-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("source.sqlite3");
    {
        let db = Database::open(&db_path).unwrap();
        for title in ["First", "Second"] {
            let chat = db.create_conversation(title, None).unwrap();
            db.insert_turn(&NewTurn {
                conversation_id: chat,
                prompt_text: format!("question for {title}"),
                prompt_prefix_len: 0,
                model: "gpt-6-astra-pro".to_owned(),
                max_output_tokens: 1_024,
                reasoning_effort: "max".to_owned(),
                reasoning_summary: "auto".to_owned(),
                service_tier: "default".to_owned(),
                background: true,
                store: true,
                parent_response_id: None,
            })
            .unwrap();
        }
    }

    let written = export_all_chats(&db_path, &dir).unwrap();
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&written).unwrap()).unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    let chats = json["conversations"].as_array().unwrap();
    assert_eq!(chats.len(), 2);
    assert!(
        chats
            .iter()
            .all(|chat| chat["turns"].as_array().unwrap().len() == 1)
    );
    assert!(json["exported_at"].is_string());
}

#[test]
fn a_time_stamp_shows_month_day_and_minute_without_a_panic() {
    assert_eq!(relative_time("2026-11-18 21:45:10"), "11-18 21:45");
    assert_eq!(relative_time("2026-11-18"), "2026-11-18");
    assert_eq!(relative_time("éééééééé"), "éééééééé");
}

#[test]
fn a_missing_attachment_fails_the_load_instead_of_being_dropped() {
    let dir = std::env::temp_dir().join(format!("pro-gui-attachments-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Database::open(&dir.join("source.sqlite3")).unwrap();
    let chat = db.create_conversation("Chat", None).unwrap();
    let turn = db
        .insert_turn(&NewTurn {
            conversation_id: chat,
            prompt_text: "What is in the picture?".to_owned(),
            prompt_prefix_len: 0,
            model: "gpt-6-astra-pro".to_owned(),
            max_output_tokens: 1_024,
            reasoning_effort: "max".to_owned(),
            reasoning_summary: "auto".to_owned(),
            service_tier: "default".to_owned(),
            background: true,
            store: true,
            parent_response_id: None,
        })
        .unwrap();
    let saved = db
        .insert_attachment(turn.id, "image/png", "cat.png", b"png bytes")
        .unwrap();
    let missing = AttachmentMeta {
        id: saved.id + 1,
        ..saved.clone()
    };

    let loaded = load_stored_attachments(&db, std::slice::from_ref(&saved));
    let with_missing = load_stored_attachments(&db, &[saved, missing]);
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);

    let loaded = loaded.unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].0.file_name, "cat.png");
    assert_eq!(loaded[0].1, b"png bytes");
    assert!(with_missing.is_err());
}

#[test]
fn a_failed_export_names_the_file_it_could_not_write() {
    let dir = std::env::temp_dir().join(format!("pro-gui-export-fail-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("source.sqlite3");
    drop(Database::open(&db_path).unwrap());

    let result = export_all_chats(&db_path, &dir.join("missing-folder"));
    let _ = std::fs::remove_dir_all(&dir);

    let banner = export_banner(result);
    assert!(
        banner.starts_with("Export failed: could not create "),
        "{banner}"
    );
    assert!(banner.contains("missing-folder"), "{banner}");
}

#[test]
fn history_keeps_pasted_blocks_but_not_the_repository_context() {
    let blocks = vec![
        PromptBlock::new(
            REPOSITORY_CONTEXT_LABEL.to_owned(),
            "# Repository context: app".to_owned(),
        ),
        PromptBlock::new("crash log".to_owned(), "panic at line 7".to_owned()),
    ];
    let (prompt, prefix_len, ranges) = assemble_blocks(&blocks, "Why does it crash?");
    let turn = Turn {
        id: 7,
        status: "completed".to_owned(),
        ..turn_with(&prompt, prefix_len)
    };
    let ranges = HashMap::from([(turn.id, ranges)]);

    let text = history_text(&turn, &ranges);
    assert!(text.contains("panic at line 7"));
    assert!(text.ends_with("Why does it crash?"));
    assert!(!text.contains("# Repository context"));
    assert_eq!(history_text(&turn, &HashMap::new()), "Why does it crash?");
}

#[test]
fn unchanged_context_is_recognised_only_along_the_real_chain() {
    let packed = "# Repository context: app\nfn main() {}";
    let (prompt, prefix_len, _) = assemble_blocks(
        &[PromptBlock::new(
            REPOSITORY_CONTEXT_LABEL.to_owned(),
            packed.to_owned(),
        )],
        "First?",
    );
    let first = Turn {
        id: 1,
        response_id: Some("resp_1".to_owned()),
        ..turn_with(&prompt, prefix_len)
    };
    let second = Turn {
        id: 2,
        response_id: Some("resp_2".to_owned()),
        parent_response_id: Some("resp_1".to_owned()),
        ..turn_with("Second?", 0)
    };
    let detached = Turn {
        id: 3,
        response_id: Some("resp_3".to_owned()),
        ..turn_with("Third?", 0)
    };
    let turns = [first, second, detached];

    assert!(chain_carries(&turns, "resp_2", packed));
    assert!(!chain_carries(
        &turns,
        "resp_2",
        "# Repository context: app\nchanged"
    ));
    assert!(!chain_carries(&turns, "resp_3", packed));
}
