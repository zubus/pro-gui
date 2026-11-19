use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use rusqlite::{Connection, params};

use crate::models::{
    AppSettings, AttachmentMeta, Conversation, Folder, NewTurn, Turn, TurnSnapshot, now_string,
};
use crate::providers;

const DEFAULT_MAX_OUTPUT_TOKENS: i64 = 128_000;

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA mmap_size = 0;
            ",
        )?;
        let db = Self { conn };
        db.init()?;
        Ok(db)
    }

    fn init(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS folders (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS conversations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL,
                folder_id INTEGER,
                max_output_tokens INTEGER NOT NULL DEFAULT 128000,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS turns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id INTEGER NOT NULL,
                prompt_text TEXT NOT NULL,
                thinking_text TEXT NOT NULL DEFAULT '',
                answer_text TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL,
                max_output_tokens INTEGER NOT NULL DEFAULT 128000,
                response_id TEXT,
                parent_response_id TEXT,
                status TEXT NOT NULL,
                error_text TEXT,
                created_at TEXT NOT NULL,
                request_started_at TEXT NOT NULL DEFAULT '',
                updated_at TEXT NOT NULL,
                FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS app_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS attachments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                turn_id INTEGER NOT NULL,
                mime TEXT NOT NULL,
                file_name TEXT NOT NULL,
                data BLOB NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY (turn_id) REFERENCES turns(id) ON DELETE CASCADE
            );
            ",
        )?;
        add_column_if_missing(
            &self.conn,
            "conversations",
            "max_output_tokens",
            "INTEGER NOT NULL DEFAULT 128000",
        )?;
        add_column_if_missing(&self.conn, "conversations", "folder_id", "INTEGER")?;
        add_column_if_missing(
            &self.conn,
            "conversations",
            "system_prompt",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        add_column_if_missing(
            &self.conn,
            "conversations",
            "workspace_path",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        add_column_if_missing(
            &self.conn,
            "conversations",
            "context_selection",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        add_column_if_missing(&self.conn, "turns", "input_tokens", "INTEGER")?;
        add_column_if_missing(&self.conn, "turns", "output_tokens", "INTEGER")?;
        add_column_if_missing(&self.conn, "turns", "reasoning_tokens", "INTEGER")?;
        add_column_if_missing(
            &self.conn,
            "turns",
            "max_output_tokens",
            "INTEGER NOT NULL DEFAULT 128000",
        )?;
        add_column_if_missing(
            &self.conn,
            "turns",
            "reasoning_effort",
            "TEXT NOT NULL DEFAULT 'xhigh'",
        )?;
        add_column_if_missing(
            &self.conn,
            "turns",
            "reasoning_summary",
            "TEXT NOT NULL DEFAULT 'detailed'",
        )?;
        add_column_if_missing(
            &self.conn,
            "turns",
            "service_tier",
            "TEXT NOT NULL DEFAULT 'priority'",
        )?;
        add_column_if_missing(
            &self.conn,
            "turns",
            "background",
            "INTEGER NOT NULL DEFAULT 1",
        )?;
        add_column_if_missing(&self.conn, "turns", "store", "INTEGER NOT NULL DEFAULT 1")?;
        add_column_if_missing(
            &self.conn,
            "turns",
            "request_started_at",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        add_column_if_missing(
            &self.conn,
            "turns",
            "prompt_prefix_len",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        let summaries_added = add_column_if_missing(
            &self.conn,
            "conversations",
            "turn_count",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        add_column_if_missing(
            &self.conn,
            "conversations",
            "last_status",
            "TEXT NOT NULL DEFAULT 'idle'",
        )?;
        add_column_if_missing(
            &self.conn,
            "conversations",
            "preview",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        self.conn.execute_batch(
            "
            CREATE INDEX IF NOT EXISTS idx_turns_status ON turns(status, conversation_id);
            CREATE INDEX IF NOT EXISTS idx_attachments_turn ON attachments(turn_id);

            CREATE TRIGGER IF NOT EXISTS turns_summary_insert AFTER INSERT ON turns BEGIN
                UPDATE conversations
                SET
                    turn_count = turn_count + 1,
                    last_status = NEW.status,
                    preview = CASE
                        WHEN NEW.answer_text <> '' THEN substr(NEW.answer_text, 1, 200)
                        ELSE substr(NEW.prompt_text, 1, 200)
                    END
                WHERE id = NEW.conversation_id;
            END;

            CREATE TRIGGER IF NOT EXISTS turns_summary_update
            AFTER UPDATE OF status, answer_text, prompt_text ON turns
            WHEN NEW.id = (SELECT max(id) FROM turns WHERE conversation_id = NEW.conversation_id)
            BEGIN
                UPDATE conversations
                SET
                    last_status = NEW.status,
                    preview = CASE
                        WHEN NEW.answer_text <> '' THEN substr(NEW.answer_text, 1, 200)
                        ELSE substr(NEW.prompt_text, 1, 200)
                    END
                WHERE id = NEW.conversation_id;
            END;

            CREATE TRIGGER IF NOT EXISTS turns_summary_delete AFTER DELETE ON turns BEGIN
                UPDATE conversations
                SET
                    turn_count = (SELECT count(*) FROM turns WHERE conversation_id = OLD.conversation_id),
                    last_status = coalesce(
                        (
                            SELECT status FROM turns
                            WHERE conversation_id = OLD.conversation_id
                            ORDER BY id DESC LIMIT 1
                        ),
                        'idle'
                    ),
                    preview = coalesce(
                        (
                            SELECT CASE
                                WHEN answer_text <> '' THEN substr(answer_text, 1, 200)
                                ELSE substr(prompt_text, 1, 200)
                            END
                            FROM turns
                            WHERE conversation_id = OLD.conversation_id
                            ORDER BY id DESC LIMIT 1
                        ),
                        ''
                    )
                WHERE id = OLD.conversation_id;
            END;
            ",
        )?;
        if summaries_added {
            self.refresh_conversation_summaries()?;
        }
        self.conn.execute(
            "
            UPDATE turns
            SET request_started_at = created_at
            WHERE request_started_at = ''
            ",
            [],
        )?;
        self.conn.execute_batch(
            "
            CREATE INDEX IF NOT EXISTS idx_turns_conversation ON turns(conversation_id, id);

            -- Where each assembled block sits inside a turn's prompt_text.
            -- Ranges, not copies: the text is already in the turn, and this
            -- is what lets a sent message's pasted blocks stay editable.
            CREATE TABLE IF NOT EXISTS turn_blocks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                turn_id INTEGER NOT NULL,
                ordinal INTEGER NOT NULL,
                label TEXT NOT NULL,
                start_byte INTEGER NOT NULL,
                end_byte INTEGER NOT NULL,
                FOREIGN KEY (turn_id) REFERENCES turns(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_turn_blocks_turn ON turn_blocks(turn_id, ordinal);
            ",
        )?;
        self.init_search_index()?;
        Ok(())
    }

    fn init_search_index(&self) -> Result<()> {
        let already_built: bool = self.conn.query_row(
            "SELECT count(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'turns_fts'",
            [],
            |row| row.get(0),
        )?;

        self.conn.execute_batch(
            "
            CREATE VIRTUAL TABLE IF NOT EXISTS turns_fts USING fts5(
                prompt_text,
                content = 'turns',
                content_rowid = 'id',
                detail = 'none',
                columnsize = 0
            );

            CREATE TRIGGER IF NOT EXISTS turns_fts_insert AFTER INSERT ON turns BEGIN
                INSERT INTO turns_fts(rowid, prompt_text) VALUES (new.id, new.prompt_text);
            END;

            CREATE TRIGGER IF NOT EXISTS turns_fts_delete AFTER DELETE ON turns BEGIN
                INSERT INTO turns_fts(turns_fts, rowid, prompt_text)
                VALUES ('delete', old.id, old.prompt_text);
            END;

            CREATE TRIGGER IF NOT EXISTS turns_fts_update
            AFTER UPDATE OF prompt_text ON turns BEGIN
                INSERT INTO turns_fts(turns_fts, rowid, prompt_text)
                VALUES ('delete', old.id, old.prompt_text);
                INSERT INTO turns_fts(rowid, prompt_text) VALUES (new.id, new.prompt_text);
            END;
            ",
        )?;

        if !already_built {
            self.conn
                .execute("INSERT INTO turns_fts(turns_fts) VALUES ('rebuild')", [])?;
        }
        Ok(())
    }

    pub fn path(&self) -> Option<PathBuf> {
        self.conn
            .path()
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
    }

    pub fn open_reader(path: &Path) -> Result<Self> {
        Ok(Self {
            conn: Connection::open(path)?,
        })
    }

    pub fn atomically<T>(&self, work: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        self.conn.execute_batch("SAVEPOINT atomically")?;
        match work(self) {
            Ok(value) => {
                self.conn.execute_batch("RELEASE atomically")?;
                Ok(value)
            }
            Err(err) => {
                let _ = self
                    .conn
                    .execute_batch("ROLLBACK TO atomically; RELEASE atomically");
                Err(err)
            }
        }
    }

    fn refresh_conversation_summaries(&self) -> Result<()> {
        self.conn.execute(
            "
            UPDATE conversations
            SET
                turn_count = (SELECT count(*) FROM turns t WHERE t.conversation_id = conversations.id),
                last_status = coalesce(
                    (
                        SELECT t.status FROM turns t
                        WHERE t.conversation_id = conversations.id
                        ORDER BY t.id DESC LIMIT 1
                    ),
                    'idle'
                ),
                preview = coalesce(
                    (
                        SELECT CASE
                            WHEN t.answer_text <> '' THEN substr(t.answer_text, 1, 200)
                            ELSE substr(t.prompt_text, 1, 200)
                        END
                        FROM turns t
                        WHERE t.conversation_id = conversations.id
                        ORDER BY t.id DESC LIMIT 1
                    ),
                    ''
                )
            ",
            [],
        )?;
        Ok(())
    }

    pub fn backup_to(&self, target: &Path) -> Result<()> {
        let target = target
            .to_str()
            .with_context(|| format!("backup path {} is not UTF-8", target.display()))?;
        self.conn.execute("VACUUM INTO ?", params![target])?;
        Ok(())
    }

    pub fn search_conversation_ids(&self, query: &str) -> Result<Option<Vec<i64>>> {
        let Some(match_expression) = fts_match_expression(query) else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare(
            "
            SELECT DISTINCT conversation_id
            FROM turns
            WHERE id IN (SELECT rowid FROM turns_fts WHERE turns_fts MATCH ?)
            ",
        )?;
        let ids = stmt
            .query_map(params![match_expression], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(Some(ids))
    }

    pub fn load_conversation_summaries(&self) -> Result<Vec<Conversation>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT
                c.id,
                c.title,
                c.folder_id,
                c.system_prompt,
                c.workspace_path,
                c.context_selection,
                c.max_output_tokens,
                c.updated_at,
                c.turn_count,
                c.last_status,
                c.preview
            FROM conversations c
            ORDER BY c.updated_at DESC, c.id DESC
            ",
        )?;

        let conversations = stmt
            .query_map([], |row| {
                let selection_json: String = row.get(5)?;
                Ok(Conversation {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    folder_id: row.get(2)?,
                    system_prompt: row.get(3)?,
                    workspace_path: row.get(4)?,
                    context_selection: serde_json::from_str(&selection_json).unwrap_or_default(),
                    max_output_tokens: clamp_max_output_tokens(row.get(6)?),
                    updated_at: row.get(7)?,
                    turn_count: row.get::<_, i64>(8)? as usize,
                    last_status: row
                        .get::<_, Option<String>>(9)?
                        .unwrap_or_else(|| "idle".to_owned()),
                    preview: row.get::<_, Option<String>>(10)?.unwrap_or_default(),
                    turns_loaded: false,
                    turns: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(conversations)
    }

    pub fn load_turns(&self, conversation_id: i64) -> Result<Vec<Turn>> {
        let mut turns_stmt = self.conn.prepare(
            "
            SELECT
                id,
                prompt_text,
                thinking_text,
                answer_text,
                model,
                max_output_tokens,
                reasoning_effort,
                reasoning_summary,
                service_tier,
                background,
                store,
                response_id,
                parent_response_id,
                status,
                error_text,
                input_tokens,
                output_tokens,
                reasoning_tokens,
                created_at,
                request_started_at,
                updated_at,
                prompt_prefix_len
            FROM turns
            WHERE conversation_id = ?
            ORDER BY id ASC
            ",
        )?;

        let mut turns = turns_stmt
            .query_map([conversation_id], |row| {
                Ok(Turn {
                    id: row.get(0)?,
                    prompt_text: row.get(1)?,
                    attachments: Vec::new(),
                    thinking_text: row.get(2)?,
                    answer_text: row.get(3)?,
                    model: row.get(4)?,
                    max_output_tokens: clamp_max_output_tokens(row.get(5)?),
                    reasoning_effort: row.get(6)?,
                    reasoning_summary: row.get(7)?,
                    service_tier: row.get(8)?,
                    background: row.get(9)?,
                    store: row.get(10)?,
                    response_id: row.get(11)?,
                    parent_response_id: row.get(12)?,
                    status: row.get(13)?,
                    error_text: row.get(14)?,
                    input_tokens: row.get(15)?,
                    output_tokens: row.get(16)?,
                    reasoning_tokens: row.get(17)?,
                    created_at: row.get(18)?,
                    request_started_at: row.get(19)?,
                    updated_at: row.get(20)?,
                    prompt_prefix_len: row.get::<_, i64>(21)?.max(0) as usize,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut attachments_stmt = self.conn.prepare(
            "
            SELECT id, mime, file_name, length(data)
            FROM attachments
            WHERE turn_id = ?
            ORDER BY id ASC
            ",
        )?;
        for turn in &mut turns {
            turn.attachments = attachments_stmt
                .query_map([turn.id], |row| {
                    Ok(AttachmentMeta {
                        id: row.get(0)?,
                        mime: row.get(1)?,
                        file_name: row.get(2)?,
                        byte_len: row.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
        }

        Ok(turns)
    }

    pub fn load_turn_blocks(&self, turn_id: i64) -> Result<Vec<(String, usize, usize)>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT label, start_byte, end_byte
            FROM turn_blocks
            WHERE turn_id = ?
            ORDER BY ordinal ASC
            ",
        )?;
        let blocks = stmt
            .query_map(params![turn_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?.max(0) as usize,
                    row.get::<_, i64>(2)?.max(0) as usize,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(blocks)
    }

    pub fn replace_turn_blocks(
        &self,
        turn_id: i64,
        blocks: &[(String, usize, usize)],
    ) -> Result<()> {
        self.atomically(|_| {
            self.conn.execute(
                "DELETE FROM turn_blocks WHERE turn_id = ?",
                params![turn_id],
            )?;
            let mut stmt = self.conn.prepare(
                "
                INSERT INTO turn_blocks (turn_id, ordinal, label, start_byte, end_byte)
                VALUES (?, ?, ?, ?, ?)
                ",
            )?;
            for (ordinal, (label, start, end)) in blocks.iter().enumerate() {
                stmt.execute(params![
                    turn_id,
                    ordinal as i64,
                    label,
                    *start as i64,
                    *end as i64
                ])?;
            }
            Ok(())
        })
    }

    pub fn conversations_with_pending_turns(&self) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT DISTINCT conversation_id
            FROM turns
            WHERE status IN ('queued', 'in_progress')
            ",
        )?;
        let ids = stmt
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(ids)
    }

    pub fn insert_attachment(
        &self,
        turn_id: i64,
        mime: &str,
        file_name: &str,
        data: &[u8],
    ) -> Result<AttachmentMeta> {
        self.conn.execute(
            "
            INSERT INTO attachments (turn_id, mime, file_name, data, created_at)
            VALUES (?, ?, ?, ?, ?)
            ",
            params![turn_id, mime, file_name, data, now_string()],
        )?;
        Ok(AttachmentMeta {
            id: self.conn.last_insert_rowid(),
            mime: mime.to_owned(),
            file_name: file_name.to_owned(),
            byte_len: data.len() as i64,
        })
    }

    pub fn load_attachment_bytes(&self, attachment_id: i64) -> Result<Vec<u8>> {
        let bytes = self.conn.query_row(
            "SELECT data FROM attachments WHERE id = ?",
            params![attachment_id],
            |row| row.get::<_, Vec<u8>>(0),
        )?;
        Ok(bytes)
    }

    pub fn create_conversation(&self, title: &str, folder_id: Option<i64>) -> Result<i64> {
        let now = now_string();
        self.conn.execute(
            "
            INSERT INTO conversations (title, folder_id, max_output_tokens, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?)
            ",
            params![title, folder_id, DEFAULT_MAX_OUTPUT_TOKENS, now, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn load_folders(&self) -> Result<Vec<Folder>> {
        let mut stmt = self.conn.prepare(
            "
            SELECT id, name
            FROM folders
            ORDER BY name COLLATE NOCASE ASC, id ASC
            ",
        )?;
        let folders = stmt
            .query_map([], |row| {
                Ok(Folder {
                    id: row.get(0)?,
                    name: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(folders)
    }

    pub fn create_folder(&self, name: &str) -> Result<i64> {
        let now = now_string();
        self.conn.execute(
            "
            INSERT INTO folders (name, created_at, updated_at)
            VALUES (?, ?, ?)
            ",
            params![name, now, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rename_folder(&self, folder_id: i64, name: &str) -> Result<()> {
        self.conn.execute(
            "
            UPDATE folders
            SET name = ?, updated_at = ?
            WHERE id = ?
            ",
            params![name, now_string(), folder_id],
        )?;
        Ok(())
    }

    pub fn delete_folder(&self, folder_id: i64) -> Result<()> {
        self.atomically(|db| {
            db.conn.execute(
                "UPDATE conversations SET folder_id = NULL WHERE folder_id = ?",
                params![folder_id],
            )?;
            db.conn
                .execute("DELETE FROM folders WHERE id = ?", params![folder_id])?;
            Ok(())
        })
    }

    pub fn set_conversation_folder(
        &self,
        conversation_id: i64,
        folder_id: Option<i64>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE conversations SET folder_id = ? WHERE id = ?",
            params![folder_id, conversation_id],
        )?;
        Ok(())
    }

    pub fn delete_conversation(&self, conversation_id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM conversations WHERE id = ?",
            params![conversation_id],
        )?;
        Ok(())
    }

    pub fn delete_turns_after(&self, conversation_id: i64, turn_id: i64) -> Result<usize> {
        let removed = self.conn.execute(
            "DELETE FROM turns WHERE conversation_id = ? AND id > ?",
            params![conversation_id, turn_id],
        )?;
        Ok(removed)
    }

    pub fn update_turn_prompt(
        &self,
        turn_id: i64,
        prompt_text: &str,
        prompt_prefix_len: usize,
        parent_response_id: Option<&str>,
    ) -> Result<()> {
        let now = now_string();
        self.conn.execute(
            "
            UPDATE turns
            SET prompt_text = ?, prompt_prefix_len = ?, parent_response_id = ?, updated_at = ?
            WHERE id = ?
            ",
            params![
                prompt_text,
                prompt_prefix_len as i64,
                parent_response_id,
                now,
                turn_id
            ],
        )?;
        Ok(())
    }

    pub fn update_conversation_context(
        &self,
        conversation_id: i64,
        workspace_path: &str,
        context_selection: &[String],
    ) -> Result<()> {
        let selection_json = serde_json::to_string(context_selection)?;
        self.conn.execute(
            "UPDATE conversations SET workspace_path = ?, context_selection = ? WHERE id = ?",
            params![workspace_path, selection_json, conversation_id],
        )?;
        Ok(())
    }

    pub fn update_conversation_system_prompt(
        &self,
        conversation_id: i64,
        system_prompt: &str,
    ) -> Result<()> {
        self.conn.execute(
            "
            UPDATE conversations
            SET system_prompt = ?, updated_at = ?
            WHERE id = ?
            ",
            params![system_prompt, now_string(), conversation_id],
        )?;
        Ok(())
    }

    pub fn rename_conversation(&self, conversation_id: i64, title: &str) -> Result<()> {
        self.conn.execute(
            "
            UPDATE conversations
            SET title = ?, updated_at = ?
            WHERE id = ?
            ",
            params![title, now_string(), conversation_id],
        )?;
        Ok(())
    }

    pub fn update_conversation_max_output_tokens(
        &self,
        conversation_id: i64,
        max_output_tokens: i64,
    ) -> Result<()> {
        self.conn.execute(
            "
            UPDATE conversations
            SET max_output_tokens = ?, updated_at = ?
            WHERE id = ?
            ",
            params![
                clamp_max_output_tokens(max_output_tokens),
                now_string(),
                conversation_id
            ],
        )?;
        Ok(())
    }

    pub fn insert_turn(&self, turn: &NewTurn) -> Result<Turn> {
        let now = now_string();
        self.conn.execute(
            "
            INSERT INTO turns (
                conversation_id,
                prompt_text,
                thinking_text,
                answer_text,
                model,
                max_output_tokens,
                reasoning_effort,
                reasoning_summary,
                service_tier,
                background,
                store,
                response_id,
                parent_response_id,
                status,
                error_text,
                created_at,
                request_started_at,
                updated_at,
                prompt_prefix_len
            )
            VALUES (?, ?, '', '', ?, ?, ?, ?, ?, ?, ?, NULL, ?, 'queued', NULL, ?, ?, ?, ?)
            ",
            params![
                turn.conversation_id,
                turn.prompt_text,
                turn.model,
                clamp_max_output_tokens(turn.max_output_tokens),
                turn.reasoning_effort,
                turn.reasoning_summary,
                turn.service_tier,
                turn.background,
                turn.store,
                turn.parent_response_id,
                now,
                now,
                now,
                turn.prompt_prefix_len as i64
            ],
        )?;
        let id = self.conn.last_insert_rowid();

        self.conn.execute(
            "
            UPDATE conversations
            SET updated_at = ?
            WHERE id = ?
            ",
            params![now, turn.conversation_id],
        )?;

        Ok(Turn {
            id,
            prompt_text: turn.prompt_text.clone(),
            attachments: Vec::new(),
            thinking_text: String::new(),
            answer_text: String::new(),
            model: turn.model.clone(),
            max_output_tokens: clamp_max_output_tokens(turn.max_output_tokens),
            reasoning_effort: turn.reasoning_effort.clone(),
            reasoning_summary: turn.reasoning_summary.clone(),
            service_tier: turn.service_tier.clone(),
            background: turn.background,
            store: turn.store,
            response_id: None,
            parent_response_id: turn.parent_response_id.clone(),
            status: "queued".to_owned(),
            error_text: None,
            input_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            created_at: now.clone(),
            request_started_at: now.clone(),
            updated_at: now,
            prompt_prefix_len: turn.prompt_prefix_len,
        })
    }

    pub fn save_turn_state(&self, turn: &Turn) -> Result<()> {
        self.atomically(|_| {
            self.conn.execute(
                "
                UPDATE turns
                SET
                    response_id = ?,
                    model = ?,
                    thinking_text = ?,
                    answer_text = ?,
                    status = ?,
                    error_text = ?,
                    input_tokens = ?,
                    output_tokens = ?,
                    reasoning_tokens = ?,
                    updated_at = ?
                WHERE id = ?
                ",
                params![
                    turn.response_id,
                    turn.model,
                    turn.thinking_text,
                    turn.answer_text,
                    turn.status,
                    turn.error_text,
                    turn.input_tokens,
                    turn.output_tokens,
                    turn.reasoning_tokens,
                    turn.updated_at,
                    turn.id
                ],
            )?;
            self.conn.execute(
                "
                UPDATE conversations
                SET updated_at = ?
                WHERE id = (SELECT conversation_id FROM turns WHERE id = ?)
                ",
                params![turn.updated_at, turn.id],
            )?;
            Ok(())
        })
    }

    pub fn update_turn(&self, turn_id: i64, snapshot: &TurnSnapshot) -> Result<()> {
        self.atomically(|_| {
            let now = now_string();
            self.conn.execute(
                "
                UPDATE turns
                SET
                    response_id = COALESCE(?, response_id),
                    model = COALESCE(?, model),
                    thinking_text = ?,
                    answer_text = ?,
                    status = ?,
                    error_text = ?,
                    input_tokens = COALESCE(?, input_tokens),
                    output_tokens = COALESCE(?, output_tokens),
                    reasoning_tokens = COALESCE(?, reasoning_tokens),
                    updated_at = ?
                WHERE id = ?
                ",
                params![
                    snapshot.response_id,
                    snapshot.model,
                    snapshot.thinking_text,
                    snapshot.answer_text,
                    snapshot.status,
                    snapshot.error_text,
                    snapshot.input_tokens,
                    snapshot.output_tokens,
                    snapshot.reasoning_tokens,
                    now,
                    turn_id
                ],
            )?;
            self.conn.execute(
                "
                UPDATE conversations
                SET updated_at = ?
                WHERE id = (SELECT conversation_id FROM turns WHERE id = ?)
                ",
                params![now, turn_id],
            )?;
            Ok(())
        })
    }

    pub fn reset_turn_for_retry(&self, turn_id: i64, service_tier: &str) -> Result<()> {
        self.atomically(|_| {
            let now = now_string();
            self.conn.execute(
                "
                UPDATE turns
                SET
                    response_id = NULL,
                    thinking_text = '',
                    answer_text = '',
                    service_tier = ?,
                    status = 'queued',
                    error_text = NULL,
                    input_tokens = NULL,
                    output_tokens = NULL,
                    reasoning_tokens = NULL,
                    request_started_at = ?,
                    updated_at = ?
                WHERE id = ?
                ",
                params![service_tier, now, now, turn_id],
            )?;
            self.conn.execute(
                "
                UPDATE conversations
                SET updated_at = ?
                WHERE id = (SELECT conversation_id FROM turns WHERE id = ?)
                ",
                params![now, turn_id],
            )?;
            Ok(())
        })
    }

    pub fn load_app_settings(&self) -> Result<AppSettings> {
        let mut settings = AppSettings::default();
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM app_settings ORDER BY key ASC")?;

        let entries = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        for (key, value) in entries {
            match key.as_str() {
                "api_key" => settings.api_key = value,
                "gemini_api_key" => settings.gemini_api_key = value,
                "anthropic_api_key" => settings.anthropic_api_key = value,
                "selected_model" => settings.selected_model = value,
                "render_markdown_answers" => {
                    settings.render_markdown_answers = parse_bool_setting(&value);
                }
                "render_latex_blocks" => {
                    settings.render_latex_blocks = parse_bool_setting(&value);
                }
                "auto_resend_failed_answers" => {
                    settings.auto_resend_failed_answers = parse_bool_setting(&value);
                }
                "use_priority_processing" => {
                    settings.use_priority_processing = parse_bool_setting(&value);
                }
                "enter_sends" => {
                    settings.enter_sends = parse_bool_setting(&value);
                }
                "show_request_chips" => {
                    settings.show_request_chips = parse_bool_setting(&value);
                }
                "show_context_panel" => {
                    settings.show_context_panel = parse_bool_setting(&value);
                }
                "paste_block_kb" => {
                    if let Ok(parsed) = value.parse::<i64>() {
                        settings.paste_block_kb = parsed.clamp(1, 4_096);
                    }
                }
                "font_size" => {
                    if let Ok(parsed) = value.parse::<f32>() {
                        settings.font_size = parsed.clamp(11.0, 22.0);
                    }
                }
                _ => {}
            }
        }

        Ok(settings)
    }

    pub fn save_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "
            INSERT INTO app_settings (key, value)
            VALUES (?, ?)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            ",
            params![key, value],
        )?;
        Ok(())
    }
}

fn parse_bool_setting(value: &str) -> bool {
    matches!(value, "1" | "true" | "TRUE" | "yes" | "on")
}

fn clamp_max_output_tokens(value: i64) -> i64 {
    value.clamp(1_024, providers::MAX_OUTPUT_TOKENS)
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<bool> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&pragma)?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let added = !columns.iter().any(|existing| existing == column);
    if added {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )?;
    }

    Ok(added)
}

fn fts_match_expression(query: &str) -> Option<String> {
    let terms = query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| format!("\"{}\"*", token.replace('"', "\"\"")))
        .collect::<Vec<_>>();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" AND "))
    }
}

#[cfg(test)]
mod tests;
