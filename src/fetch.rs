use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use reqwest::{
    StatusCode, Url,
    blocking::{Client, Response},
    header::{CONTENT_LENGTH, CONTENT_TYPE, COOKIE, LOCATION, ORIGIN, SET_COOKIE},
    redirect::Policy,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

use crate::security::normalize_code;

const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MARKER_NAME: &str = ".frankenfile-fetch.json";

#[derive(Debug, Clone)]
pub struct FetchOptions {
    pub source: String,
    pub public_url: String,
    pub cache_dir: PathBuf,
    pub output: Option<PathBuf>,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResult {
    pub drop_id: String,
    pub directory: PathBuf,
    pub file_count: u64,
    pub directory_count: u64,
    pub total_bytes: u64,
    pub archive_sha256: String,
    pub cache_hit: bool,
}

#[derive(Debug)]
struct FetchTarget {
    base_url: Url,
    code: String,
    cache_key: String,
}

pub fn fetch(options: &FetchOptions) -> Result<FetchResult> {
    ensure!(
        (1024 * 1024..=64 * 1024 * 1024 * 1024).contains(&options.max_bytes),
        "maximum fetched bytes must be between 1 MiB and 64 GiB"
    );
    let target = parse_target(&options.source, &options.public_url)?;
    let final_dir = destination(options, &target.cache_key)?;
    if let Some(result) = cached_result(&final_dir)? {
        return Ok(result);
    }
    ensure!(
        !final_dir.exists(),
        "output already exists without a valid FrankenFile cache marker: {}",
        final_dir.display()
    );

    let parent = final_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("output directory needs a parent"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create cache parent {}", parent.display()))?;
    let staging = tempfile::Builder::new()
        .prefix(".frankenfile-fetch-")
        .tempdir_in(parent)
        .context("create fetch staging directory")?;
    let archive_path = staging.path().join("bundle.zip");

    let client = Client::builder()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(15 * 60))
        .user_agent("frankenfile-cli/0.1")
        .build()
        .context("build FrankenFile HTTP client")?;
    let (drop_id, cookie) = redeem(&client, &target)?;
    let archive_sha256 = download_bundle(
        &client,
        &target.base_url,
        &drop_id,
        &cookie,
        &archive_path,
        options.max_bytes,
    )?;
    let payload_dir = staging.path().join("payload");
    let (file_count, directory_count, total_bytes) =
        extract_bundle(&archive_path, &payload_dir, options.max_bytes)?;

    let result = FetchResult {
        drop_id,
        directory: final_dir.clone(),
        file_count,
        directory_count,
        total_bytes,
        archive_sha256,
        cache_hit: false,
    };
    let marker = serde_json::to_vec_pretty(&result)?;
    let marker_path = payload_dir.join(MARKER_NAME);
    let mut marker_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&marker_path)
        .context("create FrankenFile cache marker")?;
    marker_file.write_all(&marker)?;
    marker_file.sync_all()?;
    fs::rename(&payload_dir, &final_dir)
        .with_context(|| format!("publish fetched files at {}", final_dir.display()))?;
    File::open(parent)?.sync_all()?;
    Ok(result)
}

fn parse_target(source: &str, public_url: &str) -> Result<FetchTarget> {
    let public_url = public_url.trim_end_matches('/');
    let base_url = Url::parse(public_url).context("parse configured FrankenFile public URL")?;
    validate_base_url(&base_url)?;

    let code = if let Some(code) = normalize_code(source) {
        code
    } else {
        let link = Url::parse(source).context("expected a six-character code or share link")?;
        ensure!(
            same_origin(&link, &base_url),
            "share link origin does not match the configured FrankenFile server"
        );
        ensure!(
            link.query().is_none() && link.fragment().is_none(),
            "share link must not contain a query or fragment"
        );
        let expected_prefix = format!("{}/", base_url.path().trim_end_matches('/'));
        let suffix = link
            .path()
            .strip_prefix(&expected_prefix)
            .filter(|value| !value.contains('/'))
            .ok_or_else(|| anyhow::anyhow!("share link path is not a FrankenFile pickup link"))?;
        normalize_code(suffix)
            .ok_or_else(|| anyhow::anyhow!("share link pickup code is invalid"))?
    };

    let mut hasher = Sha256::new();
    hasher.update(public_url.as_bytes());
    hasher.update([0]);
    hasher.update(code.as_bytes());
    let cache_key = hex::encode(hasher.finalize());
    Ok(FetchTarget {
        base_url,
        code,
        cache_key,
    })
}

fn validate_base_url(url: &Url) -> Result<()> {
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "configured FrankenFile public URL must not contain credentials, a query, or a fragment"
    );
    ensure!(
        url.path().starts_with('/') && url.path() != "/",
        "configured FrankenFile public URL needs a non-root base path"
    );
    let loopback_http =
        url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
    ensure!(
        url.scheme() == "https" || loopback_http,
        "FrankenFile retrieval requires HTTPS (except loopback development)"
    );
    Ok(())
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
        && left.username().is_empty()
        && left.password().is_none()
}

fn destination(options: &FetchOptions, cache_key: &str) -> Result<PathBuf> {
    let requested = options
        .output
        .clone()
        .unwrap_or_else(|| options.cache_dir.join(&cache_key[..24]));
    ensure!(
        requested.file_name().is_some(),
        "output must name a directory, not a filesystem root"
    );
    let parent = requested
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("create output parent {}", parent.display()))?;
    let canonical_parent = fs::canonicalize(parent)
        .with_context(|| format!("resolve output parent {}", parent.display()))?;
    Ok(canonical_parent.join(
        requested
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("output directory name is missing"))?,
    ))
}

fn cached_result(directory: &Path) -> Result<Option<FetchResult>> {
    if !directory.exists() {
        return Ok(None);
    }
    ensure!(
        directory.is_dir(),
        "cached FrankenFile destination is not a directory: {}",
        directory.display()
    );
    let marker_path = directory.join(MARKER_NAME);
    if !marker_path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&marker_path).context("read FrankenFile cache marker")?;
    ensure!(
        bytes.len() <= 64 * 1024,
        "FrankenFile cache marker is too large"
    );
    let mut result: FetchResult =
        serde_json::from_slice(&bytes).context("parse FrankenFile cache marker")?;
    ensure!(
        result.directory == directory,
        "FrankenFile cache marker directory does not match its location"
    );
    result.cache_hit = true;
    Ok(Some(result))
}

fn redeem(client: &Client, target: &FetchTarget) -> Result<(String, String)> {
    let origin = target.base_url.origin().ascii_serialization();
    let redeem_url = endpoint(&target.base_url, "redeem")?;
    let response = client
        .post(redeem_url)
        .header(ORIGIN, origin)
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(format!("code={}", target.code))
        .send()
        .context("redeem FrankenFile pickup code")?;
    ensure!(
        response.status() == StatusCode::SEE_OTHER,
        "FrankenFile pickup was rejected or expired (HTTP {})",
        response.status()
    );

    let location = response
        .headers()
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("FrankenFile redemption omitted its destination"))?;
    let destination = target
        .base_url
        .join(location)
        .context("parse FrankenFile redemption destination")?;
    ensure!(
        same_origin(&destination, &target.base_url)
            && destination.query().is_none()
            && destination.fragment().is_none(),
        "FrankenFile redemption destination escaped the configured server"
    );
    let prefix = format!("{}/d/", target.base_url.path().trim_end_matches('/'));
    let drop_id = destination
        .path()
        .strip_prefix(&prefix)
        .filter(|value| {
            value.len() == 24
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        .ok_or_else(|| anyhow::anyhow!("FrankenFile redemption returned an invalid drop ID"))?
        .to_string();
    let cookie = session_cookie(&response)?;
    Ok((drop_id, cookie))
}

fn session_cookie(response: &Response) -> Result<String> {
    let mut cookies = response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| {
            let pair = value.to_str().ok()?.split(';').next()?.trim();
            let (name, token) = pair.split_once('=')?;
            let valid_name = matches!(name, "__Secure-ff_session" | "ff_session");
            let valid_token = token.len() == 43
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
            (valid_name && valid_token).then(|| pair.to_string())
        });
    let cookie = cookies
        .next()
        .ok_or_else(|| anyhow::anyhow!("FrankenFile redemption omitted its session"))?;
    ensure!(
        cookies.next().is_none(),
        "FrankenFile redemption returned ambiguous sessions"
    );
    Ok(cookie)
}

fn endpoint(base_url: &Url, suffix: &str) -> Result<Url> {
    let mut url = base_url.clone();
    url.set_path(&format!(
        "{}/{}",
        base_url.path().trim_end_matches('/'),
        suffix.trim_start_matches('/')
    ));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn download_bundle(
    client: &Client,
    base_url: &Url,
    drop_id: &str,
    cookie: &str,
    archive_path: &Path,
    max_bytes: u64,
) -> Result<String> {
    let bundle_url = endpoint(base_url, &format!("d/{drop_id}/bundle"))?;
    let mut response = client
        .get(bundle_url)
        .header(COOKIE, cookie)
        .send()
        .context("download FrankenFile bundle")?;
    ensure!(
        response.status() == StatusCode::OK,
        "FrankenFile bundle download returned HTTP {}",
        response.status()
    );
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    ensure!(
        content_type == Some("application/zip"),
        "FrankenFile bundle did not return a ZIP archive"
    );
    if let Some(length) = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    {
        ensure!(
            length <= max_bytes,
            "FrankenFile bundle exceeds the configured byte limit"
        );
    }

    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(archive_path)
        .context("create temporary FrankenFile bundle")?;
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let read = response
            .read(&mut buffer)
            .context("read FrankenFile bundle")?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| anyhow::anyhow!("FrankenFile bundle size overflow"))?;
        ensure!(
            total <= max_bytes,
            "FrankenFile bundle exceeds the configured byte limit"
        );
        output.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
    }
    output.sync_all()?;
    ensure!(total > 0, "FrankenFile bundle was empty");
    Ok(hex::encode(hasher.finalize()))
}

fn extract_bundle(
    archive_path: &Path,
    destination: &Path,
    max_bytes: u64,
) -> Result<(u64, u64, u64)> {
    fs::create_dir(destination).context("create extracted FrankenFile directory")?;
    fs::set_permissions(destination, fs::Permissions::from_mode(0o700))?;
    let mut archive =
        ZipArchive::new(File::open(archive_path)?).context("open FrankenFile ZIP bundle")?;
    ensure!(
        archive.len() <= MAX_ARCHIVE_ENTRIES,
        "FrankenFile bundle exceeds the entry-count limit"
    );

    let mut seen = HashSet::new();
    let mut file_count = 0u64;
    let mut directory_count = 0u64;
    let mut total_bytes = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .context("read FrankenFile ZIP entry")?;
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| anyhow::anyhow!("FrankenFile ZIP contains an unsafe path"))?
            .to_path_buf();
        ensure!(
            !relative.as_os_str().is_empty() && seen.insert(relative.clone()),
            "FrankenFile ZIP contains an empty or duplicate path"
        );
        if let Some(mode) = entry.unix_mode() {
            let kind = mode & 0o170000;
            ensure!(
                matches!(kind, 0 | 0o040000 | 0o100000),
                "FrankenFile ZIP contains a special file"
            );
        }
        let output_path = destination.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&output_path)?;
            fs::set_permissions(&output_path, fs::Permissions::from_mode(0o700))?;
            directory_count += 1;
            continue;
        }

        total_bytes = total_bytes
            .checked_add(entry.size())
            .ok_or_else(|| anyhow::anyhow!("FrankenFile extracted-size overflow"))?;
        ensure!(
            total_bytes <= max_bytes,
            "FrankenFile extracted content exceeds the configured byte limit"
        );
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&output_path)
            .with_context(|| format!("extract {}", relative.display()))?;
        let copied = std::io::copy(&mut entry, &mut output)?;
        ensure!(
            copied == entry.size(),
            "FrankenFile ZIP entry size changed during extraction"
        );
        output.sync_all()?;
        file_count += 1;
    }
    ensure!(file_count > 0, "FrankenFile bundle contains no files");
    File::open(destination)?.sync_all()?;
    Ok((file_count, directory_count, total_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pickup_target_accepts_only_the_configured_origin_and_path() {
        let public = "https://files.example.test/frankenfile";
        let bare = parse_target("ab-23-cd", public).unwrap();
        assert_eq!(bare.code, "AB23CD");
        let linked = parse_target("https://files.example.test/frankenfile/ab23cd", public).unwrap();
        assert_eq!(linked.code, "AB23CD");

        for rejected in [
            "https://attacker.test/frankenfile/AB23CD",
            "https://files.example.test/other/AB23CD",
            "https://files.example.test/frankenfile/AB23CD?redirect=x",
            "https://user@files.example.test/frankenfile/AB23CD",
        ] {
            assert!(parse_target(rejected, public).is_err(), "{rejected}");
        }
    }
}
