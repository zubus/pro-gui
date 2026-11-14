use std::path::PathBuf;

use super::*;

struct TempDatabase {
    path: PathBuf,
}

impl TempDatabase {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "pro-gui-test-{name}-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        Self { path }
    }

    fn open(&self) -> Database {
        Database::open(&self.path).expect("test database opens")
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

fn new_turn(conversation_id: i64, prompt: &str) -> NewTurn {
    NewTurn {
        conversation_id,
        prompt_text: prompt.to_owned(),
        prompt_prefix_len: 0,
        model: "gpt-6-astra-pro".to_owned(),
        max_output_tokens: 128_000,
        reasoning_effort: "max".to_owned(),
        reasoning_summary: "auto".to_owned(),
        service_tier: "default".to_owned(),
        background: true,
        store: true,
        parent_response_id: None,
    }
}

fn summary_of(db: &Database, conversation_id: i64) -> (usize, String, String) {
    let conversation = db
        .load_conversation_summaries()
        .expect("summaries load")
        .into_iter()
        .find(|conversation| conversation.id == conversation_id)
        .expect("conversation is listed");
    (
        conversation.turn_count,
        conversation.last_status,
        conversation.preview,
    )
}

#[test]
fn chat_summaries_follow_inserts_updates_and_deletes() {
    let temp = TempDatabase::new("summaries");
    let db = temp.open();
    let chat = db.create_conversation("Chat", None).unwrap();
    assert_eq!(summary_of(&db, chat), (0, "idle".to_owned(), String::new()));

    let first = db.insert_turn(&new_turn(chat, "first question")).unwrap();
    let mut second = db.insert_turn(&new_turn(chat, "second question")).unwrap();
    assert_eq!(summary_of(&db, chat).0, 2);
    assert_eq!(summary_of(&db, chat).2, "second question");

    second.status = "completed".to_owned();
    second.answer_text = "the answer".to_owned();
    db.save_turn_state(&second).unwrap();
    assert_eq!(
        summary_of(&db, chat),
        (2, "completed".to_owned(), "the answer".to_owned())
    );

    db.delete_turns_after(chat, first.id).unwrap();
    assert_eq!(summary_of(&db, chat).0, 1);
    assert_eq!(summary_of(&db, chat).2, "first question");
}

#[test]
fn opening_an_older_database_fills_in_the_chat_summaries() {
    let temp = TempDatabase::new("migration");
    let chat = {
        let db = temp.open();
        let chat = db.create_conversation("Chat", None).unwrap();
        db.insert_turn(&new_turn(chat, "kept question")).unwrap();
        db.conn
            .execute_batch(
                "
                DROP TRIGGER turns_summary_insert;
                DROP TRIGGER turns_summary_update;
                DROP TRIGGER turns_summary_delete;
                ALTER TABLE conversations DROP COLUMN turn_count;
                ALTER TABLE conversations DROP COLUMN last_status;
                ALTER TABLE conversations DROP COLUMN preview;
                ",
            )
            .unwrap();
        chat
    };

    let db = temp.open();
    assert_eq!(
        summary_of(&db, chat),
        (1, "queued".to_owned(), "kept question".to_owned())
    );
}

#[test]
fn a_failed_atomic_write_leaves_nothing_behind() {
    let temp = TempDatabase::new("atomic");
    let db = temp.open();
    let chat = db.create_conversation("Chat", None).unwrap();

    let result: Result<()> = db.atomically(|db| {
        db.insert_turn(&new_turn(chat, "half written"))?;
        Err(anyhow::anyhow!("attachment failed"))
    });

    assert!(result.is_err());
    assert!(db.load_turns(chat).unwrap().is_empty());
    assert_eq!(summary_of(&db, chat).0, 0);
}

#[test]
fn a_backup_includes_writes_still_in_the_wal() {
    let temp = TempDatabase::new("backup");
    let db = temp.open();
    let chat = db.create_conversation("Chat", None).unwrap();
    db.insert_turn(&new_turn(chat, "not yet checkpointed"))
        .unwrap();

    let target = TempDatabase::new("backup-copy");
    db.backup_to(&target.path).unwrap();
    let copy = target.open();
    assert_eq!(copy.load_turns(chat).unwrap().len(), 1);
}
