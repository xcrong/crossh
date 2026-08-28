//! 纯逻辑笔记存储：SQLite + FTS5 + 标签，零 gpui 依赖

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
pub const MAX_CONTENT_BYTES: usize = 8 * 1024;
pub const MAX_TAGS_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Note {
    pub id: i64,
    pub content: String,
    pub tags: String,
    pub pinned: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    // 在字节边界截断并保证 UTF-8 合法
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

pub fn note_db_path() -> PathBuf {
    if let Some(path) = test_path_override() {
        return path;
    }
    let base = dirs::config_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("crossh").join("note.db")
}

// 仅测试使用：线程隔离的路径覆盖，隔离并行单测
thread_local! {
    static TEST_PATH: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn test_path_override() -> Option<PathBuf> {
    TEST_PATH.with(|c| c.borrow().clone())
}

#[cfg(test)]
pub fn set_test_path(path: PathBuf) {
    TEST_PATH.with(|c| *c.borrow_mut() = Some(path));
}

#[cfg(test)]
pub fn clear_test_path() {
    TEST_PATH.with(|c| *c.borrow_mut() = None);
}

pub struct NoteStore {
    pub path: PathBuf,
    conn: Mutex<Connection>,
}

impl NoteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        // 尝试打开，若损坏则备份
        let conn = match Connection::open(&path) {
            Ok(c) => c,
            Err(e) => {
                // 损坏：备份
                let ts = now_ts();
                let corrupt = path.with_extension(format!("corrupt.{}", ts));
                let _ = std::fs::rename(&path, &corrupt);
                log::warn!("note.db corrupt, backup to {:?}: {}", corrupt, e);
                Connection::open(&path).map_err(|e| e.to_string())?
            }
        };
        let store = Self {
            path: path.clone(),
            conn: Mutex::new(conn),
        };
        store.ensure_schema()?;
        Ok(store)
    }

    pub fn open_default() -> Result<Self, String> {
        Self::open(note_db_path())
    }

    fn ensure_schema(&self) -> Result<(), String> {
        let conn = self.conn.lock();
        // 检测是否损坏（尝试查询）
        let schema_check = conn
            .prepare("SELECT count(*) FROM sqlite_master")
            .map(|_| ())
            .map_err(|e| e.to_string());
        if let Err(e) = schema_check {
            drop(conn);
            // 备份
            let ts = now_ts();
            let corrupt = self.path.with_extension(format!("corrupt.{}", ts));
            let _ = std::fs::rename(&self.path, &corrupt);
            log::warn!("note.db schema check failed, backup {:?}: {}", corrupt, e);
            let new_conn = Connection::open(&self.path).map_err(|e| e.to_string())?;
            *self.conn.lock() = new_conn;
            return self.ensure_schema();
        }
        // PRAGMA
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA user_version=1;") // 契约 1
            .map_err(|e| e.to_string())?;

        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS notes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                content TEXT NOT NULL,
                tags TEXT NOT NULL DEFAULT '',
                pinned INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(content, tags, content='notes', content_rowid='id', tokenize='unicode61');
            CREATE TRIGGER IF NOT EXISTS notes_ai AFTER INSERT ON notes BEGIN
                INSERT INTO notes_fts(rowid, content, tags) VALUES (new.id, new.content, new.tags);
            END;
            CREATE TRIGGER IF NOT EXISTS notes_ad AFTER DELETE ON notes BEGIN
                INSERT INTO notes_fts(notes_fts, rowid, content, tags) VALUES ('delete', old.id, old.content, old.tags);
            END;
            CREATE TRIGGER IF NOT EXISTS notes_au AFTER UPDATE ON notes BEGIN
                INSERT INTO notes_fts(notes_fts, rowid, content, tags) VALUES ('delete', old.id, old.content, old.tags);
                INSERT INTO notes_fts(rowid, content, tags) VALUES (new.id, new.content, new.tags);
            END;
            "#,
        )
        .map_err(|e| e.to_string())?;

        // 兼容旧库：若旧表无 tags 列，补列
        let has_tags: bool = conn.prepare("SELECT tags FROM notes LIMIT 0").is_ok();
        if !has_tags {
            let _ = conn.execute(
                "ALTER TABLE notes ADD COLUMN tags TEXT NOT NULL DEFAULT ''",
                [],
            );
            let _ = conn.execute(
                "ALTER TABLE notes ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0",
                [],
            );
        }
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<Note>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT id, content, tags, pinned, created_at, updated_at FROM notes ORDER BY pinned DESC, updated_at DESC, id DESC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Note {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    tags: row.get(2).unwrap_or_default(),
                    pinned: row.get::<_, i64>(3)? != 0,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    pub fn search(&self, query: &str) -> Result<Vec<Note>, String> {
        let q = query.trim();
        if q.is_empty() {
            return self.list();
        }
        // 优先 FTS5
        if let Ok(res) = self.search_fts(q) {
            return Ok(res);
        }
        // 回退 LIKE
        self.search_like(q)
    }

    fn search_fts(&self, query: &str) -> Result<Vec<Note>, String> {
        // 将查询转为 FTS5 合法：用双引号包裹每个词，避免特殊字符
        // 简化：对单引号转义，空格分词后用 OR 连接
        let escaped = query.replace('"', "\"\"");
        // 使用 FTS5 的简单匹配：直接用用户输入，失败则抛错回退
        let conn = self.conn.lock();
        let sql = "SELECT n.id, n.content, n.tags, n.pinned, n.created_at, n.updated_at FROM notes n JOIN notes_fts f ON n.id = f.rowid WHERE notes_fts MATCH ? ORDER BY n.pinned DESC, n.updated_at DESC, n.id DESC";
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
        // 构造 MATCH 表达式：对空格分词，用 OR 连接以实现宽松匹配
        let match_expr = escaped
            .split_whitespace()
            .map(|w| format!("\"{}\"", w))
            .collect::<Vec<_>>()
            .join(" OR ");
        let rows = stmt
            .query_map(params![match_expr], |row| {
                Ok(Note {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    tags: row.get(2).unwrap_or_default(),
                    pinned: row.get::<_, i64>(3)? != 0,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    fn search_like(&self, query: &str) -> Result<Vec<Note>, String> {
        let conn = self.conn.lock();
        // 转义 LIKE 通配符
        let esc = query.replace('%', "\\%").replace('_', "\\_");
        let pat = format!("%{}%", esc);
        let mut stmt = conn
            .prepare("SELECT id, content, tags, pinned, created_at, updated_at FROM notes WHERE content LIKE ? ESCAPE '\\' OR tags LIKE ? ESCAPE '\\' ORDER BY pinned DESC, updated_at DESC, id DESC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![pat, pat], |row| {
                Ok(Note {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    tags: row.get(2).unwrap_or_default(),
                    pinned: row.get::<_, i64>(3)? != 0,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    pub fn create(&self, content: &str, tags: &str) -> Result<Note, String> {
        let content = truncate_bytes(content.trim(), MAX_CONTENT_BYTES);
        if content.is_empty() {
            return Err("empty_content".to_string());
        }
        let tags = truncate_bytes(tags.trim(), MAX_TAGS_BYTES);
        // 规范化标签：逗号分隔，去重，去空，trim
        let tags = normalize_tags(&tags);
        let now = now_ts();
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO notes (content, tags, pinned, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?4)",
            params![content, tags, now, now],
        )
        .map_err(|e| e.to_string())?;
        let id = conn.last_insert_rowid();
        Ok(Note {
            id,
            content,
            tags,
            pinned: false,
            created_at: now,
            updated_at: now,
        })
    }

    pub fn update(&self, id: i64, content: &str, tags: &str) -> Result<Note, String> {
        let content = truncate_bytes(content.trim(), MAX_CONTENT_BYTES);
        if content.is_empty() {
            return Err("empty_content".to_string());
        }
        let tags = truncate_bytes(tags.trim(), MAX_TAGS_BYTES);
        let tags = normalize_tags(&tags);
        let now = now_ts();
        let conn = self.conn.lock();
        let changed = conn
            .execute(
                "UPDATE notes SET content=?1, tags=?2, updated_at=?3 WHERE id=?4",
                params![content, tags, now, id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err("not_found".to_string());
        }
        let mut stmt = conn
            .prepare(
                "SELECT id, content, tags, pinned, created_at, updated_at FROM notes WHERE id=?1",
            )
            .map_err(|e| e.to_string())?;
        let note = stmt
            .query_row(params![id], |row| {
                Ok(Note {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    tags: row.get(2).unwrap_or_default(),
                    pinned: row.get::<_, i64>(3)? != 0,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?;
        Ok(note)
    }

    pub fn set_pinned(&self, id: i64, pinned: bool) -> Result<(), String> {
        let conn = self.conn.lock();
        let now = now_ts();
        let changed = conn
            .execute(
                "UPDATE notes SET pinned=?1, updated_at=?2 WHERE id=?3",
                params![if pinned { 1 } else { 0 }, now, id],
            )
            .map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err("not_found".to_string());
        }
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM notes WHERE id=?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn clear_all(&self) -> Result<(), String> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM notes", [])
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn get(&self, id: i64) -> Result<Option<Note>, String> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, content, tags, pinned, created_at, updated_at FROM notes WHERE id=?1",
            )
            .map_err(|e| e.to_string())?;
        let res = stmt
            .query_row(params![id], |row| {
                Ok(Note {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    tags: row.get(2).unwrap_or_default(),
                    pinned: row.get::<_, i64>(3)? != 0,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(res)
    }

    pub fn all_tags(&self) -> Result<Vec<String>, String> {
        let notes = self.list()?;
        let mut set = std::collections::BTreeSet::new();
        for n in notes {
            for t in n.tags.split(',') {
                let t = t.trim();
                if !t.is_empty() {
                    set.insert(t.to_string());
                }
            }
        }
        Ok(set.into_iter().collect())
    }
}

fn normalize_tags(raw: &str) -> String {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for part in raw.split(',') {
        let t = part.trim();
        if t.is_empty() {
            continue;
        }
        if seen.insert(t.to_string()) {
            out.push(t.to_string());
        }
    }
    out.join(",")
}

#[cfg(test)]
mod tests {
    #![allow(non_snake_case)]
    use super::*;
    use tempfile::TempDir;

    fn tmp_store() -> (NoteStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("note.db");
        let store = NoteStore::open(&path).unwrap();
        (store, dir)
    }

    #[test]
    fn spec_20260827_note__create_and_list() {
        let (store, _dir) = tmp_store();
        let note = store.create("hello", "work,idea").unwrap();
        assert_eq!(note.content, "hello");
        assert_eq!(note.tags, "work,idea");
        let list = store.list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, note.id);
    }

    #[test]
    fn spec_20260827_note__empty_content_rejected() {
        let (store, _dir) = tmp_store();
        assert!(store.create("   ", "").is_err());
        assert_eq!(store.list().unwrap().len(), 0);
    }

    #[test]
    fn spec_20260827_note__max_bytes_truncated() {
        let (store, _dir) = tmp_store();
        let big = "a".repeat(MAX_CONTENT_BYTES + 100);
        let note = store.create(&big, "").unwrap();
        assert!(note.content.len() <= MAX_CONTENT_BYTES);
    }

    #[test]
    fn spec_20260827_note__update_changes_updated_at() {
        let (store, _dir) = tmp_store();
        let note = store.create("a", "").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        let updated = store.update(note.id, "b", "tag1").unwrap();
        assert_eq!(updated.content, "b");
        assert_eq!(updated.tags, "tag1");
        assert!(updated.updated_at >= note.updated_at);
    }

    #[test]
    fn spec_20260827_note__pin_orders_first() {
        let (store, _dir) = tmp_store();
        let n1 = store.create("a", "").unwrap();
        let n2 = store.create("b", "").unwrap();
        store.set_pinned(n1.id, true).unwrap();
        let list = store.list().unwrap();
        assert_eq!(list[0].id, n1.id);
        assert_eq!(list[1].id, n2.id);
    }

    #[test]
    fn spec_20260827_note__search_content_and_tags() {
        let (store, _dir) = tmp_store();
        store.create("hello world", "work").unwrap();
        store.create("other", "idea").unwrap();
        let res = store.search("work").unwrap();
        assert_eq!(res.len(), 1);
        assert!(res[0].tags.contains("work"));
        let res2 = store.search("hello").unwrap();
        assert_eq!(res2.len(), 1);
    }

    #[test]
    fn spec_20260827_note__search_like_escape() {
        let (store, _dir) = tmp_store();
        store.create("a%b", "").unwrap();
        // 搜索含 % 的内容，不应注入
        let res = store.search("a%b").unwrap();
        assert_eq!(res.len(), 1);
    }

    #[test]
    fn spec_20260827_note__delete() {
        let (store, _dir) = tmp_store();
        let n = store.create("x", "").unwrap();
        store.delete(n.id).unwrap();
        assert_eq!(store.list().unwrap().len(), 0);
        // 删除不存在不报错
        store.delete(9999).unwrap();
    }

    #[test]
    fn spec_20260827_note__clear_all() {
        let (store, _dir) = tmp_store();
        store.create("a", "").unwrap();
        store.create("b", "").unwrap();
        store.clear_all().unwrap();
        assert_eq!(store.list().unwrap().len(), 0);
    }

    #[test]
    fn spec_20260827_note__tags_normalized() {
        let (store, _dir) = tmp_store();
        let n = store.create("c", " work, work , idea ,, ").unwrap();
        assert_eq!(n.tags, "work,idea");
    }

    #[test]
    fn spec_20260827_note__all_tags() {
        let (store, _dir) = tmp_store();
        store.create("a", "work,idea").unwrap();
        store.create("b", "work").unwrap();
        let tags = store.all_tags().unwrap();
        assert_eq!(tags, vec!["idea", "work"]);
    }

    #[test]
    fn spec_20260827_note__wal_and_user_version() {
        let (store, _dir) = tmp_store();
        let conn = store.conn.lock();
        let wal: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(wal.to_lowercase(), "wal");
        let ver: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ver, 1);
    }
}
