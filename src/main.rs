mod archive;
mod db;
mod fetch;
mod model;
mod security;
mod storage;
#[cfg(test)]
mod tests;
mod ui;
mod web;

use std::{net::SocketAddr, path::PathBuf, process::ExitCode, time::Duration};

use anyhow::{Result, bail, ensure};
use clap::{Parser, Subcommand};
use ipnet::IpNet;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tracing_subscriber::EnvFilter;

use crate::{
    db::Database,
    security::{MasterKey, normalize_base_path, random_token},
    storage::{CreateOptions, Storage},
    web::ServeSettings,
};

/// Loopback default that matches the default bind address and base path, so a
/// fresh checkout works with no configuration. Every deployment reachable from
/// anywhere else must set `--public-url` / `FRANKENFILE_PUBLIC_URL` to its own
/// origin: the value is what share links are built from, and the CLI refuses to
/// talk to any other origin.
const DEFAULT_PUBLIC_URL: &str = "http://127.0.0.1:18766/frankenfile";

#[derive(Debug, Parser)]
#[command(
    name = "frankenfile",
    version,
    about = "Immutable six-character file airdrops"
)]
struct Cli {
    /// Durable state directory.
    #[arg(
        long,
        env = "FRANKENFILE_DATA_DIR",
        default_value = "/var/lib/frankenfile",
        global = true
    )]
    data_dir: PathBuf,

    /// Emit stable JSON output where supported.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the authenticated HTTP service.
    Serve {
        #[arg(long, default_value = "127.0.0.1:18766")]
        bind: SocketAddr,
        #[arg(long, default_value = "/frankenfile")]
        base_path: String,
        #[arg(long, env = "FRANKENFILE_PUBLIC_URL", default_value = DEFAULT_PUBLIC_URL)]
        public_url: String,
        #[arg(long = "trusted-proxy")]
        trusted_proxies: Vec<IpNet>,
        #[arg(long, default_value_t = 10)]
        global_failure_limit: u32,
        #[arg(long, default_value_t = 5)]
        source_failure_limit: u32,
        #[arg(long, default_value = "24h", value_parser = parse_duration)]
        session_ttl: Duration,
        /// Permit a non-Secure session cookie for loopback-only development.
        #[arg(long)]
        insecure_cookie: bool,
        /// Operator password for the FrankenDrop browser console. When unset, a
        /// single-use password is generated and logged once at startup.
        #[arg(long, env = "FRANKENFILE_ADMIN_PASSWORD", hide_env_values = true)]
        admin_password: Option<String>,
        /// Upper bound for one FrankenDrop upload request body.
        #[arg(long, default_value_t = 2 * 1024 * 1024 * 1024)]
        max_upload_bytes: u64,
    },
    /// Snapshot files/directories and issue a six-character rendezvous code.
    Create {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, default_value = "15m", value_parser = parse_duration)]
        code_ttl: Duration,
        #[arg(long, default_value = "24h", value_parser = parse_duration)]
        drop_ttl: Duration,
        #[arg(long)]
        max_redemptions: Option<u32>,
        /// Issue a digits-only code (easier to read over the phone).
        #[arg(long)]
        numeric_code: bool,
        #[arg(long, env = "FRANKENFILE_PUBLIC_URL", default_value = DEFAULT_PUBLIC_URL)]
        public_url: String,
    },
    /// Issue a fresh pickup code for an active drop; the old code stops working.
    Recode {
        /// Full drop ID or a unique prefix of at least 8 characters.
        drop_id: String,
        #[arg(long, default_value = "15m", value_parser = parse_duration)]
        code_ttl: Duration,
        /// Issue a digits-only code (easier to read over the phone).
        #[arg(long)]
        numeric_code: bool,
        #[arg(long, env = "FRANKENFILE_PUBLIC_URL", default_value = DEFAULT_PUBLIC_URL)]
        public_url: String,
    },
    /// Redeem a pickup link/code and safely extract its immutable bundle.
    Get {
        /// Six-character pickup code or full FrankenFile share link.
        source: String,
        #[arg(long, env = "FRANKENFILE_PUBLIC_URL", default_value = DEFAULT_PUBLIC_URL)]
        public_url: String,
        /// Parent directory for content-addressed fetch caches.
        #[arg(
            long,
            env = "FRANKENFILE_CACHE_DIR",
            default_value = "/tmp/frankenfile-cache"
        )]
        cache_dir: PathBuf,
        /// Extract into this exact directory instead of the default cache.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Maximum compressed download and extracted logical size.
        #[arg(long, default_value_t = 2 * 1024 * 1024 * 1024)]
        max_bytes: u64,
    },
    /// List active drops (or all historical rows).
    List {
        #[arg(long)]
        all: bool,
    },
    /// Inspect one drop's manifest and lifecycle without exposing its code.
    Show { drop_id: String },
    /// Immediately revoke a drop and all of its sessions.
    Revoke { drop_id: String },
    /// Report unreachable objects/stale archives; delete only with --execute.
    Gc {
        #[arg(long)]
        execute: bool,
        /// Keep expired/revoked drop metadata and objects for this recovery grace period.
        #[arg(long, default_value = "7d", value_parser = parse_duration)]
        retention: Duration,
    },
    /// Validate database and object-store integrity.
    Doctor {
        #[arg(long)]
        deep: bool,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    unsafe {
        libc::umask(0o007);
    }
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    if let Command::Get {
        source,
        public_url,
        cache_dir,
        output,
        max_bytes,
    } = &cli.command
    {
        let result = fetch::fetch(&fetch::FetchOptions {
            source: source.clone(),
            public_url: public_url.clone(),
            cache_dir: cache_dir.clone(),
            output: output.clone(),
            max_bytes: *max_bytes,
        })?;
        if cli.json {
            println!("{}", serde_json::to_string_pretty(&result)?);
        } else {
            println!(
                "FrankenFile {}",
                if result.cache_hit {
                    "cache ready"
                } else {
                    "fetched"
                }
            );
            println!("  Directory  {}", result.directory.display());
            println!("  Drop ID    {}", result.drop_id);
            println!(
                "  Payload    {} across {} files and {} folders",
                ui::human_size(result.total_bytes),
                result.file_count,
                result.directory_count
            );
            println!("  SHA-256    {}", result.archive_sha256);
        }
        return Ok(());
    }

    let storage = Storage::new(&cli.data_dir);
    storage.prepare()?;
    let database = Database::new(storage.database_path());
    database.initialize()?;

    match cli.command {
        Command::Serve {
            bind,
            base_path,
            public_url,
            trusted_proxies,
            global_failure_limit,
            source_failure_limit,
            session_ttl,
            insecure_cookie,
            admin_password,
            max_upload_bytes,
        } => {
            ensure!(
                (1..=60).contains(&global_failure_limit),
                "global failure limit must be 1..=60 per minute"
            );
            ensure!(
                source_failure_limit >= 1 && source_failure_limit <= global_failure_limit,
                "source failure limit must be 1..=global limit"
            );
            ensure!(
                session_ttl >= Duration::from_secs(60)
                    && session_ttl <= Duration::from_secs(30 * 86400),
                "session TTL must be between 1 minute and 30 days"
            );
            let base_path = normalize_base_path(&base_path)?;
            ensure!(!base_path.is_empty(), "production base path cannot be root");
            ensure!(
                public_url.ends_with(&base_path),
                "public URL must end with the configured base path"
            );
            // No credential ships with the source: an unconfigured console gets a
            // fresh single-use password each start, announced once on stderr so
            // it never lands in the structured log or on disk.
            let generated = admin_password.is_none();
            let admin_password = match admin_password {
                Some(password) => password,
                None => random_token(15)?,
            };
            ensure!(
                admin_password.chars().count() >= 8
                    && !admin_password.chars().any(char::is_control),
                "admin password must be at least 8 printable characters"
            );
            ensure!(
                (1024 * 1024..=64 * 1024 * 1024 * 1024).contains(&max_upload_bytes),
                "max upload size must be between 1 MiB and 64 GiB"
            );
            init_tracing();
            if generated {
                eprintln!(
                    "FrankenDrop console password for this run: {admin_password}\n\
                     Set --admin-password or FRANKENFILE_ADMIN_PASSWORD to keep one across restarts."
                );
            }
            let key = MasterKey::load_or_create(&storage.master_key_path())?;
            web::serve(
                database,
                storage,
                key,
                ServeSettings {
                    bind,
                    base_path,
                    public_url,
                    trusted_proxies,
                    secure_cookie: !insecure_cookie,
                    global_failure_limit,
                    source_failure_limit,
                    session_ttl,
                    admin_password,
                    max_upload_bytes,
                },
            )
            .await?;
        }
        Command::Create {
            paths,
            title,
            code_ttl,
            drop_ttl,
            max_redemptions,
            numeric_code,
            public_url,
        } => {
            let key = MasterKey::load_or_create(&storage.master_key_path())?;
            let result = storage.create_drop(
                &database,
                &key,
                &paths,
                &CreateOptions {
                    title,
                    code_ttl,
                    drop_ttl,
                    max_redemptions,
                    public_url,
                    code_style: if numeric_code {
                        security::CodeStyle::Digits
                    } else {
                        security::CodeStyle::Alphanumeric
                    },
                },
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("FrankenFile airdrop created");
                println!("  Code       {}", result.code);
                println!("  Receiver   {}", result.url);
                println!("  Share link {}/{}", result.url, result.code);
                println!("  Drop ID    {}", result.drop_id);
                println!("  Title      {}", result.title);
                println!(
                    "  Payload    {} across {} files and {} folders",
                    ui::human_size(result.total_bytes),
                    result.file_count,
                    result.directory_count
                );
                println!("  Code until {}", format_time(result.code_expires_at));
                println!("  Drop until {}", format_time(result.drop_expires_at));
                println!("  Manifest   {}", result.manifest_hash);
            }
        }
        Command::Recode {
            drop_id,
            code_ttl,
            numeric_code,
            public_url,
        } => {
            let key = MasterKey::load_or_create(&storage.master_key_path())?;
            let style = if numeric_code {
                security::CodeStyle::Digits
            } else {
                security::CodeStyle::Alphanumeric
            };
            match storage.reissue_code(&database, &key, &drop_id, code_ttl, style, &public_url)? {
                storage::ReissueOutcome::Reissued(result) => {
                    if cli.json {
                        println!("{}", serde_json::to_string_pretty(&result)?);
                    } else {
                        println!("FrankenFile code reissued (the previous code is now invalid)");
                        println!("  Code       {}", result.code);
                        println!("  Receiver   {}", result.url);
                        println!("  Share link {}/{}", result.url, result.code);
                        println!("  Drop ID    {}", result.drop_id);
                        println!("  Title      {}", result.title);
                        println!("  Code until {}", format_time(result.code_expires_at));
                        println!("  Drop until {}", format_time(result.drop_expires_at));
                    }
                }
                storage::ReissueOutcome::NotFound => {
                    bail!("no active drop matches: {drop_id}")
                }
                storage::ReissueOutcome::Ambiguous => {
                    bail!("more than one active drop matches the prefix: {drop_id}")
                }
            }
        }
        Command::Get { .. } => unreachable!("get returns before local storage initialization"),
        Command::List { all } => {
            let now = OffsetDateTime::now_utc().unix_timestamp();
            let drops = database.list_drops(all, now)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&drops)?);
            } else if drops.is_empty() {
                println!("No {}drops.", if all { "" } else { "active " });
            } else {
                println!(
                    "{:<26}  {:<28}  {:>10}  {:>7}  {:<20}",
                    "DROP ID", "TITLE", "SIZE", "FILES", "STATE / EXPIRY"
                );
                for drop in drops {
                    let state = if drop.revoked_at.is_some() {
                        "revoked".to_string()
                    } else if drop.expires_at <= now {
                        "expired".to_string()
                    } else {
                        format!("until {}", format_time(drop.expires_at))
                    };
                    println!(
                        "{:<26}  {:<28}  {:>10}  {:>7}  {}",
                        drop.id,
                        truncate(&drop.title, 28),
                        ui::human_size(drop.total_bytes),
                        drop.file_count,
                        state
                    );
                }
            }
        }
        Command::Show { drop_id } => {
            let detail = database.require_drop(&drop_id)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&detail)?);
            } else {
                println!("{}  [{}]", detail.drop.title, detail.drop.id);
                println!(
                    "Created {} · expires {}",
                    format_time(detail.drop.created_at),
                    format_time(detail.drop.expires_at)
                );
                println!(
                    "{} in {} files / {} folders · {} redemptions",
                    ui::human_size(detail.drop.total_bytes),
                    detail.drop.file_count,
                    detail.drop.directory_count,
                    detail.drop.redemption_count
                );
                println!("Manifest {}", detail.drop.manifest_hash);
                for entry in detail.entries {
                    let kind = if entry.kind == model::EntryKind::Directory {
                        "dir "
                    } else {
                        "file"
                    };
                    println!(
                        "  {kind}  {:>10}  {}",
                        if entry.kind == model::EntryKind::File {
                            ui::human_size(entry.size)
                        } else {
                            "—".to_string()
                        },
                        entry.path
                    );
                }
            }
        }
        Command::Revoke { drop_id } => {
            let now = OffsetDateTime::now_utc().unix_timestamp();
            if !database.revoke_drop(&drop_id, now)? {
                bail!("drop not found or already revoked: {drop_id}");
            }
            if cli.json {
                println!(
                    "{{\"drop_id\":{},\"revoked_at\":{now}}}",
                    serde_json::to_string(&drop_id)?
                );
            } else {
                println!("Revoked {drop_id}; all sessions are now invalid.");
            }
        }
        Command::Gc { execute, retention } => {
            ensure!(
                retention >= Duration::from_secs(3600)
                    && retention <= Duration::from_secs(30 * 86400),
                "GC retention must be between 1 hour and 30 days"
            );
            let report = storage.garbage_collect(
                &database,
                execute,
                OffsetDateTime::now_utc().unix_timestamp(),
                retention,
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "GC {}: {} drops past retention, {} unreachable objects ({}), {} stale archives ({})",
                    if execute { "completed" } else { "dry run" },
                    report.purgable_drops,
                    report.unreachable_objects,
                    ui::human_size(report.unreachable_object_bytes),
                    report.stale_archives,
                    ui::human_size(report.stale_archive_bytes)
                );
                if !execute && (report.unreachable_objects + report.stale_archives) > 0 {
                    println!("Run again with --execute to remove these derived/unreachable files.");
                }
            }
        }
        Command::Doctor { deep } => {
            let _key = MasterKey::load_or_create(&storage.master_key_path())?;
            let report = storage.doctor(&database, deep)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "FrankenFile doctor: {}",
                    if report.healthy { "healthy" } else { "FAILED" }
                );
                println!(
                    "Database {} · {} drops · {} objects / {} checked{}",
                    report.database_check,
                    report.drops_checked,
                    report.objects_checked,
                    ui::human_size(report.object_bytes_checked),
                    if deep { " deeply" } else { "" }
                );
                if !report.missing_objects.is_empty() {
                    println!("Missing objects: {}", report.missing_objects.join(", "));
                }
                if !report.corrupt_objects.is_empty() {
                    println!("Corrupt objects: {}", report.corrupt_objects.join(", "));
                }
            }
            if !report.healthy {
                bail!("integrity checks failed");
            }
        }
    }
    Ok(())
}

fn parse_duration(value: &str) -> std::result::Result<Duration, String> {
    humantime::parse_duration(value).map_err(|error| error.to_string())
}

fn format_time(timestamp: i64) -> String {
    OffsetDateTime::from_unix_timestamp(timestamp)
        .ok()
        .and_then(|time| time.format(&Rfc3339).ok())
        .unwrap_or_else(|| timestamp.to_string())
}

fn truncate(value: &str, width: usize) -> String {
    let count = value.chars().count();
    if count <= width {
        return value.to_string();
    }
    value
        .chars()
        .take(width.saturating_sub(1))
        .collect::<String>()
        + "…"
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("frankenfile=info,tower_http=info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .try_init();
}
