use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use super::*;

const TOKEN_REVALIDATE_INTERVAL: Duration = Duration::from_secs(1);
const TOKEN_BYTES_PER_FRAME: usize = 64 * 1024;

pub(super) struct ContextRowsCache {
    key: u64,
    selection: HashSet<String>,
    rows: Vec<ContextRow>,
    totals: DirStats,
}

fn selection_key(workspace: &str, selection: &[String]) -> u64 {
    let mut hasher = DefaultHasher::new();
    workspace.hash(&mut hasher);
    selection.hash(&mut hasher);
    hasher.finish()
}

impl ProGuiApp {
    pub(super) fn pick_workspace_folder(&mut self) {
        let Some(folder) = rfd::FileDialog::new()
            .set_title("Choose repository folder")
            .pick_folder()
        else {
            return;
        };
        self.set_workspace(folder);
    }

    pub(super) fn set_workspace(&mut self, folder: PathBuf) {
        let Some(conversation_id) = self.selected_conversation_id else {
            return;
        };
        let path_string = folder.display().to_string();
        let changed = self
            .selected_conversation()
            .is_none_or(|conversation| conversation.workspace_path != path_string);
        if let Some(conversation) = self.selected_conversation_mut() {
            conversation.workspace_path = path_string.clone();
            if changed {
                conversation.context_selection.clear();
            }
        }
        self.save_context_state(conversation_id);
        self.workspace_tree = None;
        self.workspace_scan = None;
        self.context_filter.clear();
        self.context_exts.clear();
        self.context_selected_only = false;
        self.context_expanded.clear();
        self.set_context_panel(true);
    }

    pub(super) fn detach_workspace(&mut self) {
        let Some(conversation_id) = self.selected_conversation_id else {
            return;
        };
        if let Some(conversation) = self.selected_conversation_mut() {
            conversation.workspace_path.clear();
            conversation.context_selection.clear();
        }
        self.save_context_state(conversation_id);
        self.workspace_tree = None;
        self.context_expanded.clear();
    }

    pub(super) fn save_context_state(&mut self, conversation_id: i64) {
        let Some((workspace_path, selection)) = self
            .conversations
            .iter()
            .find(|conversation| conversation.id == conversation_id)
            .map(|conversation| {
                (
                    conversation.workspace_path.clone(),
                    conversation.context_selection.clone(),
                )
            })
        else {
            return;
        };
        if let Err(err) =
            self.db
                .update_conversation_context(conversation_id, &workspace_path, &selection)
        {
            self.banner = Some(format!("Failed to save context selection: {err}"));
        }
    }

    pub(super) fn ensure_workspace_tree(&mut self, ctx: &Context) {
        let workspace = self
            .selected_conversation()
            .map(|conversation| conversation.workspace_path.clone())
            .unwrap_or_default();
        if workspace.is_empty() {
            self.workspace_tree = None;
            self.workspace_scan = None;
            return;
        }
        if self
            .workspace_tree
            .as_ref()
            .is_some_and(|tree| tree.root.display().to_string() == workspace)
        {
            return;
        }
        if let Some((root, scan)) = &self.workspace_scan
            && *root == workspace
        {
            if let Ok(tree) = scan.try_recv() {
                self.workspace_tree = Some(tree);
                self.workspace_scan = None;
                self.workspace_generation += 1;
            }
            return;
        }
        if self.workspace_tree.take().is_some() {
            self.context_expanded.clear();
        }
        let (done_tx, done_rx) = mpsc::channel();
        let root = PathBuf::from(&workspace);
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            if done_tx.send(repoctx::scan_workspace(&root)).is_ok() {
                repaint.request_repaint();
            }
        });
        self.workspace_scan = Some((workspace, done_rx));
    }

    pub(super) fn ensure_selection_token_counts(&mut self, ctx: &Context) {
        let Some(conversation) = self
            .selected_conversation()
            .filter(|conversation| !conversation.workspace_path.is_empty())
        else {
            return;
        };
        let key = selection_key(
            &conversation.workspace_path,
            &conversation.context_selection,
        );
        let due = self.token_work_pending
            || self.token_check.is_none_or(|(checked, at)| {
                checked != key || at.elapsed() >= TOKEN_REVALIDATE_INTERVAL
            });
        if !due {
            return;
        }
        let root = PathBuf::from(&conversation.workspace_path);
        let selection = conversation.context_selection.clone();

        let mut counted_bytes = 0;
        let mut finished = true;
        for rel_path in &selection {
            let path = root.join(rel_path);
            let absolute = path.display().to_string();
            let stamp = file_stamp(&path);
            if self
                .file_token_cache
                .get(&absolute)
                .is_some_and(|cached| cached.stamp == stamp)
            {
                continue;
            }
            if counted_bytes >= TOKEN_BYTES_PER_FRAME {
                finished = false;
                break;
            }
            let tokens = std::fs::read_to_string(&path)
                .map(|content| repoctx::count_tokens(&content))
                .unwrap_or(0);
            self.file_token_cache
                .insert(absolute, FileTokens { stamp, tokens });
            self.file_tokens_generation += 1;
            counted_bytes += stamp.map(|(len, _)| len as usize).unwrap_or(0);
        }
        self.token_work_pending = !finished;
        if !finished {
            ctx.request_repaint();
        }
        self.token_check = Some((key, Instant::now()));
    }

    pub(super) fn context_selection_summary(&mut self) -> Option<(usize, usize, bool)> {
        let conversation = self.selected_conversation()?;
        if conversation.workspace_path.is_empty() {
            return None;
        }
        let key = {
            let mut hasher = DefaultHasher::new();
            selection_key(
                &conversation.workspace_path,
                &conversation.context_selection,
            )
            .hash(&mut hasher);
            self.file_tokens_generation.hash(&mut hasher);
            hasher.finish()
        };
        if let Some((cached_key, summary)) = self.selection_summary
            && cached_key == key
        {
            return Some(summary);
        }
        let root = PathBuf::from(&conversation.workspace_path);
        let mut tokens = 0;
        let mut all_cached = true;
        for rel_path in &conversation.context_selection {
            let absolute = root.join(rel_path).display().to_string();
            match self.file_token_cache.get(&absolute) {
                Some(cached) => tokens += cached.tokens,
                None => all_cached = false,
            }
        }
        let summary = (conversation.context_selection.len(), tokens, all_cached);
        self.selection_summary = Some((key, summary));
        Some(summary)
    }

    pub(super) fn set_context_panel(&mut self, open: bool) {
        if self.show_context_panel != open {
            self.show_context_panel = open;
            self.save_bool_setting("show_context_panel", open);
        }
    }

    pub(super) fn recent_workspaces(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.conversations
            .iter()
            .map(|conversation| conversation.workspace_path.as_str())
            .filter(|path| !path.is_empty() && seen.insert(*path))
            .filter(|path| std::path::Path::new(path).is_dir())
            .take(8)
            .map(ToOwned::to_owned)
            .collect()
    }

    pub(super) fn render_context_panel(&mut self, ctx: &Context) {
        if !self.show_context_panel {
            return;
        }
        let Some(conversation_id) = self.selected_conversation_id else {
            return;
        };
        self.ensure_workspace_tree(ctx);
        let workspace = self
            .selected_conversation()
            .map(|conversation| conversation.workspace_path.clone())
            .unwrap_or_default();
        let summary = self.context_selection_summary().unwrap_or((0, 0, true));
        let mut rows_cache = self.context_rows.take();
        let empty_selection = Vec::new();
        let selection_list = self
            .conversations
            .iter()
            .find(|conversation| Some(conversation.id) == self.selected_conversation_id)
            .map_or(&empty_selection, |conversation| {
                &conversation.context_selection
            });
        let recent = if workspace.is_empty() {
            self.recent_workspaces()
        } else {
            Vec::new()
        };

        let mut close = false;
        let mut pick_folder = false;
        let mut attach: Option<String> = None;
        let mut rescan = false;
        let mut detach = false;
        let mut copy_context = false;
        let mut events = ContextEvents::default();

        SidePanel::right("context_panel")
            .resizable(true)
            .default_width(380.0)
            .width_range(300.0..=760.0)
            .frame(
                Frame::default()
                    .fill(SIDEBAR_FILL)
                    .inner_margin(Margin::same(12)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Context").strong().size(20.0).color(TEXT_STRONG));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new("\u{d7}").frame_when_inactive(false))
                            .on_hover_text("Close (Cmd+E)")
                            .clicked()
                        {
                            close = true;
                        }
                        if !workspace.is_empty() {
                            ui.menu_button("\u{2026}", |ui| {
                                ui.set_min_width(190.0);
                                if ui.button("Change folder...").clicked() {
                                    pick_folder = true;
                                }
                                if ui
                                    .button("Rescan")
                                    .on_hover_text("Re-read the folder and refresh token counts.")
                                    .clicked()
                                {
                                    rescan = true;
                                }
                                if ui.button("Collapse all folders").clicked() {
                                    events.collapse_all = true;
                                }
                                ui.separator();
                                if ui
                                    .button(RichText::new("Detach from this chat").color(KEY_MISSING))
                                    .clicked()
                                {
                                    detach = true;
                                }
                            })
                            .response
                            .on_hover_text("Repository options");
                        }
                    });
                });
                ui.add_space(6.0);

                if workspace.is_empty() {
                    render_context_empty_state(ui, &recent, &mut attach, &mut pick_folder);
                    return;
                }
                let Some(tree) = self.workspace_tree.as_ref() else {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label(
                            RichText::new(format!("Scanning {}...", home_relative(&workspace)))
                                .color(TEXT_MUTED),
                        );
                    });
                    return;
                };

                ui.horizontal(|ui| {
                    let name = std::path::Path::new(&workspace)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| workspace.clone());
                    ui.label(RichText::new(name).strong().color(TEXT_BODY))
                        .on_hover_text(&workspace);
                    ui.add(
                        egui::Label::new(
                            RichText::new(home_relative(&workspace)).small().color(TEXT_FAINT),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&workspace);
                });
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    let clear_width = if self.context_filter.is_empty() { 0.0 } else { 28.0 };
                    let response = ui.add(
                        TextEdit::singleline(&mut self.context_filter)
                            .hint_text("Filter   views  *.py  api/*.py  !tests")
                            .desired_width(ui.available_width() - clear_width),
                    );
                    if self.context_filter_needs_focus {
                        response.request_focus();
                        self.context_filter_needs_focus = false;
                    }
                    response.on_hover_text(
                        "Space-separated terms, all must match:\n\
                         views: path contains it\n\
                         *.py, test_*: file name glob\n\
                         api/*.py: path glob (a leading / anchors at the repo root)\n\
                         !tests: exclude",
                    );
                    if !self.context_filter.is_empty()
                        && ui
                            .add(egui::Button::new("\u{d7}").small().frame_when_inactive(false))
                            .on_hover_text("Clear the filter")
                            .clicked()
                    {
                        self.context_filter.clear();
                    }
                });
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
                    if filter_chip(
                        ui,
                        &format!("Selected {}", selection_list.len()),
                        self.context_selected_only,
                    )
                    .on_hover_text("Show only the selected files")
                    .clicked()
                    {
                        self.context_selected_only = !self.context_selected_only;
                    }
                    for (extension, count) in tree.extensions.iter().take(8) {
                        let active = self.context_exts.contains(extension);
                        if filter_chip(ui, &format!(".{extension} {count}"), active)
                            .on_hover_text(format!("Only .{extension} files (combine several)"))
                            .clicked()
                        {
                            if active {
                                self.context_exts.remove(extension);
                            } else {
                                self.context_exts.insert(extension.clone());
                            }
                        }
                    }
                });

                let rows_key = {
                    let mut hasher = DefaultHasher::new();
                    self.workspace_generation.hash(&mut hasher);
                    self.file_tokens_generation.hash(&mut hasher);
                    self.context_filter.hash(&mut hasher);
                    self.context_exts.hash(&mut hasher);
                    self.context_selected_only.hash(&mut hasher);
                    self.context_expanded.hash(&mut hasher);
                    selection_list.hash(&mut hasher);
                    hasher.finish()
                };
                if rows_cache.as_ref().is_none_or(|cache| cache.key != rows_key) {
                    let selection: HashSet<String> = selection_list.iter().cloned().collect();
                    let view = ContextView {
                        tree,
                        filter: repoctx::PathFilter::parse(&self.context_filter),
                        extensions: &self.context_exts,
                        selected_only: self.context_selected_only,
                        selection: &selection,
                        token_cache: &self.file_token_cache,
                    };
                    let mut rows = Vec::new();
                    let totals = build_context_rows(
                        &view,
                        &tree.tree,
                        "",
                        0,
                        &self.context_expanded,
                        true,
                        &mut rows,
                    );
                    rows_cache = Some(ContextRowsCache {
                        key: rows_key,
                        selection,
                        rows,
                        totals,
                    });
                }
                let Some(cache) = rows_cache.as_ref() else {
                    return;
                };
                let (rows, totals, selection) = (&cache.rows, cache.totals, &cache.selection);
                let view = ContextView {
                    tree,
                    filter: repoctx::PathFilter::parse(&self.context_filter),
                    extensions: &self.context_exts,
                    selected_only: self.context_selected_only,
                    selection,
                    token_cache: &self.file_token_cache,
                };

                if view.active() {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} matching, \u{2248}{}",
                                totals.files,
                                format_compact_count(totals.tokens as i64)
                            ))
                            .small()
                            .color(TEXT_MUTED),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add_enabled(totals.selected > 0, egui::Button::new("Deselect").small())
                                .clicked()
                            {
                                events.set.push((view.selectable_in("", &totals), false));
                            }
                            if ui
                                .add_enabled(
                                    totals.selected < totals.files,
                                    egui::Button::new("Select all").small(),
                                )
                                .on_hover_text("Select every file the filters show")
                                .clicked()
                            {
                                events.set.push((view.selectable_in("", &totals), true));
                            }
                        });
                    });
                }
                ui.add_space(6.0);

                TopBottomPanel::bottom("context_footer")
                    .frame(Frame::default().inner_margin(Margin {
                        left: 0,
                        right: 0,
                        top: 8,
                        bottom: 0,
                    }))
                    .show_separator_line(true)
                    .show_inside(ui, |ui| {
                        let (files, tokens, all_cached) = summary;
                        let over = tokens >= repoctx::CONTEXT_TOKEN_WARN;
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{files} file{}, \u{2248}{}{} tokens",
                                    if files == 1 { "" } else { "s" },
                                    format_compact_count(tokens as i64),
                                    if all_cached { "" } else { "+" }
                                ))
                                .strong()
                                .color(if over {
                                    Color32::from_rgb(255, 209, 128)
                                } else if files > 0 {
                                    KEY_STORED
                                } else {
                                    TEXT_MUTED
                                }),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add_enabled(files > 0, egui::Button::new("Copy"))
                                    .on_hover_text(
                                        "Copy the packed context (file tree and selected files) to the clipboard.",
                                    )
                                    .clicked()
                                {
                                    copy_context = true;
                                }
                                if ui
                                    .add_enabled(files > 0, egui::Button::new("Clear"))
                                    .on_hover_text("Deselect everything")
                                    .clicked()
                                {
                                    events.only = Some(Vec::new());
                                }
                            });
                        });
                        ui.label(
                            RichText::new(if over {
                                "Large context. Check the selection before sending."
                            } else {
                                "Packed fresh from disk into every message."
                            })
                            .small()
                            .color(if over {
                                Color32::from_rgb(255, 209, 128)
                            } else {
                                TEXT_FAINT
                            }),
                        );
                    });

                CentralPanel::default()
                    .frame(Frame::default())
                    .show_inside(ui, |ui| {
                        if rows.is_empty() {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new(if tree.files.is_empty() {
                                    "No includable files: everything is ignored or binary."
                                } else if view.selected_only && selection.is_empty() {
                                    "Nothing selected yet."
                                } else {
                                    "No files match the filters."
                                })
                                .italics()
                                .color(TEXT_MUTED),
                            );
                            return;
                        }
                        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 8.0;
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ScrollArea::vertical()
                            .id_salt("context_tree_scroll")
                            .auto_shrink([false, false])
                            .show_rows(ui, row_height, rows.len(), |ui, range| {
                                for row in &rows[range] {
                                    render_context_row(ui, &view, row, row_height, &mut events);
                                }
                            });
                    });
            });

        self.context_rows = rows_cache;
        if close {
            self.set_context_panel(false);
        }
        if let Some(path) = attach {
            self.set_workspace(PathBuf::from(path));
        }
        if pick_folder {
            self.pick_workspace_folder();
        }
        if rescan {
            self.file_token_cache.clear();
            self.file_tokens_generation += 1;
            self.workspace_tree = None;
            self.workspace_scan = None;
        }
        if detach {
            self.detach_workspace();
        }
        if let Some(path) = events.copy_path {
            ctx.copy_text(path);
        }
        if copy_context {
            let selected = self
                .selected_conversation()
                .map(|conversation| conversation.context_selection.clone())
                .unwrap_or_default();
            let packed = repoctx::pack_context(&PathBuf::from(&workspace), &selected);
            ctx.copy_text(packed.text);
            self.banner = Some(format!(
                "Copied {} file(s) of context to the clipboard (\u{2248}{} tokens).",
                packed.included,
                format_compact_count(summary.1 as i64)
            ));
        }

        if events.collapse_all {
            self.context_expanded.clear();
        }
        if let Some(path) = events.toggle_dir
            && !self.context_expanded.remove(&path)
        {
            self.context_expanded.insert(path);
        }
        if let Some(path) = events.expand_under
            && let Some(tree) = &self.workspace_tree
            && let Some(node) = dir_node_at(&tree.tree, &path)
        {
            self.context_expanded.insert(path.clone());
            collect_dir_paths(node, &path, &mut self.context_expanded);
        }

        if events.only.is_some() || !events.set.is_empty() {
            if let Some(conversation) = self.selected_conversation_mut() {
                let mut set: HashSet<String> = match events.only {
                    Some(paths) => {
                        conversation.context_selection.clear();
                        paths.into_iter().collect()
                    }
                    None => conversation.context_selection.drain(..).collect(),
                };
                for (paths, selected) in events.set {
                    for path in paths {
                        if selected {
                            set.insert(path);
                        } else {
                            set.remove(&path);
                        }
                    }
                }
                let mut sorted: Vec<String> = set.into_iter().collect();
                sorted.sort();
                conversation.context_selection = sorted;
            }
            self.save_context_state(conversation_id);
        }
    }
}

pub(super) struct ContextView<'a> {
    pub(super) tree: &'a repoctx::WorkspaceTree,
    pub(super) filter: repoctx::PathFilter,
    pub(super) extensions: &'a BTreeSet<String>,
    pub(super) selected_only: bool,
    pub(super) selection: &'a HashSet<String>,
    pub(super) token_cache: &'a HashMap<String, FileTokens>,
}

impl ContextView<'_> {
    pub(super) fn active(&self) -> bool {
        !self.filter.is_empty() || !self.extensions.is_empty() || self.selected_only
    }

    pub(super) fn shows(&self, file: &repoctx::FileEntry) -> bool {
        (self.extensions.is_empty()
            || self
                .extensions
                .contains(&repoctx::extension_of(&file.rel_path)))
            && (!self.selected_only || self.selection.contains(&file.rel_path))
            && self.filter.matches(&file.rel_path)
    }

    pub(super) fn tokens(&self, file: &repoctx::FileEntry) -> (usize, bool) {
        if self.selection.contains(&file.rel_path)
            && let Some(cached) = self
                .token_cache
                .get(&self.tree.root.join(&file.rel_path).display().to_string())
        {
            return (cached.tokens, true);
        }
        (repoctx::approx_tokens_for_bytes(file.size), false)
    }

    pub(super) fn selectable_in(&self, dir: &str, stats: &DirStats) -> Vec<String> {
        if stats.shown == 0 {
            return Vec::new();
        }
        self.tree.files[stats.first..=stats.last]
            .iter()
            .filter(|file| {
                file.skip_reason.is_none() && file.rel_path.starts_with(dir) && self.shows(file)
            })
            .map(|file| file.rel_path.clone())
            .collect()
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct DirStats {
    pub(super) shown: usize,
    pub(super) files: usize,
    pub(super) selected: usize,
    pub(super) tokens: usize,
    pub(super) first: usize,
    pub(super) last: usize,
}

impl DirStats {
    pub(super) fn add(&mut self, other: &DirStats) {
        if other.shown == 0 {
            return;
        }
        if self.shown == 0 {
            self.first = other.first;
            self.last = other.last;
        } else {
            self.first = self.first.min(other.first);
            self.last = self.last.max(other.last);
        }
        self.shown += other.shown;
        self.files += other.files;
        self.selected += other.selected;
        self.tokens += other.tokens;
    }
}

pub(super) enum ContextRow {
    Dir {
        depth: usize,
        path: String,
        name: String,
        open: bool,
        stats: DirStats,
    },
    File {
        depth: usize,
        index: usize,
    },
}

#[derive(Default)]
pub(super) struct ContextEvents {
    pub(super) set: Vec<(Vec<String>, bool)>,
    pub(super) only: Option<Vec<String>>,
    pub(super) toggle_dir: Option<String>,
    pub(super) expand_under: Option<String>,
    pub(super) collapse_all: bool,
    pub(super) copy_path: Option<String>,
}

pub(super) fn build_context_rows(
    view: &ContextView<'_>,
    node: &repoctx::DirNode,
    prefix: &str,
    depth: usize,
    expanded: &BTreeSet<String>,
    emit: bool,
    rows: &mut Vec<ContextRow>,
) -> DirStats {
    let mut stats = DirStats::default();
    for (name, child) in &node.dirs {
        let path = format!("{prefix}{name}/");
        let open = view.active() || expanded.contains(&path);
        let at = rows.len();
        if emit {
            rows.push(ContextRow::File { depth, index: 0 });
        }
        let child_stats =
            build_context_rows(view, child, &path, depth + 1, expanded, emit && open, rows);
        if child_stats.shown == 0 {
            rows.truncate(at);
            continue;
        }
        if emit {
            rows[at] = ContextRow::Dir {
                depth,
                path,
                name: name.clone(),
                open,
                stats: child_stats,
            };
        }
        stats.add(&child_stats);
    }
    for &index in &node.files {
        let file = &view.tree.files[index];
        if !view.shows(file) {
            continue;
        }
        let mut own = DirStats {
            shown: 1,
            first: index,
            last: index,
            ..DirStats::default()
        };
        if file.skip_reason.is_none() {
            own.files = 1;
            own.tokens = view.tokens(file).0;
            own.selected = usize::from(view.selection.contains(&file.rel_path));
        }
        stats.add(&own);
        if emit {
            rows.push(ContextRow::File { depth, index });
        }
    }
    stats
}

pub(super) fn render_context_row(
    ui: &mut egui::Ui,
    view: &ContextView<'_>,
    row: &ContextRow,
    height: f32,
    events: &mut ContextEvents,
) {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    if !ui.is_rect_visible(rect) {
        return;
    }
    if response.hovered() {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(5),
            Color32::from_rgb(29, 34, 41),
        );
    }
    let depth = match row {
        ContextRow::Dir { depth, .. } | ContextRow::File { depth, .. } => *depth,
    };
    let left = rect.left() + 4.0 + depth as f32 * 14.0;
    let mid = rect.center().y;
    let checkbox_rect =
        egui::Rect::from_center_size(egui::pos2(left + 24.0, mid), egui::vec2(18.0, 18.0));
    let name_left = left + 38.0;

    let paint_right = |ui: &mut egui::Ui, text: RichText| -> f32 {
        let galley = egui::WidgetText::from(text).into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Small,
        );
        let pos = egui::pos2(
            rect.right() - 6.0 - galley.size().x,
            mid - galley.size().y / 2.0,
        );
        let width = galley.size().x;
        ui.painter().galley(pos, galley, TEXT_MUTED);
        rect.right() - 6.0 - width
    };
    let paint_name = |ui: &mut egui::Ui, text: RichText, right: f32| {
        let galley = egui::WidgetText::from(text).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            (right - name_left - 10.0).max(20.0),
            egui::TextStyle::Body,
        );
        ui.painter().galley(
            egui::pos2(name_left, mid - galley.size().y / 2.0),
            galley,
            TEXT_BODY,
        );
    };

    match row {
        ContextRow::Dir {
            path,
            name,
            open,
            stats,
            ..
        } => {
            let c = egui::pos2(left + 6.0, mid);
            let points = if *open {
                vec![
                    egui::pos2(c.x - 4.0, c.y - 2.0),
                    egui::pos2(c.x + 4.0, c.y - 2.0),
                    egui::pos2(c.x, c.y + 3.0),
                ]
            } else {
                vec![
                    egui::pos2(c.x - 2.0, c.y - 4.0),
                    egui::pos2(c.x + 3.0, c.y),
                    egui::pos2(c.x - 2.0, c.y + 4.0),
                ]
            };
            ui.painter().add(egui::Shape::convex_polygon(
                points,
                TEXT_MUTED,
                Stroke::NONE,
            ));

            let right = paint_right(
                ui,
                if stats.selected > 0 {
                    RichText::new(format!(
                        "{}/{}  \u{2248}{}",
                        stats.selected,
                        stats.files,
                        format_compact_count(stats.tokens as i64)
                    ))
                    .small()
                    .color(Color32::from_rgb(122, 196, 255))
                } else {
                    RichText::new(format!(
                        "{}  \u{2248}{}",
                        stats.files,
                        format_compact_count(stats.tokens as i64)
                    ))
                    .small()
                    .color(TEXT_FAINT)
                },
            );
            paint_name(
                ui,
                RichText::new(format!("{name}/")).strong().color(TEXT_BODY),
                right,
            );

            let all = stats.files > 0 && stats.selected == stats.files;
            let mut checked = all;
            let checkbox = ui.put(
                checkbox_rect,
                egui::Checkbox::without_text(&mut checked)
                    .indeterminate(stats.selected > 0 && !all),
            );
            if checkbox.changed() {
                events.set.push((view.selectable_in(path, stats), checked));
            }
            let response = response.on_hover_text(format!(
                "{path}\n{} files, \u{2248}{} tokens",
                stats.files,
                format_compact_count(stats.tokens as i64)
            ));
            if response.clicked() && !view.active() {
                events.toggle_dir = Some(path.clone());
            }
            response.context_menu(|ui| {
                ui.set_min_width(180.0);
                if ui.button("Select folder").clicked() {
                    events.set.push((view.selectable_in(path, stats), true));
                }
                if ui.button("Deselect folder").clicked() {
                    events.set.push((view.selectable_in(path, stats), false));
                }
                if ui.button("Only this folder").clicked() {
                    events.only = Some(view.selectable_in(path, stats));
                }
                ui.separator();
                if ui.button("Expand all inside").clicked() {
                    events.expand_under = Some(path.clone());
                }
                if ui.button("Copy path").clicked() {
                    events.copy_path = Some(path.clone());
                }
            });
        }
        ContextRow::File { index, .. } => {
            let file = &view.tree.files[*index];
            let name = file.rel_path.rsplit('/').next().unwrap_or(&file.rel_path);
            if let Some(reason) = file.skip_reason {
                let right = paint_right(ui, RichText::new(reason).small().color(TEXT_FAINT));
                paint_name(ui, RichText::new(name).color(TEXT_FAINT), right);
                let mut unchecked = false;
                ui.put(
                    checkbox_rect,
                    egui::Checkbox::without_text(&mut unchecked).indeterminate(false),
                )
                .on_disabled_hover_text(format!("Skipped: {reason}"));
                response.on_hover_text(format!(
                    "{}\nSkipped: {reason} ({})",
                    file.rel_path,
                    format_bytes(file.size as i64)
                ));
                return;
            }
            let selected = view.selection.contains(&file.rel_path);
            let (tokens, exact) = view.tokens(file);
            let right = paint_right(
                ui,
                RichText::new(format!(
                    "{}{}",
                    if exact { "" } else { "~" },
                    format_compact_count(tokens as i64)
                ))
                .small()
                .color(if selected { TEXT_MUTED } else { TEXT_FAINT }),
            );
            paint_name(
                ui,
                RichText::new(name).color(if selected { TEXT_STRONG } else { TEXT_BODY }),
                right,
            );
            let mut checked = selected;
            if ui
                .put(checkbox_rect, egui::Checkbox::without_text(&mut checked))
                .changed()
            {
                events.set.push((vec![file.rel_path.clone()], checked));
            }
            let response = response.on_hover_text(format!(
                "{}\n{}, {}{} tokens",
                file.rel_path,
                format_bytes(file.size as i64),
                if exact { "" } else { "\u{2248}" },
                format_compact_count(tokens as i64)
            ));
            if response.clicked() {
                events.set.push((vec![file.rel_path.clone()], !selected));
            }
            response.context_menu(|ui| {
                ui.set_min_width(160.0);
                if ui.button("Only this file").clicked() {
                    events.only = Some(vec![file.rel_path.clone()]);
                }
                if ui.button("Copy path").clicked() {
                    events.copy_path = Some(file.rel_path.clone());
                }
            });
        }
    }
}

pub(super) fn render_context_empty_state(
    ui: &mut egui::Ui,
    recent: &[String],
    attach: &mut Option<String>,
    pick_folder: &mut bool,
) {
    ui.add(
        egui::Label::new(
            RichText::new(
                "Give this chat a repository. The files you tick are packed into every message, read fresh from disk.",
            )
            .color(TEXT_MUTED),
        )
        .wrap(),
    );
    ui.add_space(14.0);
    if !recent.is_empty() {
        ui.label(
            RichText::new("Recent repositories")
                .small()
                .strong()
                .color(TEXT_MUTED),
        );
        ui.add_space(4.0);
        for path in recent {
            let (rect, response) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 46.0), egui::Sense::click());
            let fill = if response.hovered() {
                Color32::from_rgb(29, 34, 41)
            } else {
                Color32::TRANSPARENT
            };
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(8), fill);
            let name = std::path::Path::new(path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            let inner = rect.shrink2(egui::vec2(10.0, 6.0));
            let title = egui::WidgetText::from(RichText::new(name).strong().color(TEXT_STRONG))
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Truncate),
                    inner.width(),
                    egui::TextStyle::Body,
                );
            let sub = egui::WidgetText::from(
                RichText::new(home_relative(path)).small().color(TEXT_FAINT),
            )
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                inner.width(),
                egui::TextStyle::Small,
            );
            let title_height = title.size().y;
            ui.painter().galley(inner.left_top(), title, TEXT_STRONG);
            ui.painter().galley(
                inner.left_top() + egui::vec2(0.0, title_height + 1.0),
                sub,
                TEXT_FAINT,
            );
            if response.on_hover_text(path).clicked() {
                *attach = Some(path.clone());
            }
        }
        ui.add_space(10.0);
    }
    if ui
        .button("Open folder...")
        .on_hover_text("Choose any folder on disk.")
        .clicked()
    {
        *pick_folder = true;
    }
}

pub(super) fn home_relative(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => relative_to_home(path, &home),
        _ => path.to_owned(),
    }
}

pub(super) fn relative_to_home(path: &str, home: &str) -> String {
    match std::path::Path::new(path).strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.to_owned(),
    }
}

pub(super) fn dir_node_at<'a>(
    root: &'a repoctx::DirNode,
    path: &str,
) -> Option<&'a repoctx::DirNode> {
    path.trim_end_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .try_fold(root, |node, part| node.dirs.get(part))
}

pub(super) fn collect_dir_paths(node: &repoctx::DirNode, prefix: &str, out: &mut BTreeSet<String>) {
    for (name, child) in &node.dirs {
        let path = format!("{prefix}{name}/");
        collect_dir_paths(child, &path, out);
        out.insert(path);
    }
}

pub(super) fn file_stamp(path: &std::path::Path) -> FileStamp {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((metadata.len(), modified))
}
