use super::*;

pub(super) fn parse_answer_blocks(text: &str, latex_enabled: bool) -> Vec<AnswerBlock> {
    let mut paragraph = Vec::new();
    let mut code_block = String::new();
    let mut code_lang = String::new();
    let mut math_block = String::new();
    let mut fence: Option<usize> = None;
    let mut in_math_block = false;
    let mut blocks = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim_start();
        let ticks = trimmed.chars().take_while(|ch| *ch == '`').count();

        if let Some(opening) = fence {
            if ticks >= opening && trimmed[ticks..].trim().is_empty() {
                blocks.push(AnswerBlock::Code {
                    language: std::mem::take(&mut code_lang),
                    code: std::mem::take(&mut code_block),
                });
                fence = None;
            } else {
                if !code_block.is_empty() {
                    code_block.push('\n');
                }
                code_block.push_str(line);
            }
            continue;
        }

        if in_math_block {
            if trimmed == "\\]" {
                blocks.push(AnswerBlock::Math(std::mem::take(&mut math_block)));
                in_math_block = false;
            } else {
                if !math_block.is_empty() {
                    math_block.push('\n');
                }
                math_block.push_str(line);
            }
            continue;
        }

        if ticks >= 3 {
            flush_paragraph_block(&mut blocks, &mut paragraph);
            code_lang = trimmed[ticks..].trim().to_owned();
            fence = Some(ticks);
            continue;
        }

        if latex_enabled && trimmed == "\\[" {
            flush_paragraph_block(&mut blocks, &mut paragraph);
            in_math_block = true;
            math_block.clear();
            continue;
        }

        if trimmed.is_empty() {
            flush_paragraph_block(&mut blocks, &mut paragraph);
            blocks.push(AnswerBlock::Spacer(6.0));
            continue;
        }

        let level = trimmed.chars().take_while(|ch| *ch == '#').count();
        let after_hashes = &trimmed[level..];
        if (1..=6).contains(&level)
            && (after_hashes.is_empty() || after_hashes.starts_with([' ', '\t']))
        {
            let content = after_hashes.trim().to_owned();
            flush_paragraph_block(&mut blocks, &mut paragraph);
            blocks.push(AnswerBlock::Heading {
                level,
                text: content,
            });
            blocks.push(AnswerBlock::Spacer(4.0));
            continue;
        }

        if is_bold_title_line(trimmed) {
            let title = trimmed
                .trim_start_matches("**")
                .trim_end_matches("**")
                .trim()
                .to_owned();
            flush_paragraph_block(&mut blocks, &mut paragraph);
            blocks.push(AnswerBlock::EmphasizedTitle(title));
            blocks.push(AnswerBlock::Spacer(4.0));
            continue;
        }

        if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            flush_paragraph_block(&mut blocks, &mut paragraph);
            blocks.push(AnswerBlock::Bullet(item.to_owned()));
            continue;
        }

        if let Some(quote) = trimmed.strip_prefix("> ") {
            flush_paragraph_block(&mut blocks, &mut paragraph);
            blocks.push(AnswerBlock::Quote(quote.to_owned()));
            continue;
        }

        paragraph.push(line.to_owned());
    }

    if fence.is_some() {
        blocks.push(AnswerBlock::Code {
            language: code_lang,
            code: code_block,
        });
    } else if in_math_block {
        blocks.push(AnswerBlock::Math(math_block));
    } else {
        flush_paragraph_block(&mut blocks, &mut paragraph);
    }

    blocks
}

pub(super) fn render_answer_blocks(
    ui: &mut egui::Ui,
    blocks: &[AnswerBlock],
    search: &mut InChatSearchRender<'_>,
) {
    for block in blocks {
        match block {
            AnswerBlock::Paragraph(text) => {
                render_markdown_label(
                    ui,
                    text,
                    0.0,
                    false,
                    Color32::from_rgb(223, 229, 235),
                    search,
                );
            }
            AnswerBlock::Code { language, code } => flush_code_block(ui, language, code, search),
            AnswerBlock::Math(math) => flush_math_block(ui, math, search),
            AnswerBlock::Heading { level, text } => {
                render_markdown_heading(ui, text, *level, search)
            }
            AnswerBlock::EmphasizedTitle(text) => render_emphasized_title(ui, text, search),
            AnswerBlock::Bullet(text) => {
                render_markdown_label(
                    ui,
                    &format!("- {text}"),
                    0.0,
                    false,
                    Color32::from_rgb(223, 229, 235),
                    search,
                );
            }
            AnswerBlock::Quote(text) => {
                render_markdown_label(
                    ui,
                    text,
                    0.0,
                    false,
                    Color32::from_rgb(158, 181, 204),
                    search,
                );
            }
            AnswerBlock::Spacer(amount) => ui.add_space(*amount),
        }
    }
}

pub(super) fn flush_paragraph_block(blocks: &mut Vec<AnswerBlock>, paragraph: &mut Vec<String>) {
    if paragraph.is_empty() {
        return;
    }

    let text = paragraph.join("\n");
    blocks.push(AnswerBlock::Paragraph(text));
    paragraph.clear();
}

pub(super) fn flush_code_block(
    ui: &mut egui::Ui,
    language: &str,
    code: &str,
    search: &mut InChatSearchRender<'_>,
) {
    Frame::default()
        .fill(Color32::from_rgb(17, 20, 25))
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(51, 59, 69)))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            if !language.is_empty() {
                ui.label(
                    RichText::new(language)
                        .small()
                        .color(Color32::from_rgb(132, 177, 255)),
                );
                ui.add_space(4.0);
            }
            let scroll_id = ui.next_auto_id();
            ScrollArea::horizontal().id_salt(scroll_id).show(ui, |ui| {
                render_highlighted_label(
                    ui,
                    code,
                    0.0,
                    FontFamily::Monospace,
                    Color32::from_rgb(217, 224, 232),
                    search,
                );
            });
        });
}

pub(super) fn flush_math_block(ui: &mut egui::Ui, math: &str, search: &mut InChatSearchRender<'_>) {
    Frame::default()
        .fill(Color32::from_rgb(15, 25, 38))
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(44, 70, 98)))
        .corner_radius(egui::CornerRadius::same(10))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            let scroll_id = ui.next_auto_id();
            ScrollArea::horizontal().id_salt(scroll_id).show(ui, |ui| {
                render_highlighted_label(
                    ui,
                    math,
                    0.0,
                    FontFamily::Monospace,
                    Color32::from_rgb(168, 214, 255),
                    search,
                );
            });
        });
}

pub(super) fn render_markdown_heading(
    ui: &mut egui::Ui,
    text: &str,
    level: usize,
    search: &mut InChatSearchRender<'_>,
) {
    let size = match level {
        1 => 8.0,
        2 => 5.0,
        _ => 2.0,
    };
    render_markdown_label(
        ui,
        text,
        size,
        true,
        Color32::from_rgb(235, 241, 247),
        search,
    );
    ui.add_space(4.0);
}

pub(super) fn render_emphasized_title(
    ui: &mut egui::Ui,
    text: &str,
    search: &mut InChatSearchRender<'_>,
) {
    render_markdown_label(
        ui,
        text,
        6.0,
        true,
        Color32::from_rgb(235, 241, 247),
        search,
    );
    ui.add_space(4.0);
}

pub(super) fn is_bold_title_line(text: &str) -> bool {
    text.starts_with("**") && text.ends_with("**") && text.len() > 4
}

pub(super) fn render_highlighted_label(
    ui: &mut egui::Ui,
    text: &str,
    size_offset: f32,
    font_family: FontFamily,
    color: Color32,
    search: &mut InChatSearchRender<'_>,
) -> egui::Response {
    let size = body_size(ui) + size_offset;
    let (job, contains_current_match) =
        highlighted_text_job(text, size, font_family, color, search);
    let response = ui.label(job);
    maybe_scroll_to_current_match(&response, contains_current_match, search);
    response
}

pub(super) fn render_markdown_label(
    ui: &mut egui::Ui,
    text: &str,
    size_offset: f32,
    strong: bool,
    color: Color32,
    search: &mut InChatSearchRender<'_>,
) -> egui::Response {
    let size = body_size(ui) + size_offset;
    let (job, contains_current_match) = markdown_job(text, size, strong, color, search);
    let response = ui.label(job);
    maybe_scroll_to_current_match(&response, contains_current_match, search);
    response
}

pub(super) fn maybe_scroll_to_current_match(
    response: &egui::Response,
    contains_current_match: bool,
    search: &mut InChatSearchRender<'_>,
) {
    if contains_current_match && search.scroll_to_current {
        response.scroll_to_me(Some(Align::Center));
        search.scrolled_to_current = true;
    }
}

pub(super) fn highlighted_text_job(
    text: &str,
    size: f32,
    font_family: FontFamily,
    color: Color32,
    search: &mut InChatSearchRender<'_>,
) -> (LayoutJob, bool) {
    let mut job = LayoutJob::default();
    let format = TextFormat {
        font_id: FontId::new(size, font_family),
        color,
        ..Default::default()
    };
    let contains_current_match = append_search_highlighted(&mut job, text, &format, search);
    (job, contains_current_match)
}

pub(super) fn markdown_job(
    text: &str,
    size: f32,
    strong: bool,
    color: Color32,
    search: &mut InChatSearchRender<'_>,
) -> (LayoutJob, bool) {
    let mut job = LayoutJob::default();
    let mut contains_current_match = false;
    let base_size = if strong { size + 1.0 } else { size };
    let base_format = TextFormat {
        font_id: FontId::proportional(base_size),
        color,
        ..Default::default()
    };
    let strong_format = TextFormat {
        font_id: FontId::proportional(base_size),
        color: Color32::from_rgb(239, 244, 249),
        ..Default::default()
    };
    let code_format = TextFormat {
        font_id: FontId::new(size, FontFamily::Monospace),
        color: Color32::from_rgb(144, 204, 255),
        ..Default::default()
    };

    let format_for = |code: bool, bold: bool| match (code, bold) {
        (true, _) => &code_format,
        (false, true) => &strong_format,
        (false, false) => &base_format,
    };
    let mut chars = text.chars().peekable();
    let mut buffer = String::new();
    let mut bold = false;
    let mut code = false;

    while let Some(ch) = chars.next() {
        let toggles_code = ch == '`';
        let toggles_bold = !code && ch == '*' && chars.peek() == Some(&'*');
        if !toggles_code && !toggles_bold {
            buffer.push(ch);
            continue;
        }

        append_markdown_buffer(
            &mut job,
            &mut buffer,
            format_for(code, bold),
            search,
            &mut contains_current_match,
        );
        if toggles_code {
            code = !code;
        } else {
            chars.next();
            bold = !bold;
        }
    }

    append_markdown_buffer(
        &mut job,
        &mut buffer,
        format_for(code, bold),
        search,
        &mut contains_current_match,
    );
    (job, contains_current_match)
}

pub(super) fn append_markdown_buffer(
    job: &mut LayoutJob,
    buffer: &mut String,
    format: &TextFormat,
    search: &mut InChatSearchRender<'_>,
    contains_current_match: &mut bool,
) {
    if buffer.is_empty() {
        return;
    }
    *contains_current_match |= append_search_highlighted(job, buffer, format, search);
    buffer.clear();
}

pub(super) fn append_search_highlighted(
    job: &mut LayoutJob,
    text: &str,
    format: &TextFormat,
    search: &mut InChatSearchRender<'_>,
) -> bool {
    let ranges = search_match_ranges(text, search.query);
    if ranges.is_empty() {
        job.append(text, 0.0, format.clone());
        return false;
    }

    let mut cursor = 0;
    let mut contains_current_match = false;
    for (start, end) in ranges {
        if start > cursor {
            job.append(&text[cursor..start], 0.0, format.clone());
        }
        let is_current =
            search.current_match_index == Some(search.seen_matches) && search.is_active();
        if is_current {
            contains_current_match = true;
        }
        job.append(
            &text[start..end],
            0.0,
            search_highlight_format(format, is_current),
        );
        search.seen_matches += 1;
        cursor = end;
    }

    if cursor < text.len() {
        job.append(&text[cursor..], 0.0, format.clone());
    }
    contains_current_match
}

pub(super) fn search_highlight_format(format: &TextFormat, is_current: bool) -> TextFormat {
    let mut highlighted = format.clone();
    if is_current {
        highlighted.background = Color32::from_rgb(35, 105, 176);
        highlighted.color = Color32::WHITE;
        highlighted.expand_bg = 3.0;
    } else {
        highlighted.background = Color32::from_rgb(142, 93, 20);
        highlighted.color = Color32::from_rgb(255, 247, 205);
        highlighted.expand_bg = 2.0;
    }
    highlighted
}

pub(super) fn search_match_ranges(text: &str, query: &str) -> Vec<(usize, usize)> {
    bigtext::find_all(text, query)
}
