use std::{
    env,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use axum::{
    extract::{Query, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::Url;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const STATE_TTL_SECONDS: u64 = 10 * 60;
const TOKEN_REFRESH_MARGIN_SECONDS: u64 = 5 * 60;

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    database: Arc<Mutex<Connection>>,
    cipher: Arc<Aes256Gcm>,
    http: reqwest::Client,
}

struct Config {
    app_key: String,
    secret_key: String,
    redirect_uri: String,
}

#[derive(Debug, thiserror::Error)]
enum ApiError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("not connected")]
    NotConnected,
    #[error("upstream service failed: {0}")]
    Upstream(String),
    #[error("internal server error")]
    Internal,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::NotConnected => StatusCode::NOT_FOUND,
            Self::Upstream(_) => StatusCode::BAD_GATEWAY,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}

#[derive(Serialize)]
struct StartResponse {
    authorization_url: String,
}

#[derive(Serialize)]
struct StatusResponse {
    authorized: bool,
    user_name: Option<String>,
}

#[derive(Serialize)]
struct ClientTokenResponse {
    access_token: String,
    expires_at: u64,
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct BaiduTokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
}

struct Binding {
    access_token: String,
    refresh_token: String,
    expires_at: u64,
}

#[tokio::main]
async fn main() {
    let state = build_state().unwrap_or_else(|error| {
        eprintln!("failed to start OAuth server: {error}");
        std::process::exit(1);
    });
    let port = env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8787);
    let host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let address: SocketAddr = format!("{host}:{port}").parse().unwrap_or_else(|error| {
        eprintln!("invalid HOST or PORT: {error}");
        std::process::exit(1);
    });
    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/baidu/oauth/start", post(start_authorization))
        .route("/v1/baidu/oauth/status", get(oauth_status))
        .route("/v1/baidu/oauth/binding", delete(disconnect))
        .route("/v1/baidu/token", get(client_token))
        .route("/oauth/baidu/callback", get(oauth_callback))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .unwrap_or_else(|error| {
            eprintln!("failed to bind {address}: {error}");
            std::process::exit(1);
        });
    println!("Nini OAuth server listening on http://{address}");
    axum::serve(listener, app)
        .await
        .expect("OAuth server failed");
}

fn required_env(name: &str) -> Result<String, String> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("missing environment variable {name}"))
}

fn build_state() -> Result<AppState, String> {
    let config = Config {
        app_key: required_env("BAIDU_APP_KEY")?,
        secret_key: required_env("BAIDU_SECRET_KEY")?,
        redirect_uri: required_env("BAIDU_REDIRECT_URI")?,
    };
    Url::parse(&config.redirect_uri)
        .map_err(|error| format!("invalid BAIDU_REDIRECT_URI: {error}"))?;
    let encryption_key = STANDARD
        .decode(required_env("NINI_TOKEN_ENCRYPTION_KEY")?)
        .map_err(|_| "NINI_TOKEN_ENCRYPTION_KEY must be base64".to_owned())?;
    if encryption_key.len() != 32 {
        return Err("NINI_TOKEN_ENCRYPTION_KEY must decode to exactly 32 bytes".into());
    }
    let cipher = Aes256Gcm::new_from_slice(&encryption_key)
        .map_err(|_| "invalid NINI_TOKEN_ENCRYPTION_KEY".to_owned())?;
    let database_path = env::var("NINI_OAUTH_DATABASE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("nini-oauth.sqlite"));
    let connection = Connection::open(database_path).map_err(|error| error.to_string())?;
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS oauth_states (
               state_hash TEXT PRIMARY KEY,
               device_hash TEXT NOT NULL,
               expires_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS baidu_bindings (
               device_hash TEXT PRIMARY KEY,
               baidu_uk TEXT NOT NULL,
               user_name TEXT NULL,
               access_token_encrypted TEXT NOT NULL,
               refresh_token_encrypted TEXT NOT NULL,
               expires_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_baidu_bindings_uk ON baidu_bindings(baidu_uk);",
        )
        .map_err(|error| error.to_string())?;
    Ok(AppState {
        config: Arc::new(config),
        database: Arc::new(Mutex::new(connection)),
        cipher: Arc::new(cipher),
        http: reqwest::Client::new(),
    })
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true }))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn device_hash(headers: &HeaderMap) -> Result<String, ApiError> {
    let header = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;
    let token = header
        .strip_prefix("Bearer ")
        .ok_or(ApiError::Unauthorized)?;
    if !(43..=256).contains(&token.len()) || !token.is_ascii() {
        return Err(ApiError::Unauthorized);
    }
    Ok(hash(token))
}

fn database(state: &AppState) -> Result<MutexGuard<'_, Connection>, ApiError> {
    state.database.lock().map_err(|_| ApiError::Internal)
}

fn encrypt(state: &AppState, plaintext: &str) -> Result<String, ApiError> {
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = state
        .cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|_| ApiError::Internal)?;
    let mut payload = nonce.to_vec();
    payload.extend(ciphertext);
    Ok(STANDARD.encode(payload))
}

fn decrypt(state: &AppState, encoded: &str) -> Result<String, ApiError> {
    let payload = STANDARD.decode(encoded).map_err(|_| ApiError::Internal)?;
    if payload.len() <= 12 {
        return Err(ApiError::Internal);
    }
    let plaintext = state
        .cipher
        .decrypt(Nonce::from_slice(&payload[..12]), &payload[12..])
        .map_err(|_| ApiError::Internal)?;
    String::from_utf8(plaintext).map_err(|_| ApiError::Internal)
}

async fn start_authorization(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<StartResponse>, ApiError> {
    let device_hash = device_hash(&headers)?;
    let state_value = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let state_hash = hash(&state_value);
    {
        let database = database(&state)?;
        database
            .execute("DELETE FROM oauth_states WHERE expires_at < ?1", [now()])
            .map_err(|_| ApiError::Internal)?;
        database
            .execute(
                "INSERT INTO oauth_states(state_hash,device_hash,expires_at) VALUES(?1,?2,?3)",
                params![state_hash, device_hash, now() + STATE_TTL_SECONDS],
            )
            .map_err(|_| ApiError::Internal)?;
    }
    let mut url = Url::parse("https://openapi.baidu.com/oauth/2.0/authorize")
        .map_err(|_| ApiError::Internal)?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &state.config.app_key)
        .append_pair("redirect_uri", &state.config.redirect_uri)
        .append_pair("scope", "basic,netdisk")
        .append_pair("display", "popup")
        .append_pair("state", &state_value);
    Ok(Json(StartResponse {
        authorization_url: url.into(),
    }))
}

async fn oauth_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<StatusResponse>, ApiError> {
    let device_hash = device_hash(&headers)?;
    let database = database(&state)?;
    let user_name = database
        .query_row(
            "SELECT user_name FROM baidu_bindings WHERE device_hash=?1",
            [device_hash],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|_| ApiError::Internal)?;
    Ok(Json(StatusResponse {
        authorized: user_name.is_some(),
        user_name: user_name.flatten(),
    }))
}

async fn disconnect(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let device_hash = device_hash(&headers)?;
    database(&state)?
        .execute(
            "DELETE FROM baidu_bindings WHERE device_hash=?1",
            [device_hash],
        )
        .map_err(|_| ApiError::Internal)?;
    Ok(Json(json!({ "ok": true })))
}

async fn oauth_callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    match complete_callback(&state, query).await {
        Ok(user_name) => Html(format!(
            "<!doctype html><meta charset=utf-8><title>授权成功</title><style>body{{font:16px system-ui;background:#111;color:#eee;display:grid;place-items:center;height:100vh;margin:0}}main{{text-align:center}}h1{{color:#d9ff5b}}</style><main><h1>百度网盘登录成功</h1><p>{user_name} 已与 Nini Albums 绑定，可以关闭此窗口。</p></main>"
            , user_name = html_escape(&user_name)
        ))
        .into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Html(format!(
                "<!doctype html><meta charset=utf-8><title>授权失败</title><h1>授权失败</h1><p>{}</p>", html_escape(&error.to_string())
            )),
        )
            .into_response(),
    }
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn complete_callback(state: &AppState, query: CallbackQuery) -> Result<String, ApiError> {
    if let Some(error) = query.error {
        return Err(ApiError::Invalid(format!("百度拒绝授权：{error}")));
    }
    let code = query
        .code
        .ok_or_else(|| ApiError::Invalid("缺少 code".into()))?;
    let state_value = query
        .state
        .ok_or_else(|| ApiError::Invalid("缺少 state".into()))?;
    let state_hash = hash(&state_value);
    let device_hash = {
        let database = database(state)?;
        let record = database
            .query_row(
                "SELECT device_hash,expires_at FROM oauth_states WHERE state_hash=?1",
                [&state_hash],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
            )
            .optional()
            .map_err(|_| ApiError::Internal)?
            .ok_or_else(|| ApiError::Invalid("授权请求不存在或已使用".into()))?;
        database
            .execute(
                "DELETE FROM oauth_states WHERE state_hash=?1",
                [&state_hash],
            )
            .map_err(|_| ApiError::Internal)?;
        if record.1 < now() {
            return Err(ApiError::Invalid("授权请求已过期".into()));
        }
        record.0
    };
    let token = exchange_code(state, &code).await?;
    let (baidu_uk, user_name) = fetch_user(state, &token.access_token).await?;
    let access_encrypted = encrypt(state, &token.access_token)?;
    let refresh_encrypted = encrypt(state, &token.refresh_token)?;
    let expires_at = now().saturating_add(token.expires_in);
    database(state)?
        .execute(
            "INSERT INTO baidu_bindings(
               device_hash,baidu_uk,user_name,access_token_encrypted,refresh_token_encrypted,expires_at,updated_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(device_hash) DO UPDATE SET
               baidu_uk=excluded.baidu_uk,user_name=excluded.user_name,
               access_token_encrypted=excluded.access_token_encrypted,
               refresh_token_encrypted=excluded.refresh_token_encrypted,
               expires_at=excluded.expires_at,updated_at=excluded.updated_at",
            params![
                device_hash,
                baidu_uk,
                user_name,
                access_encrypted,
                refresh_encrypted,
                expires_at,
                now()
            ],
        )
        .map_err(|_| ApiError::Internal)?;
    Ok(user_name)
}

async fn exchange_code(state: &AppState, code: &str) -> Result<BaiduTokenResponse, ApiError> {
    let response = state
        .http
        .get("https://openapi.baidu.com/oauth/2.0/token")
        .query(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", state.config.app_key.as_str()),
            ("client_secret", state.config.secret_key.as_str()),
            ("redirect_uri", state.config.redirect_uri.as_str()),
        ])
        .send()
        .await
        .map_err(|_| ApiError::Upstream("无法连接百度 OAuth".into()))?;
    parse_baidu_response(response).await
}

async fn fetch_user(state: &AppState, access_token: &str) -> Result<(String, String), ApiError> {
    let response = state
        .http
        .get("https://pan.baidu.com/rest/2.0/xpan/nas")
        .query(&[("method", "uinfo"), ("access_token", access_token)])
        .send()
        .await
        .map_err(|_| ApiError::Upstream("无法读取百度用户信息".into()))?;
    let value: Value = parse_baidu_response(response).await?;
    let uk = match value.get("uk") {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        _ => return Err(ApiError::Upstream("百度未返回用户标识".into())),
    };
    let user_name = value
        .get("netdisk_name")
        .or_else(|| value.get("baidu_name"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("百度网盘用户")
        .to_owned();
    Ok((uk, user_name))
}

async fn parse_baidu_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, ApiError> {
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|_| ApiError::Upstream("百度响应读取失败".into()))?;
    if !status.is_success() {
        return Err(ApiError::Upstream(format!("百度返回 HTTP {status}")));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::Upstream("百度响应格式错误".into()))?;
    if value.get("error").is_some() || value.get("error_code").is_some() {
        return Err(ApiError::Upstream("百度拒绝了授权或 Token 请求".into()));
    }
    if value
        .get("errno")
        .and_then(Value::as_i64)
        .is_some_and(|errno| errno != 0)
    {
        return Err(ApiError::Upstream("百度接口返回错误".into()));
    }
    serde_json::from_value(value).map_err(|_| ApiError::Upstream("百度响应字段不完整".into()))
}

fn load_binding(state: &AppState, device_hash: &str) -> Result<Binding, ApiError> {
    let database = database(state)?;
    let encrypted = database
        .query_row(
            "SELECT access_token_encrypted,refresh_token_encrypted,expires_at
             FROM baidu_bindings WHERE device_hash=?1",
            [device_hash],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|_| ApiError::Internal)?
        .ok_or(ApiError::NotConnected)?;
    drop(database);
    Ok(Binding {
        access_token: decrypt(state, &encrypted.0)?,
        refresh_token: decrypt(state, &encrypted.1)?,
        expires_at: encrypted.2,
    })
}

async fn client_token(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ClientTokenResponse>, ApiError> {
    let device_hash = device_hash(&headers)?;
    let mut binding = load_binding(&state, &device_hash)?;
    if binding.expires_at <= now().saturating_add(TOKEN_REFRESH_MARGIN_SECONDS) {
        let refreshed = state
            .http
            .get("https://openapi.baidu.com/oauth/2.0/token")
            .query(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", binding.refresh_token.as_str()),
                ("client_id", state.config.app_key.as_str()),
                ("client_secret", state.config.secret_key.as_str()),
            ])
            .send()
            .await
            .map_err(|_| ApiError::Upstream("无法刷新百度 Token".into()))?;
        let token: BaiduTokenResponse = parse_baidu_response(refreshed).await?;
        binding = Binding {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_at: now().saturating_add(token.expires_in),
        };
        database(&state)?
            .execute(
                "UPDATE baidu_bindings SET access_token_encrypted=?1,
                 refresh_token_encrypted=?2,expires_at=?3,updated_at=?4 WHERE device_hash=?5",
                params![
                    encrypt(&state, &binding.access_token)?,
                    encrypt(&state, &binding.refresh_token)?,
                    binding.expires_at,
                    now(),
                    device_hash
                ],
            )
            .map_err(|_| ApiError::Internal)?;
    }
    Ok(Json(ClientTokenResponse {
        access_token: binding.access_token,
        expires_at: binding.expires_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> AppState {
        let connection = Connection::open_in_memory().unwrap();
        AppState {
            config: Arc::new(Config {
                app_key: "key".into(),
                secret_key: "secret".into(),
                redirect_uri: "http://localhost/callback".into(),
            }),
            database: Arc::new(Mutex::new(connection)),
            cipher: Arc::new(Aes256Gcm::new_from_slice(&[7_u8; 32]).unwrap()),
            http: reqwest::Client::new(),
        }
    }

    #[test]
    fn encrypts_tokens_with_a_random_nonce() {
        let state = test_state();
        let first = encrypt(&state, "refresh-token").unwrap();
        let second = encrypt(&state, "refresh-token").unwrap();
        assert_ne!(first, second);
        assert_eq!(decrypt(&state, &first).unwrap(), "refresh-token");
    }

    #[test]
    fn device_tokens_are_hashed_before_storage() {
        let value = hash("device-secret");
        assert_ne!(value, "device-secret");
        assert_eq!(value.len(), 64);
    }
}
