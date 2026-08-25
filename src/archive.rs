use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{BufReader, Read},
    os::unix::{
        ffi::OsStrExt,
        fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use zip::{CompressionMethod, DateTime, ZipWriter, write::SimpleFileOptions};

use crate::{
    db::Database,
    model::{ArchiveRecord, DropDetail, Entry, EntryKind},
    storage::Storage,
};

const ARCHIVE_FORMAT_VERSION: &str = "zip-v1-zip8.6.0-flate1.1.9-deflate6-fixed1980";
const DISK_RESERVE_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveScope {
    Whole,
    Folder(String),
}

impl ArchiveScope {
    pub fn label(&self) -> String {
        match self {
            Self::Whole => "whole".to_string(),
            Self::Folder(path) => format!("folder:{path}"),
        }
    }
}

pub fn cache_key(manifest_hash: &str, scope: &ArchiveScope) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ARCHIVE_FORMAT_VERSION.as_bytes());
    hasher.update(&[0]);
    hasher.update(manifest_hash.as_bytes());
    hasher.update(&[0]);
    hasher.update(scope.label().as_bytes());
    hasher.finalize().to_hex().to_string()
}

pub fn materialize(
    storage: &Storage,
    database: &Database,
    detail: &DropDetail,
    scope: &ArchiveScope,
) -> Result<ArchiveRecord> {
    let key = cache_key(&detail.drop.manifest_hash, scope);
    if let Some(existing) = database.get_archive(&key)? {
        let path = storage.archive_path(&existing.relative_path)?;
        if path.is_file() && fs::metadata(&path)?.len() == existing.size {
            return Ok(existing);
        }
    }

    let selected = selected_entries(detail, scope)?;
    let logical_bytes = selected
        .iter()
        .try_fold(0u64, |total, entry| total.checked_add(entry.size))
        .ok_or_else(|| anyhow::anyhow!("archive logical size overflow"))?;
    let free = available_space(storage.archives_dir())?;
    let required = logical_bytes
        .saturating_add(64 * 1024 * 1024)
        .saturating_add(DISK_RESERVE_BYTES);
    ensure!(
        free >= required,
        "insufficient free space to safely materialize archive (need {}, have {})",
        required,
        free
    );

    let relative_path = format!("{key}.zip");
    let final_path = storage.archive_path(&relative_path)?;
    let temp_path = storage.temp_dir().join(format!(
        "archive-{}.tmp",
        crate::security::random_token(12)?
    ));
    let result = build_zip(storage, &temp_path, &selected);
    if let Err(error) = result {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    let completed = result?;
    completed.sync_all()?;
    drop(completed);
    fs::rename(&temp_path, &final_path)?;
    fs::set_permissions(&final_path, fs::Permissions::from_mode(0o640))?;
    File::open(storage.archives_dir())?.sync_all()?;

    let (sha256_hex, sha256_base64, size) = sha256_file(&final_path)?;
    let record = ArchiveRecord {
        cache_key: key,
        drop_id: detail.drop.id.clone(),
        scope: scope.label(),
        relative_path,
        size,
        sha256_hex,
        sha256_base64,
        created_at: OffsetDateTime::now_utc().unix_timestamp(),
    };
    database.upsert_archive(&record)?;
    Ok(record)
}

fn selected_entries<'a>(detail: &'a DropDetail, scope: &ArchiveScope) -> Result<Vec<&'a Entry>> {
    match scope {
        ArchiveScope::Whole => Ok(detail.entries.iter().collect()),
        ArchiveScope::Folder(folder) => {
            let valid_root = detail.entries.iter().any(|entry| {
                entry.path == *folder && entry.kind == EntryKind::Directory && entry.is_top_level()
            });
            if !valid_root {
                bail!("folder archive scope is not a top-level manifest directory");
            }
            let prefix = format!("{folder}/");
            Ok(detail
                .entries
                .iter()
                .filter(|entry| entry.path == *folder || entry.path.starts_with(&prefix))
                .collect())
        }
    }
}

fn build_zip(storage: &Storage, path: &Path, entries: &[&Entry]) -> Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o640)
        .open(path)?;
    let mut archive = ZipWriter::new(file);
    let timestamp = DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0)
        .map_err(|_| anyhow::anyhow!("construct fixed ZIP timestamp"))?;

    for entry in entries {
        let zip_path = if entry.kind == EntryKind::Directory {
            format!("{}/", entry.path.trim_end_matches('/'))
        } else {
            entry.path.clone()
        };
        if entry.kind == EntryKind::Directory {
            let options = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .last_modified_time(timestamp)
                .unix_permissions(0o755);
            archive.add_directory(zip_path, options)?;
            continue;
        }

        let compression = if should_store(&entry.path) {
            CompressionMethod::Stored
        } else {
            CompressionMethod::Deflated
        };
        let options = SimpleFileOptions::default()
            .compression_method(compression)
            .compression_level(if compression == CompressionMethod::Deflated {
                Some(6)
            } else {
                None
            })
            .last_modified_time(timestamp)
            .unix_permissions(0o644)
            .large_file(entry.size > u32::MAX as u64);
        archive.start_file(zip_path, options)?;
        let object = storage.object_path(
            entry
                .object_hash
                .as_deref()
                .context("file object identity")?,
        )?;
        let metadata = fs::symlink_metadata(&object)?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "unsafe object in archive cache"
        );
        ensure!(
            metadata.len() == entry.size,
            "object size changed before archive build"
        );
        let mut source = BufReader::new(
            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(object)?,
        );
        std::io::copy(&mut source, &mut archive)?;
    }
    Ok(archive.finish()?)
}

fn should_store(path: &str) -> bool {
    let extension = Path::new(path)
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "zip"
            | "gz"
            | "bz2"
            | "xz"
            | "7z"
            | "rar"
            | "zst"
            | "jpg"
            | "jpeg"
            | "png"
            | "gif"
            | "webp"
            | "avif"
            | "heic"
            | "mp3"
            | "aac"
            | "ogg"
            | "flac"
            | "mp4"
            | "mkv"
            | "mov"
            | "webm"
            | "pdf"
            | "docx"
            | "xlsx"
            | "pptx"
            | "woff"
            | "woff2"
    )
}

fn sha256_file(path: &Path) -> Result<(String, String, u64)> {
    let mut file = BufReader::new(File::open(path)?);
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        size = size
            .checked_add(read as u64)
            .context("archive size overflow")?;
    }
    let bytes = digest.finalize();
    Ok((hex::encode(bytes), STANDARD.encode(bytes), size))
}

fn available_space(path: PathBuf) -> Result<u64> {
    let bytes = path.as_os_str().as_bytes();
    let c_path = CString::new(bytes).context("storage path contains NUL")?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("inspect available archive space");
    }
    Ok((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
}
