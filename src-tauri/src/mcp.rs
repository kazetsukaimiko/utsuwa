//! Loopback MCP (Model Context Protocol) server.
//!
//! Desktop-only: binds `127.0.0.1` and speaks Streamable HTTP JSON-RPC so MCP
//! clients (Grok Build sessions, etc.) can puppet the running app. Each client
//! is a named session (machine name + short topic) with its own inbox.

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use tiny_http::{Header, Method, Response, Server, StatusCode};

const DEFAULT_BIND: &str = "127.0.0.1:8787";
const JS_TIMEOUT: Duration = Duration::from_secs(12);
const SESSION_TTL: Duration = Duration::from_secs(10 * 60);
const TOPIC_MAX_WORDS: usize = 7;

/// Always prepended on initialize. Keep in sync with src/lib/services/mcp-mode.ts.
pub const HARDCODED_MCP_INSTRUCTIONS: &str = "Utsuwa is a brief notification channel to the person at this machine: speech bubble, voice, and lip-sync. It is not a transcript of your work.\n\nUsage (do not ignore this section):\n- Do the actual work in this session as usual, whether the user spoke in this TUI or sent a line through Utsuwa's chat bar.\n- Chat-bar lines arrive via take_user_message as prompt. That is a real user message; answer it.\n- You only receive what they routed to this session. Call set_session with a 1-7 word topic for this terminal.\n- Poll take_user_message regularly, including while idle between TUI turns. If you stop polling, their chat-bar lines sit unseen.\n- Call speak with only the spoken payload in text (one or two sentences). Never speak code, diffs, logs, stack traces, or essays. Do not repeat the same status.\n- Pass plain: true only when the line must be said exactly as written.\n- A reply in this TUI does not replace speak(). Notify via speak at plan, blocker, and done even when the user asked here.\n- Do not stay silent through a long stretch of tool use. If you have not spoken in a while, send one short status line. \"This is a coding turn\" is not a reason to skip speak.\n\nDefault cadence (overridden by Preferences below):\n- When you have a plan: one short line that you are starting, and that you see a way forward.\n- When you are stuck on something they must fix: one line plus what you need from them.\n- When you finish: say you are done.\n- While grinding through routine errors: stay vague. Do not narrate every failure.";

/// Default contents of the settings textarea. Keep in sync with src/lib/services/mcp-mode.ts.
pub const DEFAULT_MCP_USER_INSTRUCTIONS: &str = "Keep updates short and spoken-friendly. A few per task is enough — not every tool call.\n\nGood:\n- \"Starting the search UI — I have a plan.\"\n- \"Need a newer runtime before this will build. Can you install it?\"\n- \"Working through a few errors.\"\n- \"That's in place.\"\n\nAvoid long explanations in speak(); put those in the TUI.";

fn is_legacy_mcp_instructions(text: &str) -> bool {
    text.contains("Building the session picker, I think I have an idea")
        || text.contains("Java is too old")
        || text.contains("Ironing out the bugs now")
}

fn normalize_user_instructions(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() || is_legacy_mcp_instructions(trimmed) {
        DEFAULT_MCP_USER_INSTRUCTIONS.to_string()
    } else {
        trimmed.to_string()
    }
}

fn compose_instructions(user: &str) -> String {
    format!(
        "{}\n\nPreferences (from the user at this machine):\n{}",
        HARDCODED_MCP_INSTRUCTIONS,
        normalize_user_instructions(user)
    )
}

#[derive(Clone)]
pub struct McpState {
    pending: Arc<Mutex<HashMap<String, Sender<JsReply>>>>,
    sessions: Arc<Mutex<HashMap<String, ClientSession>>>,
    app: Arc<Mutex<Option<AppHandle>>>,
    instructions: Arc<Mutex<String>>,
}

#[derive(Debug, Clone)]
struct ClientSession {
    id: String,
    name: String,
    topic: String,
    last_seen: Instant,
    inbox: VecDeque<InboxItem>,
}

#[derive(Debug, Clone)]
struct InboxItem {
    id: String,
    text: String,
}

#[derive(Debug, Clone)]
pub struct JsReply {
    pub ok: bool,
    pub payload: Value,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct JsCommand {
    id: String,
    tool: String,
    target: String,
    session_id: String,
    speaker_name: String,
    speaker_topic: String,
    arguments: Value,
}

impl McpState {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            app: Arc::new(Mutex::new(None)),
            instructions: Arc::new(Mutex::new(DEFAULT_MCP_USER_INSTRUCTIONS.to_string())),
        }
    }

    fn instructions_text(&self) -> String {
        let user = self.instructions.lock().expect("mcp instructions").clone();
        compose_instructions(&user)
    }

    fn set_instructions(&self, text: String) {
        *self.instructions.lock().expect("mcp instructions") = normalize_user_instructions(&text);
    }

    fn attach_app(&self, app: AppHandle) {
        *self.app.lock().expect("mcp app") = Some(app);
    }

    fn register_pending(&self, id: String) -> mpsc::Receiver<JsReply> {
        let (tx, rx) = mpsc::channel();
        self.pending.lock().expect("mcp pending").insert(id, tx);
        rx
    }

    fn complete(&self, id: &str, reply: JsReply) {
        if let Some(tx) = self.pending.lock().expect("mcp pending").remove(id) {
            let _ = tx.send(reply);
        }
    }

    fn claim_session(&self, name: String, topic: String) -> ClientSession {
        let id = next_id("sess");
        let session = ClientSession {
            id: id.clone(),
            name,
            topic: clamp_topic(&topic),
            last_seen: Instant::now(),
            inbox: VecDeque::new(),
        };
        self.sessions
            .lock()
            .expect("mcp sessions")
            .insert(id, session.clone());
        self.emit_sessions();
        session
    }

    fn drop_session(&self, id: &str) {
        self.sessions.lock().expect("mcp sessions").remove(id);
        self.emit_sessions();
    }

    fn touch(&self, id: &str) -> bool {
        self.prune();
        let mut sessions = self.sessions.lock().expect("mcp sessions");
        if let Some(s) = sessions.get_mut(id) {
            s.last_seen = Instant::now();
            true
        } else {
            false
        }
    }

    fn get(&self, id: &str) -> Option<ClientSession> {
        self.prune();
        self.sessions.lock().expect("mcp sessions").get(id).cloned()
    }

    fn set_identity(&self, id: &str, name: Option<String>, topic: Option<String>) -> Option<ClientSession> {
        self.prune();
        let mut sessions = self.sessions.lock().expect("mcp sessions");
        let session = sessions.get_mut(id)?;
        if let Some(n) = name {
            let n = n.trim();
            if !n.is_empty() {
                session.name = n.to_string();
            }
        }
        if let Some(t) = topic {
            session.topic = clamp_topic(&t);
        }
        session.last_seen = Instant::now();
        let clone = session.clone();
        drop(sessions);
        self.emit_sessions();
        Some(clone)
    }

    fn enqueue(&self, id: &str, text: String) -> Result<InboxItem, String> {
        self.prune();
        let mut sessions = self.sessions.lock().expect("mcp sessions");
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| "that MCP session is no longer connected".to_string())?;
        let item = InboxItem {
            id: next_id("msg"),
            text,
        };
        session.inbox.push_back(item.clone());
        session.last_seen = Instant::now();
        drop(sessions);
        self.emit_sessions();
        Ok(item)
    }

    fn take(&self, id: &str) -> Option<InboxItem> {
        self.prune();
        let mut sessions = self.sessions.lock().expect("mcp sessions");
        let session = sessions.get_mut(id)?;
        session.last_seen = Instant::now();
        let item = session.inbox.pop_front();
        drop(sessions);
        if item.is_some() {
            self.emit_sessions();
        }
        item
    }

    fn list_public(&self) -> Value {
        self.prune();
        self.snapshot_sessions()
    }

    fn snapshot_sessions(&self) -> Value {
        let sessions = self.sessions.lock().expect("mcp sessions");
        let list: Vec<Value> = sessions
            .values()
            .map(|s| {
                json!({
                    "id": s.id,
                    "name": s.name,
                    "topic": s.topic,
                    "pending": s.inbox.len()
                })
            })
            .collect();
        json!(list)
    }

    fn prune(&self) {
        let mut sessions = self.sessions.lock().expect("mcp sessions");
        let before = sessions.len();
        sessions.retain(|_, s| s.last_seen.elapsed() < SESSION_TTL);
        let changed = sessions.len() != before;
        drop(sessions);
        if changed {
            self.emit_sessions();
        }
    }

    fn emit_sessions(&self) {
        // Snapshot first so we never hold the app lock while listing (and
        // listing never prunes, so this cannot re-enter emit_sessions).
        let payload = self.snapshot_sessions();
        if let Some(app) = self.app.lock().expect("mcp app").as_ref() {
            let _ = app.emit("mcp:sessions-changed", payload);
        }
    }
}

#[tauri::command]
pub fn mcp_reply(state: State<McpState>, id: String, ok: bool, payload: Value) {
    state.complete(&id, JsReply { ok, payload });
}

#[tauri::command]
pub fn mcp_list_sessions(state: State<McpState>) -> Value {
    state.list_public()
}

#[tauri::command]
pub fn mcp_enqueue_user(state: State<McpState>, session_id: String, text: String) -> Result<Value, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("text is required".into());
    }
    let item = state.enqueue(&session_id, text)?;
    Ok(json!({ "id": item.id, "queued": true }))
}

#[tauri::command]
pub fn mcp_host_name() -> String {
    default_host_name()
}

#[tauri::command]
pub fn mcp_set_instructions(state: State<McpState>, text: String) {
    state.set_instructions(text);
}

pub fn start(app: AppHandle, state: McpState) {
    state.attach_app(app.clone());
    let bind = std::env::var("UTSUWA_MCP_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_string());
    thread::Builder::new()
        .name("utsuwa-mcp".into())
        .spawn(move || run_server(app, state, bind))
        .expect("spawn MCP thread");
}

fn run_server(app: AppHandle, state: McpState, bind: String) {
    let server = match Server::http(&bind) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("[utsuwa-mcp] failed to bind {bind}: {err}");
            return;
        }
    };
    eprintln!("[utsuwa-mcp] listening on http://{bind}/mcp");

    for request in server.incoming_requests() {
        if let Err(err) = handle_http(&app, &state, request) {
            eprintln!("[utsuwa-mcp] request error: {err}");
        }
    }
}

fn handle_http(
    app: &AppHandle,
    state: &McpState,
    mut request: tiny_http::Request,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !host_allowed(&request) {
        request.respond(text_response(StatusCode(403), "forbidden host"))?;
        return Ok(());
    }
    if let Some(origin) = header_value(&request, "Origin") {
        if !origin_allowed(&origin) {
            request.respond(text_response(StatusCode(403), "forbidden origin"))?;
            return Ok(());
        }
    }

    let path = request.url().split('?').next().unwrap_or("/");
    let method = request.method().clone();

    if method == Method::Get && (path == "/health" || path == "/") {
        let body = json!({
            "status": "ok",
            "mcp": "http://127.0.0.1:8787/mcp",
            "name": "utsuwa",
            "hostName": default_host_name(),
            "sessions": state.list_public()
        })
        .to_string();
        request.respond(json_response(StatusCode(200), body, None))?;
        return Ok(());
    }

    if method == Method::Get && path == "/mcp" {
        let mut response = text_response(StatusCode(405), "method not allowed");
        if let Ok(h) = Header::from_bytes(b"Allow", b"POST") {
            response.add_header(h);
        }
        request.respond(response)?;
        return Ok(());
    }

    if method == Method::Delete && path == "/mcp" {
        if let Some(sid) = header_value(&request, "Mcp-Session-Id") {
            state.drop_session(&sid);
        }
        request.respond(Response::empty(StatusCode(204)))?;
        return Ok(());
    }

    if method != Method::Post || (path != "/mcp" && path != "/") {
        request.respond(text_response(StatusCode(404), "not found"))?;
        return Ok(());
    }

    let header_name = header_value(&request, "X-Utsuwa-Name");
    let header_topic = header_value(&request, "X-Utsuwa-Topic");
    let session_header = header_value(&request, "Mcp-Session-Id");

    let mut buf = Vec::new();
    std::io::Read::read_to_end(request.as_reader(), &mut buf)?;
    let body = String::from_utf8_lossy(&buf);

    if body.trim().is_empty() {
        request.respond(json_response(StatusCode(202), String::new(), session_header.as_deref()))?;
        return Ok(());
    }

    let parsed: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(err) => {
            let err_body = jsonrpc_error(Value::Null, -32700, format!("parse error: {err}"));
            request.respond(json_response(StatusCode(200), err_body.to_string(), None))?;
            return Ok(());
        }
    };

    let rpc_method = parsed.get("method").and_then(|m| m.as_str()).unwrap_or("");

    if parsed.get("id").is_none() && parsed.get("method").is_some() && rpc_method != "initialize" {
        if let Some(sid) = session_header.as_deref() {
            let _ = state.touch(sid);
        }
        request.respond(Response::empty(StatusCode(202)))?;
        return Ok(());
    }

    if rpc_method == "initialize" {
        let params = parsed.get("params");
        let name = header_name
            .or_else(|| client_info_name(params))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(default_host_name);
        let topic = header_topic.unwrap_or_default();
        let session = state.claim_session(name, topic);
        eprintln!(
            "[utsuwa-mcp] session {} name={} topic={:?}",
            session.id, session.name, session.topic
        );
        let reply = match classify_rpc(&parsed) {
            Ok(RpcAction::Respond(mut value)) => {
                if let Some(result) = value.get_mut("result") {
                    if let Some(obj) = result.as_object_mut() {
                        obj.insert("sessionId".into(), json!(session.id));
                        obj.insert("instructions".into(), json!(state.instructions_text()));
                    }
                }
                value
            }
            Err(err_val) => err_val,
            Ok(_) => jsonrpc_error(parsed.get("id").cloned().unwrap_or(Value::Null), -32603, "initialize failed".into()),
        };
        request.respond(json_response(StatusCode(200), reply.to_string(), Some(&session.id)))?;
        return Ok(());
    }

    let Some(session_id) = session_header else {
        request.respond(text_response(StatusCode(404), "missing Mcp-Session-Id"))?;
        return Ok(());
    };
    if !state.touch(&session_id) {
        request.respond(text_response(StatusCode(404), "unknown MCP session"))?;
        return Ok(());
    }

    let reply = match classify_rpc(&parsed) {
        Ok(RpcAction::Respond(value)) => value,
        Ok(RpcAction::Call { id, name, arguments }) => match name.as_str() {
            "set_session" => handle_set_session(state, &session_id, id, arguments),
            "take_user_message" => handle_take(state, &session_id, id),
            other => {
                let session = state.get(&session_id);
                call_webview(
                    app,
                    state,
                    id,
                    other.to_string(),
                    arguments,
                    session.as_ref(),
                )
            }
        },
        Err(err_val) => err_val,
    };

    request.respond(json_response(StatusCode(200), reply.to_string(), Some(&session_id)))?;
    Ok(())
}

fn handle_set_session(state: &McpState, session_id: &str, id: Value, arguments: Value) -> Value {
    let name = arguments
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let topic = arguments
        .get("topic")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    match state.set_identity(session_id, name, topic) {
        Some(session) => jsonrpc_result(
            id,
            wrap_tool_result(
                json!({
                    "id": session.id,
                    "name": session.name,
                    "topic": session.topic
                }),
                false,
            ),
        ),
        None => jsonrpc_error(id, -32000, "session gone".into()),
    }
}

fn handle_take(state: &McpState, session_id: &str, id: Value) -> Value {
    let session = state.get(session_id);
    let item = state.take(session_id);
    let payload = match (session, item) {
        (Some(s), Some(msg)) => json!({
            "empty": false,
            "id": msg.id,
            "prompt": msg.text,
            "name": s.name,
            "topic": s.topic
        }),
        (Some(s), None) => json!({
            "empty": true,
            "pending": 0,
            "name": s.name,
            "topic": s.topic
        }),
        _ => json!({ "empty": true, "pending": 0 }),
    };
    jsonrpc_result(id, wrap_tool_result(payload, false))
}

fn call_webview(
    app: &AppHandle,
    state: &McpState,
    id: Value,
    name: String,
    arguments: Value,
    session: Option<&ClientSession>,
) -> Value {
    let Some(window) = target_window(app) else {
        return jsonrpc_error(id, -32000, "no Utsuwa window is available".into());
    };

    let req_id = next_id("mcp");
    let rx = state.register_pending(req_id.clone());
    let target = window.label().to_string();
    let cmd = JsCommand {
        id: req_id.clone(),
        tool: name.clone(),
        target: target.clone(),
        session_id: session.map(|s| s.id.clone()).unwrap_or_default(),
        speaker_name: session.map(|s| s.name.clone()).unwrap_or_default(),
        speaker_topic: session.map(|s| s.topic.clone()).unwrap_or_default(),
        arguments,
    };

    if let Err(err) = app.emit_to(&target, "mcp:command", cmd) {
        state.complete(&req_id, JsReply { ok: false, payload: json!({}) });
        return jsonrpc_error(id, -32603, format!("failed to reach webview: {err}"));
    }

    match rx.recv_timeout(JS_TIMEOUT) {
        Ok(reply) => {
            let mut payload = reply.payload;
            if name == "get_status" {
                if let Some(obj) = payload.as_object_mut() {
                    if let Some(s) = session {
                        obj.insert(
                            "you".into(),
                            json!({
                                "id": s.id,
                                "name": s.name,
                                "topic": s.topic,
                                "pending": s.inbox.len()
                            }),
                        );
                    }
                    obj.insert("sessions".into(), state.list_public());
                    obj.insert("hostName".into(), json!(default_host_name()));
                    obj.insert("instructions".into(), json!(state.instructions_text()));
                }
            }
            jsonrpc_result(id, wrap_tool_result(payload, !reply.ok))
        }
        Err(RecvTimeoutError::Timeout) => jsonrpc_error(
            id,
            -32000,
            "webview did not acknowledge the command (is the app window open?)".into(),
        ),
        Err(_) => jsonrpc_error(id, -32603, "internal channel closed".into()),
    }
}

fn wrap_tool_result(payload: Value, is_error: bool) -> Value {
    let text = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error
    })
}

fn target_window(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(overlay) = app.get_webview_window("overlay") {
        if overlay.is_visible().unwrap_or(false) {
            return Some(overlay);
        }
    }
    app.get_webview_window("main")
}

fn next_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{prefix}-{t}-{n}")
}

pub fn default_host_name() -> String {
    if let Ok(name) = std::env::var("UTSUWA_MCP_NAME") {
        let name = name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    let raw = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "host".into());
    pretty_host_name(&raw)
}

pub fn pretty_host_name(hostname: &str) -> String {
    let stem = hostname.trim().split('.').next().unwrap_or("").trim();
    if stem.is_empty() {
        return "Host".into();
    }
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Host".into(),
    }
}

pub fn clamp_topic(raw: &str) -> String {
    raw.split_whitespace()
        .take(TOPIC_MAX_WORDS)
        .collect::<Vec<_>>()
        .join(" ")
}

fn client_info_name(params: Option<&Value>) -> Option<String> {
    params
        .and_then(|p| p.get("clientInfo"))
        .and_then(|i| i.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "grok" && !s.starts_with("grok-"))
}

fn host_allowed(request: &tiny_http::Request) -> bool {
    match header_value(request, "Host") {
        None => true,
        Some(host) => {
            let name = host.split(':').next().unwrap_or(&host).to_ascii_lowercase();
            name == "127.0.0.1" || name == "localhost" || name == "[::1]" || name == "::1"
        }
    }
}

fn origin_allowed(origin: &str) -> bool {
    let lower = origin.to_ascii_lowercase();
    lower.starts_with("http://127.0.0.1")
        || lower.starts_with("http://localhost")
        || lower.starts_with("http://[::1]")
        || lower == "null"
}

fn header_value(request: &tiny_http::Request, name: &'static str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

fn json_response(
    status: StatusCode,
    body: String,
    session_id: Option<&str>,
) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut response = Response::from_string(body).with_status_code(status);
    if let Ok(h) = Header::from_bytes(b"Content-Type", b"application/json") {
        response.add_header(h);
    }
    if let Some(sid) = session_id {
        if let Ok(h) = Header::from_bytes(b"Mcp-Session-Id", sid.as_bytes()) {
            response.add_header(h);
        }
    }
    response
}

fn text_response(status: StatusCode, body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body).with_status_code(status)
}

#[derive(Debug)]
pub enum RpcAction {
    Respond(Value),
    Call {
        id: Value,
        name: String,
        arguments: Value,
    },
}

pub fn classify_rpc(req: &Value) -> Result<RpcAction, Value> {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = match req.get("method").and_then(|m| m.as_str()) {
        Some(m) => m,
        None => return Err(jsonrpc_error(id, -32600, "missing method".into())),
    };
    let params = req.get("params");

    match method {
        "initialize" => Ok(RpcAction::Respond(jsonrpc_result(id, initialize_result(params)))),
        "ping" => Ok(RpcAction::Respond(jsonrpc_result(id, json!({})))),
        "tools/list" => Ok(RpcAction::Respond(jsonrpc_result(id, tools_list()))),
        "tools/call" => match parse_tool_call(params) {
            Ok((name, arguments)) => Ok(RpcAction::Call { id, name, arguments }),
            Err(msg) => Err(jsonrpc_error(id, -32602, msg)),
        },
        "notifications/initialized" | "notifications/cancelled" => {
            Ok(RpcAction::Respond(jsonrpc_result(id, json!({}))))
        }
        other => Err(jsonrpc_error(id, -32601, format!("method not found: {other}"))),
    }
}

fn initialize_result(params: Option<&Value>) -> Value {
    let requested = params
        .and_then(|p| p.get("protocolVersion"))
        .and_then(|v| v.as_str())
        .unwrap_or("2025-03-26");
    let version = match requested {
        "2024-11-05" | "2025-03-26" | "2025-06-18" | "2025-11-25" => requested,
        _ => "2025-03-26",
    };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "utsuwa", "version": env!("CARGO_PKG_VERSION") },
        "instructions": compose_instructions("")
    })
}

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "set_session",
                "description": "Set this session's display name (machine, e.g. Shizuku) and a 1-7 word topic describing what you are working on. Topic is required to tell terminals on the same machine apart.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Machine display name. Defaults to the hostname (shizuku.local → Shizuku)." },
                        "topic": { "type": "string", "description": "1-7 words, e.g. 'utsuwa MCP chat'." }
                    }
                }
            },
            {
                "name": "take_user_message",
                "description": "Take the next chat-bar message the user addressed to THIS session. Returns {empty:true} if none. `prompt` is the user's text — answer it, then speak the payload.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "speak",
                "description": "Have the avatar say a line (TTS + lip-sync + bubble). Put only the spoken payload in text. Does not call her LLM.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "Payload only, e.g. 'they finished the new utsuwa build'." },
                        "language": { "type": "string" },
                        "plain": { "type": "boolean", "description": "If true, say `text` exactly as written." }
                    },
                    "required": ["text"]
                }
            },
            {
                "name": "stop_speech",
                "description": "Interrupt current speech and clear the TTS queue.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "get_status",
                "description": "Ready flag, TTS, this session (you), and all connected sessions.",
                "inputSchema": { "type": "object", "properties": {} }
            }
        ]
    })
}

fn parse_tool_call(params: Option<&Value>) -> Result<(String, Value), String> {
    let params = params.ok_or_else(|| "missing params".to_string())?;
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing tool name".to_string())?;
    match name {
        "speak" | "stop_speech" | "get_status" | "set_session" | "take_user_message" => {
            Ok((name.to_string(), params.get("arguments").cloned().unwrap_or_else(|| json!({}))))
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

pub fn jsonrpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn jsonrpc_error(id: Value, code: i32, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_host_name_from_fqdn() {
        assert_eq!(pretty_host_name("shizuku.local"), "Shizuku");
        assert_eq!(pretty_host_name("daphne"), "Daphne");
    }

    #[test]
    fn clamp_topic_caps_words() {
        assert_eq!(clamp_topic("utsuwa MCP chat"), "utsuwa MCP chat");
        assert_eq!(
            clamp_topic("one two three four five six seven eight"),
            "one two three four five six seven"
        );
    }

    #[test]
    fn initialize_echoes_known_protocol() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "test" } }
        });
        match classify_rpc(&req).unwrap() {
            RpcAction::Respond(v) => {
                assert_eq!(v["result"]["protocolVersion"], "2025-03-26");
                assert_eq!(v["result"]["serverInfo"]["name"], "utsuwa");
                let instructions = v["result"]["instructions"].as_str().unwrap();
                assert!(instructions.contains("notification channel"));
                assert!(instructions.contains("Poll take_user_message"));
                assert!(instructions.contains("does not replace speak"));
                assert!(instructions.contains("Preferences (from the user at this machine)"));
                assert!(!instructions.contains("Building the session picker"));
                assert!(!instructions.contains("Java is too old"));
            }
            _ => panic!("expected respond"),
        }
    }

    #[test]
    fn tools_list_includes_session_tools() {
        let req = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
        match classify_rpc(&req).unwrap() {
            RpcAction::Respond(v) => {
                let names: Vec<&str> = v["result"]["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| t["name"].as_str().unwrap())
                    .collect();
                assert!(names.contains(&"set_session"));
                assert!(names.contains(&"take_user_message"));
                assert!(names.contains(&"speak"));
            }
            _ => panic!("expected respond"),
        }
    }

    #[test]
    fn tools_call_speak_is_forwarded() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "speak", "arguments": { "text": "hello" } }
        });
        match classify_rpc(&req).unwrap() {
            RpcAction::Call { name, arguments, .. } => {
                assert_eq!(name, "speak");
                assert_eq!(arguments["text"], "hello");
            }
            _ => panic!("expected call"),
        }
    }

    #[test]
    fn unknown_tool_is_invalid_params() {
        let req = json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": { "name": "explode" }
        });
        let err = classify_rpc(&req).unwrap_err();
        assert_eq!(err["error"]["code"], -32602);
    }

    #[test]
    fn unknown_method() {
        let req = json!({ "jsonrpc": "2.0", "id": 5, "method": "foo/bar" });
        let err = classify_rpc(&req).unwrap_err();
        assert_eq!(err["error"]["code"], -32601);
    }

    #[test]
    fn custom_instructions_replace_default() {
        let state = McpState::new();
        let default = state.instructions_text();
        assert!(default.contains("notification channel"));
        assert!(default.contains("Poll take_user_message"));
        assert!(default.contains("Starting the search UI"));
        assert!(!default.contains("Building the session picker"));
        state.set_instructions("  Be extremely terse.  ".into());
        let custom = state.instructions_text();
        assert!(custom.contains("notification channel"));
        assert!(custom.contains("Be extremely terse."));
        assert!(!custom.contains("Starting the search UI"));
        state.set_instructions("   ".into());
        assert!(state.instructions_text().contains("Starting the search UI"));
        state.set_instructions(
            "Building the session picker, I think I have an idea how to do this.".into(),
        );
        assert!(state.instructions_text().contains("Starting the search UI"));
    }

    #[test]
    fn grok_client_info_name_is_ignored() {
        assert_eq!(client_info_name(Some(&json!({ "clientInfo": { "name": "grok" } }))), None);
        assert_eq!(
            client_info_name(Some(&json!({ "clientInfo": { "name": "grok-4.6" } }))),
            None
        );
        assert_eq!(
            client_info_name(Some(&json!({ "clientInfo": { "name": "my-agent" } }))),
            Some("my-agent".into())
        );
    }
}
