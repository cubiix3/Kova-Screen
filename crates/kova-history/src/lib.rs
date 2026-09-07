//! Recent-captures history.
//!
//! A deliberately small SQLite table: enough to power a "Recent Captures" list
//! with thumbnails and upload state, and nothing more. No tags, no albums, no
//! full-text search -- those turn a screenshot tool into a media manager.
//!
//! # History is never load-bearing
//!
//! Every capture must still work if this database is missing, locked or
//! corrupt. [`History::open`] therefore never fails in a way the caller must
//! propagate to the user, and the app treats a history error as a warning
//! beside a successful capture rather than a failed one.
//!
//! # The delete URL is a secret
//!
//! A provider deletion link lets anyone holding it destroy the upload. It is
//! stored here so the user can revoke an upload later, but it is never logged
//! and never included in a [`std::fmt::Debug`] rendering of an entry.

use std::path::{Path, PathBuf};

use kova_screen_core::{Error, Result};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};

/// What kind of capture a row describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureKind {
    Screenshot,
    Gif,
    Video,
}

impl CaptureKind {
    fn as_str(self) -> &'static str {
        match self {
            CaptureKind::Screenshot => "screenshot",
            CaptureKind::Gif => "gif",
            CaptureKind::Video => "video",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "gif" => CaptureKind::Gif,
            "video" => CaptureKind::Video,
            // An unknown value from a newer build degrades to the common case
            // rather than dropping the row out of the list entirely.
            _ => CaptureKind::Screenshot,
        }
    }
}

/// How an upload for a capture went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UploadState {
    /// Never attempted.
    None,
    Uploaded,
    Failed,
}

impl UploadState {
    fn as_str(self) -> &'static str {
        match self {
            UploadState::None => "none",
            UploadState::Uploaded => "uploaded",
            UploadState::Failed => "failed",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "uploaded" => UploadState::Uploaded,
            "failed" => UploadState::Failed,
            _ => UploadState::None,
        }
    }
}

/// One capture.
#[derive(Clone, PartialEq, Eq, serde::Serialize)]
pub struct Entry {
    pub id: i64,
    pub path: PathBuf,
    pub file_name: String,
    pub kind: CaptureKind,
    /// Unix seconds.
    pub created_at: i64,
    /// Size on disk in bytes at the time of capture.
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
    pub upload_state: UploadState,
    pub page_url: Option<String>,
    pub direct_url: Option<String>,
    /// Deletion link. Never logged; excluded from `Debug`.
    #[serde(skip_serializing)]
    pub delete_url: Option<String>,
}

impl std::fmt::Debug for Entry {
    /// Renders everything except the deletion link.
    ///
    /// A `Debug` of an entry can end up in a log line or a crash report, and the
    /// deletion link is a capability: printing it would hand anyone reading the
    /// log the ability to destroy the user upload.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("id", &self.id)
            .field("file_name", &self.file_name)
            .field("kind", &self.kind)
            .field("created_at", &self.created_at)
            .field("size_bytes", &self.size_bytes)
            .field("upload_state", &self.upload_state)
            .field("has_delete_url", &self.delete_url.is_some())
            .finish()
    }
}

/// A new capture to record.
#[derive(Debug, Clone)]
pub struct NewEntry {
    pub path: PathBuf,
    pub kind: CaptureKind,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
}

/// The capture history database.
pub struct History {
    connection: Mutex<Connection>,
}

impl History {
    /// Opens (and if needed creates) the database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            kova_screen_core::paths::ensure_dir(parent)?;
        }
        let connection = Connection::open(path)
            .map_err(|e| Error::History(format!("could not open the history database: {e}")))?;
        Self::from_connection(connection)
    }

    /// Opens an in-memory database. Used by tests and as a last-resort fallback
    /// so the history UI still functions when the file cannot be opened.
    pub fn in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory()
            .map_err(|e| Error::History(format!("could not open an in-memory history: {e}")))?;
        Self::from_connection(connection)
    }

    fn from_connection(connection: Connection) -> Result<Self> {
        // WAL keeps a reader (the history window) from blocking a writer (a
        // capture landing), which is the whole point of not letting history
        // stall a capture. NORMAL synchronous is right for a cache-like store:
        // losing the last row to a power cut costs nothing, the file is intact.
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| Error::History(format!("could not enable wal mode: {e}")))?;
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| Error::History(format!("could not set synchronous mode: {e}")))?;
        // A capture must never block for long on a locked database.
        connection
            .busy_timeout(std::time::Duration::from_secs(2))
            .map_err(|e| Error::History(format!("could not set the busy timeout: {e}")))?;

        connection
            .execute_batch(SCHEMA)
            .map_err(|e| Error::History(format!("could not create the history schema: {e}")))?;

        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Records a capture and returns its id.
    pub fn insert(&self, entry: &NewEntry) -> Result<i64> {
        let file_name = entry
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("capture")
            .to_string();
        let path = entry.path.to_string_lossy().to_string();
        let now = now_unix();

        let connection = self.connection.lock();
        connection
            .execute(
                "INSERT INTO captures
                 (path, file_name, kind, created_at, size_bytes, width, height, upload_state)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    path,
                    file_name,
                    entry.kind.as_str(),
                    now,
                    entry.size_bytes as i64,
                    entry.width,
                    entry.height,
                    UploadState::None.as_str(),
                ],
            )
            .map_err(|e| Error::History(format!("could not record the capture: {e}")))?;

        Ok(connection.last_insert_rowid())
    }

    /// Marks a capture as uploaded and stores its links.
    pub fn set_uploaded(
        &self,
        id: i64,
        page_url: &str,
        direct_url: &str,
        delete_url: Option<&str>,
    ) -> Result<()> {
        let connection = self.connection.lock();
        connection
            .execute(
                "UPDATE captures
                 SET upload_state = ?1, page_url = ?2, direct_url = ?3, delete_url = ?4
                 WHERE id = ?5",
                params![
                    UploadState::Uploaded.as_str(),
                    page_url,
                    direct_url,
                    delete_url,
                    id
                ],
            )
            .map_err(|e| Error::History(format!("could not record the upload: {e}")))?;
        Ok(())
    }

    /// Marks an upload attempt as failed.
    ///
    /// The reason is deliberately not stored: it is shown once in a toast, and
    /// keeping server messages around indefinitely serves nobody.
    pub fn set_upload_failed(&self, id: i64) -> Result<()> {
        let connection = self.connection.lock();
        connection
            .execute(
                "UPDATE captures SET upload_state = ?1 WHERE id = ?2",
                params![UploadState::Failed.as_str(), id],
            )
            .map_err(|e| Error::History(format!("could not record the upload failure: {e}")))?;
        Ok(())
    }

    /// The most recent captures, newest first.
    pub fn recent(&self, limit: u32) -> Result<Vec<Entry>> {
        let connection = self.connection.lock();
        let mut statement = connection
            .prepare(
                "SELECT id, path, file_name, kind, created_at, size_bytes, width, height,
                        upload_state, page_url, direct_url, delete_url
                 FROM captures ORDER BY created_at DESC, id DESC LIMIT ?1",
            )
            .map_err(|e| Error::History(format!("could not read the history: {e}")))?;

        let rows = statement
            .query_map(params![limit], row_to_entry)
            .map_err(|e| Error::History(format!("could not read the history: {e}")))?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| Error::History(format!("could not read a history row: {e}")))
    }

    /// One capture by id.
    pub fn get(&self, id: i64) -> Result<Option<Entry>> {
        let connection = self.connection.lock();
        connection
            .query_row(
                "SELECT id, path, file_name, kind, created_at, size_bytes, width, height,
                        upload_state, page_url, direct_url, delete_url
                 FROM captures WHERE id = ?1",
                params![id],
                row_to_entry,
            )
            .optional()
            .map_err(|e| Error::History(format!("could not read the capture: {e}")))
    }

    /// Removes a row. Does not touch the file on disk.
    pub fn remove(&self, id: i64) -> Result<()> {
        let connection = self.connection.lock();
        connection
            .execute("DELETE FROM captures WHERE id = ?1", params![id])
            .map_err(|e| Error::History(format!("could not remove the history entry: {e}")))?;
        Ok(())
    }

    /// Clears the stored upload links for a capture.
    ///
    /// Called after the user deletes the online copy, so the row no longer
    /// offers a link that would now 404 -- and no longer holds a deletion
    /// capability that has already been spent.
    pub fn clear_upload(&self, id: i64) -> Result<()> {
        let connection = self.connection.lock();
        connection
            .execute(
                "UPDATE captures
                 SET upload_state = ?1, page_url = NULL, direct_url = NULL, delete_url = NULL
                 WHERE id = ?2",
                params![UploadState::None.as_str(), id],
            )
            .map_err(|e| Error::History(format!("could not clear the upload: {e}")))?;
        Ok(())
    }

    /// Number of rows.
    pub fn count(&self) -> Result<u32> {
        let connection = self.connection.lock();
        connection
            .query_row("SELECT COUNT(*) FROM captures", [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|n| n as u32)
            .map_err(|e| Error::History(format!("could not count the history: {e}")))
    }

    /// Trims the history to the newest `limit` rows.
    ///
    /// `limit == 0` means unlimited and does nothing. Only rows are removed;
    /// the files stay on disk, because forgetting a capture in a list must
    /// never silently delete the user screenshot.
    pub fn prune(&self, limit: u32) -> Result<u32> {
        if limit == 0 {
            return Ok(0);
        }
        let connection = self.connection.lock();
        let removed = connection
            .execute(
                "DELETE FROM captures WHERE id NOT IN (
                     SELECT id FROM captures ORDER BY created_at DESC, id DESC LIMIT ?1
                 )",
                params![limit],
            )
            .map_err(|e| Error::History(format!("could not prune the history: {e}")))?;
        Ok(removed as u32)
    }

    /// Drops rows whose file no longer exists.
    ///
    /// The user may have deleted or moved a capture in Explorer, and a list
    /// full of dead entries is worse than a shorter accurate one.
    pub fn forget_missing_files(&self) -> Result<u32> {
        let entries = self.recent(u32::MAX)?;
        let mut removed = 0;
        for entry in entries {
            if !entry.path.exists() {
                self.remove(entry.id)?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS captures (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    path         TEXT    NOT NULL,
    file_name    TEXT    NOT NULL,
    kind         TEXT    NOT NULL,
    created_at   INTEGER NOT NULL,
    size_bytes   INTEGER NOT NULL DEFAULT 0,
    width        INTEGER NOT NULL DEFAULT 0,
    height       INTEGER NOT NULL DEFAULT 0,
    upload_state TEXT    NOT NULL DEFAULT 'none',
    page_url     TEXT,
    direct_url   TEXT,
    delete_url   TEXT
);
CREATE INDEX IF NOT EXISTS captures_created_at ON captures (created_at DESC);
";

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    let path: String = row.get(1)?;
    let kind: String = row.get(3)?;
    let upload_state: String = row.get(8)?;
    let size: i64 = row.get(5)?;

    Ok(Entry {
        id: row.get(0)?,
        path: PathBuf::from(path),
        file_name: row.get(2)?,
        kind: CaptureKind::parse(&kind),
        created_at: row.get(4)?,
        // A negative size can only come from a corrupt row; clamp rather than
        // wrapping into an enormous number in the UI.
        size_bytes: size.max(0) as u64,
        width: row.get(6)?,
        height: row.get(7)?,
        upload_state: UploadState::parse(&upload_state),
        page_url: row.get(9)?,
        direct_url: row.get(10)?,
        delete_url: row.get(11)?,
    })
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history() -> History {
        History::in_memory().expect("an in-memory history")
    }

    fn new_entry(name: &str, kind: CaptureKind) -> NewEntry {
        NewEntry {
            path: std::env::temp_dir().join(name),
            kind,
            size_bytes: 4096,
            width: 1920,
            height: 1080,
        }
    }

    #[test]
    fn a_capture_round_trips() {
        let history = history();
        let id = history
            .insert(&new_entry("a.png", CaptureKind::Screenshot))
            .unwrap();

        let entry = history.get(id).unwrap().expect("the entry exists");
        assert_eq!(entry.file_name, "a.png");
        assert_eq!(entry.kind, CaptureKind::Screenshot);
        assert_eq!(entry.size_bytes, 4096);
        assert_eq!((entry.width, entry.height), (1920, 1080));
        assert_eq!(entry.upload_state, UploadState::None);
        assert_eq!(entry.delete_url, None);
    }

    #[test]
    fn recent_returns_newest_first() {
        let history = history();
        let first = history
            .insert(&new_entry("first.png", CaptureKind::Screenshot))
            .unwrap();
        let second = history
            .insert(&new_entry("second.png", CaptureKind::Gif))
            .unwrap();
        let third = history
            .insert(&new_entry("third.mp4", CaptureKind::Video))
            .unwrap();

        let ids: Vec<i64> = history.recent(10).unwrap().iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![third, second, first]);
    }

    #[test]
    fn recent_honours_its_limit() {
        let history = history();
        for i in 0..10 {
            history
                .insert(&new_entry(&format!("{i}.png"), CaptureKind::Screenshot))
                .unwrap();
        }
        assert_eq!(history.recent(3).unwrap().len(), 3);
        assert_eq!(history.recent(100).unwrap().len(), 10);
    }

    #[test]
    fn an_upload_result_is_recorded_and_readable() {
        let history = history();
        let id = history
            .insert(&new_entry("up.png", CaptureKind::Screenshot))
            .unwrap();
        history
            .set_uploaded(
                id,
                "https://vgy.me/u/abc",
                "https://i.vgy.me/abc.png",
                Some("https://vgy.me/delete/xyz"),
            )
            .unwrap();

        let entry = history.get(id).unwrap().unwrap();
        assert_eq!(entry.upload_state, UploadState::Uploaded);
        assert_eq!(entry.page_url.as_deref(), Some("https://vgy.me/u/abc"));
        assert_eq!(
            entry.direct_url.as_deref(),
            Some("https://i.vgy.me/abc.png")
        );
        assert_eq!(
            entry.delete_url.as_deref(),
            Some("https://vgy.me/delete/xyz")
        );
    }

    #[test]
    fn a_failed_upload_is_distinguishable_from_one_never_attempted() {
        let history = history();
        let id = history
            .insert(&new_entry("fail.png", CaptureKind::Screenshot))
            .unwrap();
        assert_eq!(
            history.get(id).unwrap().unwrap().upload_state,
            UploadState::None
        );

        history.set_upload_failed(id).unwrap();
        assert_eq!(
            history.get(id).unwrap().unwrap().upload_state,
            UploadState::Failed
        );
    }

    #[test]
    fn the_delete_url_never_appears_in_a_debug_rendering() {
        // A Debug of an entry can reach a log file; the deletion link is a
        // capability and must not travel with it.
        let history = history();
        let id = history
            .insert(&new_entry("secret.png", CaptureKind::Screenshot))
            .unwrap();
        let secret = "https://vgy.me/delete/SUPERSECRETTOKEN";
        history
            .set_uploaded(
                id,
                "https://vgy.me/u/a",
                "https://i.vgy.me/a.png",
                Some(secret),
            )
            .unwrap();

        let entry = history.get(id).unwrap().unwrap();
        let rendered = format!("{entry:?}");
        assert!(
            !rendered.contains("SUPERSECRETTOKEN"),
            "the delete url leaked: {rendered}"
        );
        assert!(rendered.contains("has_delete_url: true"));
        // It is still available to the code that needs it.
        assert_eq!(entry.delete_url.as_deref(), Some(secret));
    }

    #[test]
    fn the_delete_url_is_not_serialised_to_the_ui() {
        let history = history();
        let id = history
            .insert(&new_entry("ser.png", CaptureKind::Screenshot))
            .unwrap();
        history
            .set_uploaded(
                id,
                "https://vgy.me/u/a",
                "https://i.vgy.me/a.png",
                Some("https://vgy.me/delete/TOKEN"),
            )
            .unwrap();
        let json = serde_json::to_string(&history.get(id).unwrap().unwrap()).unwrap();
        assert!(
            !json.contains("TOKEN"),
            "the delete url crossed the ipc boundary: {json}"
        );
    }

    #[test]
    fn clearing_an_upload_removes_every_link() {
        let history = history();
        let id = history
            .insert(&new_entry("clear.png", CaptureKind::Screenshot))
            .unwrap();
        history
            .set_uploaded(
                id,
                "https://vgy.me/u/a",
                "https://i.vgy.me/a.png",
                Some("https://vgy.me/delete/x"),
            )
            .unwrap();

        history.clear_upload(id).unwrap();
        let entry = history.get(id).unwrap().unwrap();
        assert_eq!(entry.upload_state, UploadState::None);
        assert_eq!(entry.page_url, None);
        assert_eq!(entry.direct_url, None);
        assert_eq!(
            entry.delete_url, None,
            "a spent deletion capability was kept"
        );
    }

    #[test]
    fn removing_an_entry_leaves_the_others() {
        let history = history();
        let a = history
            .insert(&new_entry("a.png", CaptureKind::Screenshot))
            .unwrap();
        let b = history
            .insert(&new_entry("b.png", CaptureKind::Screenshot))
            .unwrap();
        history.remove(a).unwrap();
        assert!(history.get(a).unwrap().is_none());
        assert!(history.get(b).unwrap().is_some());
        assert_eq!(history.count().unwrap(), 1);
    }

    #[test]
    fn removing_a_missing_id_is_not_an_error() {
        let history = history();
        history
            .remove(9999)
            .expect("removing a missing row must not fail");
    }

    #[test]
    fn getting_a_missing_id_returns_none() {
        assert!(history().get(1234).unwrap().is_none());
    }

    #[test]
    fn pruning_keeps_the_newest_rows() {
        let history = history();
        let mut ids = Vec::new();
        for i in 0..20 {
            ids.push(
                history
                    .insert(&new_entry(&format!("{i}.png"), CaptureKind::Screenshot))
                    .unwrap(),
            );
        }
        let removed = history.prune(5).unwrap();
        assert_eq!(removed, 15);
        assert_eq!(history.count().unwrap(), 5);

        let kept: Vec<i64> = history.recent(10).unwrap().iter().map(|e| e.id).collect();
        let newest: Vec<i64> = ids.iter().rev().take(5).copied().collect();
        assert_eq!(kept, newest);
    }

    #[test]
    fn pruning_with_no_limit_keeps_everything() {
        let history = history();
        for i in 0..5 {
            history
                .insert(&new_entry(&format!("{i}.png"), CaptureKind::Screenshot))
                .unwrap();
        }
        assert_eq!(history.prune(0).unwrap(), 0);
        assert_eq!(history.count().unwrap(), 5);
    }

    #[test]
    fn pruning_below_the_row_count_is_a_no_op() {
        let history = history();
        history
            .insert(&new_entry("only.png", CaptureKind::Screenshot))
            .unwrap();
        assert_eq!(history.prune(50).unwrap(), 0);
        assert_eq!(history.count().unwrap(), 1);
    }

    #[test]
    fn entries_whose_file_vanished_are_forgotten_without_touching_others() {
        let dir = std::env::temp_dir().join("kova-history-missing");
        std::fs::create_dir_all(&dir).unwrap();
        let present = dir.join("present.png");
        std::fs::write(&present, b"x").unwrap();
        let absent = dir.join("absent.png");
        let _ = std::fs::remove_file(&absent);

        let history = history();
        history
            .insert(&NewEntry {
                path: present.clone(),
                kind: CaptureKind::Screenshot,
                size_bytes: 1,
                width: 1,
                height: 1,
            })
            .unwrap();
        history
            .insert(&NewEntry {
                path: absent,
                kind: CaptureKind::Screenshot,
                size_bytes: 1,
                width: 1,
                height: 1,
            })
            .unwrap();

        assert_eq!(history.forget_missing_files().unwrap(), 1);
        assert_eq!(history.count().unwrap(), 1);
        // The surviving file must not have been deleted from disk.
        assert!(present.exists(), "forgetting a row deleted a real capture");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_database_file_persists_across_reopen() {
        let path = std::env::temp_dir().join("kova-history-persist.db");
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }

        let id = {
            let history = History::open(&path).unwrap();
            history
                .insert(&new_entry("persist.png", CaptureKind::Screenshot))
                .unwrap()
        };

        let reopened = History::open(&path).unwrap();
        assert_eq!(reopened.get(id).unwrap().unwrap().file_name, "persist.png");

        drop(reopened);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn opening_an_existing_database_twice_does_not_duplicate_the_schema() {
        let path = std::env::temp_dir().join("kova-history-schema.db");
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
        let first = History::open(&path).unwrap();
        first
            .insert(&new_entry("one.png", CaptureKind::Screenshot))
            .unwrap();
        drop(first);

        let second = History::open(&path).unwrap();
        assert_eq!(
            second.count().unwrap(),
            1,
            "reopening reset or duplicated the schema"
        );
        drop(second);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn concurrent_inserts_from_several_threads_all_land() {
        // A capture must never be lost because the history window is open.
        let history = std::sync::Arc::new(history());
        let mut handles = Vec::new();
        for t in 0..4 {
            let history = std::sync::Arc::clone(&history);
            handles.push(std::thread::spawn(move || {
                for i in 0..25 {
                    history
                        .insert(&new_entry(
                            &format!("t{t}-{i}.png"),
                            CaptureKind::Screenshot,
                        ))
                        .expect("concurrent insert");
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(history.count().unwrap(), 100);
    }

    #[test]
    fn unknown_enum_values_degrade_instead_of_dropping_the_row() {
        // A database written by a newer build must still render.
        assert_eq!(CaptureKind::parse("something-new"), CaptureKind::Screenshot);
        assert_eq!(UploadState::parse("in-progress"), UploadState::None);
    }

    #[test]
    fn every_kind_round_trips_through_its_string_form() {
        for kind in [
            CaptureKind::Screenshot,
            CaptureKind::Gif,
            CaptureKind::Video,
        ] {
            assert_eq!(CaptureKind::parse(kind.as_str()), kind);
        }
        for state in [
            UploadState::None,
            UploadState::Uploaded,
            UploadState::Failed,
        ] {
            assert_eq!(UploadState::parse(state.as_str()), state);
        }
    }
}
