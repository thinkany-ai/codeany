//! The SQLite store behind [`crate::threads::Store`].
//!
//! Threads are kept whole in memory; saving compares each thread and item
//! with what was last written and writes only the rows that changed, so a
//! save after a turn touches a few rows instead of rewriting everything.
//! WAL mode lets a dev build and the installed app share the database.

use std::{
    cell::RefCell,
    collections::{HashMap, hash_map::DefaultHasher},
    hash::{Hash as _, Hasher as _},
    path::Path,
};

use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::Value;

use crate::threads::Thread;

const SCHEMA: &str = "
    PRAGMA journal_mode = WAL;
    CREATE TABLE IF NOT EXISTS settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS threads (
        id INTEGER PRIMARY KEY,
        updated_at INTEGER NOT NULL,
        meta TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS items (
        thread_id INTEGER NOT NULL,
        seq INTEGER NOT NULL,
        data TEXT NOT NULL,
        PRIMARY KEY (thread_id, seq)
    ) WITHOUT ROWID;
";

/// What was last written for a thread: its metadata and each item, hashed.
#[derive(Default)]
struct Written {
    meta: u64,
    items: Vec<u64>,
}

pub struct Db {
    conn: Connection,
    written: RefCell<HashMap<u64, Written>>,
    settings: RefCell<HashMap<String, u64>>,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Db")
    }
}

fn hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// A thread's fields without its items, which have their own table.
fn thread_meta(thread: &Thread) -> String {
    let mut value = serde_json::to_value(thread).unwrap_or(Value::Null);
    if let Some(map) = value.as_object_mut() {
        map.remove("items");
    }
    value.to_string()
}

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Db> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        Ok(Db {
            conn,
            written: RefCell::default(),
            settings: RefCell::default(),
        })
    }

    pub fn setting<T: serde::de::DeserializeOwned>(&self, key: &str) -> Option<T> {
        let text: String = self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| row.get(0))
            .optional()
            .ok()??;
        self.settings.borrow_mut().insert(key.to_string(), hash(&text));
        serde_json::from_str(&text).ok()
    }

    pub fn set_setting<T: serde::Serialize>(&self, key: &str, value: &T) {
        let Ok(text) = serde_json::to_string(value) else { return };
        let digest = hash(&text);
        if self.settings.borrow().get(key) == Some(&digest) {
            return;
        }
        if self
            .conn
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, text],
            )
            .is_ok()
        {
            self.settings.borrow_mut().insert(key.to_string(), digest);
        }
    }

    /// Every thread, newest first.
    pub fn load_threads(&self) -> Vec<Thread> {
        let mut threads = Vec::new();
        let Ok(mut stmt) = self.conn.prepare("SELECT id, meta FROM threads ORDER BY updated_at DESC") else {
            return threads;
        };
        let rows: Vec<(i64, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default();
        let Ok(mut items_stmt) = self
            .conn
            .prepare("SELECT data FROM items WHERE thread_id = ?1 ORDER BY seq")
        else {
            return threads;
        };
        let mut written = self.written.borrow_mut();
        for (id, meta) in rows {
            let item_texts: Vec<String> = items_stmt
                .query_map([id], |row| row.get(0))
                .map(|rows| rows.filter_map(Result::ok).collect())
                .unwrap_or_default();
            let mut value: Value = match serde_json::from_str(&meta) {
                Ok(value) => value,
                Err(_) => continue,
            };
            value["items"] = Value::Array(
                item_texts
                    .iter()
                    .filter_map(|text| serde_json::from_str(text).ok())
                    .collect(),
            );
            let Ok(thread) = serde_json::from_value::<Thread>(value) else { continue };
            written.insert(
                thread.id,
                Written {
                    meta: hash(&meta),
                    items: item_texts.iter().map(|t| hash(t)).collect(),
                },
            );
            threads.push(thread);
        }
        threads
    }

    /// Write what changed since the last save. Threads this process never
    /// saw are left alone, so another process's new threads survive.
    pub fn save_threads(&self, threads: &[Thread]) -> rusqlite::Result<()> {
        let mut written = self.written.borrow_mut();
        let tx = self.conn.unchecked_transaction()?;
        for thread in threads {
            let entry = written.entry(thread.id).or_default();
            let meta = thread_meta(thread);
            let meta_hash = hash(&meta);
            if entry.meta != meta_hash {
                tx.execute(
                    "INSERT INTO threads (id, updated_at, meta) VALUES (?1, ?2, ?3)
                     ON CONFLICT(id) DO UPDATE SET updated_at = excluded.updated_at, meta = excluded.meta",
                    params![thread.id as i64, thread.updated_at as i64, meta],
                )?;
                entry.meta = meta_hash;
            }
            for (seq, item) in thread.items.iter().enumerate() {
                let text = serde_json::to_string(item).unwrap_or_default();
                let digest = hash(&text);
                if entry.items.get(seq) == Some(&digest) {
                    continue;
                }
                tx.execute(
                    "INSERT INTO items (thread_id, seq, data) VALUES (?1, ?2, ?3)
                     ON CONFLICT(thread_id, seq) DO UPDATE SET data = excluded.data",
                    params![thread.id as i64, seq as i64, text],
                )?;
                if seq < entry.items.len() {
                    entry.items[seq] = digest;
                } else {
                    entry.items.push(digest);
                }
            }
            if entry.items.len() > thread.items.len() {
                tx.execute(
                    "DELETE FROM items WHERE thread_id = ?1 AND seq >= ?2",
                    params![thread.id as i64, thread.items.len() as i64],
                )?;
                entry.items.truncate(thread.items.len());
            }
        }
        // Threads this process wrote before and has since deleted.
        let gone: Vec<u64> = written
            .keys()
            .filter(|id| !threads.iter().any(|t| t.id == **id))
            .copied()
            .collect();
        for id in gone {
            tx.execute("DELETE FROM items WHERE thread_id = ?1", [id as i64])?;
            tx.execute("DELETE FROM threads WHERE id = ?1", [id as i64])?;
            written.remove(&id);
        }
        tx.commit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::threads::ChatItem;

    fn temp_db(name: &str) -> (Db, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("codeany-db-test-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        (Db::open(&path).unwrap(), path)
    }

    #[test]
    fn round_trips_and_writes_only_changes() {
        let (db, path) = temp_db("roundtrip");
        let mut thread = Thread::new(None, "codex".into(), "full".into());
        thread.items.push(ChatItem::User("hi".into()));
        thread.items.push(ChatItem::Assistant("he".into()));
        db.save_threads(std::slice::from_ref(&thread)).unwrap();

        // Streaming grows the last item; one row changes, one is appended.
        if let Some(ChatItem::Assistant(text)) = thread.items.last_mut() {
            text.push_str("llo");
        }
        thread.items.push(ChatItem::Error("oops".into()));
        db.save_threads(std::slice::from_ref(&thread)).unwrap();

        let reopened = Db::open(&path).unwrap();
        let loaded = reopened.load_threads();
        assert_eq!(loaded.len(), 1);
        assert!(matches!(&loaded[0].items[1], ChatItem::Assistant(t) if t == "hello"));
        assert_eq!(loaded[0].items.len(), 3);

        // Deleting in this process removes the rows.
        reopened.save_threads(&[]).unwrap();
        assert!(Db::open(&path).unwrap().load_threads().is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn another_process_threads_survive_a_save() {
        let (a, path) = temp_db("two");
        let b = Db::open(&path).unwrap();
        let mine = Thread::new(None, "claude".into(), "full".into());
        let mut theirs = Thread::new(None, "codex".into(), "full".into());
        theirs.id += 1;
        a.save_threads(std::slice::from_ref(&mine)).unwrap();
        b.save_threads(std::slice::from_ref(&theirs)).unwrap();
        // `a` never saw `theirs`, so saving its own list must not drop it.
        a.save_threads(std::slice::from_ref(&mine)).unwrap();
        assert_eq!(Db::open(&path).unwrap().load_threads().len(), 2);
        let _ = std::fs::remove_file(path);
    }
}
