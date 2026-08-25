use std::{
    collections::{BTreeSet, HashSet},
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Serialize;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use walkdir::WalkDir;

use crate::{
    db::Database,
    model::{
        CreateResult, DoctorReport, DropRecord, Entry, EntryKind, GcReport, NewDrop, RotateOutcome,
    },
    security::{CodeStyle, MasterKey, random_code, random_public_id, validate_title},
};

const MAX_FILES: u64 = 100_000;
const MAX_LOGICAL_BYTES: u64 = 100 * 1024 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Storage {
    root: PathBuf,
}

struct TempCleanup {
    path: PathBuf,
    armed: bool,
}

impl TempCleanup {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempCleanup {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[derive(Debug, Clone)]
pub enum ReissueOutcome {
    Reissued(CreateResult),
    NotFound,
    Ambiguous,
}

#[derive(Debug, Clone)]
pub struct CreateOptions {
    pub title: Option<String>,
    pub code_ttl: Duration,
    pub drop_ttl: Duration,
    pub max_redemptions: Option<u32>,
    pub public_url: String,
    pub code_style: CodeStyle,
}

#[derive(Serialize)]
struct ManifestEntry<'a> {
    path: &'a str,
    kind: EntryKind,
    object_hash: &'a Option<String>,
    sha256_hex: &'a Option<String>,
    size: u64,
    media_type: &'a Option<String>,
    unix_mode: u32,
}

impl Storage {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn database_path(&self) -> PathBuf {
        self.root.join("frankenfile.sqlite3")
    }

    pub fn master_key_path(&self) -> PathBuf {
        self.root.join("master.key")
    }

    pub fn objects_dir(&self) -> PathBuf {
        self.root.join("objects")
    }

    pub fn archives_dir(&self) -> PathBuf {
        self.root.join("archives")
    }

    pub fn temp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    pub fn prepare(&self) -> Result<()> {
        ensure_directory(&self.root, 0o2770)?;
        ensure_directory(&self.objects_dir(), 0o2770)?;
        ensure_directory(&self.archives_dir(), 0o2770)?;
        ensure_directory(&self.temp_dir(), 0o2770)?;
        Ok(())
    }

    pub fn object_path(&self, hash: &str) -> Result<PathBuf> {
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("invalid object identity");
        }
        Ok(self.objects_dir().join(&hash[..2]).join(hash))
    }

    pub fn archive_path(&self, relative: &str) -> Result<PathBuf> {
        let path = Path::new(relative);
        if path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            bail!("invalid archive cache path");
        }
        Ok(self.archives_dir().join(path))
    }

    #[cfg(test)]
    pub fn root_for_test(&self) -> &Path {
        &self.root
    }

    pub fn create_drop(
        &self,
        database: &Database,
        key: &MasterKey,
        inputs: &[PathBuf],
        options: &CreateOptions,
    ) -> Result<CreateResult> {
        self.prepare()?;
        ensure!(!inputs.is_empty(), "select at least one file or directory");
        ensure!(
            options.code_ttl >= Duration::from_secs(60),
            "code TTL must be at least 1 minute"
        );
        ensure!(
            options.code_ttl <= Duration::from_secs(24 * 3600),
            "code TTL cannot exceed 24 hours"
        );
        ensure!(
            options.drop_ttl >= options.code_ttl,
            "drop TTL must be at least the code TTL"
        );
        ensure!(
            options.drop_ttl <= Duration::from_secs(30 * 24 * 3600),
            "drop TTL cannot exceed 30 days"
        );
        if matches!(options.max_redemptions, Some(0)) {
            bail!("max redemptions must be at least 1");
        }

        let mut entries = Vec::new();
        let mut roots = HashSet::new();
        for input in inputs {
            let absolute = fs::canonicalize(input)
                .with_context(|| format!("resolve input {}", input.display()))?;
            let original = fs::symlink_metadata(input)
                .with_context(|| format!("inspect input {}", input.display()))?;
            if original.file_type().is_symlink() {
                bail!("symlink inputs are refused: {}", input.display());
            }
            let root_name =
                safe_component(absolute.file_name().and_then(|v| v.to_str()).ok_or_else(
                    || anyhow::anyhow!("input needs a UTF-8 filename: {}", input.display()),
                )?)?;
            if !roots.insert(root_name.clone()) {
                bail!("two selected roots have the same name: {root_name}");
            }

            if original.is_file() {
                entries.push(self.capture_file(&absolute, &root_name)?);
            } else if original.is_dir() {
                entries.push(directory_entry(&root_name));
                for item in WalkDir::new(&absolute)
                    .follow_links(false)
                    .min_depth(1)
                    .sort_by_file_name()
                {
                    let item = item.with_context(|| format!("walk {}", absolute.display()))?;
                    let source = item.path();
                    let metadata = fs::symlink_metadata(source)
                        .with_context(|| format!("inspect {}", source.display()))?;
                    if metadata.file_type().is_symlink() {
                        bail!("symlinks are refused: {}", source.display());
                    }
                    let relative = source
                        .strip_prefix(&absolute)
                        .context("derive relative source path")?;
                    let manifest_path = manifest_path(&root_name, relative)?;
                    if metadata.is_dir() {
                        entries.push(directory_entry(&manifest_path));
                    } else if metadata.is_file() {
                        entries.push(self.capture_file(source, &manifest_path)?);
                    } else {
                        bail!("special files are refused: {}", source.display());
                    }
                    if entries.len() as u64 > MAX_FILES {
                        bail!("drop exceeds the {MAX_FILES} entry safety limit");
                    }
                }
            } else {
                bail!("special files are refused: {}", input.display());
            }
        }

        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let mut seen = BTreeSet::new();
        for entry in &entries {
            if !seen.insert(&entry.path) {
                bail!("duplicate manifest path: {}", entry.path);
            }
        }
        let file_count = entries.iter().filter(|e| e.kind == EntryKind::File).count() as u64;
        let directory_count = entries
            .iter()
            .filter(|e| e.kind == EntryKind::Directory)
            .count() as u64;
        let total_bytes = entries
            .iter()
            .try_fold(0u64, |sum, e| sum.checked_add(e.size))
            .ok_or_else(|| anyhow::anyhow!("logical size overflow"))?;
        ensure!(
            total_bytes <= MAX_LOGICAL_BYTES,
            "drop exceeds the 100 GiB logical-size safety limit"
        );

        let manifest = entries
            .iter()
            .map(|entry| ManifestEntry {
                path: &entry.path,
                kind: entry.kind,
                object_hash: &entry.object_hash,
                sha256_hex: &entry.sha256_hex,
                size: entry.size,
                media_type: &entry.media_type,
                unix_mode: entry.unix_mode,
            })
            .collect::<Vec<_>>();
        let manifest_bytes = serde_json::to_vec(&manifest)?;
        let manifest_hash = blake3::hash(&manifest_bytes).to_hex().to_string();
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let code_expires_at = now + i64::try_from(options.code_ttl.as_secs())?;
        let expires_at = now + i64::try_from(options.drop_ttl.as_secs())?;
        let default_title = if roots.len() == 1 {
            roots
                .iter()
                .next()
                .cloned()
                .unwrap_or_else(|| "Airdrop".to_string())
        } else {
            format!("Airdrop · {} roots", roots.len())
        };
        let title = validate_title(options.title.as_deref().unwrap_or(&default_title))?;

        let (code, code_tag) = (0..128)
            .find_map(|_| {
                let code = random_code(options.code_style).ok()?;
                let tag = key.code_tag(&code);
                match database.code_tag_exists(&tag) {
                    Ok(false) => Some((code, tag)),
                    _ => None,
                }
            })
            .ok_or_else(|| anyhow::anyhow!("could not allocate a unique active code"))?;
        let drop_id = random_public_id()?;
        for entry in &mut entries {
            entry.drop_id = drop_id.clone();
        }
        let record = DropRecord {
            id: drop_id.clone(),
            title: title.clone(),
            manifest_hash: manifest_hash.clone(),
            created_at: now,
            expires_at,
            code_expires_at,
            revoked_at: None,
            max_redemptions: options.max_redemptions,
            redemption_count: 0,
            total_bytes,
            file_count,
            directory_count,
        };
        database.insert_drop(&NewDrop {
            record,
            code_tag,
            entries,
        })?;

        Ok(CreateResult {
            drop_id,
            code,
            url: options.public_url.trim_end_matches('/').to_string(),
            title,
            created_at: now,
            code_expires_at,
            drop_expires_at: expires_at,
            total_bytes,
            file_count,
            directory_count,
            manifest_hash,
        })
    }

    /// Allocate a fresh pickup code for an active drop (addressed by ID or
    /// unique ID prefix) and atomically retire the old one. Sessions already
    /// redeemed stay valid; only the rendezvous code changes.
    pub fn reissue_code(
        &self,
        database: &Database,
        key: &MasterKey,
        reference: &str,
        code_ttl: Duration,
        code_style: CodeStyle,
        public_url: &str,
    ) -> Result<ReissueOutcome> {
        ensure!(
            code_ttl >= Duration::from_secs(60),
            "code TTL must be at least 1 minute"
        );
        ensure!(
            code_ttl <= Duration::from_secs(24 * 3600),
            "code TTL cannot exceed 24 hours"
        );
        let reference = reference.trim();
        ensure!(
            reference.len() >= 8
                && reference
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')),
            "drop reference must be at least 8 characters of its ID"
        );
        let (code, code_tag) = (0..128)
            .find_map(|_| {
                let code = random_code(code_style).ok()?;
                let tag = key.code_tag(&code);
                match database.code_tag_exists(&tag) {
                    Ok(false) => Some((code, tag)),
                    _ => None,
                }
            })
            .ok_or_else(|| anyhow::anyhow!("could not allocate a unique active code"))?;
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let outcome = database.rotate_code(
            reference,
            &code_tag,
            now,
            i64::try_from(code_ttl.as_secs())?,
        )?;
        Ok(match outcome {
            RotateOutcome::Rotated(record) => ReissueOutcome::Reissued(CreateResult {
                drop_id: record.id,
                code,
                url: public_url.trim_end_matches('/').to_string(),
                title: record.title,
                created_at: record.created_at,
                code_expires_at: record.code_expires_at,
                drop_expires_at: record.expires_at,
                total_bytes: record.total_bytes,
                file_count: record.file_count,
                directory_count: record.directory_count,
                manifest_hash: record.manifest_hash,
            }),
            RotateOutcome::NotFound => ReissueOutcome::NotFound,
            RotateOutcome::Ambiguous => ReissueOutcome::Ambiguous,
        })
    }

    fn capture_file(&self, source: &Path, manifest_path: &str) -> Result<Entry> {
        let before = fs::symlink_metadata(source)
            .with_context(|| format!("inspect {}", source.display()))?;
        ensure!(
            before.is_file() && !before.file_type().is_symlink(),
            "only non-symlink regular files can be captured"
        );
        let mut input = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(source)
            .with_context(|| format!("open {} without following links", source.display()))?;
        let opened = input.metadata()?;
        ensure!(
            same_identity(&before, &opened),
            "source was replaced while opening: {}",
            source.display()
        );

        let temp_name = format!("object-{}.tmp", crate::security::random_token(12)?);
        let temp_path = self.temp_dir().join(temp_name);
        let mut cleanup = TempCleanup::new(temp_path.clone());
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o640)
            .open(&temp_path)?;
        let mut blake = blake3::Hasher::new();
        let mut sha = Sha256::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        let mut copied = 0u64;
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            blake.update(&buffer[..read]);
            sha.update(&buffer[..read]);
            output.write_all(&buffer[..read])?;
            copied = copied
                .checked_add(read as u64)
                .ok_or_else(|| anyhow::anyhow!("file size overflow"))?;
            if copied > MAX_LOGICAL_BYTES {
                bail!(
                    "single file exceeds the 100 GiB safety limit: {}",
                    source.display()
                );
            }
        }
        output.sync_all()?;
        let after = input.metadata()?;
        if !stable_during_copy(&opened, &after) || copied != after.len() {
            bail!("source changed during capture: {}", source.display());
        }

        let object_hash = blake.finalize().to_hex().to_string();
        let sha_bytes = sha.finalize();
        let sha256_hex = hex::encode(sha_bytes);
        let sha256_base64 = STANDARD.encode(sha_bytes);
        let object_path = self.object_path(&object_hash)?;
        let parent = object_path.parent().context("object parent")?;
        ensure_directory(parent, 0o2770)?;
        if object_path.exists() {
            let existing = fs::symlink_metadata(&object_path)?;
            ensure!(
                existing.is_file() && !existing.file_type().is_symlink(),
                "existing object path is unsafe"
            );
            ensure!(
                existing.len() == copied,
                "existing object length disagrees with its identity"
            );
            let existing_hash = blake3_file(&object_path)?;
            ensure!(
                existing_hash == object_hash,
                "existing object failed content-identity verification"
            );
            fs::remove_file(&temp_path)?;
            cleanup.disarm();
        } else {
            fs::rename(&temp_path, &object_path)?;
            cleanup.disarm();
            fs::set_permissions(&object_path, fs::Permissions::from_mode(0o640))?;
            sync_directory(parent)?;
        }

        Ok(Entry {
            id: 0,
            drop_id: String::new(),
            path: manifest_path.to_string(),
            kind: EntryKind::File,
            object_hash: Some(object_hash),
            sha256_hex: Some(sha256_hex),
            sha256_base64: Some(sha256_base64),
            size: copied,
            media_type: Some(
                mime_guess::from_path(manifest_path)
                    .first_or_octet_stream()
                    .essence_str()
                    .to_string(),
            ),
            unix_mode: 0o644,
        })
    }

    pub fn doctor(&self, database: &Database, deep: bool) -> Result<DoctorReport> {
        let database_check = database.quick_check()?;
        let drops = database.list_drops(true, OffsetDateTime::now_utc().unix_timestamp())?;
        let references = database.referenced_objects()?;
        let mut missing = Vec::new();
        let mut corrupt = Vec::new();
        let mut bytes = 0u64;
        for (hash, expected_size) in &references {
            let path = self.object_path(hash)?;
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
                    bytes = bytes.saturating_add(meta.len());
                    if meta.len() != *expected_size || (deep && blake3_file(&path)? != *hash) {
                        corrupt.push(hash.clone());
                    }
                }
                _ => missing.push(hash.clone()),
            }
        }
        Ok(DoctorReport {
            healthy: database_check == "ok" && missing.is_empty() && corrupt.is_empty(),
            database_check,
            drops_checked: drops.len() as u64,
            objects_checked: references.len() as u64,
            object_bytes_checked: bytes,
            missing_objects: missing,
            corrupt_objects: corrupt,
            deep,
        })
    }

    pub fn garbage_collect(
        &self,
        database: &Database,
        execute: bool,
        now: i64,
        retention: Duration,
    ) -> Result<GcReport> {
        let retention_seconds = retention.as_secs();
        let retention_i64 =
            i64::try_from(retention_seconds).context("retention duration overflow")?;
        let drop_cutoff = now.saturating_sub(retention_i64);
        let audit_cutoff = now.saturating_sub(90 * 24 * 3600);
        let (purgable_drops, expired_sessions, old_audit_events) =
            database.retention_counts(now, drop_cutoff, audit_cutoff)?;
        let referenced: HashSet<String> = database
            .referenced_objects_after_retention(drop_cutoff)?
            .into_iter()
            .map(|(hash, _)| hash)
            .collect();
        let mut object_candidates = Vec::new();
        if self.objects_dir().exists() {
            for entry in WalkDir::new(self.objects_dir())
                .min_depth(1)
                .follow_links(false)
            {
                let entry = entry?;
                if entry.file_type().is_file() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.len() == 64 && !referenced.contains(&name) {
                        object_candidates
                            .push((entry.path().to_path_buf(), entry.metadata()?.len()));
                    }
                }
            }
        }
        let mut archive_candidates = Vec::new();
        for archive in database.archive_records()? {
            let inactive = database
                .get_drop(&archive.drop_id)?
                .map(|d| d.drop.revoked_at.is_some() || d.drop.expires_at <= now)
                .unwrap_or(true);
            let path = self.archive_path(&archive.relative_path)?;
            if inactive || !path.exists() {
                archive_candidates.push((archive.cache_key, path, archive.size));
            }
        }
        let mut deleted_files = 0u64;
        let mut deleted_bytes = 0u64;
        let mut purged_drops = 0u64;
        let mut purged_sessions = 0u64;
        let mut purged_audit_events = 0u64;
        if execute {
            (purged_drops, purged_sessions, purged_audit_events) =
                database.apply_retention(now, drop_cutoff, audit_cutoff)?;
            for (path, size) in &object_candidates {
                fs::remove_file(path)?;
                deleted_files += 1;
                deleted_bytes = deleted_bytes.saturating_add(*size);
            }
            for (key, path, size) in &archive_candidates {
                if path.exists() {
                    fs::remove_file(path)?;
                }
                database.remove_archive_record(key)?;
                deleted_files += 1;
                deleted_bytes = deleted_bytes.saturating_add(*size);
            }
        }
        Ok(GcReport {
            dry_run: !execute,
            retention_seconds,
            purgable_drops,
            expired_sessions,
            old_audit_events,
            unreachable_objects: object_candidates.len() as u64,
            unreachable_object_bytes: object_candidates.iter().map(|(_, n)| *n).sum(),
            stale_archives: archive_candidates.len() as u64,
            stale_archive_bytes: archive_candidates.iter().map(|(_, _, n)| *n).sum(),
            deleted_files,
            deleted_bytes,
            purged_drops,
            purged_sessions,
            purged_audit_events,
        })
    }
}

fn ensure_directory(path: &Path, mode: u32) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("create directory {}", path.display()))?;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "unsafe storage directory: {}",
        path.display()
    );
    // Object shards may be created by either the root-operated CLI or the
    // restricted web service. Both use the same group and mode, but only the
    // owner may chmod an existing shard. Avoid mutating already-correct shared
    // directories while still repairing a mode that is actually wrong.
    if directory_mode_needs_update(&metadata, mode) {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

fn directory_mode_needs_update(metadata: &fs::Metadata, expected: u32) -> bool {
    metadata.permissions().mode() & 0o7777 != expected
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

pub fn safe_component(value: &str) -> Result<String> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, '/' | '\\'))
        || value
            .chars()
            .any(|c| matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
    {
        bail!("unsafe path component: {value:?}");
    }
    Ok(value.to_string())
}

fn manifest_path(root: &str, relative: &Path) -> Result<String> {
    let mut parts = vec![root.to_string()];
    for component in relative.components() {
        match component {
            Component::Normal(value) => {
                let text = value
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("filenames must be valid UTF-8"))?;
                parts.push(safe_component(text)?);
            }
            _ => bail!("unsafe relative path"),
        }
    }
    Ok(parts.join("/"))
}

fn directory_entry(path: &str) -> Entry {
    Entry {
        id: 0,
        drop_id: String::new(),
        path: path.to_string(),
        kind: EntryKind::Directory,
        object_hash: None,
        sha256_hex: None,
        sha256_base64: None,
        size: 0,
        media_type: None,
        unix_mode: 0o755,
    }
}

fn same_identity(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.dev() == b.dev() && a.ino() == b.ino() && a.file_type().is_file() && b.file_type().is_file()
}

fn stable_during_copy(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    same_identity(a, b)
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}

pub fn blake3_file(path: &Path) -> Result<String> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod directory_tests {
    use super::*;

    #[test]
    fn correct_shared_directory_mode_needs_no_mutation() {
        let temp = tempfile::tempdir().expect("temporary directory");
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o2770))
            .expect("set directory mode");
        let metadata = fs::symlink_metadata(temp.path()).expect("inspect directory");

        assert!(!directory_mode_needs_update(&metadata, 0o2770));
        assert!(directory_mode_needs_update(&metadata, 0o2775));
    }
}
