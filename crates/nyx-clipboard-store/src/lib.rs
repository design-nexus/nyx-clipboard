//! The clipboard history in SQLite (`~/.local/share/nyx/clipboard.db`, WAL, 0600).
//!
//! An entry's plain text lives in `entries.text` (indexed by FTS5 with the trigram
//! tokenizer, so any three characters find a substring); its other representations in
//! `reps`, small ones inline and large ones (and every image) as files in the blobs
//! folder named after the entry's hash. The same content copied again is the same entry,
//! moved to the top. Pinned entries are never removed by retention.

use nyx_clipboard_core::model::{Captured, DETAIL_TEXT, Detail, Entry, Kind, Rep};
use nyx_clipboard_core::prefs::Settings;
use nyx_clipboard_core::query::{self, Search, View};
use nyx_clipboard_core::snippet::Snippet;
use nyx_clipboard_core::{classify, fuzzy, paths};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use rusqlite::Error;
pub type Result<T> = std::result::Result<T, Error>;

/// Representations larger than this are kept as files.
const INLINE_MAX: usize = 64 * 1024;
/// How many recent entries a fuzzy search looks through, and how much of each.
const FUZZY_WINDOW: u32 = 2000;
const FUZZY_CHARS: u32 = 600;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS entries(
  id INTEGER PRIMARY KEY,
  hash TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL,
  preview TEXT NOT NULL,
  text TEXT NOT NULL DEFAULT '',
  size INTEGER NOT NULL,
  chars INTEGER NOT NULL,
  mimes TEXT NOT NULL,
  app TEXT,
  created REAL NOT NULL,
  last_used REAL NOT NULL,
  uses INTEGER NOT NULL DEFAULT 1,
  pinned INTEGER NOT NULL DEFAULT 0,
  pinned_at REAL,
  favorite INTEGER NOT NULL DEFAULT 0,
  width INTEGER,
  height INTEGER
);
CREATE INDEX IF NOT EXISTS entries_recent ON entries(last_used DESC);
CREATE INDEX IF NOT EXISTS entries_pinned ON entries(pinned, pinned_at DESC);
CREATE TABLE IF NOT EXISTS reps(
  entry INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
  mime TEXT NOT NULL,
  data BLOB,
  file TEXT,
  size INTEGER NOT NULL,
  PRIMARY KEY(entry, mime)
);
CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(text, preview, content='entries', content_rowid='id', tokenize='trigram');
CREATE TRIGGER IF NOT EXISTS entries_ai AFTER INSERT ON entries BEGIN
  INSERT INTO entries_fts(rowid, text, preview) VALUES (new.id, new.text, new.preview);
END;
CREATE TRIGGER IF NOT EXISTS entries_ad AFTER DELETE ON entries BEGIN
  INSERT INTO entries_fts(entries_fts, rowid, text, preview) VALUES ('delete', old.id, old.text, old.preview);
END;
CREATE TRIGGER IF NOT EXISTS entries_au AFTER UPDATE OF text, preview ON entries BEGIN
  INSERT INTO entries_fts(entries_fts, rowid, text, preview) VALUES ('delete', old.id, old.text, old.preview);
  INSERT INTO entries_fts(rowid, text, preview) VALUES (new.id, new.text, new.preview);
END;
CREATE TABLE IF NOT EXISTS snippets(
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  body TEXT NOT NULL,
  created REAL NOT NULL,
  updated REAL NOT NULL
);
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
"#;

const COLUMNS: &str = "id, kind, preview, size, chars, mimes, app, created, last_used, uses, pinned, favorite, width, height, hash";

pub struct Store {
    conn: Connection,
    blobs: PathBuf,
}

/// What an insert did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inserted {
    pub id: i64,
    /// A new entry (false: the same content was already there and moved to the top).
    pub new: bool,
}

/// Counts for the status line and Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    pub entries: u64,
    pub pinned: u64,
    pub snippets: u64,
    pub bytes: u64,
}

fn ext_for(mime: &str) -> &str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/tiff" => "tiff",
        _ => "bin",
    }
}

fn slug(mime: &str) -> String {
    mime.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

impl Store {
    /// Open (or create) the history at `db`, with large copies in `blobs`.
    pub fn open(db: &Path, blobs: &Path) -> Result<Store> {
        if let Some(dir) = db.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = paths::private_dir(blobs);
        let conn = Connection::open(db)?;
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(db, std::fs::Permissions::from_mode(0o600));
        }
        Self::setup(conn, blobs)
    }

    /// A store that lives only in memory (tests), with blobs in `blobs`.
    pub fn in_memory(blobs: &Path) -> Result<Store> {
        let _ = std::fs::create_dir_all(blobs);
        Self::setup(Connection::open_in_memory()?, blobs)
    }

    fn setup(conn: Connection, blobs: &Path) -> Result<Store> {
        conn.busy_timeout(std::time::Duration::from_secs(3))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "secure_delete", "ON")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn, blobs: blobs.to_path_buf() })
    }

    pub fn default_paths() -> (PathBuf, PathBuf) {
        (paths::database(), paths::blobs_dir())
    }

    /// Keep a copy. The same content again only moves its entry to the top.
    pub fn insert(&mut self, c: &Captured, app: Option<&str>, now: f64) -> Result<Inserted> {
        let existing: Option<i64> =
            self.conn.query_row("SELECT id FROM entries WHERE hash = ?1", [&c.hash], |r| r.get(0)).optional()?;
        if let Some(id) = existing {
            self.conn.execute(
                "UPDATE entries SET last_used = ?2, uses = uses + 1, app = COALESCE(?3, app) WHERE id = ?1",
                params![id, now, app],
            )?;
            return Ok(Inserted { id, new: false });
        }
        let mimes: Vec<&str> = c.reps.iter().map(|r| r.mime.as_str()).collect();
        let (w, h) = c.dims.map(|(w, h)| (Some(w), Some(h))).unwrap_or((None, None));
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO entries(hash, kind, preview, text, size, chars, mimes, app, created, last_used, width, height)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?10, ?11)",
            params![
                c.hash,
                c.kind.as_str(),
                c.preview,
                c.text,
                c.size() as i64,
                c.text.chars().count() as i64,
                serde_json::to_string(&mimes).unwrap_or_else(|_| "[]".into()),
                app,
                now,
                w,
                h
            ],
        )?;
        let id = tx.last_insert_rowid();
        let mut written = Vec::new();
        for rep in &c.reps {
            // The text is in `entries.text` already.
            if rep.mime == classify::TEXT {
                continue;
            }
            let as_file = rep.data.len() > INLINE_MAX || rep.mime.starts_with("image/");
            if as_file {
                let name = if rep.mime.starts_with("image/") {
                    format!("{}.{}", c.hash, ext_for(&rep.mime))
                } else {
                    format!("{}-{}", c.hash, slug(&rep.mime))
                };
                let path = self.blobs.join(&name);
                if let Err(e) = paths::atomic_write(&path, &rep.data) {
                    for p in &written {
                        let _ = std::fs::remove_file(p);
                    }
                    return Err(Error::ToSqlConversionFailure(Box::new(e)));
                }
                written.push(path);
                tx.execute(
                    "INSERT INTO reps(entry, mime, file, size) VALUES (?1, ?2, ?3, ?4)",
                    params![id, rep.mime, name, rep.data.len() as i64],
                )?;
            } else {
                tx.execute(
                    "INSERT INTO reps(entry, mime, data, size) VALUES (?1, ?2, ?3, ?4)",
                    params![id, rep.mime, rep.data, rep.data.len() as i64],
                )?;
            }
        }
        tx.commit()?;
        Ok(Inserted { id, new: true })
    }

    /// Moves an entry to the top (it was pasted or copied again).
    pub fn touch(&self, id: i64, now: f64) -> Result<bool> {
        Ok(self.conn.execute("UPDATE entries SET last_used = ?2, uses = uses + 1 WHERE id = ?1", params![id, now])? > 0)
    }

    fn row_entry(&self, r: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
        let kind: String = r.get(1)?;
        let mimes: String = r.get(5)?;
        let w: Option<u32> = r.get(12)?;
        let h: Option<u32> = r.get(13)?;
        let hash: String = r.get(14)?;
        let kind = Kind::parse(&kind).unwrap_or_default();
        let mimes: Vec<String> = serde_json::from_str(&mimes).unwrap_or_default();
        let image = (kind == Kind::Image)
            .then(|| mimes.iter().find(|m| m.starts_with("image/")).map(|m| self.blobs.join(format!("{hash}.{}", ext_for(m)))))
            .flatten()
            .map(|p| p.to_string_lossy().into_owned());
        Ok(Entry {
            id: r.get(0)?,
            kind,
            preview: r.get(2)?,
            size: r.get::<_, i64>(3)? as u64,
            chars: r.get::<_, i64>(4)? as u64,
            mimes,
            app: r.get(6)?,
            created: r.get(7)?,
            last_used: r.get(8)?,
            uses: r.get::<_, i64>(9)? as u32,
            pinned: r.get::<_, i64>(10)? != 0,
            favorite: r.get::<_, i64>(11)? != 0,
            image,
            dims: w.zip(h),
        })
    }

    pub fn entry(&self, id: i64) -> Result<Option<Entry>> {
        self.conn.query_row(&format!("SELECT {COLUMNS} FROM entries WHERE id = ?1"), [id], |r| self.row_entry(r)).optional()
    }

    /// The newest entry.
    pub fn latest(&self) -> Result<Option<Entry>> {
        self.conn
            .query_row(&format!("SELECT {COLUMNS} FROM entries ORDER BY last_used DESC LIMIT 1"), [], |r| self.row_entry(r))
            .optional()
    }

    /// The newest entry's text, for the `{clipboard}` placeholder.
    pub fn latest_text(&self) -> Option<String> {
        self.conn
            .query_row("SELECT text FROM entries WHERE kind != 'image' ORDER BY last_used DESC LIMIT 1", [], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    /// The entry with its text (cut at DETAIL_TEXT) and any HTML.
    pub fn detail(&self, id: i64) -> Result<Option<Detail>> {
        let Some(entry) = self.entry(id)? else { return Ok(None) };
        let full: String = self.conn.query_row("SELECT text FROM entries WHERE id = ?1", [id], |r| r.get(0))?;
        let (text, truncated) = cut(&full, DETAIL_TEXT);
        let html = self.rep(id, classify::HTML)?.map(|d| cut(&String::from_utf8_lossy(&d), DETAIL_TEXT).0);
        let files = if entry.kind == Kind::Files { full.lines().map(str::to_string).collect() } else { Vec::new() };
        Ok(Some(Detail { entry, text, truncated, html, files }))
    }

    /// The full plain text of an entry.
    pub fn text(&self, id: i64) -> Result<Option<String>> {
        self.conn.query_row("SELECT text FROM entries WHERE id = ?1", [id], |r| r.get(0)).optional()
    }

    /// One stored representation's bytes.
    pub fn rep(&self, id: i64, mime: &str) -> Result<Option<Vec<u8>>> {
        if mime == classify::TEXT {
            return Ok(self.text(id)?.map(String::into_bytes));
        }
        let row: Option<(Option<Vec<u8>>, Option<String>)> = self
            .conn
            .query_row("SELECT data, file FROM reps WHERE entry = ?1 AND mime = ?2", params![id, mime], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        Ok(match row {
            Some((Some(d), _)) => Some(d),
            Some((None, Some(f))) => std::fs::read(self.blobs.join(f)).ok(),
            _ => None,
        })
    }

    /// Every representation, ready to put back on the clipboard, best first.
    pub fn reps(&self, id: i64) -> Result<Vec<Rep>> {
        let Some(entry) = self.entry(id)? else { return Ok(Vec::new()) };
        let mut out = Vec::new();
        for mime in &entry.mimes {
            if let Some(data) = self.rep(id, mime)? {
                out.push(Rep { mime: mime.clone(), data });
            }
        }
        Ok(out)
    }

    pub fn set_pinned(&self, id: i64, pinned: bool, now: f64) -> Result<bool> {
        let at = pinned.then_some(now);
        Ok(self.conn.execute("UPDATE entries SET pinned = ?2, pinned_at = ?3 WHERE id = ?1", params![id, pinned as i64, at])? > 0)
    }

    pub fn set_favorite(&self, id: i64, favorite: bool) -> Result<bool> {
        Ok(self.conn.execute("UPDATE entries SET favorite = ?2 WHERE id = ?1", params![id, favorite as i64])? > 0)
    }

    fn files_of(&self, ids: &[i64]) -> Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        let mut stmt = self.conn.prepare("SELECT file FROM reps WHERE entry = ?1 AND file IS NOT NULL")?;
        for id in ids {
            for f in stmt.query_map([id], |r| r.get::<_, String>(0))? {
                out.push(self.blobs.join(f?));
            }
        }
        Ok(out)
    }

    /// Delete entries and their files. Returns how many went.
    pub fn delete(&mut self, ids: &[i64]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let files = self.files_of(ids)?;
        let tx = self.conn.transaction()?;
        let mut n = 0;
        for id in ids {
            n += tx.execute("DELETE FROM entries WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        for f in files {
            let _ = std::fs::remove_file(f);
        }
        Ok(n)
    }

    /// Delete the history; pinned entries stay unless `pinned_too`.
    pub fn clear(&mut self, pinned_too: bool) -> Result<usize> {
        let sql = if pinned_too { "SELECT id FROM entries" } else { "SELECT id FROM entries WHERE pinned = 0" };
        let ids: Vec<i64> = self.conn.prepare(sql)?.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        self.delete(&ids)
    }

    /// Apply the retention settings: age, count and total size. Pinned entries stay.
    pub fn prune(&mut self, s: &Settings, now: f64) -> Result<usize> {
        let mut doomed: Vec<i64> = Vec::new();
        if s.max_age_days > 0 {
            let before = now - f64::from(s.max_age_days) * 86_400.0;
            let mut stmt = self.conn.prepare("SELECT id FROM entries WHERE pinned = 0 AND last_used < ?1")?;
            doomed.extend(stmt.query_map([before], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?);
        }
        if s.max_entries > 0 {
            let mut stmt = self.conn.prepare("SELECT id FROM entries WHERE pinned = 0 ORDER BY last_used DESC LIMIT -1 OFFSET ?1")?;
            doomed.extend(stmt.query_map([s.max_entries], |r| r.get::<_, i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?);
        }
        doomed.sort_unstable();
        doomed.dedup();
        let mut n = self.delete(&doomed)?;
        let limit = u64::from(s.max_total_mb) * 1024 * 1024;
        let mut total = self.stats()?.bytes;
        if total > limit {
            let mut stmt = self.conn.prepare("SELECT id, size FROM entries WHERE pinned = 0 ORDER BY last_used ASC")?;
            let rows: Vec<(i64, i64)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
            drop(stmt);
            let mut over = Vec::new();
            for (id, size) in rows {
                if total <= limit {
                    break;
                }
                total = total.saturating_sub(size as u64);
                over.push(id);
            }
            n += self.delete(&over)?;
        }
        if n > 0 {
            let _ = self.conn.execute("INSERT INTO entries_fts(entries_fts) VALUES ('optimize')", []);
        }
        Ok(n)
    }

    /// Search the history (see `query::parse` for prefixes). Without text, newest first
    /// (Pinned: most recently pinned first); with text, best match first.
    pub fn search(&self, s: &Search) -> Result<Vec<Entry>> {
        let p = query::parse(&s.text);
        let mut cond: Vec<String> = Vec::new();
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        if s.view == View::Pinned || p.pinned {
            cond.push("pinned = 1".into());
        }
        if s.view == View::Favorites || p.favorites {
            cond.push("favorite = 1".into());
        }
        if let Some(k) = s.kind.or(p.kind) {
            cond.push(format!("kind = ?{}", args.len() + 1));
            args.push(k.as_str().to_string().into());
        }
        let filter = if cond.is_empty() { String::new() } else { format!("WHERE {}", cond.join(" AND ")) };
        let order = if s.view == View::Pinned { "pinned_at DESC, last_used DESC" } else { "last_used DESC" };
        let limit = s.limit();
        if p.text.is_empty() {
            let sql = format!("SELECT {COLUMNS} FROM entries {filter} ORDER BY {order} LIMIT {limit} OFFSET {}", s.offset);
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(args.iter()), |r| self.row_entry(r))?;
            return rows.collect();
        }
        // Substrings through the index, then fuzzy matches among recent entries.
        let mut scores: HashMap<i64, (i32, f64)> = HashMap::new();
        if p.text.chars().count() >= 3 {
            let phrase = format!("\"{}\"", p.text.replace('"', "\"\""));
            let and = if cond.is_empty() { String::new() } else { format!("AND {}", cond.join(" AND ")) };
            let sql = format!(
                "SELECT e.id, e.preview, substr(e.text, 1, {FUZZY_CHARS}), e.last_used FROM entries_fts f JOIN entries e ON e.id = f.rowid
                 WHERE entries_fts MATCH ?{} {and} ORDER BY e.last_used DESC LIMIT 1000",
                args.len() + 1
            );
            let mut a = args.clone();
            a.push(phrase.into());
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(a.iter()), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, f64>(3)?))
            })?;
            for row in rows {
                let (id, preview, head, used) = row?;
                let sc = fuzzy::score(&p.text, &preview).or_else(|| fuzzy::score(&p.text, &head)).unwrap_or(9_000);
                scores.insert(id, (sc + 1_000, used));
            }
        }
        let sql = format!(
            "SELECT id, preview, substr(text, 1, {FUZZY_CHARS}), last_used FROM entries {filter} ORDER BY last_used DESC LIMIT {FUZZY_WINDOW}"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, f64>(3)?))
        })?;
        for row in rows {
            let (id, preview, head, used) = row?;
            if scores.contains_key(&id) {
                continue;
            }
            let best = fuzzy::score(&p.text, &preview).max(fuzzy::score(&p.text, &head));
            if let Some(sc) = best {
                scores.insert(id, (sc, used));
            }
        }
        let mut ranked: Vec<(i64, (i32, f64))> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(b.1.1.total_cmp(&a.1.1)));
        let page: Vec<i64> = ranked.into_iter().skip(s.offset as usize).take(limit as usize).map(|(id, _)| id).collect();
        let mut out = Vec::with_capacity(page.len());
        for id in page {
            if let Some(e) = self.entry(id)? {
                out.push(e);
            }
        }
        Ok(out)
    }

    pub fn stats(&self) -> Result<Stats> {
        self.conn.query_row(
            "SELECT (SELECT COUNT(*) FROM entries), (SELECT COUNT(*) FROM entries WHERE pinned = 1),
                    (SELECT COUNT(*) FROM snippets), (SELECT COALESCE(SUM(size), 0) FROM entries)",
            [],
            |r| {
                Ok(Stats {
                    entries: r.get::<_, i64>(0)? as u64,
                    pinned: r.get::<_, i64>(1)? as u64,
                    snippets: r.get::<_, i64>(2)? as u64,
                    bytes: r.get::<_, i64>(3)? as u64,
                })
            },
        )
    }

    // ---- Snippets

    pub fn snippets(&self, query: &str) -> Result<Vec<Snippet>> {
        let mut stmt = self.conn.prepare("SELECT id, name, body, created, updated FROM snippets ORDER BY name COLLATE NOCASE")?;
        let rows = stmt.query_map([], |r| {
            Ok(Snippet { id: r.get(0)?, name: r.get(1)?, body: r.get(2)?, created: r.get(3)?, updated: r.get(4)? })
        })?;
        let all: Vec<Snippet> = rows.collect::<rusqlite::Result<_>>()?;
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Ok(all);
        }
        let mut scored: Vec<(i32, Snippet)> = all
            .into_iter()
            .filter_map(|s| {
                let sc = fuzzy::score(&q, &s.name).map(|x| x + 500).max(fuzzy::score(&q, &s.body));
                sc.map(|x| (x, s))
            })
            .collect();
        scored.sort_by_key(|x| std::cmp::Reverse(x.0));
        Ok(scored.into_iter().map(|(_, s)| s).collect())
    }

    pub fn snippet(&self, id: i64) -> Result<Option<Snippet>> {
        self.conn
            .query_row("SELECT id, name, body, created, updated FROM snippets WHERE id = ?1", [id], |r| {
                Ok(Snippet { id: r.get(0)?, name: r.get(1)?, body: r.get(2)?, created: r.get(3)?, updated: r.get(4)? })
            })
            .optional()
    }

    /// Save a snippet (a new one when `id` is 0). Returns its id.
    pub fn save_snippet(&self, s: &Snippet, now: f64) -> Result<i64> {
        let name = if s.name.trim().is_empty() { nyx_clipboard_core::classify::preview(&s.body) } else { s.name.trim().to_string() };
        if s.id > 0 && self.conn.execute("UPDATE snippets SET name = ?2, body = ?3, updated = ?4 WHERE id = ?1", params![s.id, name, s.body, now])? > 0 {
            return Ok(s.id);
        }
        self.conn.execute("INSERT INTO snippets(name, body, created, updated) VALUES (?1, ?2, ?3, ?3)", params![name, s.body, now])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_snippet(&self, id: i64) -> Result<bool> {
        Ok(self.conn.execute("DELETE FROM snippets WHERE id = ?1", [id])? > 0)
    }

    // ---- Meta

    pub fn meta(&self, key: &str) -> Option<String> {
        self.conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional().ok().flatten()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute("INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2", params![key, value])?;
        Ok(())
    }
}

/// `s` cut to at most `max` bytes on a character boundary.
fn cut(s: &str, max: usize) -> (String, bool) {
    if s.len() <= max {
        return (s.to_string(), false);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (s[..end].to_string(), true)
}

#[cfg(test)]
mod tests;
