//! Persisted state (RNF-07): contacts, the transfer queue and history.
//!
//! SQLite in WAL mode with `synchronous = FULL`, so a checkpoint that was
//! committed survives a crash or power loss. All access goes through one
//! connection behind a mutex; queries are short.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, params};

use crate::types::{ConnInfo, ConnKind, Contact, Direction, TransferStatus, now_secs};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(
        "the database was created by a newer version of Gigapair (schema {0}); update Gigapair"
    )]
    TooNew(i64),
    #[error("corrupt record in the database: {0}")]
    Corrupt(String),
}

pub type DbResult<T> = Result<T, DbError>;

const SCHEMA_VERSION: i64 = 1;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS contacts (
    node_id     TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    auto_accept INTEGER NOT NULL DEFAULT 0,
    added_at    INTEGER NOT NULL,
    last_seen   INTEGER,
    -- Last known address (JSON EndpointAddr), used alongside discovery.
    addr        TEXT
);

CREATE TABLE IF NOT EXISTS transfers (
    id          TEXT PRIMARY KEY,
    direction   TEXT NOT NULL,
    peer        TEXT NOT NULL,
    peer_name   TEXT NOT NULL,
    status      TEXT NOT NULL,
    label       TEXT NOT NULL,
    total_size  INTEGER NOT NULL,
    file_count  INTEGER NOT NULL,
    done_bytes  INTEGER NOT NULL DEFAULT 0,
    dest_dir    TEXT,
    conn_kind   TEXT,
    relay       TEXT,
    error       TEXT,
    created_at  INTEGER NOT NULL,
    finished_at INTEGER,
    -- Sender: JSON list of the paths the user chose, for "retry".
    sources     TEXT
);
CREATE INDEX IF NOT EXISTS transfers_status ON transfers(status);

CREATE TABLE IF NOT EXISTS transfer_files (
    transfer_id     TEXT NOT NULL REFERENCES transfers(id) ON DELETE CASCADE,
    idx             INTEGER NOT NULL,
    rel_path        TEXT NOT NULL,
    size            INTEGER NOT NULL,
    mtime_ns        INTEGER NOT NULL,
    root_hash       TEXT,
    blocks_hash     TEXT,
    block_hashes    BLOB,
    src_path        TEXT,
    verified_blocks INTEGER NOT NULL DEFAULT 0,
    done            INTEGER NOT NULL DEFAULT 0,
    final_path      TEXT,
    PRIMARY KEY (transfer_id, idx)
);

CREATE TABLE IF NOT EXISTS transfer_dirs (
    transfer_id TEXT NOT NULL REFERENCES transfers(id) ON DELETE CASCADE,
    rel_path    TEXT NOT NULL,
    mtime_ns    INTEGER NOT NULL,
    PRIMARY KEY (transfer_id, rel_path)
);

-- Final name chosen for each top-level entry of a received transfer.
CREATE TABLE IF NOT EXISTS transfer_roots (
    transfer_id TEXT NOT NULL REFERENCES transfers(id) ON DELETE CASCADE,
    wire_root   TEXT NOT NULL,
    final_name  TEXT NOT NULL,
    PRIMARY KEY (transfer_id, wire_root)
);
"#;

/// One row of `transfers`.
#[derive(Debug, Clone, PartialEq)]
pub struct TransferRow {
    pub id: String,
    pub direction: Direction,
    pub peer: String,
    pub peer_name: String,
    pub status: TransferStatus,
    pub label: String,
    pub total_size: u64,
    pub file_count: u64,
    pub done_bytes: u64,
    pub dest_dir: Option<String>,
    pub conn: Option<ConnInfo>,
    pub error: Option<String>,
    pub created_at: i64,
    pub finished_at: Option<i64>,
    pub sources: Vec<String>,
}

/// One row of `transfer_files`.
#[derive(Debug, Clone, PartialEq)]
pub struct FileRow {
    pub idx: u32,
    /// `/`-separated relative path, as sent on the wire.
    pub rel_path: String,
    pub size: u64,
    pub mtime_ns: i64,
    /// BLAKE3 of the whole file, hex.
    pub root_hash: Option<String>,
    /// BLAKE3 of the block hash list, hex.
    pub blocks_hash: Option<String>,
    /// Concatenated 32-byte BLAKE3 hashes of each block.
    pub block_hashes: Option<Vec<u8>>,
    /// Sender: absolute path of the source file.
    pub src_path: Option<String>,
    /// Receiver: blocks verified and synced to disk.
    pub verified_blocks: u64,
    pub done: bool,
    pub final_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirRow {
    pub rel_path: String,
    pub mtime_ns: i64,
}

pub struct Db {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Db")
    }
}

impl Db {
    pub fn open(path: &Path) -> DbResult<Self> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> DbResult<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> DbResult<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(DbError::TooNew(version));
        }
        conn.execute_batch(SCHEMA)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock leaves SQLite itself consistent, so
        // recover the guard instead of propagating the poison.
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    // ---- contacts ----

    pub fn upsert_contact(&self, node_id: &str, name: &str, auto_accept: bool) -> DbResult<()> {
        self.lock().execute(
            "INSERT INTO contacts (node_id, name, auto_accept, added_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(node_id) DO UPDATE SET name = excluded.name, auto_accept = excluded.auto_accept",
            params![node_id, name, auto_accept, now_secs()],
        )?;
        Ok(())
    }

    pub fn contacts(&self) -> DbResult<Vec<Contact>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT node_id, name, auto_accept, last_seen, added_at FROM contacts ORDER BY name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], contact_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn contact(&self, node_id: &str) -> DbResult<Option<Contact>> {
        Ok(self
            .lock()
            .query_row(
                "SELECT node_id, name, auto_accept, last_seen, added_at FROM contacts WHERE node_id = ?1",
                [node_id],
                contact_from_row,
            )
            .optional()?)
    }

    /// Returns true if a contact was removed.
    pub fn remove_contact(&self, node_id: &str) -> DbResult<bool> {
        Ok(self
            .lock()
            .execute("DELETE FROM contacts WHERE node_id = ?1", [node_id])?
            > 0)
    }

    pub fn rename_contact(&self, node_id: &str, name: &str) -> DbResult<bool> {
        Ok(self.lock().execute(
            "UPDATE contacts SET name = ?2 WHERE node_id = ?1",
            params![node_id, name],
        )? > 0)
    }

    pub fn set_auto_accept(&self, node_id: &str, on: bool) -> DbResult<bool> {
        Ok(self.lock().execute(
            "UPDATE contacts SET auto_accept = ?2 WHERE node_id = ?1",
            params![node_id, on],
        )? > 0)
    }

    pub fn set_contact_addr(&self, node_id: &str, addr_json: &str) -> DbResult<()> {
        self.lock().execute(
            "UPDATE contacts SET addr = ?2 WHERE node_id = ?1",
            params![node_id, addr_json],
        )?;
        Ok(())
    }

    pub fn contact_addr(&self, node_id: &str) -> DbResult<Option<String>> {
        Ok(self
            .lock()
            .query_row(
                "SELECT addr FROM contacts WHERE node_id = ?1",
                [node_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    pub fn touch_contact(&self, node_id: &str) -> DbResult<()> {
        self.lock().execute(
            "UPDATE contacts SET last_seen = ?2 WHERE node_id = ?1",
            params![node_id, now_secs()],
        )?;
        Ok(())
    }

    // ---- transfers ----

    pub fn insert_transfer(
        &self,
        t: &TransferRow,
        files: &[FileRow],
        dirs: &[DirRow],
    ) -> DbResult<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO transfers (id, direction, peer, peer_name, status, label, total_size, file_count,
                                    done_bytes, dest_dir, conn_kind, relay, error, created_at, finished_at,
                                    sources)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                t.id,
                t.direction.as_str(),
                t.peer,
                t.peer_name,
                t.status.as_str(),
                t.label,
                t.total_size as i64,
                t.file_count as i64,
                t.done_bytes as i64,
                t.dest_dir,
                t.conn.as_ref().map(|c| conn_kind_str(c.kind)),
                t.conn.as_ref().and_then(|c| c.relay.clone()),
                t.error,
                t.created_at,
                t.finished_at,
                serde_json::to_string(&t.sources).unwrap_or_else(|_| "[]".into()),
            ],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO transfer_files (transfer_id, idx, rel_path, size, mtime_ns, root_hash,
                                             blocks_hash, block_hashes, src_path, verified_blocks, done,
                                             final_path)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            )?;
            for f in files {
                stmt.execute(params![
                    t.id,
                    f.idx,
                    f.rel_path,
                    f.size as i64,
                    f.mtime_ns,
                    f.root_hash,
                    f.blocks_hash,
                    f.block_hashes,
                    f.src_path,
                    f.verified_blocks as i64,
                    f.done,
                    f.final_path,
                ])?;
            }
            let mut stmt = tx.prepare(
                "INSERT INTO transfer_dirs (transfer_id, rel_path, mtime_ns) VALUES (?1, ?2, ?3)",
            )?;
            for d in dirs {
                stmt.execute(params![t.id, d.rel_path, d.mtime_ns])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn transfer(&self, id: &str) -> DbResult<Option<TransferRow>> {
        let conn = self.lock();
        let row = conn
            .query_row(
                &format!("{TRANSFER_SELECT} WHERE id = ?1"),
                [id],
                transfer_from_row,
            )
            .optional()?;
        row.transpose()
    }

    /// Transfers that are not finished, oldest first.
    pub fn active_transfers(&self) -> DbResult<Vec<TransferRow>> {
        self.query_transfers(
            &format!(
                "{TRANSFER_SELECT} WHERE status NOT IN ('completed','rejected','cancelled','failed') ORDER BY created_at, id"
            ),
            [],
        )
    }

    /// Finished transfers, newest first.
    pub fn history(&self, limit: u32) -> DbResult<Vec<TransferRow>> {
        self.query_transfers(
            &format!(
                "{TRANSFER_SELECT} WHERE status IN ('completed','rejected','cancelled','failed')
                 ORDER BY COALESCE(finished_at, created_at) DESC LIMIT ?1"
            ),
            [limit],
        )
    }

    fn query_transfers(
        &self,
        sql: &str,
        params: impl rusqlite::Params,
    ) -> DbResult<Vec<TransferRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params, transfer_from_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row??);
        }
        Ok(out)
    }

    /// Finds a transfer by full id or unique prefix.
    pub fn resolve_transfer_id(&self, prefix: &str) -> DbResult<Vec<String>> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT id FROM transfers WHERE id LIKE ?1 || '%' LIMIT 2")?;
        let rows = stmt.query_map([prefix], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn set_status(&self, id: &str, status: TransferStatus) -> DbResult<()> {
        let finished = status.is_terminal().then(now_secs);
        self.lock().execute(
            "UPDATE transfers SET status = ?2, finished_at = COALESCE(?3, finished_at) WHERE id = ?1",
            params![id, status.as_str(), finished],
        )?;
        Ok(())
    }

    pub fn set_error(&self, id: &str, error: Option<&str>) -> DbResult<()> {
        self.lock().execute(
            "UPDATE transfers SET error = ?2 WHERE id = ?1",
            params![id, error],
        )?;
        Ok(())
    }

    pub fn set_done_bytes(&self, id: &str, done: u64) -> DbResult<()> {
        self.lock().execute(
            "UPDATE transfers SET done_bytes = ?2 WHERE id = ?1",
            params![id, done as i64],
        )?;
        Ok(())
    }

    pub fn set_dest_dir(&self, id: &str, dir: &str) -> DbResult<()> {
        self.lock().execute(
            "UPDATE transfers SET dest_dir = ?2 WHERE id = ?1",
            params![id, dir],
        )?;
        Ok(())
    }

    pub fn set_conn(&self, id: &str, conn: Option<&ConnInfo>) -> DbResult<()> {
        self.lock().execute(
            "UPDATE transfers SET conn_kind = ?2, relay = ?3 WHERE id = ?1",
            params![
                id,
                conn.map(|c| conn_kind_str(c.kind)),
                conn.and_then(|c| c.relay.clone())
            ],
        )?;
        Ok(())
    }

    pub fn delete_transfer(&self, id: &str) -> DbResult<()> {
        self.lock()
            .execute("DELETE FROM transfers WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn clear_history(&self) -> DbResult<usize> {
        Ok(self.lock().execute(
            "DELETE FROM transfers WHERE status IN ('completed','rejected','cancelled','failed')",
            [],
        )?)
    }

    // ---- files ----

    pub fn files(&self, transfer_id: &str) -> DbResult<Vec<FileRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT idx, rel_path, size, mtime_ns, root_hash, blocks_hash, block_hashes, src_path,
                    verified_blocks, done, final_path
             FROM transfer_files WHERE transfer_id = ?1 ORDER BY idx",
        )?;
        let rows = stmt.query_map([transfer_id], |r| {
            Ok(FileRow {
                idx: r.get(0)?,
                rel_path: r.get(1)?,
                size: r.get::<_, i64>(2)? as u64,
                mtime_ns: r.get(3)?,
                root_hash: r.get(4)?,
                blocks_hash: r.get(5)?,
                block_hashes: r.get(6)?,
                src_path: r.get(7)?,
                verified_blocks: r.get::<_, i64>(8)? as u64,
                done: r.get(9)?,
                final_path: r.get(10)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn dirs(&self, transfer_id: &str) -> DbResult<Vec<DirRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT rel_path, mtime_ns FROM transfer_dirs WHERE transfer_id = ?1 ORDER BY rel_path",
        )?;
        let rows = stmt.query_map([transfer_id], |r| {
            Ok(DirRow {
                rel_path: r.get(0)?,
                mtime_ns: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn set_file_hashes(
        &self,
        transfer_id: &str,
        idx: u32,
        root_hash: &str,
        blocks_hash: &str,
        block_hashes: &[u8],
    ) -> DbResult<()> {
        self.lock().execute(
            "UPDATE transfer_files SET root_hash = ?3, blocks_hash = ?4, block_hashes = ?5
             WHERE transfer_id = ?1 AND idx = ?2",
            params![transfer_id, idx, root_hash, blocks_hash, block_hashes],
        )?;
        Ok(())
    }

    pub fn set_block_hashes(&self, transfer_id: &str, idx: u32, blocks: &[u8]) -> DbResult<()> {
        self.lock().execute(
            "UPDATE transfer_files SET block_hashes = ?3 WHERE transfer_id = ?1 AND idx = ?2",
            params![transfer_id, idx, blocks],
        )?;
        Ok(())
    }

    pub fn set_verified_blocks(&self, transfer_id: &str, idx: u32, blocks: u64) -> DbResult<()> {
        self.lock().execute(
            "UPDATE transfer_files SET verified_blocks = ?3 WHERE transfer_id = ?1 AND idx = ?2",
            params![transfer_id, idx, blocks as i64],
        )?;
        Ok(())
    }

    pub fn set_file_done(&self, transfer_id: &str, idx: u32, final_path: &str) -> DbResult<()> {
        self.lock().execute(
            "UPDATE transfer_files SET done = 1, final_path = ?3 WHERE transfer_id = ?1 AND idx = ?2",
            params![transfer_id, idx, final_path],
        )?;
        Ok(())
    }

    /// Resets a file's progress (after a failed final check).
    pub fn reset_file(&self, transfer_id: &str, idx: u32) -> DbResult<()> {
        self.lock().execute(
            "UPDATE transfer_files SET verified_blocks = 0, done = 0, final_path = NULL
             WHERE transfer_id = ?1 AND idx = ?2",
            params![transfer_id, idx],
        )?;
        Ok(())
    }

    // ---- roots ----

    pub fn root_name(&self, transfer_id: &str, wire_root: &str) -> DbResult<Option<String>> {
        Ok(self
            .lock()
            .query_row(
                "SELECT final_name FROM transfer_roots WHERE transfer_id = ?1 AND wire_root = ?2",
                params![transfer_id, wire_root],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_root_name(
        &self,
        transfer_id: &str,
        wire_root: &str,
        final_name: &str,
    ) -> DbResult<()> {
        self.lock().execute(
            "INSERT OR REPLACE INTO transfer_roots (transfer_id, wire_root, final_name) VALUES (?1, ?2, ?3)",
            params![transfer_id, wire_root, final_name],
        )?;
        Ok(())
    }
}

const TRANSFER_SELECT: &str =
    "SELECT id, direction, peer, peer_name, status, label, total_size, file_count,
    done_bytes, dest_dir, conn_kind, relay, error, created_at, finished_at, sources FROM transfers";

fn conn_kind_str(k: ConnKind) -> &'static str {
    match k {
        ConnKind::Direct => "direct",
        ConnKind::Relay => "relay",
    }
}

fn contact_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Contact> {
    Ok(Contact {
        node_id: r.get(0)?,
        name: r.get(1)?,
        auto_accept: r.get(2)?,
        online: false,
        last_seen: r.get(3)?,
        added_at: r.get(4)?,
    })
}

fn transfer_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DbResult<TransferRow>> {
    let direction: String = r.get(1)?;
    let status: String = r.get(4)?;
    let conn_kind: Option<String> = r.get(10)?;
    let relay: Option<String> = r.get(11)?;
    let Some(direction) = Direction::parse(&direction) else {
        return Ok(Err(DbError::Corrupt(format!("direction {direction:?}"))));
    };
    let Some(status) = TransferStatus::parse(&status) else {
        return Ok(Err(DbError::Corrupt(format!("status {status:?}"))));
    };
    let conn = match conn_kind.as_deref() {
        Some("direct") => Some(ConnInfo {
            kind: ConnKind::Direct,
            relay: None,
        }),
        Some("relay") => Some(ConnInfo {
            kind: ConnKind::Relay,
            relay,
        }),
        _ => None,
    };
    Ok(Ok(TransferRow {
        id: r.get(0)?,
        direction,
        peer: r.get(2)?,
        peer_name: r.get(3)?,
        status,
        label: r.get(5)?,
        total_size: r.get::<_, i64>(6)? as u64,
        file_count: r.get::<_, i64>(7)? as u64,
        done_bytes: r.get::<_, i64>(8)? as u64,
        dest_dir: r.get(9)?,
        conn,
        error: r.get(12)?,
        created_at: r.get(13)?,
        finished_at: r.get(14)?,
        sources: r
            .get::<_, Option<String>>(15)?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, status: TransferStatus) -> TransferRow {
        TransferRow {
            id: id.into(),
            direction: Direction::Receive,
            peer: "peer".into(),
            peer_name: "Ana".into(),
            status,
            label: "talk.mp4".into(),
            total_size: 10,
            file_count: 1,
            done_bytes: 0,
            dest_dir: None,
            conn: None,
            error: None,
            created_at: 1,
            finished_at: None,
            sources: vec!["/a".into()],
        }
    }

    fn file(idx: u32) -> FileRow {
        FileRow {
            idx,
            rel_path: format!("f{idx}"),
            size: 10,
            mtime_ns: 5,
            root_hash: None,
            blocks_hash: None,
            block_hashes: None,
            src_path: None,
            verified_blocks: 0,
            done: false,
            final_path: None,
        }
    }

    #[test]
    fn contacts_crud() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_contact("n1", "Ana", false).unwrap();
        db.upsert_contact("n2", "bob", true).unwrap();
        let list = db.contacts().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "Ana");
        assert!(db.set_auto_accept("n1", true).unwrap());
        assert!(db.contact("n1").unwrap().unwrap().auto_accept);
        assert!(db.rename_contact("n1", "Ana B").unwrap());
        assert!(db.remove_contact("n1").unwrap());
        assert!(!db.remove_contact("n1").unwrap());
        assert!(db.contact("n1").unwrap().is_none());
    }

    #[test]
    fn transfers_and_files() {
        let db = Db::open_in_memory().unwrap();
        db.insert_transfer(
            &sample("t1", TransferStatus::Queued),
            &[file(0), file(1)],
            &[DirRow {
                rel_path: "d".into(),
                mtime_ns: 3,
            }],
        )
        .unwrap();
        db.insert_transfer(&sample("t2", TransferStatus::Completed), &[], &[])
            .unwrap();

        assert_eq!(db.active_transfers().unwrap().len(), 1);
        assert_eq!(db.history(10).unwrap().len(), 1);

        db.set_verified_blocks("t1", 1, 7).unwrap();
        db.set_block_hashes("t1", 1, &[1, 2, 3]).unwrap();
        let files = db.files("t1").unwrap();
        assert_eq!(files[1].verified_blocks, 7);
        assert_eq!(files[1].block_hashes.as_deref(), Some(&[1u8, 2, 3][..]));
        assert_eq!(db.dirs("t1").unwrap().len(), 1);

        db.set_conn(
            "t1",
            Some(&ConnInfo {
                kind: ConnKind::Relay,
                relay: Some("r.example".into()),
            }),
        )
        .unwrap();
        db.set_status("t1", TransferStatus::Failed).unwrap();
        let t = db.transfer("t1").unwrap().unwrap();
        assert_eq!(t.status, TransferStatus::Failed);
        assert!(t.finished_at.is_some());
        assert_eq!(t.conn.unwrap().kind, ConnKind::Relay);
        assert_eq!(t.sources, vec!["/a".to_string()]);

        assert_eq!(db.resolve_transfer_id("t").unwrap().len(), 2);
        assert_eq!(
            db.resolve_transfer_id("t1").unwrap(),
            vec!["t1".to_string()]
        );

        db.set_root_name("t1", "talks", "talks (1)").unwrap();
        assert_eq!(
            db.root_name("t1", "talks").unwrap().as_deref(),
            Some("talks (1)")
        );

        db.delete_transfer("t1").unwrap();
        assert!(db.files("t1").unwrap().is_empty());
    }

    #[test]
    fn survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.db");
        {
            let db = Db::open(&path).unwrap();
            db.upsert_contact("n1", "Ana", false).unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.contacts().unwrap().len(), 1);
    }
}
