use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use keyring::Entry;
use reqwest::blocking::{multipart, Client};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    error::{AppError, AppResult},
    export,
    models::{
        BaiduDirectoryListing, BaiduFolder, BaiduStatus, ExportFailure, ExportPlan, ExportSummary,
    },
};

const SERVICE: &str = "com.nini-albums.baidu-netdisk";
const TOKEN_ACCOUNT: &str = "local-oauth-token-v1";
const LEGACY_ACCOUNTS: [&str; 4] = [
    "installation-token",
    "access-token",
    "app-credentials",
    "oauth-token",
];
const APP_ROOT: &str = "/apps/尼尼相册";
const CHUNK_SIZE: usize = 4 * 1024 * 1024;
const OAUTH_CALLBACK: &str = "http://127.0.0.1:8787/oauth/baidu/callback";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

static LOGIN_RUNNING: AtomicBool = AtomicBool::new(false);
static LOGIN_ERROR: Mutex<Option<String>> = Mutex::new(None);

#[derive(Debug, Serialize, Deserialize)]
struct StoredToken {
    access_token: String,
    refresh_token: String,
    expires_at: u64,
    user_name: String,
}

#[derive(Debug, Deserialize)]
struct BaiduTokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
}

fn invalid(message: impl Into<String>) -> AppError {
    AppError::InvalidInput(message.into())
}

fn entry(account: &str) -> AppResult<Entry> {
    Entry::new(SERVICE, account).map_err(|error| invalid(format!("无法访问系统凭据存储：{error}")))
}

fn load_json<T: for<'de> Deserialize<'de>>(account: &str) -> AppResult<Option<T>> {
    let value = match entry(account)?.get_password() {
        Ok(value) => value,
        Err(keyring::Error::NoEntry) => return Ok(None),
        Err(error) => return Err(invalid(format!("无法读取系统凭据：{error}"))),
    };
    serde_json::from_str(&value)
        .map(Some)
        .map_err(|error| invalid(format!("系统凭据格式无效：{error}")))
}

fn save_json<T: Serialize>(account: &str, value: &T) -> AppResult<()> {
    let value = serde_json::to_string(value).map_err(|error| invalid(error.to_string()))?;
    entry(account)?
        .set_password(&value)
        .map_err(|error| invalid(format!("无法写入系统凭据：{error}")))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn embedded_value(name: &str, compiled: Option<&'static str>) -> Option<String> {
    std::env::var(name)
        .ok()
        .or_else(|| compiled.map(str::to_owned))
        .filter(|value| !value.trim().is_empty())
}

fn oauth_credentials() -> AppResult<(String, String)> {
    let app_key = embedded_value("NINI_BAIDU_APP_KEY", option_env!("NINI_BAIDU_APP_KEY"))
        .ok_or_else(|| invalid("当前构建未配置百度 AppKey"))?;
    let secret_key = embedded_value(
        "NINI_BAIDU_SECRET_KEY",
        option_env!("NINI_BAIDU_SECRET_KEY"),
    )
    .ok_or_else(|| invalid("当前构建未配置百度 SecretKey"))?;
    Ok((app_key, secret_key))
}

pub fn status() -> AppResult<BaiduStatus> {
    cleanup_legacy_credentials();
    let token = load_json::<StoredToken>(TOKEN_ACCOUNT)?;
    let error = LOGIN_ERROR.lock().ok().and_then(|error| error.clone());
    Ok(BaiduStatus {
        configured: oauth_credentials().is_ok(),
        authorized: token.is_some(),
        user_name: token.map(|token| token.user_name),
        error,
    })
}

fn cleanup_legacy_credentials() {
    for account in LEGACY_ACCOUNTS {
        if let Ok(entry) = entry(account) {
            let _ = entry.delete_credential();
        }
    }
}

pub fn open_authorization() -> AppResult<String> {
    let (app_key, secret_key) = oauth_credentials()?;
    LOGIN_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .map_err(|_| invalid("百度网盘登录正在进行中"))?;
    if let Ok(mut error) = LOGIN_ERROR.lock() {
        *error = None;
    }
    let listener = TcpListener::bind("127.0.0.1:8787").map_err(|error| {
        LOGIN_RUNNING.store(false, Ordering::SeqCst);
        invalid(format!(
            "无法启动登录回调服务，请确认 8787 端口未被占用：{error}"
        ))
    })?;
    listener.set_nonblocking(true).map_err(|error| {
        LOGIN_RUNNING.store(false, Ordering::SeqCst);
        AppError::Io(error)
    })?;
    let state = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let mut authorization_url =
        reqwest::Url::parse("https://openapi.baidu.com/oauth/2.0/authorize")
            .expect("Baidu authorization URL is valid");
    authorization_url
        .query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &app_key)
        .append_pair("redirect_uri", OAUTH_CALLBACK)
        .append_pair("scope", "basic,netdisk")
        .append_pair("display", "popup")
        .append_pair("state", &state);
    let url = authorization_url.to_string();
    let status = open_browser(&url).inspect_err(|_| {
        LOGIN_RUNNING.store(false, Ordering::SeqCst);
    })?;
    if !status.success() {
        LOGIN_RUNNING.store(false, Ordering::SeqCst);
        return Err(invalid("无法打开系统浏览器，请复制授权链接后手动打开"));
    }
    thread::spawn(move || {
        if let Err(error) = wait_for_callback(listener, &state, &app_key, &secret_key) {
            if let Ok(mut login_error) = LOGIN_ERROR.lock() {
                *login_error = Some(error.to_string());
            }
        }
        LOGIN_RUNNING.store(false, Ordering::SeqCst);
    });
    Ok(url)
}

pub fn disconnect() -> AppResult<()> {
    if let Ok(mut error) = LOGIN_ERROR.lock() {
        *error = None;
    }
    match entry(TOKEN_ACCOUNT)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(invalid(format!("无法删除授权信息：{error}"))),
    }
}

fn open_browser(url: &str) -> AppResult<std::process::ExitStatus> {
    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg(url).status()?;
    #[cfg(target_os = "windows")]
    let status = Command::new("cmd")
        .args(["/C", "start", "", url])
        .status()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open").arg(url).status()?;
    Ok(status)
}

fn access_token() -> AppResult<String> {
    let mut token =
        load_json::<StoredToken>(TOKEN_ACCOUNT)?.ok_or_else(|| invalid("请先登录百度网盘"))?;
    if token.expires_at > now().saturating_add(300) {
        return Ok(token.access_token);
    }
    let (app_key, secret_key) = oauth_credentials()?;
    let response = Client::new()
        .get("https://openapi.baidu.com/oauth/2.0/token")
        .query(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", token.refresh_token.as_str()),
            ("client_id", app_key.as_str()),
            ("client_secret", secret_key.as_str()),
        ])
        .send()
        .map_err(network_error)?;
    let refreshed: BaiduTokenResponse = oauth_response(response)?;
    token.access_token = refreshed.access_token;
    token.refresh_token = refreshed.refresh_token;
    token.expires_at = now().saturating_add(refreshed.expires_in);
    save_json(TOKEN_ACCOUNT, &token)?;
    Ok(token.access_token)
}

fn wait_for_callback(
    listener: TcpListener,
    expected_state: &str,
    app_key: &str,
    secret_key: &str,
) -> AppResult<()> {
    let started = Instant::now();
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                return handle_callback(&mut stream, expected_state, app_key, secret_key);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if started.elapsed() >= LOGIN_TIMEOUT {
                    return Err(invalid("百度网盘登录等待超时"));
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn handle_callback(
    stream: &mut TcpStream,
    expected_state: &str,
    app_key: &str,
    secret_key: &str,
) -> AppResult<()> {
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    let mut buffer = [0_u8; 16 * 1024];
    let read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| invalid("登录回调请求格式无效"))?;
    let callback_url = reqwest::Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|error| invalid(format!("登录回调地址无效：{error}")))?;
    if callback_url.path() != "/oauth/baidu/callback" {
        write_callback_response(stream, false, "登录回调路径无效")?;
        return Err(invalid("登录回调路径无效"));
    }
    let parameters: HashMap<_, _> = callback_url.query_pairs().into_owned().collect();
    let result = complete_local_authorization(&parameters, expected_state, app_key, secret_key);
    match &result {
        Ok(user_name) => write_callback_response(stream, true, user_name)?,
        Err(error) => write_callback_response(stream, false, &error.to_string())?,
    }
    result.map(|_| ())
}

fn complete_local_authorization(
    parameters: &HashMap<String, String>,
    expected_state: &str,
    app_key: &str,
    secret_key: &str,
) -> AppResult<String> {
    if let Some(error) = parameters.get("error") {
        return Err(invalid(format!("百度拒绝授权：{error}")));
    }
    let state = parameters
        .get("state")
        .ok_or_else(|| invalid("登录回调缺少 state"))?;
    if state != expected_state {
        return Err(invalid("登录回调 state 校验失败"));
    }
    let code = parameters
        .get("code")
        .ok_or_else(|| invalid("登录回调缺少授权码"))?;
    let response = Client::new()
        .get("https://openapi.baidu.com/oauth/2.0/token")
        .query(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("client_id", app_key),
            ("client_secret", secret_key),
            ("redirect_uri", OAUTH_CALLBACK),
        ])
        .send()
        .map_err(network_error)?;
    let token: BaiduTokenResponse = oauth_response(response)?;
    let user_name = fetch_user_name(&token.access_token)?;
    save_json(
        TOKEN_ACCOUNT,
        &StoredToken {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_at: now().saturating_add(token.expires_in),
            user_name: user_name.clone(),
        },
    )?;
    Ok(user_name)
}

fn fetch_user_name(access_token: &str) -> AppResult<String> {
    let response = Client::new()
        .get("https://pan.baidu.com/rest/2.0/xpan/nas")
        .query(&[("method", "uinfo"), ("access_token", access_token)])
        .send()
        .map_err(network_error)?;
    let value: Value = response_json(response)?;
    Ok(value
        .get("netdisk_name")
        .or_else(|| value.get("baidu_name"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("百度网盘用户")
        .to_owned())
}

fn oauth_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::blocking::Response,
) -> AppResult<T> {
    let status = response.status();
    let bytes = response.bytes().map_err(network_error)?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| invalid(format!("百度 OAuth 响应无法解析：{error}")))?;
    if !status.is_success() || value.get("error").is_some() {
        let message = value
            .get("error_description")
            .or_else(|| value.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("未知错误");
        return Err(invalid(format!("百度 OAuth 返回错误：{message}")));
    }
    serde_json::from_value(value)
        .map_err(|error| invalid(format!("百度 OAuth 响应字段不完整：{error}")))
}

fn write_callback_response(stream: &mut TcpStream, success: bool, message: &str) -> AppResult<()> {
    let title = if success {
        "登录成功"
    } else {
        "登录失败"
    };
    let color = if success { "#d9ff5b" } else { "#ff6b81" };
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>{title}</title><style>body{{font:16px system-ui;background:#111;color:#eee;display:grid;place-items:center;height:100vh;margin:0}}main{{text-align:center}}h1{{color:{color}}}</style><main><h1>百度网盘{title}</h1><p>{}</p><p>现在可以关闭此窗口。</p></main>",
        html_escape(message)
    );
    let status = if success { "200 OK" } else { "400 Bad Request" };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()?;
    Ok(())
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn network_error(error: reqwest::Error) -> AppError {
    invalid(format!("百度网盘网络请求失败：{error}"))
}

fn response_json<T: for<'de> Deserialize<'de>>(
    response: reqwest::blocking::Response,
) -> AppResult<T> {
    let status = response.status();
    let bytes = response.bytes().map_err(network_error)?;
    if !status.is_success() {
        return Err(invalid(format!("百度网盘请求失败（HTTP {status}）")));
    }
    if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
        if let Some(code) = value.get("error_code").and_then(Value::as_i64) {
            let message = value
                .get("error_description")
                .or_else(|| value.get("error_msg"))
                .and_then(Value::as_str)
                .unwrap_or("未知错误");
            return Err(invalid(format!("百度网盘返回错误 {code}：{message}")));
        }
        if let Some(errno) = value.get("errno").and_then(Value::as_i64) {
            if errno != 0 {
                return Err(invalid(format!("百度网盘返回错误码 {errno}")));
            }
        }
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| invalid(format!("百度网盘响应无法解析：{error}")))
}

fn validate_remote_path(path: &str) -> AppResult<()> {
    if path != APP_ROOT && !path.starts_with(&format!("{APP_ROOT}/")) {
        return Err(invalid("只能选择“我的应用数据/尼尼相册”内的目录"));
    }
    if path.split('/').any(|part| part == "..") {
        return Err(invalid("云盘目录不能包含 .."));
    }
    Ok(())
}

fn list_entries(token: &str, directory: &str, limit: usize) -> AppResult<Vec<Value>> {
    validate_remote_path(directory)?;
    let response = Client::new()
        .get("https://pan.baidu.com/rest/2.0/xpan/file")
        .query(&[
            ("method", "list"),
            ("access_token", token),
            ("dir", directory),
            ("order", "name"),
            ("start", "0"),
            ("limit", &limit.to_string()),
            ("web", "1"),
        ])
        .send()
        .map_err(network_error)?;
    let value: Value = response_json(response)?;
    Ok(value
        .get("list")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

pub fn list_folders(directory: Option<String>) -> AppResult<BaiduDirectoryListing> {
    let directory = directory.unwrap_or_else(|| APP_ROOT.to_owned());
    validate_remote_path(&directory)?;
    let token = access_token()?;
    let entries = list_entries(&token, &directory, 1000)?;
    let folders = entries
        .iter()
        .filter(|entry| entry.get("isdir").and_then(Value::as_i64) == Some(1))
        .filter_map(|entry| {
            Some(BaiduFolder {
                name: entry.get("server_filename")?.as_str()?.to_owned(),
                path: entry.get("path")?.as_str()?.to_owned(),
            })
        })
        .collect();
    Ok(BaiduDirectoryListing {
        current_path: directory,
        is_empty: entries.is_empty(),
        folders,
    })
}

fn safe_folder_name(name: &str) -> AppResult<&str> {
    let name = name.trim();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|character| character.is_control() || "/\\:*?\"<>|".contains(character))
    {
        return Err(invalid("文件夹名称为空或包含非法字符"));
    }
    Ok(name)
}

fn create_directory_with_token(token: &str, path: &str) -> AppResult<()> {
    validate_remote_path(path)?;
    let response = Client::new()
        .post("https://pan.baidu.com/rest/2.0/xpan/file")
        .query(&[("method", "create"), ("access_token", token)])
        .form(&[("path", path), ("isdir", "1"), ("rtype", "0")])
        .send()
        .map_err(network_error)?;
    let _: Value = response_json(response)?;
    Ok(())
}

pub fn create_folder(parent: String, name: String) -> AppResult<String> {
    validate_remote_path(&parent)?;
    let name = safe_folder_name(&name)?;
    let path = format!("{}/{}", parent.trim_end_matches('/'), name);
    let token = access_token()?;
    create_directory_with_token(&token, &path)?;
    Ok(path)
}

fn ensure_empty(token: &str, path: &str) -> AppResult<()> {
    if list_entries(token, path, 1)?.is_empty() {
        Ok(())
    } else {
        Err(invalid(format!("百度网盘导出根目录必须为空：{path}")))
    }
}

pub fn plan_export(database_path: &Path, root: &str) -> AppResult<ExportPlan> {
    let token = access_token()?;
    ensure_empty(&token, root)?;
    export::content_plan(database_path)
}

fn split_file<C>(path: &Path, checkpoint: &mut C) -> AppResult<(u64, Vec<String>)>
where
    C: FnMut() -> AppResult<()>,
{
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut hashes = Vec::new();
    let mut buffer = vec![0_u8; CHUNK_SIZE];
    loop {
        checkpoint()?;
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hashes.push(format!("{:x}", md5::compute(&buffer[..read])));
    }
    if hashes.is_empty() {
        hashes.push(format!("{:x}", md5::compute([])));
    }
    Ok((size, hashes))
}

fn upload_file<C>(
    token: &str,
    source: &Path,
    remote_path: &str,
    modified_time: f64,
    checkpoint: &mut C,
) -> AppResult<()>
where
    C: FnMut() -> AppResult<()>,
{
    let client = Client::new();
    let (size, hashes) = split_file(source, checkpoint)?;
    let block_list = serde_json::to_string(&hashes).map_err(|error| invalid(error.to_string()))?;
    let response = client
        .post("https://pan.baidu.com/rest/2.0/xpan/file")
        .query(&[("method", "precreate"), ("access_token", token)])
        .form(&[
            ("path", remote_path.to_owned()),
            ("size", size.to_string()),
            ("isdir", "0".to_owned()),
            ("autoinit", "1".to_owned()),
            ("rtype", "2".to_owned()),
            ("block_list", block_list.clone()),
        ])
        .send()
        .map_err(network_error)?;
    let precreate: Value = response_json(response)?;
    let upload_id = precreate
        .get("uploadid")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("百度网盘未返回 uploadid"))?;

    let locate = client
        .get("https://d.pcs.baidu.com/rest/2.0/pcs/file")
        .query(&[
            ("method", "locateupload"),
            ("appid", "250528"),
            ("access_token", token),
            ("path", remote_path),
            ("uploadid", upload_id),
            ("upload_version", "2.0"),
        ])
        .send()
        .map_err(network_error)?;
    let locate: Value = response_json(locate)?;
    let server = locate
        .get("servers")
        .and_then(Value::as_array)
        .and_then(|servers| servers.first())
        .and_then(|server| server.get("server"))
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("百度网盘未返回上传服务器"))?;
    if !server.starts_with("https://") {
        return Err(invalid("百度网盘返回了不安全的上传地址"));
    }

    let mut file = File::open(source)?;
    for part_index in 0..hashes.len() {
        checkpoint()?;
        let mut chunk = vec![0_u8; CHUNK_SIZE];
        let read = file.read(&mut chunk)?;
        chunk.truncate(read);
        let file_name = source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("photo");
        let form = multipart::Form::new().part(
            "file",
            multipart::Part::bytes(chunk).file_name(file_name.to_owned()),
        );
        let response = client
            .post(format!(
                "{}/rest/2.0/pcs/superfile2",
                server.trim_end_matches('/')
            ))
            .query(&[
                ("method", "upload"),
                ("access_token", token),
                ("type", "tmpfile"),
                ("path", remote_path),
                ("uploadid", upload_id),
                ("partseq", &part_index.to_string()),
            ])
            .multipart(form)
            .send()
            .map_err(network_error)?;
        let _: Value = response_json(response)?;
    }

    checkpoint()?;
    let timestamp = modified_time.max(0.0).trunc() as u64;
    let response = client
        .post("https://pan.baidu.com/rest/2.0/xpan/file")
        .query(&[("method", "create"), ("access_token", token)])
        .form(&[
            ("path", remote_path.to_owned()),
            ("size", size.to_string()),
            ("isdir", "0".to_owned()),
            ("rtype", "2".to_owned()),
            ("uploadid", upload_id.to_owned()),
            ("block_list", block_list),
            ("local_ctime", timestamp.to_string()),
            ("local_mtime", timestamp.to_string()),
        ])
        .send()
        .map_err(network_error)?;
    let _: Value = response_json(response)?;
    Ok(())
}

fn renamed_file_names(display_name: &str, names: &[String; 2], index: usize) -> [String; 2] {
    names.each_ref().map(|name| {
        let extension = Path::new(name)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if extension.is_empty() {
            format!("{display_name}_{index}")
        } else {
            format!("{display_name}_{index}.{extension}")
        }
    })
}

pub fn export_photos<F, C>(
    database_path: &Path,
    root: &str,
    strategy: &str,
    mut progress: F,
    mut checkpoint: C,
) -> AppResult<ExportSummary>
where
    F: FnMut(usize, usize, &str),
    C: FnMut() -> AppResult<()>,
{
    validate_remote_path(root)?;
    let token = access_token()?;
    ensure_empty(&token, root)?;
    let plan = export::content_plan(database_path)?;
    let manifest = export::manifest(database_path)?;
    let total_files = manifest.len() * 2;
    let mut summary = ExportSummary {
        untagged: plan.untagged,
        skipped_incomplete: plan.skipped_incomplete,
        skipped_missing: plan.skipped_missing,
        skipped_hidden: plan.skipped_hidden,
        skipped_trashed: plan.skipped_trashed,
        ..Default::default()
    };
    let mut directories = HashSet::from([root.to_owned()]);
    let mut occupied = HashSet::new();
    let mut completed_files = 0;
    progress(0, total_files, "");

    for item in manifest {
        checkpoint()?;
        let mut directory = root.trim_end_matches('/').to_owned();
        for component in &item.tag_components {
            directory.push('/');
            directory.push_str(component);
            if directories.insert(directory.clone()) {
                create_directory_with_token(&token, &directory)?;
            }
        }
        let original_names = [item.files[0].name.clone(), item.files[1].name.clone()];
        let original_keys = original_names
            .each_ref()
            .map(|name| format!("{directory}/{}", name.to_lowercase()));
        let conflict = original_keys.iter().any(|path| occupied.contains(path));
        if conflict && strategy == "skip" {
            summary.skipped += 1;
            completed_files += 2;
            progress(completed_files, total_files, "");
            continue;
        }
        let names = if conflict && strategy == "rename" {
            summary.renamed += 1;
            let mut index = 1;
            loop {
                let candidate = renamed_file_names(&item.display_name, &original_names, index);
                let keys = candidate
                    .each_ref()
                    .map(|name| format!("{directory}/{}", name.to_lowercase()));
                if keys.iter().all(|path| !occupied.contains(path)) {
                    break candidate;
                }
                index += 1;
            }
        } else {
            original_names
        };
        occupied.extend(
            names
                .each_ref()
                .map(|name| format!("{directory}/{}", name.to_lowercase())),
        );

        let item_start = completed_files;
        let mut item_failed = None;
        for (file, name) in item.files.iter().zip(names.iter()) {
            checkpoint()?;
            progress(completed_files, total_files, name);
            let remote_path = format!("{directory}/{name}");
            match upload_file(
                &token,
                &file.source,
                &remote_path,
                file.modified_time,
                &mut checkpoint,
            ) {
                Ok(()) => completed_files += 1,
                Err(AppError::Cancelled) => return Err(AppError::Cancelled),
                Err(error) => {
                    item_failed = Some(error.to_string());
                    break;
                }
            }
        }
        if let Some(reason) = item_failed {
            summary.failed += 1;
            summary.failures.push(ExportFailure {
                photo_id: item.photo_id,
                name: item.display_name,
                reason,
            });
        } else {
            summary.succeeded += 1;
        }
        completed_files = (item_start + 2).min(total_files);
        progress(completed_files, total_files, "");
    }
    progress(completed_files, total_files, "");
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_paths_outside_app_directory() {
        assert!(validate_remote_path("/apps/尼尼相册/旅行").is_ok());
        assert!(validate_remote_path("/我的资源").is_err());
        assert!(validate_remote_path("/apps/尼尼相册/../别处").is_err());
    }

    #[test]
    fn renames_a_pair_with_one_suffix() {
        let result = renamed_file_names("DSC0001", &["a.JPG".into(), "a.ARW".into()], 2);
        assert_eq!(result, ["DSC0001_2.JPG", "DSC0001_2.ARW"]);
    }

    #[test]
    fn rejects_a_callback_with_the_wrong_state_before_token_exchange() {
        let parameters = HashMap::from([
            ("state".into(), "unexpected".into()),
            ("code".into(), "authorization-code".into()),
        ]);
        let result = complete_local_authorization(&parameters, "expected", "app", "secret");
        assert!(
            matches!(result, Err(AppError::InvalidInput(message)) if message.contains("state"))
        );
    }
}
