use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Directory,
}

impl EntryKind {
    pub fn as_i64(self) -> i64 {
        match self {
            Self::File => 1,
            Self::Directory => 2,
        }
    }

    pub fn from_i64(value: i64) -> rusqlite::Result<Self> {
        match value {
            1 => Ok(Self::File),
            2 => Ok(Self::Directory),
            _ => Err(rusqlite::Error::IntegralValueOutOfRange(0, value)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: i64,
    pub drop_id: String,
    pub path: String,
    pub kind: EntryKind,
    pub object_hash: Option<String>,
    pub sha256_hex: Option<String>,
    pub sha256_base64: Option<String>,
    pub size: u64,
    pub media_type: Option<String>,
    pub unix_mode: u32,
}

impl Entry {
    pub fn filename(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    pub fn is_top_level(&self) -> bool {
        !self.path.contains('/')
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropRecord {
    pub id: String,
    pub title: String,
    pub manifest_hash: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub code_expires_at: i64,
    pub revoked_at: Option<i64>,
    pub max_redemptions: Option<u32>,
    pub redemption_count: u32,
    pub total_bytes: u64,
    pub file_count: u64,
    pub directory_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropDetail {
    #[serde(flatten)]
    pub drop: DropRecord,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone)]
pub struct NewDrop {
    pub record: DropRecord,
    pub code_tag: Vec<u8>,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateResult {
    pub drop_id: String,
    pub code: String,
    pub url: String,
    pub title: String,
    pub created_at: i64,
    pub code_expires_at: i64,
    pub drop_expires_at: i64,
    pub total_bytes: u64,
    pub file_count: u64,
    pub directory_count: u64,
    pub manifest_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveRecord {
    pub cache_key: String,
    pub drop_id: String,
    pub scope: String,
    pub relative_path: String,
    pub size: u64,
    pub sha256_hex: String,
    pub sha256_base64: String,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub enum RotateOutcome {
    Rotated(DropRecord),
    NotFound,
    Ambiguous,
}

#[derive(Debug, Clone)]
pub enum RedeemResult {
    Success {
        drop_id: String,
        session_expires_at: i64,
    },
    Rejected,
}

#[derive(Debug, Clone, Serialize)]
pub struct GcReport {
    pub dry_run: bool,
    pub retention_seconds: u64,
    pub purgable_drops: u64,
    pub expired_sessions: u64,
    pub old_audit_events: u64,
    pub unreachable_objects: u64,
    pub unreachable_object_bytes: u64,
    pub stale_archives: u64,
    pub stale_archive_bytes: u64,
    pub deleted_files: u64,
    pub deleted_bytes: u64,
    pub purged_drops: u64,
    pub purged_sessions: u64,
    pub purged_audit_events: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub healthy: bool,
    pub database_check: String,
    pub drops_checked: u64,
    pub objects_checked: u64,
    pub object_bytes_checked: u64,
    pub missing_objects: Vec<String>,
    pub corrupt_objects: Vec<String>,
    pub deep: bool,
}
