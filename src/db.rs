use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::model::{
    ArchiveRecord, DropDetail, DropRecord, Entry, EntryKind, NewDrop, RedeemResult, RotateOutcome,
};

#[derive(Clone, Debug)]
pub struct Database {
    path: PathBuf,
}

impl Database {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    fn open(&self) -> Result<Connection> {
        let connection = Connection::open(&self.path)
            .with_context(|| format!("open database {}", self.path.display()))?;
        connection.busy_timeout(Duration::from_secs(8))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "secure_delete", "FAST")?;
        Ok(connection)
    }

    pub fn initialize(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = self.open()?;
        connection.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS drops (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                manifest_hash TEXT NOT NULL,
                code_tag BLOB NOT NULL UNIQUE,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                code_expires_at INTEGER NOT NULL,
                revoked_at INTEGER,
                max_redemptions INTEGER,
                redemption_count INTEGER NOT NULL DEFAULT 0,
                total_bytes INTEGER NOT NULL,
                file_count INTEGER NOT NULL,
                directory_count INTEGER NOT NULL,
                CHECK (expires_at > created_at),
                CHECK (code_expires_at > created_at),
                CHECK (code_expires_at <= expires_at),
                CHECK (redemption_count >= 0)
            );
            CREATE INDEX IF NOT EXISTS drops_active_code_idx
                ON drops(code_tag, code_expires_at, expires_at) WHERE revoked_at IS NULL;

            CREATE TABLE IF NOT EXISTS entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                drop_id TEXT NOT NULL REFERENCES drops(id) ON DELETE CASCADE,
                path TEXT NOT NULL,
                kind INTEGER NOT NULL,
                object_hash TEXT,
                sha256_hex TEXT,
                sha256_base64 TEXT,
                size INTEGER NOT NULL,
                media_type TEXT,
                unix_mode INTEGER NOT NULL,
                UNIQUE(drop_id, path),
                CHECK (kind IN (1, 2)),
                CHECK ((kind = 1 AND object_hash IS NOT NULL AND sha256_hex IS NOT NULL)
                    OR (kind = 2 AND object_hash IS NULL AND size = 0))
            );
            CREATE INDEX IF NOT EXISTS entries_drop_idx ON entries(drop_id, path);
            CREATE INDEX IF NOT EXISTS entries_object_idx ON entries(object_hash) WHERE object_hash IS NOT NULL;

            CREATE TABLE IF NOT EXISTS sessions (
                tag BLOB PRIMARY KEY,
                drop_id TEXT NOT NULL REFERENCES drops(id) ON DELETE CASCADE,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL,
                revoked_at INTEGER
            );
            CREATE INDEX IF NOT EXISTS sessions_drop_idx ON sessions(drop_id, expires_at);

            CREATE TABLE IF NOT EXISTS admin_sessions (
                tag BLOB PRIMARY KEY,
                created_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS redemption_failures (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                occurred_at INTEGER NOT NULL,
                source_tag BLOB NOT NULL
            );
            CREATE INDEX IF NOT EXISTS redemption_failures_time_idx ON redemption_failures(occurred_at);
            CREATE INDEX IF NOT EXISTS redemption_failures_source_idx ON redemption_failures(source_tag, occurred_at);

            CREATE TABLE IF NOT EXISTS archives (
                cache_key TEXT PRIMARY KEY,
                drop_id TEXT NOT NULL REFERENCES drops(id) ON DELETE CASCADE,
                scope TEXT NOT NULL,
                relative_path TEXT NOT NULL,
                size INTEGER NOT NULL,
                sha256_hex TEXT NOT NULL,
                sha256_base64 TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS archives_drop_idx ON archives(drop_id);

            CREATE TABLE IF NOT EXISTS audit_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                occurred_at INTEGER NOT NULL,
                event TEXT NOT NULL,
                drop_id TEXT,
                detail TEXT
            );
            CREATE INDEX IF NOT EXISTS audit_events_time_idx ON audit_events(occurred_at);

            PRAGMA user_version = 1;
            "#,
        )?;
        Ok(())
    }

    pub fn quick_check(&self) -> Result<String> {
        let connection = self.open()?;
        let result: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        Ok(result)
    }

    pub fn code_tag_exists(&self, code_tag: &[u8]) -> Result<bool> {
        let connection = self.open()?;
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM drops WHERE code_tag = ?1",
            params![code_tag],
            |row| row.get(0),
        )?;
        Ok(count != 0)
    }

    pub fn insert_drop(&self, new: &NewDrop) -> Result<()> {
        let mut connection = self.open()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let r = &new.record;
        let total_bytes =
            i64::try_from(r.total_bytes).context("drop size exceeds SQLite integer range")?;
        let file_count =
            i64::try_from(r.file_count).context("file count exceeds SQLite integer range")?;
        let directory_count = i64::try_from(r.directory_count)
            .context("directory count exceeds SQLite integer range")?;
        tx.execute(
            "INSERT INTO drops (id, title, manifest_hash, code_tag, created_at, expires_at, code_expires_at, revoked_at, max_redemptions, redemption_count, total_bytes, file_count, directory_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                r.id, r.title, r.manifest_hash, new.code_tag, r.created_at, r.expires_at,
                r.code_expires_at, r.revoked_at, r.max_redemptions, r.redemption_count,
                total_bytes, file_count, directory_count,
            ],
        )?;
        {
            let mut statement = tx.prepare(
                "INSERT INTO entries (drop_id, path, kind, object_hash, sha256_hex, sha256_base64, size, media_type, unix_mode)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for entry in &new.entries {
                let size =
                    i64::try_from(entry.size).context("entry size exceeds SQLite integer range")?;
                statement.execute(params![
                    r.id,
                    entry.path,
                    entry.kind.as_i64(),
                    entry.object_hash,
                    entry.sha256_hex,
                    entry.sha256_base64,
                    size,
                    entry.media_type,
                    entry.unix_mode,
                ])?;
            }
        }
        tx.execute(
            "INSERT INTO audit_events (occurred_at, event, drop_id, detail) VALUES (?1, 'drop_created', ?2, ?3)",
            params![r.created_at, r.id, format!("files={} bytes={}", r.file_count, r.total_bytes)],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn map_drop(row: &rusqlite::Row<'_>) -> rusqlite::Result<DropRecord> {
        Ok(DropRecord {
            id: row.get(0)?,
            title: row.get(1)?,
            manifest_hash: row.get(2)?,
            created_at: row.get(3)?,
            expires_at: row.get(4)?,
            code_expires_at: row.get(5)?,
            revoked_at: row.get(6)?,
            max_redemptions: row.get(7)?,
            redemption_count: row.get(8)?,
            total_bytes: get_u64(row, 9)?,
            file_count: get_u64(row, 10)?,
            directory_count: get_u64(row, 11)?,
        })
    }

    fn map_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
        Ok(Entry {
            id: row.get(0)?,
            drop_id: row.get(1)?,
            path: row.get(2)?,
            kind: EntryKind::from_i64(row.get(3)?)?,
            object_hash: row.get(4)?,
            sha256_hex: row.get(5)?,
            sha256_base64: row.get(6)?,
            size: get_u64(row, 7)?,
            media_type: row.get(8)?,
            unix_mode: row.get(9)?,
        })
    }

    pub fn list_drops(&self, include_inactive: bool, now: i64) -> Result<Vec<DropRecord>> {
        let connection = self.open()?;
        let sql = if include_inactive {
            "SELECT id,title,manifest_hash,created_at,expires_at,code_expires_at,revoked_at,max_redemptions,redemption_count,total_bytes,file_count,directory_count FROM drops ORDER BY created_at DESC"
        } else {
            "SELECT id,title,manifest_hash,created_at,expires_at,code_expires_at,revoked_at,max_redemptions,redemption_count,total_bytes,file_count,directory_count FROM drops WHERE revoked_at IS NULL AND expires_at > ?1 ORDER BY created_at DESC"
        };
        let mut statement = connection.prepare(sql)?;
        let mapped = if include_inactive {
            statement.query_map([], Self::map_drop)?
        } else {
            statement.query_map(params![now], Self::map_drop)?
        };
        Ok(mapped.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_drop(&self, id: &str) -> Result<Option<DropDetail>> {
        let connection = self.open()?;
        let drop = connection.query_row(
            "SELECT id,title,manifest_hash,created_at,expires_at,code_expires_at,revoked_at,max_redemptions,redemption_count,total_bytes,file_count,directory_count FROM drops WHERE id=?1",
            params![id],
            Self::map_drop,
        ).optional()?;
        let Some(drop) = drop else {
            return Ok(None);
        };
        let mut statement = connection.prepare(
            "SELECT id,drop_id,path,kind,object_hash,sha256_hex,sha256_base64,size,media_type,unix_mode FROM entries WHERE drop_id=?1 ORDER BY path ASC",
        )?;
        let entries = statement
            .query_map(params![id], Self::map_entry)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some(DropDetail { drop, entries }))
    }

    pub fn get_entry(&self, drop_id: &str, entry_id: i64) -> Result<Option<Entry>> {
        let connection = self.open()?;
        Ok(connection.query_row(
            "SELECT id,drop_id,path,kind,object_hash,sha256_hex,sha256_base64,size,media_type,unix_mode FROM entries WHERE drop_id=?1 AND id=?2",
            params![drop_id, entry_id],
            Self::map_entry,
        ).optional()?)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn redeem(
        &self,
        code_tag: &[u8],
        source_tag: &[u8],
        session_tag: &[u8],
        now: i64,
        requested_session_expires_at: i64,
        global_limit: u32,
        source_limit: u32,
    ) -> Result<RedeemResult> {
        let mut connection = self.open()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM redemption_failures WHERE occurred_at < ?1",
            params![now - 3600],
        )?;
        tx.execute(
            "DELETE FROM sessions WHERE expires_at < ?1",
            params![now - 86_400],
        )?;
        let global_count: u32 = tx.query_row(
            "SELECT COUNT(*) FROM redemption_failures WHERE occurred_at >= ?1",
            params![now - 60],
            |row| row.get(0),
        )?;
        let source_count: u32 = tx.query_row(
            "SELECT COUNT(*) FROM redemption_failures WHERE occurred_at >= ?1 AND source_tag=?2",
            params![now - 60, source_tag],
            |row| row.get(0),
        )?;
        if global_count >= global_limit || source_count >= source_limit {
            tx.execute(
                "INSERT INTO audit_events (occurred_at,event,detail)
                 SELECT ?1,'redeem_throttled',?2
                 WHERE NOT EXISTS (SELECT 1 FROM audit_events WHERE event='redeem_throttled' AND occurred_at>=?1-60)",
                params![now, format!("global={global_count} source={source_count}")],
            )?;
            tx.commit()?;
            return Ok(RedeemResult::Rejected);
        }

        let candidate: Option<(String, i64)> = tx
            .query_row(
                "SELECT id,expires_at FROM drops
             WHERE code_tag=?1 AND revoked_at IS NULL AND expires_at>?2 AND code_expires_at>?2
               AND (max_redemptions IS NULL OR redemption_count < max_redemptions)
             LIMIT 1",
                params![code_tag, now],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        if let Some((drop_id, drop_expires_at)) = candidate {
            let session_expires_at = requested_session_expires_at.min(drop_expires_at);
            tx.execute(
                "INSERT INTO sessions (tag,drop_id,created_at,expires_at,last_seen_at,revoked_at) VALUES (?1,?2,?3,?4,?3,NULL)",
                params![session_tag, drop_id, now, session_expires_at],
            )?;
            tx.execute(
                "UPDATE drops SET redemption_count=redemption_count+1 WHERE id=?1",
                params![drop_id],
            )?;
            tx.execute(
                "INSERT INTO audit_events (occurred_at,event,drop_id) VALUES (?1,'redeem_success',?2)",
                params![now, drop_id],
            )?;
            tx.commit()?;
            Ok(RedeemResult::Success {
                drop_id,
                session_expires_at,
            })
        } else {
            tx.execute(
                "INSERT INTO redemption_failures (occurred_at,source_tag) VALUES (?1,?2)",
                params![now, source_tag],
            )?;
            tx.execute(
                "INSERT INTO audit_events (occurred_at,event) VALUES (?1,'redeem_rejected')",
                params![now],
            )?;
            tx.commit()?;
            Ok(RedeemResult::Rejected)
        }
    }

    /// Current one-minute failure pressure (global, per-source) shared by code
    /// redemption and the FrankenDrop admin gate, pruning stale rows first.
    pub fn failure_counts(&self, source_tag: &[u8], now: i64) -> Result<(u32, u32)> {
        let connection = self.open()?;
        connection.execute(
            "DELETE FROM redemption_failures WHERE occurred_at < ?1",
            params![now - 3600],
        )?;
        let global: u32 = connection.query_row(
            "SELECT COUNT(*) FROM redemption_failures WHERE occurred_at >= ?1",
            params![now - 60],
            |row| row.get(0),
        )?;
        let source: u32 = connection.query_row(
            "SELECT COUNT(*) FROM redemption_failures WHERE occurred_at >= ?1 AND source_tag=?2",
            params![now - 60, source_tag],
            |row| row.get(0),
        )?;
        Ok((global, source))
    }

    pub fn record_failure(&self, source_tag: &[u8], now: i64, event: &str) -> Result<()> {
        let mut connection = self.open()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO redemption_failures (occurred_at,source_tag) VALUES (?1,?2)",
            params![now, source_tag],
        )?;
        tx.execute(
            "INSERT INTO audit_events (occurred_at,event) VALUES (?1,?2)",
            params![now, event],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn validate_session(&self, tag: &[u8], drop_id: &str, now: i64) -> Result<bool> {
        let connection = self.open()?;
        let changed = connection.execute(
            "UPDATE sessions SET last_seen_at=?1
             WHERE tag=?2 AND drop_id=?3 AND revoked_at IS NULL AND expires_at>?1
               AND EXISTS (SELECT 1 FROM drops WHERE id=?3 AND revoked_at IS NULL AND expires_at>?1)",
            params![now, tag, drop_id],
        )?;
        Ok(changed == 1)
    }

    pub fn create_admin_session(&self, tag: &[u8], now: i64, expires_at: i64) -> Result<()> {
        let mut connection = self.open()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM admin_sessions WHERE expires_at < ?1",
            params![now],
        )?;
        tx.execute(
            "INSERT INTO admin_sessions (tag,created_at,expires_at) VALUES (?1,?2,?3)",
            params![tag, now, expires_at],
        )?;
        tx.execute(
            "INSERT INTO audit_events (occurred_at,event) VALUES (?1,'console_unlocked')",
            params![now],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn validate_admin_session(&self, tag: &[u8], now: i64) -> Result<bool> {
        let connection = self.open()?;
        let count: i64 = connection.query_row(
            "SELECT COUNT(*) FROM admin_sessions WHERE tag=?1 AND expires_at>?2",
            params![tag, now],
            |row| row.get(0),
        )?;
        Ok(count == 1)
    }

    pub fn delete_admin_session(&self, tag: &[u8]) -> Result<()> {
        self.open()?
            .execute("DELETE FROM admin_sessions WHERE tag=?1", params![tag])?;
        Ok(())
    }

    /// Swap an active drop's code tag for a fresh one, restarting the
    /// redemption window. The drop may be addressed by a unique ID prefix;
    /// the old code stops working the moment the transaction commits.
    pub fn rotate_code(
        &self,
        reference: &str,
        code_tag: &[u8],
        now: i64,
        code_ttl_seconds: i64,
    ) -> Result<RotateOutcome> {
        let mut connection = self.open()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Drop IDs are base64url, so `_` is the only LIKE metacharacter to escape.
        let pattern = format!("{}%", reference.replace('_', "\\_"));
        let ids = {
            let mut statement = tx.prepare(
                "SELECT id FROM drops
                 WHERE id LIKE ?1 ESCAPE '\\' AND revoked_at IS NULL AND expires_at > ?2
                 LIMIT 2",
            )?;
            statement
                .query_map(params![pattern, now], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let id = match ids.as_slice() {
            [] => return Ok(RotateOutcome::NotFound),
            [id] => id.clone(),
            _ => return Ok(RotateOutcome::Ambiguous),
        };
        tx.execute(
            "UPDATE drops SET code_tag=?2, code_expires_at=MIN(expires_at, ?3) WHERE id=?1",
            params![id, code_tag, now.saturating_add(code_ttl_seconds)],
        )?;
        tx.execute(
            "INSERT INTO audit_events (occurred_at,event,drop_id) VALUES (?1,'code_rotated',?2)",
            params![now, id],
        )?;
        let record = tx.query_row(
            "SELECT id,title,manifest_hash,created_at,expires_at,code_expires_at,revoked_at,max_redemptions,redemption_count,total_bytes,file_count,directory_count FROM drops WHERE id=?1",
            params![id],
            Self::map_drop,
        )?;
        tx.commit()?;
        Ok(RotateOutcome::Rotated(record))
    }

    pub fn revoke_drop(&self, id: &str, now: i64) -> Result<bool> {
        let mut connection = self.open()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE drops SET revoked_at=?2 WHERE id=?1 AND revoked_at IS NULL",
            params![id, now],
        )?;
        if changed == 1 {
            tx.execute(
                "UPDATE sessions SET revoked_at=?2 WHERE drop_id=?1 AND revoked_at IS NULL",
                params![id, now],
            )?;
            tx.execute(
                "INSERT INTO audit_events (occurred_at,event,drop_id) VALUES (?1,'drop_revoked',?2)",
                params![now, id],
            )?;
        }
        tx.commit()?;
        Ok(changed == 1)
    }

    pub fn referenced_objects(&self) -> Result<Vec<(String, u64)>> {
        let connection = self.open()?;
        let mut statement = connection.prepare(
            "SELECT object_hash,MAX(size) FROM entries WHERE object_hash IS NOT NULL GROUP BY object_hash",
        )?;
        Ok(statement
            .query_map([], |row| Ok((row.get(0)?, get_u64(row, 1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn referenced_objects_after_retention(&self, cutoff: i64) -> Result<Vec<(String, u64)>> {
        let connection = self.open()?;
        let mut statement = connection.prepare(
            "SELECT e.object_hash,MAX(e.size)
             FROM entries e JOIN drops d ON d.id=e.drop_id
             WHERE e.object_hash IS NOT NULL
               AND NOT (d.expires_at < ?1 OR (d.revoked_at IS NOT NULL AND d.revoked_at < ?1))
             GROUP BY e.object_hash",
        )?;
        Ok(statement
            .query_map(params![cutoff], |row| Ok((row.get(0)?, get_u64(row, 1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn retention_counts(
        &self,
        now: i64,
        drop_cutoff: i64,
        audit_cutoff: i64,
    ) -> Result<(u64, u64, u64)> {
        let connection = self.open()?;
        let drops: i64 = connection.query_row(
            "SELECT COUNT(*) FROM drops WHERE expires_at < ?1 OR (revoked_at IS NOT NULL AND revoked_at < ?1)",
            params![drop_cutoff],
            |row| row.get(0),
        )?;
        let sessions: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sessions WHERE expires_at < ?1",
            params![now],
            |row| row.get(0),
        )?;
        let audit: i64 = connection.query_row(
            "SELECT COUNT(*) FROM audit_events WHERE occurred_at < ?1",
            params![audit_cutoff],
            |row| row.get(0),
        )?;
        Ok((
            drops.max(0) as u64,
            sessions.max(0) as u64,
            audit.max(0) as u64,
        ))
    }

    pub fn apply_retention(
        &self,
        now: i64,
        drop_cutoff: i64,
        audit_cutoff: i64,
    ) -> Result<(u64, u64, u64)> {
        let mut connection = self.open()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let sessions =
            tx.execute("DELETE FROM sessions WHERE expires_at < ?1", params![now])? as u64;
        let drops = tx.execute(
            "DELETE FROM drops WHERE expires_at < ?1 OR (revoked_at IS NOT NULL AND revoked_at < ?1)",
            params![drop_cutoff],
        )? as u64;
        let audit = tx.execute(
            "DELETE FROM audit_events WHERE occurred_at < ?1",
            params![audit_cutoff],
        )? as u64;
        tx.execute(
            "INSERT INTO audit_events (occurred_at,event,detail) VALUES (?1,'gc_completed',?2)",
            params![
                now,
                format!("drops={drops} sessions={sessions} audit={audit}")
            ],
        )?;
        tx.commit()?;
        Ok((drops, sessions, audit))
    }

    pub fn upsert_archive(&self, archive: &ArchiveRecord) -> Result<()> {
        let connection = self.open()?;
        let size =
            i64::try_from(archive.size).context("archive size exceeds SQLite integer range")?;
        connection.execute(
            "INSERT INTO archives (cache_key,drop_id,scope,relative_path,size,sha256_hex,sha256_base64,created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(cache_key) DO UPDATE SET relative_path=excluded.relative_path,size=excluded.size,sha256_hex=excluded.sha256_hex,sha256_base64=excluded.sha256_base64,created_at=excluded.created_at",
            params![archive.cache_key, archive.drop_id, archive.scope, archive.relative_path, size, archive.sha256_hex, archive.sha256_base64, archive.created_at],
        )?;
        Ok(())
    }

    pub fn get_archive(&self, cache_key: &str) -> Result<Option<ArchiveRecord>> {
        let connection = self.open()?;
        Ok(connection.query_row(
            "SELECT cache_key,drop_id,scope,relative_path,size,sha256_hex,sha256_base64,created_at FROM archives WHERE cache_key=?1",
            params![cache_key],
            |row| Ok(ArchiveRecord {
                cache_key: row.get(0)?, drop_id: row.get(1)?, scope: row.get(2)?, relative_path: row.get(3)?,
                size: get_u64(row, 4)?, sha256_hex: row.get(5)?, sha256_base64: row.get(6)?, created_at: row.get(7)?,
            }),
        ).optional()?)
    }

    pub fn archive_records(&self) -> Result<Vec<ArchiveRecord>> {
        let connection = self.open()?;
        let mut statement = connection.prepare(
            "SELECT cache_key,drop_id,scope,relative_path,size,sha256_hex,sha256_base64,created_at FROM archives",
        )?;
        Ok(statement
            .query_map([], |row| {
                Ok(ArchiveRecord {
                    cache_key: row.get(0)?,
                    drop_id: row.get(1)?,
                    scope: row.get(2)?,
                    relative_path: row.get(3)?,
                    size: get_u64(row, 4)?,
                    sha256_hex: row.get(5)?,
                    sha256_base64: row.get(6)?,
                    created_at: row.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn remove_archive_record(&self, cache_key: &str) -> Result<()> {
        self.open()?.execute(
            "DELETE FROM archives WHERE cache_key=?1",
            params![cache_key],
        )?;
        Ok(())
    }

    pub fn require_drop(&self, id: &str) -> Result<DropDetail> {
        self.get_drop(id)?
            .ok_or_else(|| anyhow::anyhow!("drop not found: {id}"))
    }
}

fn get_u64(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}
