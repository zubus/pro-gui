use super::*;

fn document(lines: usize) -> String {
    (0..lines)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn counts_lines_the_way_an_editor_does() {
    assert_eq!(count_lines(""), 1);
    assert_eq!(count_lines("a"), 1);
    assert_eq!(count_lines("a\n"), 1);
    assert_eq!(count_lines("a\nb"), 2);
    assert_eq!(count_lines("a\nb\n"), 2);
    assert_eq!(count_lines("\n"), 1);
}

#[test]
fn a_delta_is_the_smallest_change_not_the_whole_window() {
    let delta = Delta::between(100, "hello world", "hello brave world").unwrap();
    assert_eq!(delta.at, 106);
    assert_eq!(delta.removed, "");
    assert_eq!(delta.inserted, "brave ");
    assert!(Delta::between(0, "same", "same").is_none());

    let delta = Delta::between(0, "caf\u{e9} x", "caf\u{e9} y").unwrap();
    assert_eq!(delta.removed, "x");
    assert_eq!(delta.inserted, "y");
}

#[test]
fn undo_and_redo_walk_the_document_back_and_forward() {
    let mut text = "one two three".to_owned();
    let mut history = History::default();

    history.record(0, &text, "one TWO three");
    text = "one TWO three".to_owned();
    history.record(0, &text, "one TWO four");
    text = "one TWO four".to_owned();

    assert!(history.can_undo());
    history.undo(&mut text);
    assert_eq!(text, "one TWO three");
    history.undo(&mut text);
    assert_eq!(text, "one two three");
    assert!(!history.can_undo());
    assert!(history.can_redo());

    history.redo(&mut text);
    assert_eq!(text, "one TWO three");
    history.redo(&mut text);
    assert_eq!(text, "one TWO four");
}

#[test]
fn typing_coalesces_into_one_undo_step() {
    let mut text = "start".to_owned();
    let mut history = History::default();
    for character in "ing here".chars() {
        let next = format!("{text}{character}");
        history.record(0, &text, &next);
        text = next;
    }
    assert_eq!(text, "starting here");
    assert_eq!(history.undo.len(), 1, "a run of typing is one step");
    history.undo(&mut text);
    assert_eq!(text, "start");
}

#[test]
fn find_and_replace_run_over_the_whole_document() {
    let mut text = "Alpha beta ALPHA gamma alpha".to_owned();
    assert_eq!(find_all(&text, "alpha").len(), 3);
    assert_eq!(find_all(&text, "zzz").len(), 0);
    assert_eq!(find_all(&text, "  "), Vec::new());

    let mut history = History::default();
    let replaced = replace_all(&mut text, &mut history, "alpha", "OMEGA");
    assert_eq!(replaced, 3);
    assert_eq!(text, "OMEGA beta OMEGA gamma OMEGA");

    history.undo(&mut text);
    assert_eq!(text, "Alpha beta ALPHA gamma alpha");
}

#[test]
fn find_matches_go_stale_with_the_document_not_its_length() {
    let mut text = "alpha beta alpha".to_owned();
    let mut find = Find {
        query: "alpha".to_owned(),
        ..Default::default()
    };
    let mut revision = 1;
    find.refresh(&text, revision);
    assert_eq!(find.matches.len(), 2);

    text = "alpha beta XXXXX".to_owned();
    revision += 1;
    find.refresh(&text, revision);
    assert_eq!(
        find.matches.len(),
        1,
        "matches survived an edit that kept the length"
    );

    text = "alpha alpha alpha".to_owned();
    find.refresh(&text, revision);
    assert_eq!(find.matches.len(), 1);
}

#[test]
fn small_text_needs_no_document_editor() {
    assert!(!needs_document_editor("short"));
    assert!(!needs_document_editor(&document(DOCUMENT_LINES)));
    assert!(needs_document_editor(&document(DOCUMENT_LINES + 1)));
    assert!(needs_document_editor(&"x".repeat(DOCUMENT_BYTES + 1)));
}

#[test]
fn a_stale_undo_never_splits_a_character() {
    let mut history = History::default();
    history.record(1, "", "b");
    let mut text = "\u{e9}".to_owned();
    history.undo(&mut text);
    assert_eq!(text, "\u{e9}");
}
