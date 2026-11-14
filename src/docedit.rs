use std::ops::Range;

use eframe::egui::{
    self, Color32, FontFamily, FontId, Rect, Response, Sense, Stroke, Ui, text::LayoutJob,
};

use crate::bigtext::{Find, History};

const OVERSCAN_ROWS: usize = 2;

pub struct DocumentEditor {
    line_starts: Vec<usize>,
    indexed_len: usize,
    pub cursor: usize,
    pub anchor: usize,
    preferred_column: Option<usize>,
    measured_width: f32,
    preedit: Option<Range<usize>>,
    pub history: History,
    pub find: Find,
    pub revision: u64,
    pub focused: bool,
    pub return_key: Option<egui::KeyboardShortcut>,
    wants_focus: bool,
    caret_rect: Option<Rect>,
    scroll_to_cursor: bool,
}

impl Default for DocumentEditor {
    fn default() -> Self {
        Self {
            line_starts: vec![0],
            indexed_len: usize::MAX,
            cursor: 0,
            anchor: 0,
            preferred_column: None,
            measured_width: 0.0,
            preedit: None,
            history: History::default(),
            find: Find::default(),
            revision: 0,
            focused: false,
            return_key: None,
            wants_focus: false,
            caret_rect: None,
            scroll_to_cursor: false,
        }
    }
}

impl DocumentEditor {
    pub fn request_focus(&mut self) {
        self.wants_focus = true;
    }

    pub fn sync(&mut self, text: &str) {
        if self.indexed_len != text.len() {
            self.rebuild(text);
        }
    }

    pub fn invalidate(&mut self) {
        self.indexed_len = usize::MAX;
    }

    fn rebuild(&mut self, text: &str) {
        self.line_starts.clear();
        self.line_starts.push(0);
        self.line_starts.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(index, _)| index + 1),
        );
        self.indexed_len = text.len();
        self.cursor = clamp_to_boundary(text, self.cursor);
        self.anchor = clamp_to_boundary(text, self.anchor);
    }

    fn reindex(&mut self, at: usize, removed: usize, inserted: &str) {
        let first = self.line_of(at) + 1;
        let removal_end = at + removed;
        let mut last = first;
        while last < self.line_starts.len() && self.line_starts[last] <= removal_end {
            last += 1;
        }

        let fresh: Vec<usize> = inserted
            .bytes()
            .enumerate()
            .filter(|(_, byte)| *byte == b'\n')
            .map(|(index, _)| at + index + 1)
            .collect();
        let shift = inserted.len() as isize - removed as isize;
        let tail = first + fresh.len();
        self.line_starts.splice(first..last, fresh);
        for start in &mut self.line_starts[tail..] {
            *start = start.saturating_add_signed(shift);
        }
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn line_of(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(line) => line,
            Err(next) => next.saturating_sub(1),
        }
    }

    pub fn line_range(&self, text: &str, line: usize) -> Range<usize> {
        let start = self.line_starts.get(line).copied().unwrap_or(text.len());
        let end = match self.line_starts.get(line + 1) {
            Some(next) => next.saturating_sub(1),
            None => text.len(),
        };
        start..end.max(start)
    }

    pub fn selection(&self) -> Range<usize> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }

    pub fn has_selection(&self) -> bool {
        self.cursor != self.anchor
    }

    pub fn select_all(&mut self, text: &str) {
        self.anchor = 0;
        self.cursor = text.len();
        self.scroll_to_cursor = true;
    }

    pub fn move_to(&mut self, text: &str, offset: usize, extend: bool) {
        self.cursor = clamp_to_boundary(text, offset);
        if !extend {
            self.anchor = self.cursor;
        }
        self.scroll_to_cursor = true;
    }

    pub fn replace(&mut self, text: &mut String, range: Range<usize>, insert: &str) {
        self.edit(text, range, insert, true);
    }

    fn edit_silently(&mut self, text: &mut String, range: Range<usize>, insert: &str) {
        self.edit(text, range, insert, false);
    }

    fn edit(&mut self, text: &mut String, range: Range<usize>, insert: &str, record: bool) {
        let start = clamp_to_boundary(text, range.start);
        let end = clamp_to_boundary(text, range.end).max(start);
        if start == end && insert.is_empty() {
            return;
        }

        if record {
            self.history.record(start, &text[start..end], insert);
        }
        text.replace_range(start..end, insert);
        self.reindex(start, end - start, insert);
        self.indexed_len = text.len();
        self.cursor = start + insert.len();
        self.anchor = self.cursor;
        self.preferred_column = None;
        self.revision += 1;
        self.scroll_to_cursor = true;
    }

    pub fn insert(&mut self, text: &mut String, insert: &str) {
        let range = if self.has_selection() {
            self.selection()
        } else {
            self.cursor..self.cursor
        };
        self.replace(text, range, insert);
    }

    fn delete(&mut self, text: &mut String, backwards: bool, by_word: bool) {
        let range = if self.has_selection() {
            self.selection()
        } else if backwards {
            let from = if by_word {
                word_start(text, self.cursor)
            } else {
                previous_boundary(text, self.cursor)
            };
            from..self.cursor
        } else {
            let to = if by_word {
                word_end(text, self.cursor)
            } else {
                next_boundary(text, self.cursor)
            };
            self.cursor..to
        };
        if !range.is_empty() {
            self.replace(text, range, "");
        }
    }

    fn undo_redo(&mut self, text: &mut String, redo: bool) {
        let at = if redo {
            self.history.redo(text)
        } else {
            self.history.undo(text)
        };
        if let Some(at) = at {
            self.invalidate();
            self.sync(text);
            self.move_to(text, at, false);
            self.revision += 1;
        }
    }

    pub fn offset_at(&self, text: &str, line: usize, column: usize) -> usize {
        let range = self.line_range(text, line);
        match text[range.clone()].char_indices().nth(column) {
            Some((index, _)) => range.start + index,
            None => range.end,
        }
    }

    pub fn column_of(&self, text: &str, offset: usize) -> usize {
        let start = self.line_starts[self.line_of(offset)];
        text[start..offset.max(start)].chars().count()
    }

    fn move_vertically(&mut self, text: &str, rows: isize, extend: bool) {
        let line = self.line_of(self.cursor);
        let column = self
            .preferred_column
            .unwrap_or_else(|| self.column_of(text, self.cursor));
        let target = line
            .saturating_add_signed(rows)
            .min(self.line_count().saturating_sub(1));
        let offset = self.offset_at(text, target, column);
        self.move_to(text, offset, extend);
        self.preferred_column = Some(column);
    }

    fn move_horizontally(&mut self, text: &str, forward: bool, by_word: bool, extend: bool) {
        if !extend && self.has_selection() {
            let selection = self.selection();
            let offset = if forward {
                selection.end
            } else {
                selection.start
            };
            self.move_to(text, offset, false);
            self.preferred_column = None;
            return;
        }
        let offset = match (forward, by_word) {
            (true, true) => word_end(text, self.cursor),
            (true, false) => next_boundary(text, self.cursor),
            (false, true) => word_start(text, self.cursor),
            (false, false) => previous_boundary(text, self.cursor),
        };
        self.move_to(text, offset, extend);
        self.preferred_column = None;
    }

    pub fn show(&mut self, ui: &mut Ui, text: &mut String, height: f32, salt: &str) -> Response {
        self.sync(text);

        let size = ui
            .style()
            .text_styles
            .get(&egui::TextStyle::Monospace)
            .map_or(14.0, |font| font.size);
        let font = FontId::new(size - 1.0, FontFamily::Monospace);
        let points_per_pixel = ui.ctx().pixels_per_point();
        let row_height = (ui.fonts_mut(|fonts| fonts.row_height(&font)) * points_per_pixel).round()
            / points_per_pixel;
        let id = ui.make_persistent_id(salt);

        let mut response = egui::ScrollArea::both()
            .id_salt(salt)
            .max_height(height)
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                self.show_viewport(ui, text, id, &font, row_height, viewport)
            })
            .inner;

        if std::mem::take(&mut self.wants_focus) {
            ui.memory_mut(|memory| memory.request_focus(id));
        }
        self.focused = ui.memory(|memory| memory.has_focus(id));
        if self.focused {
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    id,
                    egui::EventFilter {
                        tab: false,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: false,
                    },
                );
            });
            if self.handle_input(ui, text) {
                response.mark_changed();
            }
            let cursor_rect = self.caret_rect.unwrap_or_else(|| {
                Rect::from_min_size(response.rect.min, egui::vec2(1.0, row_height))
            });
            ui.ctx().output_mut(|output| {
                output.ime = Some(egui::output::IMEOutput {
                    rect: response.rect,
                    cursor_rect,
                });
            });
        }
        response
    }

    fn show_viewport(
        &mut self,
        ui: &mut Ui,
        text: &str,
        id: egui::Id,
        font: &FontId,
        row_height: f32,
        viewport: Rect,
    ) -> Response {
        let lines = self.line_count();
        self.caret_rect = None;
        let content_width = self.measured_width.max(ui.available_width());
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(content_width, lines as f32 * row_height),
            Sense::click_and_drag(),
        );
        let response = response.on_hover_cursor(egui::CursorIcon::Text);

        let first =
            ((viewport.min.y / row_height).floor().max(0.0) as usize).saturating_sub(OVERSCAN_ROWS);
        let last = (((viewport.max.y / row_height).ceil() as usize) + OVERSCAN_ROWS).min(lines);

        let selection = self.selection();
        let selection_fill = ui.visuals().selection.bg_fill;
        let text_color = ui.visuals().widgets.inactive.text_color();
        let painter = ui.painter().to_owned();
        let matches = &self.find.matches;
        let mut widest: f32 = 0.0;

        for line in first..last {
            let line_range = self.line_range(text, line);
            let top = rect.min.y + line as f32 * row_height;
            let galley = layout_line(
                ui,
                &text[line_range.clone()],
                font.clone(),
                text_color,
                matches,
                line_range.start,
            );
            widest = widest.max(galley.size().x);

            if selection.start < line_range.end.max(selection.start)
                && selection.end > line_range.start
            {
                let from = selection.start.max(line_range.start);
                let to = selection.end.min(line_range.end);
                let x0 = galley
                    .pos_from_cursor(char_cursor(text, line_range.start, from))
                    .min
                    .x;
                let mut x1 = galley
                    .pos_from_cursor(char_cursor(text, line_range.start, to))
                    .min
                    .x;
                if selection.end > line_range.end {
                    x1 += row_height * 0.4;
                }
                painter.rect_filled(
                    Rect::from_min_max(
                        egui::pos2(rect.min.x + x0, top),
                        egui::pos2(rect.min.x + x1, top + row_height),
                    ),
                    0.0,
                    selection_fill,
                );
            }

            painter.galley(egui::pos2(rect.min.x, top), galley.clone(), text_color);

            if self.focused && !self.has_selection() && self.line_of(self.cursor) == line {
                let x = galley
                    .pos_from_cursor(char_cursor(text, line_range.start, self.cursor))
                    .min
                    .x;
                let caret = [
                    egui::pos2(rect.min.x + x, top),
                    egui::pos2(rect.min.x + x, top + row_height),
                ];
                self.caret_rect = Some(Rect::from_two_pos(caret[0], caret[1]));
                painter.line_segment(
                    caret,
                    Stroke::new(1.0_f32, ui.visuals().text_cursor.stroke.color),
                );
            }
        }
        self.measured_width = self.measured_width.max(widest + 24.0);

        if response.is_pointer_button_down_on()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let offset = self.offset_at_pos(ui, text, rect, font, row_height, pos);
            let extend = response.dragged() || ui.input(|input| input.modifiers.shift);
            self.move_to(text, offset, extend);
            self.scroll_to_cursor = false;
            ui.memory_mut(|memory| memory.request_focus(id));
        }
        if response.double_clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let offset = self.offset_at_pos(ui, text, rect, font, row_height, pos);
            self.anchor = word_start(text, next_boundary(text, offset));
            self.cursor = word_end(text, self.anchor);
        }

        if self.scroll_to_cursor {
            let line = self.line_of(self.cursor);
            let top = rect.min.y + line as f32 * row_height;
            ui.scroll_to_rect(
                Rect::from_min_size(egui::pos2(rect.min.x, top), egui::vec2(8.0, row_height)),
                None,
            );
            self.scroll_to_cursor = false;
        }

        response
    }

    fn offset_at_pos(
        &self,
        ui: &Ui,
        text: &str,
        rect: Rect,
        font: &FontId,
        row_height: f32,
        pos: egui::Pos2,
    ) -> usize {
        let line = (((pos.y - rect.min.y) / row_height).floor().max(0.0) as usize)
            .min(self.line_count().saturating_sub(1));
        let range = self.line_range(text, line);
        let galley = layout_line(
            ui,
            &text[range.clone()],
            font.clone(),
            Color32::WHITE,
            &[],
            range.start,
        );
        let ccursor = galley.cursor_from_pos(egui::vec2(pos.x - rect.min.x, 0.0));
        self.offset_at(text, line, ccursor.index)
    }

    fn handle_input(&mut self, ui: &mut Ui, text: &mut String) -> bool {
        let before = self.revision;
        let events = ui.input(|input| input.events.clone());

        for event in events {
            match event {
                egui::Event::Text(typed) if !typed.is_empty() => {
                    self.finish_preedit(text);
                    self.insert(text, &typed);
                }
                egui::Event::Paste(pasted) => {
                    self.finish_preedit(text);
                    self.insert(text, &pasted);
                }
                egui::Event::Copy => {
                    if ui.input(|input| input.modifiers.shift) {
                        ui.ctx().copy_text(text.clone());
                    } else if self.has_selection() {
                        ui.ctx().copy_text(text[self.selection()].to_owned());
                    }
                }
                egui::Event::Cut => {
                    if self.has_selection() {
                        ui.ctx().copy_text(text[self.selection()].to_owned());
                        self.replace(text, self.selection(), "");
                    }
                }
                egui::Event::Ime(egui::ImeEvent::Preedit(preview)) => {
                    let range = self.preedit.clone().unwrap_or(self.cursor..self.cursor);
                    self.edit_silently(text, range, &preview);
                    let start = self.cursor - preview.len();
                    self.preedit = (!preview.is_empty()).then_some(start..self.cursor);
                }
                egui::Event::Ime(egui::ImeEvent::Commit(committed)) => {
                    self.finish_preedit(text);
                    self.insert(text, &committed);
                }
                egui::Event::Ime(egui::ImeEvent::Disabled) => {
                    self.finish_preedit(text);
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => self.handle_key(text, key, modifiers),
                _ => {}
            }
        }
        self.revision != before
    }

    fn finish_preedit(&mut self, text: &mut String) {
        if let Some(range) = self.preedit.take() {
            self.edit_silently(text, range, "");
        }
    }

    fn handle_key(&mut self, text: &mut String, key: egui::Key, modifiers: egui::Modifiers) {
        let extend = modifiers.shift;
        let by_word = modifiers.alt;
        let to_edge = modifiers.command;

        match key {
            egui::Key::ArrowLeft => {
                if to_edge {
                    let line = self.line_of(self.cursor);
                    let offset = self.line_range(text, line).start;
                    self.move_to(text, offset, extend);
                } else {
                    self.move_horizontally(text, false, by_word, extend);
                }
            }
            egui::Key::ArrowRight => {
                if to_edge {
                    let line = self.line_of(self.cursor);
                    let offset = self.line_range(text, line).end;
                    self.move_to(text, offset, extend);
                } else {
                    self.move_horizontally(text, true, by_word, extend);
                }
            }
            egui::Key::ArrowUp => {
                if to_edge {
                    self.move_to(text, 0, extend);
                } else {
                    self.move_vertically(text, -1, extend);
                }
            }
            egui::Key::ArrowDown => {
                if to_edge {
                    let end = text.len();
                    self.move_to(text, end, extend);
                } else {
                    self.move_vertically(text, 1, extend);
                }
            }
            egui::Key::PageUp => self.move_vertically(text, -30, extend),
            egui::Key::PageDown => self.move_vertically(text, 30, extend),
            egui::Key::Home => {
                let offset = if to_edge {
                    0
                } else {
                    self.line_range(text, self.line_of(self.cursor)).start
                };
                self.move_to(text, offset, extend);
            }
            egui::Key::End => {
                let offset = if to_edge {
                    text.len()
                } else {
                    self.line_range(text, self.line_of(self.cursor)).end
                };
                self.move_to(text, offset, extend);
            }
            egui::Key::Backspace => {
                self.finish_preedit(text);
                self.delete(text, true, by_word || to_edge);
            }
            egui::Key::Delete => {
                self.finish_preedit(text);
                self.delete(text, false, by_word || to_edge);
            }
            egui::Key::Enter
                if !modifiers.command
                    && self
                        .return_key
                        .is_none_or(|shortcut| modifiers.matches_logically(shortcut.modifiers)) =>
            {
                self.finish_preedit(text);
                self.insert(text, "\n");
            }
            egui::Key::A if to_edge => self.select_all(text),
            egui::Key::Z if to_edge => self.undo_redo(text, modifiers.shift),
            egui::Key::Y if to_edge => self.undo_redo(text, true),
            egui::Key::F if to_edge => {
                self.find.open = true;
                self.find.needs_focus = true;
            }
            _ => {}
        }
    }

    pub fn reveal(&mut self, text: &str, offset: usize, select_len: usize) {
        self.cursor = clamp_to_boundary(text, offset + select_len);
        self.anchor = clamp_to_boundary(text, offset);
        self.scroll_to_cursor = true;
    }
}

fn layout_line(
    ui: &Ui,
    line: &str,
    font: FontId,
    color: Color32,
    matches: &[(usize, usize)],
    line_start: usize,
) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob {
        text: line.to_owned(),
        break_on_newline: false,
        ..Default::default()
    };
    job.wrap.max_width = f32::INFINITY;

    let plain = egui::TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    let hit = egui::TextFormat {
        font_id: font,
        color: Color32::BLACK,
        background: Color32::from_rgb(255, 214, 102),
        ..Default::default()
    };

    let line_end = line_start + line.len();
    let mut cursor = 0;
    let first = matches.partition_point(|(_, end)| *end <= line_start);
    for (start, end) in &matches[first..] {
        if *start >= line_end {
            break;
        }
        let from = start.saturating_sub(line_start).max(cursor);
        let to = (end - line_start).min(line.len());
        if to <= from || !line.is_char_boundary(from) || !line.is_char_boundary(to) {
            continue;
        }
        if from > cursor {
            job.sections.push(egui::text::LayoutSection {
                leading_space: 0.0,
                byte_range: cursor..from,
                format: plain.clone(),
            });
        }
        job.sections.push(egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: from..to,
            format: hit.clone(),
        });
        cursor = to;
    }
    if cursor < line.len() || job.sections.is_empty() {
        job.sections.push(egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: cursor..line.len(),
            format: plain,
        });
    }
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}

fn char_cursor(text: &str, line_start: usize, offset: usize) -> egui::text::CCursor {
    let offset = offset.clamp(line_start, text.len());
    egui::text::CCursor::new(text[line_start..offset].chars().count())
}

fn clamp_to_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn next_boundary(text: &str, offset: usize) -> usize {
    let offset = clamp_to_boundary(text, offset);
    text[offset..]
        .chars()
        .next()
        .map(|character| offset + character.len_utf8())
        .unwrap_or(offset)
}

fn previous_boundary(text: &str, offset: usize) -> usize {
    let offset = clamp_to_boundary(text, offset);
    text[..offset]
        .chars()
        .next_back()
        .map(|character| offset - character.len_utf8())
        .unwrap_or(0)
}

fn is_word_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

fn word_start(text: &str, offset: usize) -> usize {
    let mut at = clamp_to_boundary(text, offset);
    while at > 0 {
        let previous = previous_boundary(text, at);
        let character = text[previous..at].chars().next().unwrap_or(' ');
        if is_word_char(character) {
            break;
        }
        at = previous;
    }
    while at > 0 {
        let previous = previous_boundary(text, at);
        let character = text[previous..at].chars().next().unwrap_or(' ');
        if !is_word_char(character) {
            break;
        }
        at = previous;
    }
    at
}

fn word_end(text: &str, offset: usize) -> usize {
    let mut at = clamp_to_boundary(text, offset);
    while at < text.len() {
        let character = text[at..].chars().next().unwrap_or(' ');
        if is_word_char(character) {
            break;
        }
        at += character.len_utf8();
    }
    while at < text.len() {
        let character = text[at..].chars().next().unwrap_or(' ');
        if !is_word_char(character) {
            break;
        }
        at += character.len_utf8();
    }
    at
}

#[cfg(test)]
mod tests;
