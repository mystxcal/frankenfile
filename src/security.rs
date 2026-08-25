use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

use anyhow::{Context, Result, bail};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use zeroize::{Zeroize, ZeroizeOnDrop};

const ATTR_CHAR_EXCLUSIONS: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'{')
    .add(b'}');

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct MasterKey([u8; 32]);

impl MasterKey {
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            let metadata = fs::symlink_metadata(path)
                .with_context(|| format!("inspect master key {}", path.display()))?;
            if !metadata.file_type().is_file() {
                bail!("master key is not a regular file: {}", path.display());
            }
            let mode = metadata.permissions().mode() & 0o777;
            if mode & 0o007 != 0 {
                bail!("master key is accessible to other users (mode {mode:o})");
            }
            let mut bytes = Vec::with_capacity(32);
            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(path)
                .with_context(|| format!("open master key {}", path.display()))?
                .read_to_end(&mut bytes)?;
            if bytes.len() != 32 {
                bytes.zeroize();
                bail!("master key must contain exactly 32 bytes");
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            bytes.zeroize();
            return Ok(Self(key));
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).context("obtain OS randomness for master key")?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o640)
            .open(path)
            .with_context(|| format!("create master key {}", path.display()))?;
        file.write_all(&key)?;
        file.sync_all()?;
        Ok(Self(key))
    }

    pub fn tag(&self, domain: &'static [u8], value: &[u8]) -> Vec<u8> {
        let mut hasher = blake3::Hasher::new_keyed(&self.0);
        hasher.update(domain);
        hasher.update(&[0]);
        hasher.update(value);
        hasher.finalize().as_bytes().to_vec()
    }

    pub fn code_tag(&self, code: &str) -> Vec<u8> {
        self.tag(b"frankenfile/code/v1", code.as_bytes())
    }

    pub fn session_tag(&self, token: &str) -> Vec<u8> {
        self.tag(b"frankenfile/session/v1", token.as_bytes())
    }

    pub fn source_tag(&self, source: &str) -> Vec<u8> {
        self.tag(b"frankenfile/source/v1", source.as_bytes())
    }

    pub fn admin_tag(&self, password: &str) -> Vec<u8> {
        self.tag(b"frankenfile/admin/v1", password.as_bytes())
    }

    pub fn admin_session_tag(&self, token: &str) -> Vec<u8> {
        self.tag(b"frankenfile/admin-session/v1", token.as_bytes())
    }
}

pub fn random_token(bytes: usize) -> Result<String> {
    let mut value = vec![0u8; bytes];
    getrandom::fill(&mut value).context("obtain OS randomness")?;
    let encoded = URL_SAFE_NO_PAD.encode(&value);
    value.zeroize();
    Ok(encoded)
}

pub fn random_public_id() -> Result<String> {
    random_token(18)
}

pub fn random_session() -> Result<String> {
    random_token(32)
}

/// Pickup-code alphabets. Codes are always six characters; the alphanumeric
/// style excludes visually ambiguous glyphs (0/O, 1/I/L, U/V confusion).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodeStyle {
    #[default]
    Alphanumeric,
    Digits,
}

pub const CODE_LENGTH: usize = 6;
const CODE_ALPHABET: &[u8; 30] = b"ABCDEFGHJKMNPQRSTVWXYZ23456789";

pub fn random_code(style: CodeStyle) -> Result<String> {
    match style {
        CodeStyle::Digits => {
            const SPACE: u64 = 1_000_000;
            const ZONE: u64 = ((u32::MAX as u64 + 1) / SPACE) * SPACE;
            loop {
                let mut bytes = [0u8; 4];
                getrandom::fill(&mut bytes).context("obtain OS randomness for code")?;
                let value = u32::from_le_bytes(bytes) as u64;
                if value < ZONE {
                    return Ok(format!("{:06}", value % SPACE));
                }
            }
        }
        CodeStyle::Alphanumeric => {
            const ZONE: u8 = (u8::MAX / CODE_ALPHABET.len() as u8) * CODE_ALPHABET.len() as u8;
            let mut code = String::with_capacity(CODE_LENGTH);
            while code.len() < CODE_LENGTH {
                let mut bytes = [0u8; 16];
                getrandom::fill(&mut bytes).context("obtain OS randomness for code")?;
                for byte in bytes {
                    if byte < ZONE && code.len() < CODE_LENGTH {
                        code.push(
                            CODE_ALPHABET[(byte % CODE_ALPHABET.len() as u8) as usize] as char,
                        );
                    }
                }
            }
            Ok(code)
        }
    }
}

/// Canonicalize a submitted pickup code: trim, drop separators people add when
/// reading codes aloud, and uppercase. Returns `None` unless the result is
/// exactly six ASCII alphanumerics, so legacy digit codes stay redeemable.
pub fn normalize_code(input: &str) -> Option<String> {
    let compact: String = input
        .trim()
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '\u{2010}'..='\u{2015}'))
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if compact.len() == CODE_LENGTH && compact.bytes().all(|b| b.is_ascii_alphanumeric()) {
        Some(compact)
    } else {
        None
    }
}

pub fn random_delay_ms() -> u64 {
    let mut bytes = [0u8; 2];
    if getrandom::fill(&mut bytes).is_err() {
        return 375;
    }
    300 + (u16::from_le_bytes(bytes) as u64 % 201)
}

pub fn normalize_base_path(value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed == "/" {
        return Ok(String::new());
    }
    let normalized = format!("/{}", trimmed.trim_matches('/'));
    if normalized.contains("//") || normalized.contains('?') || normalized.contains('#') {
        bail!("invalid base path");
    }
    Ok(normalized)
}

pub fn safe_ascii_filename(name: &str, fallback: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ') {
            out.push(c);
        } else if c.is_ascii() {
            out.push('_');
        }
    }
    let out = out.trim().trim_matches('.').to_string();
    if out.is_empty() {
        fallback.to_string()
    } else {
        out.chars().take(100).collect()
    }
}

pub fn content_disposition(name: &str) -> String {
    let fallback = safe_ascii_filename(name, "download").replace('"', "_");
    let encoded = utf8_percent_encode(name, ATTR_CHAR_EXCLUSIONS).to_string();
    format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}

pub fn validate_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 120 {
        bail!("title must contain 1 to 120 characters");
    }
    if title.chars().any(|c| c.is_control()) {
        bail!("title cannot contain control characters");
    }
    Ok(title.to_string())
}
