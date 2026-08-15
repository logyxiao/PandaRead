// 手机扫码阅读：内嵌 HTTP + SSE 服务器，局域网直连 + cloudflared 公网隧道。
// 参照 OpenPi 的 remote-access 模型：配对 token（12h 滑动、sha256 哈希存储）、
// 公网 6 位配对码（5 分钟轮换、timing-safe 比较、IP 限流）、Bearer 鉴权 + Origin 校验。
// 会话全部驻留内存，随应用退出销毁；停止服务器即踢出所有设备。

use crate::documents;
use crate::models::*;
use crate::AppState;
use parking_lot::Mutex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{self, BufRead, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

const PORT_FIRST: u16 = 43117;
const PORT_LAST: u16 = 43127;
const SESSION_LIFETIME: Duration = Duration::from_secs(12 * 3600);
const PAIRING_LIFETIME: Duration = Duration::from_secs(5 * 60);
const BODY_LIMIT: usize = 64 * 1024;
const PAIR_ATTEMPT_LIMIT: usize = 8;
const PAIR_ATTEMPT_WINDOW: Duration = Duration::from_secs(60);
const TUNNEL_TIMEOUT: Duration = Duration::from_secs(25);
const CLOUDFLARED_VERSION: &str = "2026.7.2";
const CLOUDFLARED_SHA256: &str = "0588df58494a6cadd38b9deb6078908a5054063c80784d92fdb8d4a5f3de1c67";

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDevice {
    pub device_id: String,
    pub name: String,
    pub ip: String,
    pub last_seen: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub running: bool,
    pub port: u16,
    pub lan_urls: Vec<String>,
    pub public_address: Option<String>,
    pub tunnel_state: String, // off | starting | on | error
    pub tunnel_error: Option<String>,
    pub pairing_code: String,
    pub devices: Vec<RemoteDevice>,
}

pub struct RemoteManager {
    app: AppHandle,
    state: Arc<AppState>,
    inner: Mutex<RemoteInner>,
}

struct RemoteInner {
    server: Option<ServerHandle>,
    tunnel: Option<TunnelHandle>,
    tunnel_starting: bool,
    public_address: Option<String>,
    tunnel_error: Option<String>,
    sessions: Vec<Session>,
    pairing_code: (String, Instant),
    pair_attempts: Vec<(String, Instant)>,
    subscribers: Vec<Sender<String>>,
}

struct ServerHandle {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct TunnelHandle {
    child: Child,
}

struct Session {
    token_hash: String,
    device_id: String,
    device_name: String,
    ip: String,
    origin_host: String,
    last_seen: Instant,
}

impl RemoteManager {
    pub fn new(app: AppHandle, state: Arc<AppState>) -> Self {
        Self {
            app,
            state,
            inner: Mutex::new(RemoteInner {
                server: None,
                tunnel: None,
                tunnel_starting: false,
                public_address: None,
                tunnel_error: None,
                sessions: Vec::new(),
                pairing_code: (generate_pairing_code(), Instant::now()),
                pair_attempts: Vec::new(),
                subscribers: Vec::new(),
            }),
        }
    }

    pub fn start(self: &Arc<Self>) -> Result<RemoteStatus, AppError> {
        let mut inner = self.inner.lock();
        if inner.server.is_some() {
            return Ok(self.status_locked(&mut inner));
        }
        let mut bound: Option<(Server, u16)> = None;
        for port in PORT_FIRST..=PORT_LAST {
            if let Ok(server) = Server::http(("0.0.0.0", port)) {
                bound = Some((server, port));
                break;
            }
        }
        let Some((server, port)) = bound else {
            return Err(AppError::Message(format!(
                "没有可用端口（{PORT_FIRST}-{PORT_LAST} 均被占用），请关闭占用程序后重试"
            )));
        };
        let mgr = self.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stop_flag.load(Ordering::SeqCst) {
                match server.recv_timeout(Duration::from_millis(300)) {
                    Ok(Some(request)) => {
                        let mgr = mgr.clone();
                        std::thread::spawn(move || mgr.handle_request(request));
                    }
                    Ok(None) => continue,
                    Err(_) => break,
                }
            }
        });
        inner.server = Some(ServerHandle { port, stop, thread: Some(thread) });
        Ok(self.status_locked(&mut inner))
    }

    pub fn stop(&self) {
        let mut inner = self.inner.lock();
        inner.subscribers.clear();
        inner.sessions.clear();
        if let Some(tunnel) = inner.tunnel.take() {
            kill_tunnel(tunnel);
        }
        inner.tunnel_starting = false;
        inner.public_address = None;
        inner.tunnel_error = None;
        if let Some(handle) = inner.server.take() {
            handle.stop.store(true, Ordering::SeqCst);
            if let Some(thread) = handle.thread {
                let _ = thread.join();
            }
        }
    }

    /// 退出前调用：停止服务器并结束隧道，确保端口与子进程释放。
    pub fn stop_all(&self) {
        self.stop();
    }

    pub fn status(&self) -> RemoteStatus {
        self.status_locked(&mut self.inner.lock())
    }

    fn status_locked(&self, inner: &mut RemoteInner) -> RemoteStatus {
        let (running, port) = match &inner.server {
            Some(handle) => (true, handle.port),
            None => (false, PORT_FIRST),
        };
        let tunnel_state = if inner.tunnel.is_some() {
            "on"
        } else if inner.tunnel_starting {
            "starting"
        } else if inner.tunnel_error.is_some() {
            "error"
        } else {
            "off"
        };
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        RemoteStatus {
            running,
            port,
            lan_urls: lan_urls(port),
            public_address: inner.public_address.clone(),
            tunnel_state: tunnel_state.to_string(),
            tunnel_error: inner.tunnel_error.clone(),
            pairing_code: self.current_pairing_code(inner),
            devices: inner
                .sessions
                .iter()
                .map(|session| RemoteDevice {
                    device_id: session.device_id.clone(),
                    name: session.device_name.clone(),
                    ip: session.ip.clone(),
                    last_seen: now,
                })
                .collect(),
        }
    }

    /// 桌面端打开文稿时推送给所有已连接的手机（双向跟随）。
    pub fn broadcast_desktop_open(&self, document_id: &str, title: &str) {
        let event = format!(
            "event: desktop-open\ndata: {}\n\n",
            serde_json::json!({ "documentId": document_id, "title": title })
        );
        self.broadcast(event);
    }

    pub fn tunnel_start(self: &Arc<Self>) -> Result<(), AppError> {
        let mut inner = self.inner.lock();
        let Some(server) = &inner.server else {
            return Err(AppError::Message("请先启动手机阅读服务".into()));
        };
        let port = server.port;
        if inner.tunnel.is_some() || inner.tunnel_starting {
            return Ok(());
        }
        inner.tunnel_starting = true;
        inner.tunnel_error = None;
        drop(inner);
        let mgr = self.clone();
        std::thread::spawn(move || {
            let result = start_tunnel_process(&mgr, port);
            let mut inner = mgr.inner.lock();
            inner.tunnel_starting = false;
            match result {
                Ok(tunnel) => inner.tunnel = Some(tunnel),
                Err(error) => inner.tunnel_error = Some(error.to_string()),
            }
        });
        Ok(())
    }

    pub fn tunnel_stop(&self) {
        let mut inner = self.inner.lock();
        inner.tunnel_starting = false;
        inner.public_address = None;
        if let Some(tunnel) = inner.tunnel.take() {
            kill_tunnel(tunnel);
        }
    }

    // ---------- HTTP ----------

    fn handle_request(&self, mut request: Request) {
        let url = request.url().to_string();
        let (path, query) = match url.split_once('?') {
            Some((p, q)) => (p.to_string(), Some(q.to_string())),
            None => (url.clone(), None),
        };
        let ip = request
            .remote_addr()
            .map(|addr| addr.ip().to_string())
            .unwrap_or_default();
        let host = request
            .headers()
            .iter()
            .find(|header| header.field.equiv("Host"))
            .map(|header| header.value.as_str().to_string())
            .unwrap_or_default();
        let method = request.method().clone();

        if matches!(path.as_str(), "/" | "/remote.html" | "/index.html") && method == Method::Get {
            return self.respond_page(request);
        }
        if path == "/api/pair" {
            return self.handle_pair(request, method, ip, host);
        }
        if !path.starts_with("/api/") {
            return respond_text(request, 404, "Not Found");
        }

        let token = if path == "/api/events" {
            query
                .as_deref()
                .and_then(|query| query.split('&').find_map(|part| part.strip_prefix("token=")))
                .map(|value| value.to_string())
        } else {
            bearer_token(&request)
        };
        let Some(token) = token else {
            return respond_text(request, 401, "未授权");
        };
        let Some(device_name) = self.authenticate(&token, &host) else {
            return respond_text(request, 401, "会话已失效，请重新扫码");
        };

        match (method, path.as_str()) {
            (Method::Get, "/api/state") => {
                let result = self.state.db.snapshot();
                match result {
                    Ok(snapshot) => respond_json(request, 200, &serde_json::json!({
                        "libraries": snapshot.libraries,
                        "documents": snapshot.documents,
                        "tree": snapshot.tree,
                        "session": snapshot.session,
                    })),
                    Err(error) => respond_text(request, 500, &error.to_string()),
                }
            }
            (Method::Get, document_path) if document_path.starts_with("/api/document/") => {
                let document_id = &document_path["/api/document/".len()..];
                match documents::read(&self.state, document_id) {
                    Ok(content) => respond_json(request, 200, &serde_json::to_value(content).unwrap_or_default()),
                    Err(error) => respond_text(request, 404, &error.to_string()),
                }
            }
            (Method::Post, "/api/progress") => {
                let result = read_json_body(&mut request).and_then(|body| {
                    let progress: ReadingProgress = serde_json::from_value(body)
                        .map_err(|_| AppError::Message("进度数据格式错误".into()))?;
                    self.state.db.save_progress(progress)?;
                    // 桌面侧栏"已阅/继续阅读"即时刷新
                    let _ = self.app.emit("library-changed", ());
                    Ok(())
                });
                match result {
                    Ok(()) => respond_json(request, 200, &serde_json::json!({ "ok": true })),
                    Err(error) => respond_text(request, 400, &error.to_string()),
                }
            }
            (Method::Post, "/api/shelf") => {
                let result = read_json_body(&mut request).and_then(|body| {
                    let document_id = body
                        .get("documentId")
                        .and_then(|value| value.as_str())
                        .ok_or_else(|| AppError::Message("缺少 documentId".into()))?;
                    let shelf = body
                        .get("shelf")
                        .and_then(|value| value.as_str())
                        .ok_or_else(|| AppError::Message("缺少 shelf".into()))?;
                    self.state.db.update_shelf(document_id, shelf)?;
                    let _ = self.app.emit("library-changed", ());
                    Ok(())
                });
                match result {
                    Ok(()) => respond_json(request, 200, &serde_json::json!({ "ok": true })),
                    Err(error) => respond_text(request, 400, &error.to_string()),
                }
            }
            (Method::Post, "/api/previews") => {
                let result = read_json_body(&mut request).and_then(|body| {
                    let ids = body
                        .get("ids")
                        .and_then(|value| value.as_array())
                        .ok_or_else(|| AppError::Message("缺少 ids".into()))?;
                    if ids.len() > 500 {
                        return Err(AppError::Message("一次最多预览 500 篇".into()));
                    }
                    let mut cards = Vec::with_capacity(ids.len());
                    for id in ids {
                        if let Some(id) = id.as_str() {
                            if let Ok(paragraphs) = documents::preview(&self.state, id, 3, 80) {
                                cards.push(serde_json::json!({ "documentId": id, "paragraphs": paragraphs }));
                            }
                        }
                    }
                    Ok(cards)
                });
                match result {
                    Ok(cards) => respond_json(request, 200, &serde_json::json!({ "cards": cards })),
                    Err(error) => respond_text(request, 400, &error.to_string()),
                }
            }
            (Method::Post, "/api/open") => {
                let result = read_json_body(&mut request).and_then(|body| {
                    let document_id = body
                        .get("documentId")
                        .and_then(|value| value.as_str())
                        .ok_or_else(|| AppError::Message("缺少 documentId".into()))?;
                    let title = self
                        .state
                        .db
                        .documents()?
                        .iter()
                        .find(|document| document.id == document_id)
                        .map(|document| document.title.clone())
                        .unwrap_or_default();
                    let _ = self.app.emit(
                        "remote:phone-open",
                        serde_json::json!({ "documentId": document_id, "title": title, "deviceName": device_name }),
                    );
                    Ok(())
                });
                match result {
                    Ok(()) => respond_json(request, 200, &serde_json::json!({ "ok": true })),
                    Err(error) => respond_text(request, 400, &error.to_string()),
                }
            }
            (Method::Get, "/api/events") => self.respond_events(request),
            _ => respond_text(request, 404, "Not Found"),
        }
    }

    /// 校验 Bearer token 并滑动续期；返回设备名。
    fn authenticate(&self, token: &str, host: &str) -> Option<String> {
        let token_hash = hash_token(token);
        let mut inner = self.inner.lock();
        let session = inner.sessions.iter_mut().find(|session| session.token_hash == token_hash)?;
        if session.origin_host != host || session.last_seen.elapsed() > SESSION_LIFETIME {
            return None;
        }
        session.last_seen = Instant::now();
        Some(session.device_name.clone())
    }

    fn handle_pair(&self, request: Request, method: Method, ip: String, host: String) {
        let require_code = self.requires_pairing_code(&host);
        // 设备 IP 展示：经隧道访问时 remote_addr 恒为 127.0.0.1，改为标记公网
        let ip = if require_code { "公网".to_string() } else { ip };
        if method == Method::Get {
            return respond_json(request, 200, &serde_json::json!({ "requirePairingCode": require_code }));
        }
        // 限流只针对实际的配对尝试（POST），不影响手机端刷新页面
        if !self.allow_pair_attempt(&ip) {
            return respond_text(request, 429, "尝试次数过多，请稍后再试");
        }
        let mut request = request;
        let body = match read_json_body(&mut request) {
            Ok(body) => body,
            Err(error) => return respond_text(request, 400, &error.to_string()),
        };
        if require_code {
            let code = body.get("code").and_then(|value| value.as_str()).unwrap_or_default();
            if !self.check_pairing_code(code) {
                return respond_text(request, 403, "配对码错误");
            }
        }
        let device_id = Uuid::new_v4().simple().to_string();
        let token = Uuid::new_v4().simple().to_string();
        let device_name = body
            .get("deviceName")
            .and_then(|value| value.as_str())
            .filter(|name| !name.is_empty())
            .map(|name| name.chars().take(24).collect())
            .unwrap_or_else(|| "手机设备".to_string());
        let mut inner = self.inner.lock();
        inner.sessions.push(Session {
            token_hash: hash_token(&token),
            device_id: device_id.clone(),
            device_name,
            ip,
            origin_host: host,
            last_seen: Instant::now(),
        });
        respond_json(request, 200, &serde_json::json!({
            "token": token,
            "deviceId": device_id,
            "expiresIn": SESSION_LIFETIME.as_secs(),
        }));
    }

    fn respond_page(&self, request: Request) {
        let mut response = Response::from_string(MOBILE_PAGE.to_string());
        response.add_header(Header::from_bytes(
            b"Content-Type",
            b"text/html; charset=utf-8",
        ).unwrap());
        // 页面随应用版本更新，禁止浏览器缓存，避免手机端一直拿到旧界面
        response.add_header(Header::from_bytes(
            b"Cache-Control",
            b"no-cache, no-store, must-revalidate",
        ).unwrap());
        response.add_header(Header::from_bytes(
            b"Content-Security-Policy",
            b"default-src 'self'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'",
        ).unwrap());
        response.add_header(Header::from_bytes(b"X-Content-Type-Options", b"nosniff").unwrap());
        let _ = request.respond(response);
    }

    fn respond_events(&self, request: Request) {
        let (sender, receiver) = mpsc::channel::<String>();
        self.inner.lock().subscribers.push(sender);
        let headers = vec![
            Header::from_bytes(b"Content-Type", b"text/event-stream; charset=utf-8").unwrap(),
            Header::from_bytes(b"Cache-Control", b"no-cache").unwrap(),
        ];
        let response = Response::new(
            StatusCode(200),
            headers,
            SseReader { receiver, buffer: Vec::new(), cursor: 0 },
            None,
            None,
        );
        let _ = request.respond(response);
    }

    fn broadcast(&self, payload: String) {
        let mut inner = self.inner.lock();
        inner.subscribers.retain(|sender| sender.send(payload.clone()).is_ok());
    }

    fn current_pairing_code(&self, inner: &mut RemoteInner) -> String {
        // 5 分钟轮换：读取时发现过期则重新生成
        if inner.pairing_code.1.elapsed() > PAIRING_LIFETIME {
            inner.pairing_code = (generate_pairing_code(), Instant::now());
        }
        inner.pairing_code.0.clone()
    }

    fn check_pairing_code(&self, input: &str) -> bool {
        let mut inner = self.inner.lock();
        timing_safe_eq(&self.current_pairing_code(&mut inner), input.trim())
    }

    fn requires_pairing_code(&self, host: &str) -> bool {
        host.contains("trycloudflare.com")
    }

    fn allow_pair_attempt(&self, ip: &str) -> bool {
        let now = Instant::now();
        let mut inner = self.inner.lock();
        inner
            .pair_attempts
            .retain(|(_, time)| now.duration_since(*time) < PAIR_ATTEMPT_WINDOW);
        let count = inner.pair_attempts.iter().filter(|(addr, _)| addr == ip).count();
        if count >= PAIR_ATTEMPT_LIMIT {
            return false;
        }
        inner.pair_attempts.push((ip.to_string(), now));
        true
    }
}

// ---------- 工具函数 ----------

fn hash_token(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

fn timing_safe_eq(a: &str, b: &str) -> bool {
    Sha256::digest(a.as_bytes()) == Sha256::digest(b.as_bytes())
}

fn generate_pairing_code() -> String {
    let hex = Uuid::new_v4().simple().to_string();
    let digits: String = hex.chars().filter(|c| c.is_ascii_digit()).take(6).collect();
    if digits.len() == 6 {
        digits
    } else {
        // 概率极低：uuid 中数字不足 6 位时用时间兜底
        let millis = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().subsec_millis();
        format!("{:06}", millis)
    }
}

fn bearer_token(request: &Request) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv("Authorization"))
        .and_then(|header| header.value.as_str().strip_prefix("Bearer "))
        .map(|token| token.to_string())
}

fn read_json_body(request: &mut Request) -> Result<serde_json::Value, AppError> {
    for header in request.headers() {
        if header.field.equiv("Content-Length") {
            if let Ok(length) = header.value.as_str().parse::<usize>() {
                if length > BODY_LIMIT {
                    return Err(AppError::Message("请求体过大".into()));
                }
            }
        }
    }
    let mut bytes = Vec::new();
    request
        .as_reader()
        .take((BODY_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| AppError::Message(format!("读取请求失败：{error}")))?;
    if bytes.len() > BODY_LIMIT {
        return Err(AppError::Message("请求体过大".into()));
    }
    serde_json::from_slice(&bytes).map_err(|_| AppError::Message("请求体不是合法 JSON".into()))
}

fn respond_json(request: Request, status: u16, value: &serde_json::Value) {
    let mut response = Response::from_string(value.to_string()).with_status_code(StatusCode(status));
    response.add_header(Header::from_bytes(b"Content-Type", b"application/json; charset=utf-8").unwrap());
    let _ = request.respond(response);
}

fn respond_text(request: Request, status: u16, text: &str) {
    let mut response = Response::from_string(text.to_string()).with_status_code(StatusCode(status));
    response.add_header(Header::from_bytes(b"Content-Type", b"text/plain; charset=utf-8").unwrap());
    let _ = request.respond(response);
}

fn lan_urls(port: u16) -> Vec<String> {
    let mut urls = Vec::new();
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for interface in interfaces {
            if interface.is_loopback() {
                continue;
            }
            if let std::net::IpAddr::V4(ip) = interface.ip() {
                if ip.is_private() {
                    urls.push(format!("http://{ip}:{port}"));
                }
            }
        }
    }
    urls.sort_by_key(|url| if url.starts_with("http://192.168.") { 0 } else { 1 });
    urls
}

// ---------- SSE ----------

/// 从广播通道读取事件；15 秒无消息时输出心跳注释保持连接。
struct SseReader {
    receiver: mpsc::Receiver<String>,
    buffer: Vec<u8>,
    cursor: usize,
}

impl Read for SseReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.cursor < self.buffer.len() {
                let count = std::cmp::min(buf.len(), self.buffer.len() - self.cursor);
                buf[..count].copy_from_slice(&self.buffer[self.cursor..self.cursor + count]);
                self.cursor += count;
                return Ok(count);
            }
            self.buffer.clear();
            self.cursor = 0;
            match self.receiver.recv_timeout(Duration::from_secs(15)) {
                Ok(message) => self.buffer = message.into_bytes(),
                Err(RecvTimeoutError::Timeout) => self.buffer = b": ping\n\n".to_vec(),
                Err(RecvTimeoutError::Disconnected) => return Ok(0),
            }
        }
    }
}

// ---------- cloudflared 隧道 ----------

fn start_tunnel_process(mgr: &Arc<RemoteManager>, port: u16) -> Result<TunnelHandle, AppError> {
    let binary = ensure_cloudflared(&mgr.state.data_dir)?;
    let mut child = Command::new(&binary)
        .args(["tunnel", "--url", &format!("http://127.0.0.1:{port}"), "--no-autoupdate"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped()) // cloudflared 日志（含公网 URL）输出到 stderr
        .spawn()
        .map_err(|error| AppError::Message(format!("启动 cloudflared 失败：{error}")))?;
    let stderr = child.stderr.take().ok_or_else(|| AppError::Message("无法读取 cloudflared 输出".into()))?;
    let (sender, receiver) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut reader = io::BufReader::new(stderr);
        let mut line = String::new();
        while reader.read_line(&mut line).is_ok() && !line.is_empty() {
            if let Some(start) = line.find("https://") {
                let url = line[start..].split_whitespace().next().unwrap_or_default().to_string();
                if url.contains("trycloudflare.com") {
                    let _ = sender.send(url);
                }
            }
            line.clear();
        }
    });
    let url = match receiver.recv_timeout(TUNNEL_TIMEOUT) {
        Ok(url) => url,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AppError::Message(
                "隧道连接超时（25 秒），请检查网络后重试".into(),
            ));
        }
    };
    mgr.inner.lock().public_address = Some(url);
    Ok(TunnelHandle { child })
}

fn kill_tunnel(mut tunnel: TunnelHandle) {
    let _ = tunnel.child.kill();
    let _ = tunnel.child.wait();
}

/// 优先使用系统 PATH 中的 cloudflared；否则下载固定版本并校验 sha256 到 data_dir/bin。
fn ensure_cloudflared(data_dir: &PathBuf) -> Result<PathBuf, AppError> {
    if let Ok(output) = Command::new("which").arg("cloudflared").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
    }
    if !cfg!(target_os = "macos") {
        return Err(AppError::Message("当前系统暂不支持公网隧道（仅 macOS）".into()));
    }
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86_64") {
        "amd64"
    } else {
        return Err(AppError::Message("当前架构暂不支持公网隧道".into()));
    };
    let bin_dir = data_dir.join("bin");
    let target = bin_dir.join("cloudflared");
    if target.exists() {
        if verify_sha256(&target) {
            return Ok(target);
        }
        let _ = std::fs::remove_file(&target);
    }
    std::fs::create_dir_all(&bin_dir)
        .map_err(|error| AppError::Message(format!("创建目录失败：{error}")))?;
    let url = format!(
        "https://github.com/cloudflare/cloudflared/releases/download/{CLOUDFLARED_VERSION}/cloudflared-darwin-{arch}.tgz"
    );
    let archive = bin_dir.join("cloudflared.tgz");
    let status = Command::new("curl")
        .args(["-L", "--fail", "--connect-timeout", "15", "--max-time", "300", "-o"])
        .arg(&archive)
        .arg(&url)
        .status()
        .map_err(|error| AppError::Message(format!("无法调用 curl 下载 cloudflared：{error}")))?;
    if !status.success() {
        let _ = std::fs::remove_file(&archive);
        return Err(AppError::Message(
            "cloudflared 下载失败，请检查网络后重试".into(),
        ));
    }
    let status = Command::new("tar")
        .args(["-xzf"])
        .arg(&archive)
        .arg("-C")
        .arg(&bin_dir)
        .status()
        .map_err(|error| AppError::Message(format!("无法解压 cloudflared：{error}")))?;
    let _ = std::fs::remove_file(&archive);
    if !status.success() || !target.exists() {
        return Err(AppError::Message("cloudflared 解压失败".into()));
    }
    if !verify_sha256(&target) {
        let _ = std::fs::remove_file(&target);
        return Err(AppError::Message(
            "cloudflared 校验失败，已删除文件，请重试".into(),
        ));
    }
    let _ = Command::new("chmod").arg("+x").arg(&target).status();
    Ok(target)
}

fn verify_sha256(path: &PathBuf) -> bool {
    std::fs::read(path)
        .map(|bytes| format!("{:x}", Sha256::digest(&bytes)) == CLOUDFLARED_SHA256)
        .unwrap_or(false)
}

// ---------- 手机页面 ----------

const MOBILE_PAGE: &str = include_str!("../remote.html");
