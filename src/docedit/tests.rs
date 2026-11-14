use super::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next() % bound as u64) as usize
        }
    }
}

fn editor_for(text: &str) -> DocumentEditor {
    let mut editor = DocumentEditor::default();
    editor.sync(text);
    editor
}

fn assert_index_is_truthful(editor: &DocumentEditor, text: &str) {
    let mut expected = vec![0];
    expected.extend(
        text.bytes()
            .enumerate()
            .filter(|(_, byte)| *byte == b'\n')
            .map(|(index, _)| index + 1),
    );
    assert_eq!(editor.line_starts, expected, "line index drifted");
    assert!(
        text.is_char_boundary(editor.cursor),
        "cursor off a boundary"
    );
    assert!(
        text.is_char_boundary(editor.anchor),
        "anchor off a boundary"
    );
}

#[test]
fn the_line_index_survives_arbitrary_edits() {
    for seed in 1..=40u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut text = "alpha\nbeta\ngamma\ndelta\n".repeat(6);
        let mut editor = editor_for(&text);

        for step in 0..120 {
            let at = rng.below(text.len() + 1);
            let len = rng.below(12);
            let end = (at + len).min(text.len());
            let insert = match rng.below(4) {
                0 => "".to_owned(),
                1 => "\n".to_owned(),
                2 => "x".repeat(rng.below(6)),
                _ => format!("{}\n{}", "y".repeat(rng.below(4)), "z".repeat(rng.below(4))),
            };
            editor.replace(&mut text, at..end, &insert);
            assert_index_is_truthful(&editor, &text);
            assert!(
                editor.cursor <= text.len(),
                "seed {seed} step {step}: cursor past the end"
            );
        }
    }
}

#[test]
fn undo_and_redo_return_the_document_exactly() {
    for seed in 1..=25u64 {
        let mut rng = Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D));
        let mut text = "one two three\nfour five six\nseven\n".repeat(4);
        let original = text.clone();
        let mut editor = editor_for(&text);

        let mut snapshots = vec![text.clone()];
        for _ in 0..40 {
            let at = rng.below(text.len() + 1);
            let end = (at + rng.below(8)).min(text.len());
            let insert = if rng.below(2) == 0 {
                "\n".to_owned()
            } else {
                "Q".repeat(rng.below(5))
            };
            editor.replace(&mut text, at..end, &insert);
            snapshots.push(text.clone());
        }

        while editor.history.can_undo() {
            editor.undo_redo(&mut text, false);
            assert_index_is_truthful(&editor, &text);
        }
        assert_eq!(text, original, "seed {seed}: undo did not reach the start");

        while editor.history.can_redo() {
            editor.undo_redo(&mut text, true);
            assert_index_is_truthful(&editor, &text);
        }
        assert_eq!(
            text,
            *snapshots.last().unwrap(),
            "seed {seed}: redo did not reach the end"
        );
    }
}

#[test]
fn multibyte_documents_never_split_a_character() {
    let mut text = "caf\u{e9} na\u{ef}ve\n\u{6f22}\u{5b57}\u{306e}\u{884c}\ndone\n".to_owned();
    let mut editor = editor_for(&text);

    for offset in 0..=text.len() {
        let clamped = clamp_to_boundary(&text, offset);
        assert!(text.is_char_boundary(clamped));
    }

    editor.move_to(&text, 0, false);
    for _ in 0..text.chars().count() {
        editor.move_horizontally(&text, true, false, false);
        assert!(text.is_char_boundary(editor.cursor));
    }
    assert_eq!(editor.cursor, text.len());

    editor.move_to(&text, 3, false);
    editor.insert(&mut text, "\u{1f600}");
    assert_index_is_truthful(&editor, &text);
    assert!(text.contains('\u{1f600}'));
}

#[test]
fn vertical_movement_keeps_its_column_across_short_lines() {
    let text = "aaaaaaaaaa\nbb\ncccccccccc\n".to_owned();
    let mut editor = editor_for(&text);

    editor.move_to(&text, 8, false);
    editor.move_vertically(&text, 1, false);
    assert_eq!(editor.column_of(&text, editor.cursor), 2);
    editor.move_vertically(&text, 1, false);
    assert_eq!(
        editor.column_of(&text, editor.cursor),
        8,
        "the column was not remembered across the short line"
    );
}

#[test]
fn word_navigation_lands_on_word_edges() {
    let text = "  hello world_42, adios  ".to_owned();
    assert_eq!(word_end(&text, 0), 7);
    assert_eq!(word_end(&text, 7), 16);
    assert_eq!(word_start(&text, 16), 8);
    assert_eq!(word_start(&text, 7), 2);
    assert_eq!(word_end(&text, text.len()), text.len());
    assert_eq!(word_start(&text, 0), 0);
}

#[test]
fn lines_offsets_and_columns_round_trip() {
    let text = "first\nsecond line\n\nfourth\n".to_owned();
    let editor = editor_for(&text);
    assert_eq!(editor.line_count(), 5);

    for offset in 0..=text.len() {
        let line = editor.line_of(offset);
        let column = editor.column_of(&text, offset);
        let back = editor.offset_at(&text, line, column);
        assert_eq!(back, offset, "offset {offset} did not round-trip");
        let range = editor.line_range(&text, line);
        assert!(range.contains(&offset) || offset == range.end);
    }
}

#[test]
fn selection_edits_replace_exactly_what_was_selected() {
    let mut text = "keep REPLACE keep".to_owned();
    let mut editor = editor_for(&text);
    editor.anchor = 5;
    editor.cursor = 12;
    assert_eq!(&text[editor.selection()], "REPLACE");

    editor.insert(&mut text, "new");
    assert_eq!(text, "keep new keep");
    assert_eq!(editor.cursor, 8);
    assert!(!editor.has_selection());

    editor.undo_redo(&mut text, false);
    assert_eq!(text, "keep REPLACE keep");
}

struct Harness {
    ctx: egui::Context,
    clock: f64,
}

impl Harness {
    fn new() -> Self {
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |_| {});
        Self { ctx, clock: 0.0 }
    }

    fn frame(&mut self, editor: &mut DocumentEditor, text: &mut String, events: Vec<egui::Event>) {
        let _ = self.frame_output(editor, text, events, egui::Modifiers::NONE);
    }

    fn frame_output(
        &mut self,
        editor: &mut DocumentEditor,
        text: &mut String,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) -> egui::FullOutput {
        self.clock += 1.0;
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            time: Some(self.clock),
            events,
            modifiers,
            ..Default::default()
        };
        self.ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let id = ui.make_persistent_id("probe");
                ui.memory_mut(|memory| memory.request_focus(id));
                editor.show(ui, text, 400.0, "probe");
            });
        })
    }
}

fn key(key: egui::Key, modifiers: egui::Modifiers) -> Vec<egui::Event> {
    vec![egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }]
}

fn big_document(lines: usize) -> String {
    (0..lines)
        .map(|index| format!("line {index} of the document"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_widget_types_and_undoes_anywhere_in_the_document() {
    let mut harness = Harness::new();
    let mut text = big_document(40_000);
    let original = text.clone();
    let mut editor = DocumentEditor::default();

    harness.frame(&mut editor, &mut text, vec![]);

    let offset = text.find("line 30000").unwrap();
    editor.move_to(&text, offset, false);
    harness.frame(
        &mut editor,
        &mut text,
        vec![egui::Event::Text("MARK".to_owned())],
    );
    assert!(text.contains("MARKline 30000"), "the edit did not land");

    editor.move_to(&text, 0, false);
    harness.frame(
        &mut editor,
        &mut text,
        vec![egui::Event::Text("TOP".to_owned())],
    );
    assert!(text.starts_with("TOP"));

    harness.frame(
        &mut editor,
        &mut text,
        key(egui::Key::Z, egui::Modifiers::COMMAND),
    );
    assert!(!text.starts_with("TOP"), "first undo did not apply");
    harness.frame(
        &mut editor,
        &mut text,
        key(egui::Key::Z, egui::Modifiers::COMMAND),
    );
    assert_eq!(text, original, "undo did not reach the second edit");
    assert_index_is_truthful(&editor, &text);
}

#[test]
fn select_all_reaches_the_whole_document_not_a_slice() {
    let mut harness = Harness::new();
    let mut text = big_document(20_000);
    let mut editor = DocumentEditor::default();
    harness.frame(&mut editor, &mut text, vec![]);

    harness.frame(
        &mut editor,
        &mut text,
        key(egui::Key::A, egui::Modifiers::COMMAND),
    );
    assert_eq!(
        editor.selection(),
        0..text.len(),
        "select all covered only part of the document"
    );

    harness.frame(
        &mut editor,
        &mut text,
        vec![egui::Event::Text("replaced".to_owned())],
    );
    assert_eq!(text, "replaced");
    harness.frame(
        &mut editor,
        &mut text,
        key(egui::Key::Z, egui::Modifiers::COMMAND),
    );
    assert_eq!(editor.line_count(), 20_000);
}

#[test]
fn ime_composition_commits_once_into_the_document() {
    let mut harness = Harness::new();
    let mut text = big_document(5_000);
    let original_len = text.len();
    let mut editor = DocumentEditor::default();
    harness.frame(&mut editor, &mut text, vec![]);
    editor.move_to(&text, 0, false);

    harness.frame(
        &mut editor,
        &mut text,
        vec![egui::Event::Ime(egui::ImeEvent::Enabled)],
    );
    harness.frame(
        &mut editor,
        &mut text,
        vec![egui::Event::Ime(egui::ImeEvent::Preedit(
            "\u{304b}".to_owned(),
        ))],
    );
    harness.frame(
        &mut editor,
        &mut text,
        vec![egui::Event::Ime(egui::ImeEvent::Preedit(
            "\u{304b}\u{3093}".to_owned(),
        ))],
    );
    harness.frame(
        &mut editor,
        &mut text,
        vec![egui::Event::Ime(egui::ImeEvent::Commit(
            "\u{6f22}".to_owned(),
        ))],
    );
    harness.frame(
        &mut editor,
        &mut text,
        vec![egui::Event::Ime(egui::ImeEvent::Disabled)],
    );

    assert_eq!(
        text.matches('\u{6f22}').count(),
        1,
        "committed more than once"
    );
    assert!(!text.contains('\u{304b}'), "a preedit survived the commit");
    assert_eq!(
        text.len(),
        original_len + '\u{6f22}'.len_utf8(),
        "the document changed by more than the committed character"
    );
    assert_index_is_truthful(&editor, &text);
}

#[test]
fn undoing_an_ime_commit_restores_the_text_before_composition() {
    let mut harness = Harness::new();
    let mut text = big_document(200);
    let original = text.clone();
    let mut editor = DocumentEditor::default();
    harness.frame(&mut editor, &mut text, vec![]);
    editor.move_to(&text, 0, false);

    harness.frame(
        &mut editor,
        &mut text,
        vec![egui::Event::Ime(egui::ImeEvent::Preedit(
            "\u{304b}\u{3093}".to_owned(),
        ))],
    );
    harness.frame(
        &mut editor,
        &mut text,
        vec![egui::Event::Ime(egui::ImeEvent::Commit(
            "\u{6f22}".to_owned(),
        ))],
    );
    assert!(text.starts_with('\u{6f22}'));

    harness.frame(
        &mut editor,
        &mut text,
        key(egui::Key::Z, egui::Modifiers::COMMAND),
    );
    assert_eq!(
        text, original,
        "undo brought back preedit text that was never in the document"
    );
}

#[test]
fn an_abandoned_composition_leaves_nothing_behind() {
    let mut harness = Harness::new();
    let mut text = big_document(200);
    let original = text.clone();
    let mut editor = DocumentEditor::default();
    harness.frame(&mut editor, &mut text, vec![]);
    editor.move_to(&text, 0, false);

    harness.frame(
        &mut editor,
        &mut text,
        vec![egui::Event::Ime(egui::ImeEvent::Preedit(
            "\u{304b}\u{304d}".to_owned(),
        ))],
    );
    assert!(text.contains('\u{304b}'), "the preedit should be visible");
    harness.frame(
        &mut editor,
        &mut text,
        vec![egui::Event::Ime(egui::ImeEvent::Disabled)],
    );
    assert_eq!(
        text, original,
        "an abandoned composition was left in the document"
    );
}

#[test]
fn deleting_backwards_walks_characters_then_words() {
    let mut text = "hello world".to_owned();
    let mut editor = editor_for(&text);
    editor.move_to(&text, text.len(), false);

    editor.delete(&mut text, true, false);
    assert_eq!(text, "hello worl");
    editor.delete(&mut text, true, true);
    assert_eq!(text, "hello ");
    assert_index_is_truthful(&editor, &text);
}

#[test]
fn a_focused_document_asks_the_platform_for_ime() {
    let mut harness = Harness::new();
    let mut text = big_document(50);
    let mut editor = DocumentEditor::default();
    harness.frame(&mut editor, &mut text, vec![]);
    let output = harness.frame_output(&mut editor, &mut text, vec![], egui::Modifiers::NONE);
    let ime = output
        .platform_output
        .ime
        .expect("a focused document editor must enable IME");
    assert!(ime.rect.contains(ime.cursor_rect.min));
}

#[test]
fn copy_with_shift_copies_the_whole_document() {
    let mut harness = Harness::new();
    let mut text = big_document(30);
    let mut editor = DocumentEditor::default();
    harness.frame(&mut editor, &mut text, vec![]);
    editor.move_to(&text, 0, false);

    let output = harness.frame_output(
        &mut editor,
        &mut text,
        vec![egui::Event::Copy],
        egui::Modifiers::SHIFT | egui::Modifiers::COMMAND,
    );
    let copied = output
        .platform_output
        .commands
        .iter()
        .find_map(|command| match command {
            egui::OutputCommand::CopyText(copied) => Some(copied.clone()),
            _ => None,
        });
    assert_eq!(copied.as_deref(), Some(text.as_str()));
}
