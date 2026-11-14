use super::*;

impl ProGuiApp {
    pub(super) fn add_pending_attachment(
        &mut self,
        file_name: String,
        mime: String,
        bytes: Vec<u8>,
    ) {
        if self.pending_attachments.len() >= MAX_ATTACHMENTS_PER_MESSAGE {
            self.banner = Some(format!(
                "At most {MAX_ATTACHMENTS_PER_MESSAGE} attachments per message."
            ));
            return;
        }
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            self.banner = Some(format!(
                "\"{file_name}\" is larger than the 20 MB attachment limit."
            ));
            return;
        }
        if bytes.is_empty() {
            self.banner = Some(format!("\"{file_name}\" is empty."));
            return;
        }
        let key = self.next_attachment_key;
        self.next_attachment_key += 1;
        self.pending_attachments.push(PendingAttachment {
            key,
            file_name,
            mime,
            bytes,
        });
        self.banner = None;
    }

    pub(super) fn attach_files_dialog(&mut self) {
        let Some(paths) = rfd::FileDialog::new()
            .set_title("Attach images or PDFs")
            .add_filter(
                "Images & PDFs",
                &["png", "jpg", "jpeg", "gif", "webp", "pdf"],
            )
            .add_filter("PDF documents", &["pdf"])
            .add_filter("Images", &["png", "jpg", "jpeg", "gif", "webp"])
            .pick_files()
        else {
            return;
        };
        for path in paths {
            self.attach_file_path(&path);
        }
    }

    pub(super) fn attach_file_path(&mut self, path: &std::path::Path) {
        let Some(mime) = attachment_mime_for_path(path) else {
            self.banner = Some(format!(
                "\"{}\" is not a supported file (png, jpg, gif, webp, pdf).",
                path.display()
            ));
            return;
        };
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment".to_owned());
        if self.pending_attachments.len() >= MAX_ATTACHMENTS_PER_MESSAGE {
            self.banner = Some(format!(
                "At most {MAX_ATTACHMENTS_PER_MESSAGE} attachments per message."
            ));
            return;
        }
        if std::fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_ATTACHMENT_BYTES as u64) {
            self.banner = Some(format!(
                "\"{file_name}\" is larger than the 20 MB attachment limit."
            ));
            return;
        }
        match std::fs::read(path) {
            Ok(bytes) => self.add_pending_attachment(file_name, mime.to_owned(), bytes),
            Err(err) => {
                self.banner = Some(format!("Could not read \"{file_name}\": {err}"));
            }
        }
    }

    pub(super) fn add_dropped_files(&mut self, dropped: Vec<egui::DroppedFile>) {
        for file in dropped {
            if let Some(path) = file.path {
                self.attach_file_path(&path);
            } else if let Some(bytes) = file.bytes {
                let is_pdf =
                    file.mime == "application/pdf" || file.name.to_lowercase().ends_with(".pdf");
                let mime = if is_pdf {
                    "application/pdf".to_owned()
                } else if file.mime.starts_with("image/") {
                    file.mime.clone()
                } else {
                    "image/png".to_owned()
                };
                let name = if file.name.is_empty() {
                    if is_pdf {
                        "dropped-document.pdf".to_owned()
                    } else {
                        "dropped-image".to_owned()
                    }
                } else {
                    file.name.clone()
                };
                self.add_pending_attachment(name, mime, bytes.to_vec());
            }
        }
    }

    pub(super) fn try_attach_clipboard_image(&mut self, ctx: &Context) {
        let Ok(mut clipboard) = arboard::Clipboard::new() else {
            return;
        };
        let Ok(image) = clipboard.get_image() else {
            return;
        };
        let (width, height) = (image.width as u32, image.height as u32);
        let rgba = image.bytes.into_owned();
        let name = format!("pasted-{}.png", Local::now().format("%H%M%S"));
        let done = self.image_tx.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let Some(rgba) = image::RgbaImage::from_raw(width, height, rgba) else {
                return;
            };
            let mut png = Vec::new();
            if image::DynamicImage::ImageRgba8(rgba)
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .is_ok()
                && done.send(ImageResult::Pasted { name, png }).is_ok()
            {
                repaint.request_repaint();
            }
        });
    }

    pub(super) fn ensure_attachment_textures(&mut self, ctx: &Context) {
        while let Ok(result) = self.image_rx.try_recv() {
            match result {
                ImageResult::Pending { key, image } => {
                    let texture = image.map(|image| thumbnail_texture(ctx, key, image));
                    self.pending_textures.insert(key, texture);
                }
                ImageResult::Stored { id, image } => {
                    let texture = image.map(|image| thumbnail_texture(ctx, id as u64, image));
                    self.attachment_textures.insert(id, texture);
                }
                ImageResult::Pasted { name, png } => {
                    self.add_pending_attachment(name, "image/png".to_owned(), png);
                }
            }
        }

        let pending: Vec<(u64, Vec<u8>)> = self
            .pending_attachments
            .iter()
            .filter(|attachment| !self.pending_textures.contains_key(&attachment.key))
            .map(|attachment| {
                let bytes = if attachment.mime.starts_with("image/") {
                    attachment.bytes.clone()
                } else {
                    Vec::new()
                };
                (attachment.key, bytes)
            })
            .collect();
        for (key, bytes) in pending {
            self.pending_textures.insert(key, None);
            if !bytes.is_empty() {
                let done = self.image_tx.clone();
                let repaint = ctx.clone();
                std::thread::spawn(move || {
                    let image = decode_thumbnail(&bytes);
                    if done.send(ImageResult::Pending { key, image }).is_ok() {
                        repaint.request_repaint();
                    }
                });
            }
        }

        let missing: Vec<(i64, String)> = self
            .selected_conversation()
            .map(|conversation| {
                conversation
                    .turns
                    .iter()
                    .flat_map(|turn| turn.attachments.iter())
                    .filter(|attachment| !self.attachment_textures.contains_key(&attachment.id))
                    .map(|attachment| (attachment.id, attachment.mime.clone()))
                    .collect()
            })
            .unwrap_or_default();
        for (id, mime) in missing {
            self.attachment_textures.insert(id, None);
            if !mime.starts_with("image/") {
                continue;
            }
            let Ok(bytes) = self.db.load_attachment_bytes(id) else {
                continue;
            };
            let done = self.image_tx.clone();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let image = decode_thumbnail(&bytes);
                if done.send(ImageResult::Stored { id, image }).is_ok() {
                    repaint.request_repaint();
                }
            });
        }
    }
}

pub(super) enum ImageResult {
    Pending {
        key: u64,
        image: Option<egui::ColorImage>,
    },
    Stored {
        id: i64,
        image: Option<egui::ColorImage>,
    },
    Pasted {
        name: String,
        png: Vec<u8>,
    },
}

fn thumbnail_texture(ctx: &Context, key: u64, image: egui::ColorImage) -> TextureHandle {
    ctx.load_texture(
        format!("attachment-{key}"),
        image,
        egui::TextureOptions::LINEAR,
    )
}

pub(super) fn attachment_mime_for_path(path: &std::path::Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "pdf" => Some("application/pdf"),
        _ => None,
    }
}

pub(super) fn attachment_data_url(mime: &str, bytes: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

pub(super) fn decode_thumbnail(bytes: &[u8]) -> Option<egui::ColorImage> {
    let decoded = image::load_from_memory(bytes).ok()?;
    let thumb = decoded.thumbnail(320, 320).to_rgba8();
    let size = [thumb.width() as usize, thumb.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(
        size,
        thumb.as_raw(),
    ))
}

pub(super) fn format_bytes(len: i64) -> String {
    if len >= 1_048_576 {
        format!("{:.1} MB", len as f64 / 1_048_576.0)
    } else if len >= 1_024 {
        format!("{:.0} KB", len as f64 / 1_024.0)
    } else {
        format!("{len} B")
    }
}
