use anyhow::Context as _;

use super::*;

pub(super) fn slugify_title(title: &str) -> String {
    let mut slug = String::new();
    for ch in title.chars() {
        if slug.len() >= 40 {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_owned();
    if slug.is_empty() {
        "chat".to_owned()
    } else {
        slug
    }
}

pub(super) fn export_directory() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let downloads = PathBuf::from(home).join("Downloads");
        if downloads.is_dir() {
            return downloads;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub(super) fn write_export_file(slug: &str, extension: &str, content: &str) -> Result<PathBuf> {
    let timestamp = Local::now().format("%Y%m%d-%H%M%S");
    let path = export_directory().join(format!("pro-gui-{slug}-{timestamp}.{extension}"));
    std::fs::write(&path, content)
        .with_context(|| format!("could not write {}", path.display()))?;
    Ok(path)
}

pub(super) fn export_banner(result: Result<PathBuf>) -> String {
    match result {
        Ok(path) => format!("Exported to {}", path.display()),
        Err(err) => format!("Export failed: {err:#}"),
    }
}

pub(super) fn conversation_markdown(conversation: &Conversation) -> String {
    let mut out = format!("# {}\n\n", conversation.title);
    if !conversation.system_prompt.trim().is_empty() {
        out.push_str("## System prompt\n\n");
        out.push_str(conversation.system_prompt.trim());
        out.push_str("\n\n");
    }
    for turn in &conversation.turns {
        out.push_str(&format!(
            "## You ({})\n\n{}\n\n",
            turn.created_at, turn.prompt_text
        ));
        for attachment in &turn.attachments {
            out.push_str(&format!(
                "*Attachment: {} ({}, {})*\n",
                attachment.file_name,
                attachment.mime,
                format_bytes(attachment.byte_len)
            ));
        }
        if !turn.attachments.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "## {} ({}, {})\n\n",
            turn.model, turn.updated_at, turn.status
        ));
        out.push_str(&format!(
            "`model={} reasoning.effort={} service_tier={} max_output_tokens={}`\n\n",
            turn.model, turn.reasoning_effort, turn.service_tier, turn.max_output_tokens
        ));
        if let Some(error) = &turn.error_text {
            out.push_str(&format!("> Error: {error}\n\n"));
        }
        if !turn.answer_text.is_empty() {
            out.push_str(&turn.answer_text);
            out.push_str("\n\n");
        }
        if !turn.thinking_text.is_empty() {
            out.push_str("<details>\n<summary>Thinking</summary>\n\n");
            out.push_str(&turn.thinking_text);
            out.push_str("\n\n</details>\n\n");
        }
    }
    out
}

pub(super) fn conversation_json_value(conversation: &Conversation) -> serde_json::Value {
    let turns = conversation
        .turns
        .iter()
        .map(|turn| {
            let attachments = turn
                .attachments
                .iter()
                .map(|attachment| {
                    serde_json::json!({
                        "name": attachment.file_name,
                        "mime": attachment.mime,
                        "bytes": attachment.byte_len,
                    })
                })
                .collect::<Vec<_>>();
            serde_json::json!({
                "id": turn.id,
                "prompt": turn.prompt_text,
                "attachments": attachments,
                "answer": turn.answer_text,
                "thinking": turn.thinking_text,
                "model": turn.model,
                "reasoning_effort": turn.reasoning_effort,
                "reasoning_summary": turn.reasoning_summary,
                "service_tier": turn.service_tier,
                "max_output_tokens": turn.max_output_tokens,
                "background": turn.background,
                "store": turn.store,
                "response_id": turn.response_id,
                "parent_response_id": turn.parent_response_id,
                "status": turn.status,
                "error": turn.error_text,
                "input_tokens": turn.input_tokens,
                "output_tokens": turn.output_tokens,
                "reasoning_tokens": turn.reasoning_tokens,
                "created_at": turn.created_at,
                "updated_at": turn.updated_at,
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "title": conversation.title,
        "system_prompt": conversation.system_prompt,
        "updated_at": conversation.updated_at,
        "turns": turns,
    })
}

pub(super) fn conversation_json(conversation: &Conversation) -> Result<String> {
    Ok(serde_json::to_string_pretty(&conversation_json_value(
        conversation,
    ))?)
}

pub(super) fn export_all_chats(
    db_path: &std::path::Path,
    out_dir: &std::path::Path,
) -> Result<PathBuf> {
    use std::io::Write;

    let db = Database::open_reader(db_path)?;
    let conversations = db.load_conversation_summaries()?;
    let timestamp = Local::now().format("%Y%m%d-%H%M%S");
    let path = out_dir.join(format!("pro-gui-all-chats-{timestamp}.json"));
    let file = std::fs::File::create(&path)
        .with_context(|| format!("could not create {}", path.display()))?;
    let mut out = std::io::BufWriter::new(file);

    write!(
        out,
        "{{\"exported_at\":{},\"conversations\":[",
        serde_json::Value::String(now_string())
    )?;
    for (index, mut conversation) in conversations.into_iter().enumerate() {
        conversation.turns = db.load_turns(conversation.id)?;
        if index > 0 {
            out.write_all(b",")?;
        }
        serde_json::to_writer(&mut out, &conversation_json_value(&conversation))?;
    }
    out.write_all(b"]}\n")?;
    out.flush()?;
    Ok(path)
}

pub(super) fn backup_database_file(db: &Database) -> Result<PathBuf> {
    let timestamp = Local::now().format("%Y%m%d-%H%M%S");
    let target = export_directory().join(format!("pro-gui-backup-{timestamp}.sqlite3"));
    db.backup_to(&target)
        .with_context(|| format!("could not back up to {}", target.display()))?;
    Ok(target)
}
