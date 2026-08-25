use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{ConnectInfo, DefaultBodyLimit, Multipart, Path, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use cookie::{Cookie, SameSite};
use ipnet::IpNet;
use serde::Deserialize;
use subtle::ConstantTimeEq as _;
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;
use tower_http::trace::TraceLayer;

use crate::{
    archive::{self, ArchiveScope},
    db::Database,
    model::{CreateResult, EntryKind, RedeemResult},
    security::{
        CodeStyle, MasterKey, content_disposition, normalize_code, random_delay_ms, random_session,
        random_token,
    },
    storage::{CreateOptions, ReissueOutcome, Storage, safe_component},
    ui,
};

const SESSION_COOKIE: &str = "__Secure-ff_session";
const DEVELOPMENT_SESSION_COOKIE: &str = "ff_session";

#[derive(Debug, Clone)]
pub struct ServeSettings {
    pub bind: SocketAddr,
    pub base_path: String,
    pub public_url: String,
    pub trusted_proxies: Vec<IpNet>,
    pub secure_cookie: bool,
    pub global_failure_limit: u32,
    pub source_failure_limit: u32,
    pub session_ttl: Duration,
    pub admin_password: String,
    pub max_upload_bytes: u64,
}

#[derive(Clone)]
struct AppState {
    database: Database,
    storage: Storage,
    key: Arc<MasterKey>,
    settings: ServeSettings,
    origin: String,
    archive_locks: Arc<tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

#[derive(Debug, Deserialize)]
struct RedeemForm {
    code: String,
}

#[derive(Debug, Deserialize)]
struct UnlockForm {
    password: String,
}

#[derive(Debug, Deserialize)]
struct RecodeForm {
    #[serde(default)]
    password: String,
    reference: String,
    #[serde(default)]
    code_ttl: String,
    #[serde(default)]
    code_style: String,
}

#[derive(Debug, Deserialize)]
struct RevokeForm {
    #[serde(default)]
    password: String,
    reference: String,
}

#[derive(Debug, Clone)]
struct Representation {
    path: PathBuf,
    size: u64,
    sha256_hex: String,
    sha256_base64: String,
    media_type: String,
    download_name: String,
    last_modified: i64,
}

pub async fn serve(
    database: Database,
    storage: Storage,
    key: MasterKey,
    settings: ServeSettings,
) -> Result<()> {
    let origin = origin_from_public_url(&settings.public_url)?;
    let state = AppState {
        database,
        storage,
        key: Arc::new(key),
        settings: settings.clone(),
        origin,
        archive_locks: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
    };
    let upload_limit = usize::try_from(settings.max_upload_bytes).unwrap_or(usize::MAX);
    let inner = Router::new()
        .route("/", get(landing))
        .route("/{code}", get(code_link))
        .route("/redeem", post(redeem).layer(DefaultBodyLimit::max(1024)))
        .route(
            "/drop",
            get(frankendrop)
                .post(frankendrop_submit)
                .layer(DefaultBodyLimit::max(upload_limit)),
        )
        .route(
            "/drop/recode",
            post(frankendrop_recode).layer(DefaultBodyLimit::max(2048)),
        )
        .route(
            "/drop/unlock",
            post(frankendrop_unlock_submit).layer(DefaultBodyLimit::max(1024)),
        )
        .route("/drop/lock", post(frankendrop_lock))
        .route(
            "/drop/revoke",
            post(frankendrop_revoke).layer(DefaultBodyLimit::max(1024)),
        )
        .route("/healthz", get(health))
        .route("/d/{drop_id}", get(show_drop))
        .route("/d/{drop_id}/file/{entry_id}", get(download_file))
        .route("/d/{drop_id}/bundle", get(download_bundle))
        .route("/d/{drop_id}/folder/{entry_id}", get(download_folder));
    // `/base/` is what people get when they type the URL by hand or a client
    // helpfully appends a slash; send it to the receiver page instead of the
    // generic failure surface.
    let app = Router::new()
        .nest(&settings.base_path, inner)
        .route(&format!("{}/", settings.base_path), get(base_path_redirect))
        .fallback(not_found)
        .layer(middleware::from_fn(security_headers))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(settings.bind)
        .await
        .with_context(|| format!("bind FrankenFile to {}", settings.bind))?;
    tracing::info!(bind=%settings.bind, base_path=%settings.base_path, "frankenfile service ready");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("serve HTTP")?;
    Ok(())
}

async fn base_path_redirect(State(state): State<AppState>) -> Response {
    Redirect::permanent(&state.settings.base_path).into_response()
}

async fn landing(State(state): State<AppState>) -> Response {
    ui::landing(&state.settings.base_path, false, None).into_response()
}

async fn code_link(State(state): State<AppState>, Path(code): Path<String>) -> Response {
    match normalize_code(&code) {
        Some(code) => ui::landing(&state.settings.base_path, false, Some(&code)).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            ui::service_error(&state.settings.base_path),
        )
            .into_response(),
    }
}

async fn redeem(
    State(state): State<AppState>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let source = client_source(&state, remote, &headers);
    let source_tag = state.key.source_tag(&source);
    let parsed = serde_urlencoded::from_bytes::<RedeemForm>(&body).ok();
    let code = parsed.as_ref().and_then(|form| normalize_code(&form.code));
    let origin_valid = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(|value| value == state.origin)
        .unwrap_or(true);
    let code_tag = match code {
        Some(ref code) if origin_valid => state.key.code_tag(code),
        _ => state.key.tag(b"frankenfile/rejected-input/v1", b"uniform"),
    };
    let token = match random_session() {
        Ok(token) => token,
        Err(error) => return internal_error(&state.settings.base_path, error),
    };
    let session_tag = state.key.session_tag(&token);
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let session_expires = now.saturating_add(state.settings.session_ttl.as_secs() as i64);
    let database = state.database.clone();
    let global = state.settings.global_failure_limit;
    let per_source = state.settings.source_failure_limit;
    let result = tokio::task::spawn_blocking(move || {
        database.redeem(
            &code_tag,
            &source_tag,
            &session_tag,
            now,
            session_expires,
            global,
            per_source,
        )
    })
    .await;
    let outcome = match result {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => return internal_error(&state.settings.base_path, error),
        Err(error) => return internal_error(&state.settings.base_path, error.into()),
    };
    match outcome {
        RedeemResult::Success {
            drop_id,
            session_expires_at,
        } => {
            let max_age = (session_expires_at - now).max(1);
            let cookie = Cookie::build((session_cookie_name(&state), token))
                .http_only(true)
                .secure(state.settings.secure_cookie)
                .same_site(SameSite::Strict)
                .path(state.settings.base_path.clone())
                .max_age(cookie::time::Duration::seconds(max_age))
                .build()
                .to_string();
            let target = format!("{}/d/{drop_id}", state.settings.base_path);
            let mut response = Redirect::to(&target).into_response();
            *response.status_mut() = StatusCode::SEE_OTHER;
            if let Ok(value) = HeaderValue::from_str(&cookie) {
                response.headers_mut().insert(header::SET_COOKIE, value);
            }
            response
        }
        RedeemResult::Rejected => {
            tokio::time::sleep(Duration::from_millis(random_delay_ms())).await;
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                ui::landing(&state.settings.base_path, true, None),
            )
                .into_response()
        }
    }
}

const MAX_UPLOAD_FILES: usize = 400;
const ADMIN_COOKIE: &str = "__Secure-ff_admin";
const DEVELOPMENT_ADMIN_COOKIE: &str = "ff_admin";
const ADMIN_SESSION_TTL_SECS: i64 = 30 * 60;

fn admin_cookie_name(state: &AppState) -> &'static str {
    if state.settings.secure_cookie {
        ADMIN_COOKIE
    } else {
        DEVELOPMENT_ADMIN_COOKIE
    }
}

async fn authorize_admin(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(token) = session_cookie(headers, admin_cookie_name(state)) else {
        return false;
    };
    if token.len() != 43
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return false;
    }
    let tag = state.key.admin_session_tag(&token);
    let database = state.database.clone();
    let now = OffsetDateTime::now_utc().unix_timestamp();
    matches!(
        tokio::task::spawn_blocking(move || database.validate_admin_session(&tag, now)).await,
        Ok(Ok(true))
    )
}

/// Render the unlocked console with fresh drop rows and optional error banners.
async fn console_page(
    state: &AppState,
    status: StatusCode,
    error: Option<String>,
    reissue_error: Option<String>,
) -> Response {
    let database = state.database.clone();
    let now = OffsetDateTime::now_utc().unix_timestamp();
    match tokio::task::spawn_blocking(move || database.list_drops(false, now)).await {
        Ok(Ok(drops)) => (
            status,
            ui::frankendrop_console(
                &state.settings.base_path,
                &ui::ConsoleView {
                    drops: &drops,
                    now,
                    error: error.as_deref(),
                    reissue_error: reissue_error.as_deref(),
                },
            ),
        )
            .into_response(),
        Ok(Err(error)) => internal_error(&state.settings.base_path, error),
        Err(error) => internal_error(&state.settings.base_path, error.into()),
    }
}

/// Route a console failure to the right surface: the unlocked dashboard when an
/// admin session exists, the unlock panel otherwise.
async fn console_error(
    state: &AppState,
    admin: bool,
    status: StatusCode,
    message: &str,
    reissue: bool,
) -> Response {
    if admin {
        let (error, reissue_error) = if reissue {
            (None, Some(message.to_string()))
        } else {
            (Some(message.to_string()), None)
        };
        console_page(state, status, error, reissue_error).await
    } else {
        (
            status,
            ui::frankendrop_unlock(&state.settings.base_path, Some(message)),
        )
            .into_response()
    }
}

async fn over_failure_budget(state: &AppState, source_tag: &[u8], now: i64) -> Result<bool> {
    let database = state.database.clone();
    let tag = source_tag.to_vec();
    let (global, per_source) =
        tokio::task::spawn_blocking(move || database.failure_counts(&tag, now))
            .await
            .context("join failure-count task")??;
    Ok(global >= state.settings.global_failure_limit
        || per_source >= state.settings.source_failure_limit)
}

/// Record a failed password attempt and stall like the redeem path does.
async fn punish_failure(state: &AppState, source_tag: Vec<u8>, now: i64) {
    let database = state.database.clone();
    let _ = tokio::task::spawn_blocking(move || {
        database.record_failure(&source_tag, now, "frankendrop_rejected")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(random_delay_ms())).await;
}

fn origin_ok(state: &AppState, headers: &HeaderMap) -> bool {
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(|value| value == state.origin)
        .unwrap_or(true)
}

fn password_ok(state: &AppState, supplied: &str) -> bool {
    let expected = state.key.admin_tag(&state.settings.admin_password);
    let received = state.key.admin_tag(supplied.trim());
    expected.ct_eq(&received).into()
}

async fn frankendrop(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if authorize_admin(&state, &headers).await {
        console_page(&state, StatusCode::OK, None, None).await
    } else {
        ui::frankendrop_unlock(&state.settings.base_path, None).into_response()
    }
}

async fn frankendrop_unlock_submit(
    State(state): State<AppState>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let base = state.settings.base_path.clone();
    let source = client_source(&state, remote, &headers);
    let source_tag = state.key.source_tag(&source);
    let now = OffsetDateTime::now_utc().unix_timestamp();
    match over_failure_budget(&state, &source_tag, now).await {
        Ok(true) => {
            tokio::time::sleep(Duration::from_millis(random_delay_ms())).await;
            return (
                StatusCode::TOO_MANY_REQUESTS,
                ui::frankendrop_unlock(
                    &base,
                    Some("Too many attempts. Wait a minute and try again."),
                ),
            )
                .into_response();
        }
        Ok(false) => {}
        Err(error) => return internal_error(&base, error),
    }
    if !origin_ok(&state, &headers) {
        return (
            StatusCode::FORBIDDEN,
            ui::frankendrop_unlock(&base, Some("Cross-origin submissions are refused.")),
        )
            .into_response();
    }
    let password = serde_urlencoded::from_bytes::<UnlockForm>(&body)
        .map(|form| form.password)
        .unwrap_or_default();
    if !password_ok(&state, &password) {
        punish_failure(&state, source_tag, now).await;
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            ui::frankendrop_unlock(&base, Some("That operator password didn’t work.")),
        )
            .into_response();
    }
    let token = match random_session() {
        Ok(token) => token,
        Err(error) => return internal_error(&base, error),
    };
    let tag = state.key.admin_session_tag(&token);
    let expires_at = now + ADMIN_SESSION_TTL_SECS;
    let database = state.database.clone();
    let stored =
        tokio::task::spawn_blocking(move || database.create_admin_session(&tag, now, expires_at))
            .await;
    match stored {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return internal_error(&base, error),
        Err(error) => return internal_error(&base, error.into()),
    }
    let cookie = Cookie::build((admin_cookie_name(&state), token))
        .http_only(true)
        .secure(state.settings.secure_cookie)
        .same_site(SameSite::Strict)
        .path(base.clone())
        .max_age(cookie::time::Duration::seconds(ADMIN_SESSION_TTL_SECS))
        .build()
        .to_string();
    let mut response = Redirect::to(&format!("{base}/drop")).into_response();
    *response.status_mut() = StatusCode::SEE_OTHER;
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

async fn frankendrop_lock(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let base = state.settings.base_path.clone();
    if let Some(token) = session_cookie(&headers, admin_cookie_name(&state)) {
        let tag = state.key.admin_session_tag(&token);
        let database = state.database.clone();
        let _ = tokio::task::spawn_blocking(move || database.delete_admin_session(&tag)).await;
    }
    let cookie = Cookie::build((admin_cookie_name(&state), ""))
        .http_only(true)
        .secure(state.settings.secure_cookie)
        .same_site(SameSite::Strict)
        .path(base.clone())
        .max_age(cookie::time::Duration::seconds(0))
        .build()
        .to_string();
    let mut response = Redirect::to(&format!("{base}/drop")).into_response();
    *response.status_mut() = StatusCode::SEE_OTHER;
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

async fn frankendrop_revoke(
    State(state): State<AppState>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let base = state.settings.base_path.clone();
    let admin = authorize_admin(&state, &headers).await;
    let source = client_source(&state, remote, &headers);
    let source_tag = state.key.source_tag(&source);
    let now = OffsetDateTime::now_utc().unix_timestamp();
    if !origin_ok(&state, &headers) {
        return console_error(
            &state,
            admin,
            StatusCode::FORBIDDEN,
            "Cross-origin submissions are refused.",
            false,
        )
        .await;
    }
    let Ok(form) = serde_urlencoded::from_bytes::<RevokeForm>(&body) else {
        return console_error(
            &state,
            admin,
            StatusCode::UNPROCESSABLE_ENTITY,
            "The form could not be read. Try again.",
            false,
        )
        .await;
    };
    if !admin {
        match over_failure_budget(&state, &source_tag, now).await {
            Ok(true) => {
                tokio::time::sleep(Duration::from_millis(random_delay_ms())).await;
                return console_error(
                    &state,
                    false,
                    StatusCode::TOO_MANY_REQUESTS,
                    "Too many attempts. Wait a minute and try again.",
                    false,
                )
                .await;
            }
            Ok(false) => {}
            Err(error) => return internal_error(&base, error),
        }
        if !password_ok(&state, &form.password) {
            punish_failure(&state, source_tag, now).await;
            return console_error(
                &state,
                false,
                StatusCode::UNPROCESSABLE_ENTITY,
                "That operator password didn’t work.",
                false,
            )
            .await;
        }
    }
    let reference = form.reference.trim().to_string();
    if reference.len() < 8
        || reference.len() > 64
        || !reference
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return console_error(
            &state,
            admin,
            StatusCode::UNPROCESSABLE_ENTITY,
            "Revoking needs the drop's full ID.",
            false,
        )
        .await;
    }
    let database = state.database.clone();
    let revoked = tokio::task::spawn_blocking(move || database.revoke_drop(&reference, now)).await;
    match revoked {
        Ok(Ok(true)) => {
            let mut response = Redirect::to(&format!("{base}/drop")).into_response();
            *response.status_mut() = StatusCode::SEE_OTHER;
            response
        }
        Ok(Ok(false)) => {
            console_error(
                &state,
                admin,
                StatusCode::UNPROCESSABLE_ENTITY,
                "No active drop matches that ID — it may already be revoked or expired.",
                false,
            )
            .await
        }
        Ok(Err(error)) => internal_error(&base, error),
        Err(error) => internal_error(&base, error.into()),
    }
}

async fn frankendrop_recode(
    State(state): State<AppState>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let base = state.settings.base_path.clone();
    let admin = authorize_admin(&state, &headers).await;
    let source = client_source(&state, remote, &headers);
    let source_tag = state.key.source_tag(&source);
    let now = OffsetDateTime::now_utc().unix_timestamp();
    if !origin_ok(&state, &headers) {
        return console_error(
            &state,
            admin,
            StatusCode::FORBIDDEN,
            "Cross-origin submissions are refused.",
            true,
        )
        .await;
    }
    let Ok(form) = serde_urlencoded::from_bytes::<RecodeForm>(&body) else {
        return console_error(
            &state,
            admin,
            StatusCode::UNPROCESSABLE_ENTITY,
            "The form could not be read. Try again.",
            true,
        )
        .await;
    };
    if !admin {
        match over_failure_budget(&state, &source_tag, now).await {
            Ok(true) => {
                tokio::time::sleep(Duration::from_millis(random_delay_ms())).await;
                return console_error(
                    &state,
                    false,
                    StatusCode::TOO_MANY_REQUESTS,
                    "Too many attempts. Wait a minute and try again.",
                    true,
                )
                .await;
            }
            Ok(false) => {}
            Err(error) => return internal_error(&base, error),
        }
        if !password_ok(&state, &form.password) {
            punish_failure(&state, source_tag, now).await;
            return console_error(
                &state,
                false,
                StatusCode::UNPROCESSABLE_ENTITY,
                "That operator password didn’t work.",
                true,
            )
            .await;
        }
    }

    let code_ttl = parse_choice(
        &form.code_ttl,
        &[
            ("15m", Duration::from_secs(15 * 60)),
            ("1h", Duration::from_secs(3600)),
            ("6h", Duration::from_secs(6 * 3600)),
            ("24h", Duration::from_secs(24 * 3600)),
        ],
    )
    .unwrap_or(Duration::from_secs(15 * 60));
    let code_style = match form.code_style.as_str() {
        "digits" => CodeStyle::Digits,
        _ => CodeStyle::Alphanumeric,
    };

    let storage = state.storage.clone();
    let database = state.database.clone();
    let key = state.key.clone();
    let public_url = state.settings.public_url.clone();
    let reference = form.reference.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        storage.reissue_code(
            &database,
            &key,
            &reference,
            code_ttl,
            code_style,
            &public_url,
        )
    })
    .await;
    match outcome {
        Ok(Ok(ReissueOutcome::Reissued(result))) => {
            ui::frankendrop_recoded(&base, &result).into_response()
        }
        Ok(Ok(ReissueOutcome::NotFound)) => {
            console_error(
                &state,
                admin,
                StatusCode::UNPROCESSABLE_ENTITY,
                "No active drop matches that reference.",
                true,
            )
            .await
        }
        Ok(Ok(ReissueOutcome::Ambiguous)) => {
            console_error(
                &state,
                admin,
                StatusCode::UNPROCESSABLE_ENTITY,
                "That reference matches more than one drop — paste more of the ID.",
                true,
            )
            .await
        }
        Ok(Err(error)) => {
            console_error(
                &state,
                admin,
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("Reissue failed: {error}"),
                true,
            )
            .await
        }
        Err(error) => internal_error(&base, error.into()),
    }
}

enum DropRejection {
    /// Wrong or missing operator password — counts against the failure budget.
    Unauthorized,
    /// Well-authenticated but unusable submission; message is user-facing.
    Invalid(String),
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for DropRejection {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error)
    }
}

async fn frankendrop_submit(
    State(state): State<AppState>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Response {
    let base = state.settings.base_path.clone();
    let admin = authorize_admin(&state, &headers).await;
    let source = client_source(&state, remote, &headers);
    let source_tag = state.key.source_tag(&source);
    let now = OffsetDateTime::now_utc().unix_timestamp();

    if !admin {
        match over_failure_budget(&state, &source_tag, now).await {
            Ok(true) => {
                tokio::time::sleep(Duration::from_millis(random_delay_ms())).await;
                return console_error(
                    &state,
                    false,
                    StatusCode::TOO_MANY_REQUESTS,
                    "Too many attempts. Wait a minute and try again.",
                    false,
                )
                .await;
            }
            Ok(false) => {}
            Err(error) => return internal_error(&base, error),
        }
    }
    if !origin_ok(&state, &headers) {
        return console_error(
            &state,
            admin,
            StatusCode::FORBIDDEN,
            "Cross-origin submissions are refused.",
            false,
        )
        .await;
    }

    let upload_dir = match random_token(12) {
        Ok(token) => state
            .storage
            .temp_dir()
            .join(format!("frankendrop-{token}")),
        Err(error) => return internal_error(&base, error),
    };
    let outcome = ingest_frankendrop(&state, multipart, &upload_dir, admin).await;
    let _ = tokio::fs::remove_dir_all(&upload_dir).await;

    match outcome {
        Ok(result) => ui::frankendrop_created(&base, &result).into_response(),
        Err(DropRejection::Unauthorized) => {
            punish_failure(&state, source_tag, now).await;
            console_error(
                &state,
                false,
                StatusCode::UNPROCESSABLE_ENTITY,
                "That operator password didn’t work.",
                false,
            )
            .await
        }
        Err(DropRejection::Invalid(message)) => {
            console_error(
                &state,
                admin,
                StatusCode::UNPROCESSABLE_ENTITY,
                &message,
                false,
            )
            .await
        }
        Err(DropRejection::Internal(error)) => internal_error(&base, error),
    }
}

/// Stream the multipart submission, refusing any file bytes until either an
/// admin session pre-authorized the request or an in-form operator password
/// (placed before the file field) has been verified, then publish the captured
/// files through the same immutable pipeline as the CLI.
async fn ingest_frankendrop(
    state: &AppState,
    mut multipart: Multipart,
    upload_dir: &std::path::Path,
    already_authorized: bool,
) -> Result<CreateResult, DropRejection> {
    let mut authorized = already_authorized;
    let mut title: Option<String> = None;
    let mut code_ttl = Duration::from_secs(15 * 60);
    let mut drop_ttl = Duration::from_secs(24 * 3600);
    let mut max_redemptions: Option<u32> = None;
    let mut code_style = CodeStyle::Alphanumeric;
    let mut files: Vec<PathBuf> = Vec::new();
    let mut used_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    loop {
        let field = multipart
            .next_field()
            .await
            .map_err(|_| DropRejection::Invalid("The upload was interrupted. Try again.".into()))?;
        let Some(mut field) = field else { break };
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "password" => {
                let supplied = read_small_text(&mut field).await?;
                if !authorized {
                    authorized = password_ok(state, &supplied);
                    if !authorized {
                        return Err(DropRejection::Unauthorized);
                    }
                }
            }
            "title" => {
                let value = read_small_text(&mut field).await?;
                let value = value.trim().to_string();
                if !value.is_empty() {
                    title = Some(value);
                }
            }
            "code_ttl" => {
                code_ttl = parse_choice(
                    &read_small_text(&mut field).await?,
                    &[
                        ("15m", Duration::from_secs(15 * 60)),
                        ("1h", Duration::from_secs(3600)),
                        ("6h", Duration::from_secs(6 * 3600)),
                        ("24h", Duration::from_secs(24 * 3600)),
                    ],
                )
                .ok_or_else(|| DropRejection::Invalid("Choose a valid code window.".into()))?;
            }
            "drop_ttl" => {
                drop_ttl = parse_choice(
                    &read_small_text(&mut field).await?,
                    &[
                        ("1h", Duration::from_secs(3600)),
                        ("6h", Duration::from_secs(6 * 3600)),
                        ("24h", Duration::from_secs(24 * 3600)),
                        ("3d", Duration::from_secs(3 * 86400)),
                        ("7d", Duration::from_secs(7 * 86400)),
                        ("30d", Duration::from_secs(30 * 86400)),
                    ],
                )
                .ok_or_else(|| DropRejection::Invalid("Choose a valid expiry.".into()))?;
            }
            "max_redemptions" => {
                let value = read_small_text(&mut field).await?;
                let value = value.trim().to_string();
                if !value.is_empty() {
                    let parsed: u32 = value.parse().map_err(|_| {
                        DropRejection::Invalid("Download limit must be a number.".into())
                    })?;
                    if !(1..=10_000).contains(&parsed) {
                        return Err(DropRejection::Invalid(
                            "Download limit must be between 1 and 10,000.".into(),
                        ));
                    }
                    max_redemptions = Some(parsed);
                }
            }
            "code_style" => {
                code_style = match read_small_text(&mut field).await?.as_str() {
                    "digits" => CodeStyle::Digits,
                    _ => CodeStyle::Alphanumeric,
                };
            }
            "files" => {
                if !authorized {
                    return Err(DropRejection::Unauthorized);
                }
                let Some(raw_name) = field.file_name().map(str::to_string) else {
                    continue;
                };
                let basename = raw_name
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or_default()
                    .to_string();
                if basename.is_empty() {
                    // Browsers submit one empty file part when nothing is chosen.
                    continue;
                }
                if files.len() >= MAX_UPLOAD_FILES {
                    return Err(DropRejection::Invalid(format!(
                        "A FrankenDrop can hold at most {MAX_UPLOAD_FILES} files."
                    )));
                }
                let safe = safe_component(&basename)
                    .unwrap_or_else(|_| format!("upload-{}.bin", files.len() + 1));
                let unique = uniquify_name(&safe, &mut used_names);
                tokio::fs::create_dir_all(upload_dir)
                    .await
                    .map_err(|error| DropRejection::Internal(error.into()))?;
                let path = upload_dir.join(&unique);
                let mut output = tokio::fs::File::create(&path)
                    .await
                    .map_err(|error| DropRejection::Internal(error.into()))?;
                loop {
                    let chunk = field.chunk().await.map_err(|_| {
                        DropRejection::Invalid("The upload was interrupted. Try again.".into())
                    })?;
                    let Some(chunk) = chunk else { break };
                    output
                        .write_all(&chunk)
                        .await
                        .map_err(|error| DropRejection::Internal(error.into()))?;
                }
                output
                    .flush()
                    .await
                    .map_err(|error| DropRejection::Internal(error.into()))?;
                files.push(path);
            }
            _ => {
                // Drain and ignore unknown fields without buffering them.
                while let Ok(Some(_)) = field.chunk().await {}
            }
        }
    }

    if !authorized {
        return Err(DropRejection::Unauthorized);
    }
    if files.is_empty() {
        return Err(DropRejection::Invalid(
            "Choose at least one file to drop.".into(),
        ));
    }

    let storage = state.storage.clone();
    let database = state.database.clone();
    let key = state.key.clone();
    let options = CreateOptions {
        title,
        code_ttl,
        drop_ttl,
        max_redemptions,
        public_url: state.settings.public_url.clone(),
        code_style,
    };
    let created =
        tokio::task::spawn_blocking(move || storage.create_drop(&database, &key, &files, &options))
            .await
            .map_err(|error| DropRejection::Internal(error.into()))?;
    created.map_err(|error| DropRejection::Invalid(format!("Publishing failed: {error}")))
}

async fn read_small_text(
    field: &mut axum::extract::multipart::Field<'_>,
) -> Result<String, DropRejection> {
    let mut value = String::new();
    loop {
        let chunk = field
            .chunk()
            .await
            .map_err(|_| DropRejection::Invalid("The form could not be read. Try again.".into()))?;
        let Some(chunk) = chunk else { break };
        if value.len() + chunk.len() > 4096 {
            return Err(DropRejection::Invalid("A form field was too long.".into()));
        }
        value.push_str(&String::from_utf8_lossy(&chunk));
    }
    Ok(value)
}

fn parse_choice<T: Clone>(value: &str, allowed: &[(&str, T)]) -> Option<T> {
    allowed
        .iter()
        .find(|(key, _)| *key == value.trim())
        .map(|(_, parsed)| parsed.clone())
}

/// Keep uploaded filenames distinct within one submission by suffixing the stem.
fn uniquify_name(name: &str, used: &mut std::collections::HashSet<String>) -> String {
    if used.insert(name.to_string()) {
        return name.to_string();
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    for counter in 2.. {
        let candidate = format!("{stem}-{counter}{extension}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("name space exhausted")
}

async fn show_drop(
    State(state): State<AppState>,
    Path(drop_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorize(&state, &headers, &drop_id).await {
        return Redirect::to(&state.settings.base_path).into_response();
    }
    let database = state.database.clone();
    let id = drop_id.clone();
    match tokio::task::spawn_blocking(move || database.get_drop(&id)).await {
        Ok(Ok(Some(detail))) => ui::drop_page(&state.settings.base_path, &detail).into_response(),
        Ok(Ok(None)) => Redirect::to(&state.settings.base_path).into_response(),
        Ok(Err(error)) => internal_error(&state.settings.base_path, error),
        Err(error) => internal_error(&state.settings.base_path, error.into()),
    }
}

async fn download_file(
    State(state): State<AppState>,
    Path((drop_id, entry_id)): Path<(String, i64)>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if !authorize(&state, &headers, &drop_id).await {
        return Redirect::to(&state.settings.base_path).into_response();
    }
    let database = state.database.clone();
    let id = drop_id.clone();
    let fetched = tokio::task::spawn_blocking(move || {
        let detail = database.require_drop(&id)?;
        let entry = database.get_entry(&id, entry_id)?;
        Ok::<_, anyhow::Error>((detail, entry))
    })
    .await;
    let (detail, entry) = match fetched {
        Ok(Ok((detail, Some(entry)))) if entry.kind == EntryKind::File => (detail, entry),
        Ok(Ok(_)) => return StatusCode::NOT_FOUND.into_response(),
        Ok(Err(error)) => return internal_error(&state.settings.base_path, error),
        Err(error) => return internal_error(&state.settings.base_path, error.into()),
    };
    let path = match entry
        .object_hash
        .as_deref()
        .map(|hash| state.storage.object_path(hash))
        .transpose()
    {
        Ok(Some(path)) => path,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let download_name = entry.filename().to_string();
    let representation = Representation {
        path,
        size: entry.size,
        sha256_hex: entry.sha256_hex.unwrap_or_default(),
        sha256_base64: entry.sha256_base64.unwrap_or_default(),
        media_type: entry
            .media_type
            .unwrap_or_else(|| "application/octet-stream".to_string()),
        download_name,
        last_modified: detail.drop.created_at,
    };
    serve_representation(method, &headers, representation).await
}

async fn download_bundle(
    State(state): State<AppState>,
    Path(drop_id): Path<String>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    archive_response(state, drop_id, ArchiveScope::Whole, method, headers).await
}

async fn download_folder(
    State(state): State<AppState>,
    Path((drop_id, entry_id)): Path<(String, i64)>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if !authorize(&state, &headers, &drop_id).await {
        return Redirect::to(&state.settings.base_path).into_response();
    }
    let database = state.database.clone();
    let id = drop_id.clone();
    let entry = tokio::task::spawn_blocking(move || database.get_entry(&id, entry_id)).await;
    let folder = match entry {
        Ok(Ok(Some(entry))) if entry.kind == EntryKind::Directory && entry.is_top_level() => {
            entry.path
        }
        Ok(Ok(_)) => return StatusCode::NOT_FOUND.into_response(),
        Ok(Err(error)) => return internal_error(&state.settings.base_path, error),
        Err(error) => return internal_error(&state.settings.base_path, error.into()),
    };
    archive_response(
        state,
        drop_id,
        ArchiveScope::Folder(folder),
        method,
        headers,
    )
    .await
}

async fn archive_response(
    state: AppState,
    drop_id: String,
    scope: ArchiveScope,
    method: Method,
    headers: HeaderMap,
) -> Response {
    if !authorize(&state, &headers, &drop_id).await {
        return Redirect::to(&state.settings.base_path).into_response();
    }
    let database = state.database.clone();
    let id = drop_id.clone();
    let detail = match tokio::task::spawn_blocking(move || database.require_drop(&id)).await {
        Ok(Ok(detail)) => detail,
        Ok(Err(error)) => return internal_error(&state.settings.base_path, error),
        Err(error) => return internal_error(&state.settings.base_path, error.into()),
    };
    let key = archive::cache_key(&detail.drop.manifest_hash, &scope);
    let lock = {
        let mut locks = state.archive_locks.lock().await;
        locks
            .entry(key)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    let _guard = lock.lock().await;
    let storage = state.storage.clone();
    let database = state.database.clone();
    let detail_for_build = detail.clone();
    let scope_for_build = scope.clone();
    let archive = match tokio::task::spawn_blocking(move || {
        archive::materialize(&storage, &database, &detail_for_build, &scope_for_build)
    })
    .await
    {
        Ok(Ok(archive)) => archive,
        Ok(Err(error)) => return internal_error(&state.settings.base_path, error),
        Err(error) => return internal_error(&state.settings.base_path, error.into()),
    };
    let path = match state.storage.archive_path(&archive.relative_path) {
        Ok(path) => path,
        Err(error) => return internal_error(&state.settings.base_path, error),
    };
    let base_name = match scope {
        ArchiveScope::Whole => detail.drop.title,
        ArchiveScope::Folder(folder) => folder,
    };
    let representation = Representation {
        path,
        size: archive.size,
        sha256_hex: archive.sha256_hex,
        sha256_base64: archive.sha256_base64,
        media_type: "application/zip".to_string(),
        download_name: format!("{base_name}.zip"),
        last_modified: archive.created_at,
    };
    serve_representation(method, &headers, representation).await
}

async fn authorize(state: &AppState, headers: &HeaderMap, drop_id: &str) -> bool {
    let Some(token) = session_cookie(headers, session_cookie_name(state)) else {
        return false;
    };
    if token.len() != 43
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return false;
    }
    let tag = state.key.session_tag(&token);
    let database = state.database.clone();
    let id = drop_id.to_string();
    let now = OffsetDateTime::now_utc().unix_timestamp();
    matches!(
        tokio::task::spawn_blocking(move || database.validate_session(&tag, &id, now)).await,
        Ok(Ok(true))
    )
}

fn session_cookie(headers: &HeaderMap, expected_name: &str) -> Option<String> {
    let mut matches = Vec::new();
    for header_value in headers.get_all(header::COOKIE) {
        let Ok(value) = header_value.to_str() else {
            continue;
        };
        for pair in value.split(';') {
            if let Ok(cookie) = Cookie::parse(pair.trim().to_string())
                && cookie.name() == expected_name
            {
                matches.push(cookie.value().to_string());
            }
        }
    }
    if matches.len() == 1 {
        matches.pop()
    } else {
        None
    }
}

fn session_cookie_name(state: &AppState) -> &'static str {
    if state.settings.secure_cookie {
        SESSION_COOKIE
    } else {
        DEVELOPMENT_SESSION_COOKIE
    }
}

async fn serve_representation(
    method: Method,
    headers: &HeaderMap,
    representation: Representation,
) -> Response {
    let etag = format!("\"{}\"", representation.sha256_hex);
    let last_modified = httpdate::fmt_http_date(
        std::time::UNIX_EPOCH + Duration::from_secs(representation.last_modified.max(0) as u64),
    );
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|h| h.to_str().ok())
        .map(|value| etag_matches(value, &etag))
        .unwrap_or(false)
    {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        insert_header(response.headers_mut(), header::ETAG, &etag);
        insert_header(
            response.headers_mut(),
            header::LAST_MODIFIED,
            &last_modified,
        );
        return response;
    }

    let mut selected = None;
    if let Some(range_header) = headers.get(header::RANGE).and_then(|h| h.to_str().ok()) {
        let if_range_allows = headers
            .get(header::IF_RANGE)
            .and_then(|h| h.to_str().ok())
            .map(|value| if_range_matches(value, &etag, representation.last_modified))
            .unwrap_or(true);
        if if_range_allows {
            match parse_range(range_header, representation.size) {
                RangeDecision::Satisfiable(start, end) => selected = Some((start, end)),
                RangeDecision::Unsatisfiable => {
                    let mut response = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
                    insert_header(
                        response.headers_mut(),
                        header::CONTENT_RANGE,
                        &format!("bytes */{}", representation.size),
                    );
                    insert_common_download_headers(
                        response.headers_mut(),
                        &representation,
                        &etag,
                        &last_modified,
                        false,
                    );
                    return response;
                }
                RangeDecision::Ignore => {}
            }
        }
    }

    let (status, start, end) = if let Some((start, end)) = selected {
        (StatusCode::PARTIAL_CONTENT, start, end)
    } else if representation.size == 0 {
        (StatusCode::OK, 0, 0)
    } else {
        (StatusCode::OK, 0, representation.size - 1)
    };
    let length = if representation.size == 0 {
        0
    } else {
        end - start + 1
    };
    let body = if method == Method::HEAD || length == 0 {
        Body::empty()
    } else {
        match tokio::fs::File::open(&representation.path).await {
            Ok(mut file) => {
                if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
                Body::from_stream(ReaderStream::new(file.take(length)))
            }
            Err(_) => return StatusCode::NOT_FOUND.into_response(),
        }
    };
    let mut response = Response::new(body);
    *response.status_mut() = status;
    insert_common_download_headers(
        response.headers_mut(),
        &representation,
        &etag,
        &last_modified,
        status == StatusCode::OK,
    );
    insert_header(
        response.headers_mut(),
        header::CONTENT_LENGTH,
        &length.to_string(),
    );
    if status == StatusCode::PARTIAL_CONTENT {
        insert_header(
            response.headers_mut(),
            header::CONTENT_RANGE,
            &format!("bytes {start}-{end}/{}", representation.size),
        );
    }
    response
}

fn insert_common_download_headers(
    headers: &mut HeaderMap,
    representation: &Representation,
    etag: &str,
    last_modified: &str,
    complete_content: bool,
) {
    insert_header(headers, header::CONTENT_TYPE, &representation.media_type);
    insert_header(
        headers,
        header::CONTENT_DISPOSITION,
        &content_disposition(&representation.download_name),
    );
    insert_header(headers, header::ACCEPT_RANGES, "bytes");
    insert_header(headers, header::ETAG, etag);
    insert_header(headers, header::LAST_MODIFIED, last_modified);
    insert_header(
        headers,
        header::CACHE_CONTROL,
        "private, no-store, max-age=0",
    );
    insert_header(
        headers,
        HeaderName::from_static("repr-digest"),
        &format!("sha-256=:{}:", representation.sha256_base64),
    );
    if complete_content {
        insert_header(
            headers,
            HeaderName::from_static("content-digest"),
            &format!("sha-256=:{}:", representation.sha256_base64),
        );
    }
}

fn insert_header(
    headers: &mut HeaderMap,
    name: impl axum::http::header::IntoHeaderName,
    value: &str,
) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(name, value);
    }
}

#[derive(Debug, PartialEq, Eq)]
enum RangeDecision {
    Satisfiable(u64, u64),
    Unsatisfiable,
    Ignore,
}

fn parse_range(value: &str, size: u64) -> RangeDecision {
    let Some(spec) = value.strip_prefix("bytes=") else {
        return RangeDecision::Ignore;
    };
    if spec.contains(',') {
        return RangeDecision::Ignore;
    }
    let Some((start, end)) = spec.split_once('-') else {
        return RangeDecision::Ignore;
    };
    if start.is_empty() {
        let Ok(suffix) = end.parse::<u64>() else {
            return RangeDecision::Ignore;
        };
        if suffix == 0 || size == 0 {
            return RangeDecision::Unsatisfiable;
        }
        let length = suffix.min(size);
        return RangeDecision::Satisfiable(size - length, size - 1);
    }
    let Ok(start) = start.parse::<u64>() else {
        return RangeDecision::Ignore;
    };
    if start >= size {
        return RangeDecision::Unsatisfiable;
    }
    if end.is_empty() {
        return RangeDecision::Satisfiable(start, size - 1);
    }
    let Ok(end) = end.parse::<u64>() else {
        return RangeDecision::Ignore;
    };
    if end < start {
        return RangeDecision::Ignore;
    }
    RangeDecision::Satisfiable(start, end.min(size - 1))
}

fn etag_matches(value: &str, etag: &str) -> bool {
    value.trim() == "*" || value.split(',').any(|part| part.trim() == etag)
}

fn if_range_matches(value: &str, etag: &str, last_modified: i64) -> bool {
    if value.trim() == etag {
        return true;
    }
    httpdate::parse_http_date(value)
        .ok()
        .and_then(|date| date.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| last_modified <= duration.as_secs() as i64)
        .unwrap_or(false)
}

async fn health(State(state): State<AppState>) -> Response {
    let database = state.database.clone();
    match tokio::task::spawn_blocking(move || database.quick_check()).await {
        Ok(Ok(result)) if result == "ok" => (StatusCode::OK, "ok\n").into_response(),
        _ => (StatusCode::SERVICE_UNAVAILABLE, "unavailable\n").into_response(),
    }
}

async fn not_found(State(state): State<AppState>) -> Response {
    (
        StatusCode::NOT_FOUND,
        ui::service_error(&state.settings.base_path),
    )
        .into_response()
}

fn internal_error(base_path: &str, error: anyhow::Error) -> Response {
    tracing::error!(error=%error, "request failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        ui::service_error(base_path),
    )
        .into_response()
}

fn client_source(state: &AppState, remote: SocketAddr, headers: &HeaderMap) -> String {
    let trusted = state
        .settings
        .trusted_proxies
        .iter()
        .any(|network| network.contains(&remote.ip()));
    if trusted
        && let Some(value) = headers.get("x-forwarded-for").and_then(|h| h.to_str().ok())
        && let Some(ip) = value
            .split(',')
            .next()
            .and_then(|v| IpAddr::from_str(v.trim()).ok())
    {
        return ip.to_string();
    }
    remote.ip().to_string()
}

fn origin_from_public_url(url: &str) -> Result<String> {
    let (scheme, rest) = url
        .split_once("://")
        .context("public URL needs an http or https scheme")?;
    if !matches!(scheme, "http" | "https") {
        bail!("public URL scheme must be http or https");
    }
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        bail!("public URL needs a safe host");
    }
    Ok(format!("{scheme}://{authority}"))
}

async fn security_headers(request: Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    insert_header(headers, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    insert_header(headers, header::REFERRER_POLICY, "same-origin");
    insert_header(
        headers,
        header::STRICT_TRANSPORT_SECURITY,
        "max-age=31536000; includeSubDomains",
    );
    insert_header(
        headers,
        HeaderName::from_static("content-security-policy"),
        "default-src 'none'; style-src 'unsafe-inline'; img-src 'self' data:; form-action 'self'; frame-ancestors 'none'; base-uri 'none'; object-src 'none'; connect-src 'self'",
    );
    insert_header(
        headers,
        HeaderName::from_static("permissions-policy"),
        "accelerometer=(), camera=(), geolocation=(), gyroscope=(), microphone=(), payment=(), usb=()",
    );
    insert_header(
        headers,
        HeaderName::from_static("cross-origin-opener-policy"),
        "same-origin",
    );
    insert_header(
        headers,
        HeaderName::from_static("cross-origin-resource-policy"),
        "same-origin",
    );
    insert_header(
        headers,
        header::CACHE_CONTROL,
        "private, no-store, max-age=0",
    );
    response
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
    tracing::info!("graceful shutdown requested");
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use http_body_util::BodyExt;
    use sha2::Digest as _;

    #[test]
    fn byte_range_parser_covers_open_suffix_and_errors() {
        assert_eq!(
            parse_range("bytes=2-5", 10),
            RangeDecision::Satisfiable(2, 5)
        );
        assert_eq!(
            parse_range("bytes=7-", 10),
            RangeDecision::Satisfiable(7, 9)
        );
        assert_eq!(
            parse_range("bytes=-4", 10),
            RangeDecision::Satisfiable(6, 9)
        );
        assert_eq!(parse_range("bytes=20-30", 10), RangeDecision::Unsatisfiable);
        assert_eq!(parse_range("items=1-2", 10), RangeDecision::Ignore);
        assert_eq!(parse_range("bytes=4-2", 10), RangeDecision::Ignore);
        assert_eq!(parse_range("bytes=0-1,4-5", 10), RangeDecision::Ignore);
    }

    #[test]
    fn public_origin_is_strictly_derived() {
        assert_eq!(
            origin_from_public_url("https://example.test/frankenfile").unwrap(),
            "https://example.test"
        );
        assert!(origin_from_public_url("file:///tmp/no").is_err());
        assert!(origin_from_public_url("https://user@example.test/path").is_err());
    }

    #[tokio::test]
    async fn immutable_representation_supports_full_head_range_if_range_and_416() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("payload.bin");
        let bytes = b"0123456789abcdefghijklmnopqrstuvwxyz";
        std::fs::write(&path, bytes).unwrap();
        let digest = sha2::Sha256::digest(bytes);
        let representation = Representation {
            path,
            size: bytes.len() as u64,
            sha256_hex: hex::encode(digest),
            sha256_base64: base64::engine::general_purpose::STANDARD.encode(digest),
            media_type: "application/octet-stream".to_string(),
            download_name: "payload.bin".to_string(),
            last_modified: 1_700_000_000,
        };

        let full =
            serve_representation(Method::GET, &HeaderMap::new(), representation.clone()).await;
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.headers()[header::ACCEPT_RANGES], "bytes");
        assert_eq!(
            full.into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .as_ref(),
            bytes
        );

        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, HeaderValue::from_static("bytes=4-11"));
        headers.insert(
            header::IF_RANGE,
            HeaderValue::from_str(&format!("\"{}\"", representation.sha256_hex)).unwrap(),
        );
        let partial = serve_representation(Method::GET, &headers, representation.clone()).await;
        assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(partial.headers()[header::CONTENT_RANGE], "bytes 4-11/36");
        assert_eq!(
            partial
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .as_ref(),
            &bytes[4..=11]
        );

        let head =
            serve_representation(Method::HEAD, &HeaderMap::new(), representation.clone()).await;
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(head.headers()[header::CONTENT_LENGTH], "36");
        assert!(
            head.into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .is_empty()
        );

        let mut unsatisfiable = HeaderMap::new();
        unsatisfiable.insert(header::RANGE, HeaderValue::from_static("bytes=999-1000"));
        let response = serve_representation(Method::GET, &unsatisfiable, representation).await;
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes */36");
    }
}
