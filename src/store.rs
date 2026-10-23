//! SQLite persistence. The schema is versioned with `PRAGMA user_version`.
//!
//! One connection sits behind a mutex that is held for the whole of every
//! statement or transaction, so writers can never interleave.
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::config::ensure_private_dir;
use crate::models::{Message, Notebook, Source};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS notebooks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    description TEXT DEFAULT '',
    model TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE IF NOT EXISTS sources (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    notebook_id INTEGER NOT NULL,
    path TEXT NOT NULL,
    filename TEXT NOT NULL,
    mime TEXT,
    ext TEXT,
    size INTEGER DEFAULT 0,
    content_text TEXT DEFAULT '',
    token_estimate INTEGER DEFAULT 0,
    enabled INTEGER DEFAULT 1,
    status TEXT DEFAULT 'ok',
    error TEXT,
    added_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(notebook_id) REFERENCES notebooks(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id INTEGER NOT NULL,
    idx INTEGER NOT NULL,
    text TEXT NOT NULL,
    FOREIGN KEY(source_id) REFERENCES sources(id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    notebook_id INTEGER NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    model TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY(notebook_id) REFERENCES notebooks(id) ON DELETE CASCADE
);
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    text,
    content='chunks',
    content_rowid='id'
);
CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
  INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
  INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE ON chunks BEGIN
  INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES ('delete', old.id, old.text);
  INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
END;
";

const MIGRATION_2: &str = "
CREATE INDEX IF NOT EXISTS idx_sources_notebook ON sources(notebook_id);
CREATE INDEX IF NOT EXISTS idx_chunks_source ON chunks(source_id);
CREATE INDEX IF NOT EXISTS idx_messages_notebook ON messages(notebook_id);
";

const SOURCE_COLUMNS: &str = "id, notebook_id, path, filename, mime, ext, size, content_text, \
    token_estimate, enabled, status, error, added_at";
const SOURCE_META_COLUMNS: &str = "id, notebook_id, path, filename, mime, ext, size, '' AS content_text, \
    token_estimate, enabled, status, error, added_at";
const NOTEBOOK_COLUMNS: &str = "id, name, description, model, created_at, updated_at";
const MESSAGE_COLUMNS: &str = "id, notebook_id, role, content, model, created_at";

/// Everything `replace_source_content` rewrites on a source.
pub struct SourceUpdate {
    pub content_text: String,
    pub token_estimate: usize,
    pub size: u64,
    pub mime: Option<String>,
    pub ext: Option<String>,
    pub filename: String,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
}

fn notebook(r: &Row) -> rusqlite::Result<Notebook> {
    Ok(Notebook {
        id: r.get(0)?,
        name: r.get(1)?,
        description: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        model: r.get(3)?,
        created_at: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
        updated_at: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
    })
}

fn source(r: &Row) -> rusqlite::Result<Source> {
    Ok(Source {
        id: r.get(0)?,
        notebook_id: r.get(1)?,
        path: r.get(2)?,
        filename: r.get(3)?,
        mime: r.get(4)?,
        ext: r.get(5)?,
        size: r.get::<_, Option<i64>>(6)?.unwrap_or(0).max(0) as u64,
        content_text: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
        token_estimate: r.get::<_, Option<i64>>(8)?.unwrap_or(0).max(0) as usize,
        enabled: r.get::<_, Option<i64>>(9)?.map(|v| v != 0).unwrap_or(true),
        status: r
            .get::<_, Option<String>>(10)?
            .unwrap_or_else(|| "ok".into()),
        error: r.get(11)?,
        added_at: r.get::<_, Option<String>>(12)?.unwrap_or_default(),
    })
}

fn message(r: &Row) -> rusqlite::Result<Message> {
    Ok(Message {
        id: r.get(0)?,
        notebook_id: r.get(1)?,
        role: r.get(2)?,
        content: r.get(3)?,
        model: r.get(4)?,
        created_at: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
    })
}

fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; \
         PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;",
    )?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version < 1 {
        conn.execute_batch(SCHEMA)?;
    }
    if version < 2 {
        conn.execute_batch(MIGRATION_2)?;
    }
    if version < 2 {
        conn.execute_batch("PRAGMA user_version = 2;")?;
    }
    Ok(())
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            ensure_private_dir(dir)?;
        }
        let conn = Connection::open(path)?;
        init(&conn)?;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        init(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    // Notebooks

    pub fn create_notebook(&self, name: &str, description: &str) -> Result<i64> {
        let c = self.lock();
        c.execute(
            "INSERT INTO notebooks (name, description) VALUES (?1, ?2)",
            params![name, description],
        )?;
        Ok(c.last_insert_rowid())
    }

    pub fn list_notebooks_with_counts(&self) -> Result<Vec<(Notebook, usize)>> {
        let c = self.lock();
        let mut stmt = c.prepare(
            "SELECT n.id, n.name, n.description, n.model, n.created_at, n.updated_at, COUNT(s.id)
             FROM notebooks AS n LEFT JOIN sources AS s ON s.notebook_id = n.id
             GROUP BY n.id ORDER BY n.updated_at DESC, n.id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((notebook(r)?, r.get::<_, i64>(6)?.max(0) as usize))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn get_notebook(&self, id: i64) -> Result<Option<Notebook>> {
        let c = self.lock();
        Ok(c.query_row(
            &format!("SELECT {NOTEBOOK_COLUMNS} FROM notebooks WHERE id = ?1"),
            [id],
            notebook,
        )
        .optional()?)
    }

    /// Update only the fields passed. One UPDATE, so concurrent partial updates
    /// (a rename and a model change) cannot clobber each other.
    pub fn update_notebook(
        &self,
        id: i64,
        name: Option<&str>,
        description: Option<&str>,
        model: Option<&str>,
    ) -> Result<()> {
        self.lock().execute(
            "UPDATE notebooks SET name = COALESCE(?1, name), description = COALESCE(?2, description),
             model = COALESCE(?3, model), updated_at = CURRENT_TIMESTAMP WHERE id = ?4",
            params![name, description, model, id],
        )?;
        Ok(())
    }

    pub fn delete_notebook(&self, id: i64) -> Result<bool> {
        Ok(self
            .lock()
            .execute("DELETE FROM notebooks WHERE id = ?1", [id])?
            > 0)
    }

    // Sources

    /// Insert a source and its chunks atomically.
    pub fn add_source_with_chunks(&self, s: &Source, chunks: &[String]) -> Result<i64> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        tx.execute(
            "INSERT INTO sources (notebook_id, path, filename, mime, ext, size, content_text, token_estimate, enabled, status, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![s.notebook_id, s.path, s.filename, s.mime, s.ext, s.size as i64, s.content_text,
                    s.token_estimate as i64, s.enabled as i64, s.status, s.error],
        )?;
        let id = tx.last_insert_rowid();
        insert_chunks(&tx, id, chunks)?;
        tx.commit()?;
        Ok(id)
    }

    pub fn list_sources(&self, notebook_id: i64) -> Result<Vec<Source>> {
        self.query_sources(
            &format!("SELECT {SOURCE_COLUMNS} FROM sources WHERE notebook_id = ?1 ORDER BY id ASC"),
            notebook_id,
        )
    }

    /// Metadata only: `content_text` is left empty so large sources stay out of memory.
    pub fn list_source_meta(&self, notebook_id: i64) -> Result<Vec<Source>> {
        self.query_sources(
            &format!(
                "SELECT {SOURCE_META_COLUMNS} FROM sources WHERE notebook_id = ?1 ORDER BY id ASC"
            ),
            notebook_id,
        )
    }

    /// Enabled, healthy sources with full text, for prompt building.
    pub fn list_sources_for_context(&self, notebook_id: i64) -> Result<Vec<Source>> {
        self.query_sources(
            &format!("SELECT {SOURCE_COLUMNS} FROM sources WHERE notebook_id = ?1 AND enabled = 1 AND status = 'ok' ORDER BY id ASC"),
            notebook_id,
        )
    }

    fn query_sources(&self, sql: &str, notebook_id: i64) -> Result<Vec<Source>> {
        let c = self.lock();
        let mut stmt = c.prepare(sql)?;
        let rows = stmt.query_map([notebook_id], source)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn get_source(&self, id: i64) -> Result<Option<Source>> {
        let c = self.lock();
        Ok(c.query_row(
            &format!("SELECT {SOURCE_COLUMNS} FROM sources WHERE id = ?1"),
            [id],
            source,
        )
        .optional()?)
    }

    pub fn toggle_source(&self, id: i64, enabled: bool) -> Result<()> {
        self.lock().execute(
            "UPDATE sources SET enabled = ?1 WHERE id = ?2",
            params![enabled as i64, id],
        )?;
        Ok(())
    }

    pub fn delete_source(&self, id: i64) -> Result<bool> {
        Ok(self
            .lock()
            .execute("DELETE FROM sources WHERE id = ?1", [id])?
            > 0)
    }

    /// Mark a source broken without discarding its existing content.
    pub fn mark_source_error(&self, id: i64, error: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE sources SET status = 'error', error = ?1, enabled = 0 WHERE id = ?2",
            params![error, id],
        )?;
        Ok(())
    }

    /// Atomically replace a source's text, metadata and chunks.
    pub fn replace_source_content(
        &self,
        id: i64,
        u: &SourceUpdate,
        chunks: &[String],
    ) -> Result<()> {
        let status = if u.error.is_none() { "ok" } else { "error" };
        let mut c = self.lock();
        let tx = c.transaction()?;
        tx.execute(
            "UPDATE sources SET content_text = ?1, token_estimate = ?2, size = ?3, mime = ?4, ext = ?5,
             filename = ?6, status = ?7, error = ?8, enabled = CASE WHEN ?7 = 'ok' THEN 1 ELSE 0 END WHERE id = ?9",
            params![u.content_text, u.token_estimate as i64, u.size as i64, u.mime, u.ext, u.filename, status, u.error, id],
        )?;
        tx.execute("DELETE FROM chunks WHERE source_id = ?1", [id])?;
        insert_chunks(&tx, id, chunks)?;
        tx.commit()?;
        Ok(())
    }

    // Messages

    pub fn add_message(&self, m: &Message) -> Result<Message> {
        let c = self.lock();
        c.execute(
            "INSERT INTO messages (notebook_id, role, content, model) VALUES (?1, ?2, ?3, ?4)",
            params![m.notebook_id, m.role, m.content, m.model],
        )?;
        let mut saved = m.clone();
        saved.id = c.last_insert_rowid();
        Ok(saved)
    }

    /// The newest `limit` messages (all when `None`), oldest first.
    pub fn list_messages(&self, notebook_id: i64, limit: Option<usize>) -> Result<Vec<Message>> {
        let c = self.lock();
        let mut stmt = c.prepare(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages WHERE notebook_id = ?1 ORDER BY id DESC LIMIT ?2"
        ))?;
        let limit = limit.map(|l| l.max(1) as i64).unwrap_or(-1);
        let mut msgs: Vec<Message> = stmt
            .query_map(params![notebook_id, limit], message)?
            .collect::<rusqlite::Result<_>>()?;
        msgs.reverse();
        Ok(msgs)
    }

    pub fn clear_messages(&self, notebook_id: i64) -> Result<bool> {
        Ok(self
            .lock()
            .execute("DELETE FROM messages WHERE notebook_id = ?1", [notebook_id])?
            > 0)
    }

    /// BM25-ranked chunk search within a notebook: `(source_id, filename, text)`.
    pub fn search_chunks(
        &self,
        notebook_id: i64,
        match_query: &str,
        limit: usize,
    ) -> Result<Vec<(i64, String, String)>> {
        if match_query.is_empty() {
            return Ok(Vec::new());
        }
        let c = self.lock();
        let mut stmt = c.prepare(
            "SELECT c.source_id, s.filename, c.text FROM chunks_fts
             JOIN chunks c ON c.id = chunks_fts.rowid JOIN sources s ON s.id = c.source_id
             WHERE chunks_fts MATCH ?1 AND s.notebook_id = ?2 AND s.enabled = 1 AND s.status = 'ok'
             ORDER BY bm25(chunks_fts) LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![match_query, notebook_id, limit.max(1) as i64],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

fn insert_chunks(
    tx: &rusqlite::Transaction,
    source_id: i64,
    chunks: &[String],
) -> rusqlite::Result<()> {
    let mut stmt =
        tx.prepare_cached("INSERT INTO chunks (source_id, idx, text) VALUES (?1, ?2, ?3)")?;
    for (idx, text) in chunks.iter().enumerate() {
        stmt.execute(params![source_id, idx as i64, text])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(nb: i64, name: &str, text: &str) -> Source {
        Source {
            notebook_id: nb,
            path: format!("/x/{name}"),
            filename: name.into(),
            content_text: text.into(),
            ..Default::default()
        }
    }

    #[test]
    fn notebook_crud_and_partial_update() {
        let s = Store::open_in_memory().unwrap();
        let id = s.create_notebook("A", "desc").unwrap();
        s.update_notebook(id, None, None, Some("m/x")).unwrap();
        s.update_notebook(id, Some("B"), None, None).unwrap();
        let nb = s.get_notebook(id).unwrap().unwrap();
        assert_eq!(
            (
                nb.name.as_str(),
                nb.description.as_str(),
                nb.model.as_deref()
            ),
            ("B", "desc", Some("m/x"))
        );
        assert_eq!(s.list_notebooks_with_counts().unwrap().len(), 1);
        assert!(s.delete_notebook(id).unwrap());
        assert!(s.get_notebook(id).unwrap().is_none());
    }

    #[test]
    fn sources_chunks_and_cascade() {
        let s = Store::open_in_memory().unwrap();
        let nb = s.create_notebook("A", "").unwrap();
        let sid = s
            .add_source_with_chunks(
                &src(nb, "a.txt", "alpha beta"),
                &["alpha beta".into(), "gamma delta".into()],
            )
            .unwrap();
        assert_eq!(s.list_source_meta(nb).unwrap()[0].content_text, "");
        assert_eq!(s.list_sources(nb).unwrap()[0].content_text, "alpha beta");
        assert_eq!(s.search_chunks(nb, "\"gamma\"*", 5).unwrap()[0].1, "a.txt");

        s.toggle_source(sid, false).unwrap();
        assert!(s.search_chunks(nb, "\"gamma\"*", 5).unwrap().is_empty());
        assert!(s.list_sources_for_context(nb).unwrap().is_empty());
        s.toggle_source(sid, true).unwrap();

        // Deleting the notebook cascades to sources and the FTS index.
        s.delete_notebook(nb).unwrap();
        assert!(s.get_source(sid).unwrap().is_none());
        let nb2 = s.create_notebook("B", "").unwrap();
        assert!(s.search_chunks(nb2, "\"gamma\"*", 5).unwrap().is_empty());
    }

    #[test]
    fn replace_content_swaps_chunks_atomically() {
        let s = Store::open_in_memory().unwrap();
        let nb = s.create_notebook("A", "").unwrap();
        let sid = s
            .add_source_with_chunks(&src(nb, "a.txt", "old"), &["oldword".into()])
            .unwrap();
        let u = SourceUpdate {
            content_text: "new".into(),
            token_estimate: 1,
            size: 3,
            mime: None,
            ext: None,
            filename: "a.txt".into(),
            error: None,
        };
        s.replace_source_content(sid, &u, &["newword".into()])
            .unwrap();
        assert!(s.search_chunks(nb, "\"oldword\"*", 5).unwrap().is_empty());
        assert_eq!(s.search_chunks(nb, "\"newword\"*", 5).unwrap().len(), 1);

        let bad = SourceUpdate {
            error: Some("boom".into()),
            content_text: String::new(),
            ..u
        };
        s.replace_source_content(sid, &bad, &[]).unwrap();
        let got = s.get_source(sid).unwrap().unwrap();
        assert_eq!((got.status.as_str(), got.enabled), ("error", false));
    }

    #[test]
    fn messages_limit_returns_newest_oldest_first() {
        let s = Store::open_in_memory().unwrap();
        let nb = s.create_notebook("A", "").unwrap();
        for i in 0..5 {
            s.add_message(&Message::new(nb, "user", format!("m{i}")))
                .unwrap();
        }
        let got: Vec<_> = s
            .list_messages(nb, Some(2))
            .unwrap()
            .into_iter()
            .map(|m| m.content)
            .collect();
        assert_eq!(got, ["m3", "m4"]);
        assert!(s.clear_messages(nb).unwrap());
        assert!(s.list_messages(nb, None).unwrap().is_empty());
    }

    #[test]
    fn mark_source_error_keeps_content() {
        let s = Store::open_in_memory().unwrap();
        let nb = s.create_notebook("A", "").unwrap();
        let sid = s
            .add_source_with_chunks(&src(nb, "a.txt", "keep"), &[])
            .unwrap();
        s.mark_source_error(sid, "gone").unwrap();
        let got = s.get_source(sid).unwrap().unwrap();
        assert_eq!(
            (got.content_text.as_str(), got.status.as_str(), got.enabled),
            ("keep", "error", false)
        );
    }
}
