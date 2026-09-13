//! Loopback MCP (Model Context Protocol) server.
//!
//! Desktop-only: binds `127.0.0.1` and speaks Streamable HTTP JSON-RPC so MCP
//! clients (local coding agents, etc.) can puppet the running app. Each client
//! is a named session (machine name + short topic) with its own inbox.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State};
use tiny_http::{Header, Method, Response, Server, StatusCode};

const DEFAULT_BIND: &str = "127.0.0.1:8787";
const JS_TIMEOUT: Duration = Duration::from_secs(12);
const SESSION_TTL: Duration = Duration::from_secs(10 * 60);
const UNCLAIMED_TTL: Duration = Duration::from_secs(90);
const PRUNE_INTERVAL: Duration = Duration::from_secs(15);
const POLL_INTERVAL_SECS: u64 = 3;
const TOPIC_MAX_WORDS: usize = 7;

/// Always prepended on initialize. Keep in sync with src/lib/services/mcp-mode.ts.
pub const HARDCODED_MCP_INSTRUCTIONS: &str = "Utsuwa is a brief notification channel to the person at this machine: speech bubble, voice, and lip-sync. It is not a transcript of your work.\n\nUsage (do not ignore this section):\n- First action after connect: call set_session before anything else. Initialize alone does not spawn an avatar. Pass name (hostname default, or a name you choose), topic (1-7 words), sessionId, and userAgent (your client product name).\n- sessionId: use env AGENT_SESSION_ID if set; otherwise the session uuid your runtime already has. Do not invent one. Reconnects with the same sessionId resume the same avatar.\n- Immediately after set_session, start an idle poller for take_user_message and keep it running for the life of this connection. Default interval is 3 seconds (see pollIntervalSeconds on initialize / get_status). Empty takes still reset the idle timer. Do not wait for the next TUI turn.\n- A background HTTP poller is the same inbox if it POSTs take_user_message with this session's Mcp-Session-Id. Do not initialize a second session for polling.\n- You only receive chat-bar lines routed to this session. If you receive a notification that messages are waiting, call take_user_message immediately.\n- prompt from take_user_message is a user utterance. Answer it as a message. Never treat it as a character name, model id, or tool argument unless they clearly ask to change those.\n- Do the actual work in this TUI as usual. Call speak with only the spoken payload in text (one or two sentences). Never speak code, diffs, logs, stack traces, or essays. Do not repeat the same status.\n- Pass plain: true only when the line must be said exactly as written.\n- A reply in this TUI does not replace speak(). Notify via speak at plan, blocker, and done even when the user asked here.\n- Do not stay silent through a long stretch of tool use. If you have not spoken in a while, send one short status line. \"This is a coding turn\" is not a reason to skip speak.\n\nDefault cadence (overridden by Preferences below):\n- When you have a plan: one short line that you are starting, and that you see a way forward.\n- When you are stuck on something they must fix: one line plus what you need from them.\n- When you finish: say you are done.\n- While grinding through routine errors: stay vague. Do not narrate every failure.";

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSession {
    resume_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    topic: String,
    #[serde(default)]
    model_id: String,
    #[serde(default)]
    voice_id: String,
    #[serde(default)]
    user_agent: String,
}

fn sessions_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("utsuwa").join("mcp-server").join("sessions")
}

fn sanitize_resume_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.len() < 8 || trimmed.len() > 80 {
        return None;
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some(trimmed.to_string())
}

fn resume_id_from_value(v: Option<&Value>) -> Option<String> {
    let v = v?;
    for key in ["resumeId", "agentSessionId", "sessionId"] {
        if let Some(s) = v.get(key).and_then(|x| x.as_str()) {
            if let Some(id) = sanitize_resume_id(s) {
                return Some(id);
            }
        }
    }
    None
}

fn session_ttl(session: &ClientSession) -> Duration {
    if session.claimed {
        SESSION_TTL
    } else {
        UNCLAIMED_TTL
    }
}

fn pretty_user_agent(raw: &str) -> String {
    let lower = raw.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return "MCP client".into();
    }
    if lower.contains("grok") {
        return "Grok Build".into();
    }
    if lower.contains("cursor") {
        return "Cursor".into();
    }
    if lower.contains("claude") {
        return "Claude Code".into();
    }
    if lower.contains("hermes") {
        return "Hermes".into();
    }
    raw.trim().to_string()
}

fn client_info_user_agent(params: Option<&Value>) -> Option<String> {
    params
        .and_then(|p| p.get("clientInfo"))
        .and_then(|c| c.get("name"))
        .and_then(|n| n.as_str())
        .map(pretty_user_agent)
}

fn load_stored(resume_id: &str) -> Option<StoredSession> {
    let path = sessions_dir().join(format!("{resume_id}.json"));
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save_stored(session: &ClientSession) {
    if session.resume_id.is_empty() {
        return;
    }
    let dir = sessions_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let stored = StoredSession {
        resume_id: session.resume_id.clone(),
        name: session.name.clone(),
        topic: session.topic.clone(),
        model_id: session.model_id.clone(),
        voice_id: session.voice_id.clone(),
        user_agent: session.user_agent.clone(),
    };
    if let Ok(body) = serde_json::to_string_pretty(&stored) {
        let path = dir.join(format!("{}.json", session.resume_id));
        let _ = fs::write(path, body);
    }
}

#[derive(Clone)]
pub struct McpState {
    pending: Arc<Mutex<HashMap<String, Sender<JsReply>>>>,
    sessions: Arc<Mutex<HashMap<String, ClientSession>>>,
    app: Arc<Mutex<Option<AppHandle>>>,
    instructions: Arc<Mutex<String>>,
    sse: Arc<Mutex<HashMap<String, Vec<Sender<String>>>>>,
}

#[derive(Debug, Clone)]
struct ClientSession {
    id: String,
    name: String,
    topic: String,
    model_id: String,
    voice_id: String,
    resume_id: String,
    user_agent: String,
    claimed: bool,
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
    speaker_voice: String,
    arguments: Value,
}

impl McpState {
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            app: Arc::new(Mutex::new(None)),
            instructions: Arc::new(Mutex::new(DEFAULT_MCP_USER_INSTRUCTIONS.to_string())),
            sse: Arc::new(Mutex::new(HashMap::new())),
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

    fn claim_session(
        &self,
        name: String,
        topic: String,
        resume_id: Option<String>,
        user_agent: String,
    ) -> ClientSession {
        if let Some(rid) = resume_id.as_ref() {
            if let Some(existing) = self.get_by_resume(rid) {
                self.touch(&existing.id);
                return existing;
            }
            let stored = load_stored(rid);
            let session = ClientSession {
                id: rid.clone(),
                name: stored
                    .as_ref()
                    .map(|s| s.name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or(name),
                topic: clamp_topic(
                    &stored
                        .as_ref()
                        .map(|s| s.topic.clone())
                        .filter(|t| !t.is_empty())
                        .unwrap_or(topic),
                ),
                model_id: stored.as_ref().map(|s| s.model_id.clone()).unwrap_or_default(),
                voice_id: stored.as_ref().map(|s| s.voice_id.clone()).unwrap_or_default(),
                resume_id: rid.clone(),
                user_agent: stored
                    .as_ref()
                    .map(|s| s.user_agent.clone())
                    .filter(|a| !a.is_empty())
                    .unwrap_or(user_agent),
                claimed: true,
                last_seen: Instant::now(),
                inbox: VecDeque::new(),
            };
            self.sessions
                .lock()
                .expect("mcp sessions")
                .insert(rid.clone(), session.clone());
            save_stored(&session);
            self.emit_sessions();
            return session;
        }
        let id = next_id("sess");
        let session = ClientSession {
            id: id.clone(),
            name,
            topic: clamp_topic(&topic),
            model_id: String::new(),
            voice_id: String::new(),
            resume_id: String::new(),
            user_agent,
            claimed: false,
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

    fn get_by_resume(&self, resume_id: &str) -> Option<ClientSession> {
        self.sessions
            .lock()
            .expect("mcp sessions")
            .values()
            .find(|s| s.resume_id == resume_id || s.id == resume_id)
            .cloned()
    }

    fn bind_resume(&self, session_id: &str, resume_id: String) -> Option<ClientSession> {
        self.prune();
        let stored = load_stored(&resume_id);
        let mut sessions = self.sessions.lock().expect("mcp sessions");
        let stale: Vec<String> = sessions
            .values()
            .filter(|s| s.id != session_id && (s.resume_id == resume_id || s.id == resume_id))
            .map(|s| s.id.clone())
            .collect();
        let mut stolen = stored;
        for id in &stale {
            if let Some(old) = sessions.remove(id) {
                if stolen.is_none() {
                    stolen = Some(StoredSession {
                        resume_id: resume_id.clone(),
                        name: old.name,
                        topic: old.topic,
                        model_id: old.model_id,
                        voice_id: old.voice_id,
                        user_agent: old.user_agent,
                    });
                }
            }
        }
        let session = sessions.get_mut(session_id)?;
        session.resume_id = resume_id;
        session.claimed = true;
        if let Some(prev) = stolen {
            if session.topic.is_empty() && !prev.topic.is_empty() {
                session.topic = prev.topic;
            }
            if session.model_id.is_empty() && !prev.model_id.is_empty() {
                session.model_id = prev.model_id;
            }
            if session.voice_id.is_empty() && !prev.voice_id.is_empty() {
                session.voice_id = prev.voice_id;
            }
            if session.name == default_host_name() && !prev.name.is_empty() {
                session.name = prev.name;
            }
            if session.user_agent.is_empty() && !prev.user_agent.is_empty() {
                session.user_agent = prev.user_agent;
            }
        }
        session.last_seen = Instant::now();
        let clone = session.clone();
        drop(sessions);
        save_stored(&clone);
        self.emit_sessions();
        Some(clone)
    }

    fn drop_session(&self, id: &str) {
        self.sessions.lock().expect("mcp sessions").remove(id);
        self.sse.lock().expect("mcp sse").remove(id);
        self.emit_sessions();
    }

    fn touch(&self, id: &str) -> bool {
        // Heartbeat first so an inbox poll cannot lose the race to prune.
        let mut sessions = self.sessions.lock().expect("mcp sessions");
        if let Some(s) = sessions.get_mut(id) {
            s.last_seen = Instant::now();
            drop(sessions);
            self.prune();
            return true;
        }
        drop(sessions);
        self.prune();
        false
    }

    fn get(&self, id: &str) -> Option<ClientSession> {
        self.prune();
        self.sessions.lock().expect("mcp sessions").get(id).cloned()
    }

    fn set_identity(
        &self,
        id: &str,
        name: Option<String>,
        topic: Option<String>,
        user_agent: Option<String>,
    ) -> Option<ClientSession> {
        self.prune();
        let mut sessions = self.sessions.lock().expect("mcp sessions");
        let session = sessions.get_mut(id)?;
        session.claimed = true;
        if let Some(n) = name {
            let n = n.trim();
            if !n.is_empty() {
                session.name = n.to_string();
            }
        }
        if let Some(t) = topic {
            session.topic = clamp_topic(&t);
        }
        if let Some(ua) = user_agent {
            let ua = pretty_user_agent(&ua);
            if ua != "MCP client" {
                session.user_agent = ua;
            }
        }
        session.last_seen = Instant::now();
        let clone = session.clone();
        drop(sessions);
        save_stored(&clone);
        self.emit_sessions();
        Some(clone)
    }

    fn set_appearance(
        &self,
        id: &str,
        model_id: Option<String>,
        voice_id: Option<String>,
    ) -> Option<ClientSession> {
        self.prune();
        let mut sessions = self.sessions.lock().expect("mcp sessions");
        let session = sessions.get_mut(id)?;
        if let Some(model) = model_id {
            session.model_id = model.trim().to_string();
        }
        if let Some(voice) = voice_id {
            session.voice_id = voice.trim().to_string();
        }
        session.last_seen = Instant::now();
        let clone = session.clone();
        drop(sessions);
        save_stored(&clone);
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
        let pending = session.inbox.len();
        drop(sessions);
        self.emit_sessions();
        self.notify_inbox(&id, pending);
        Ok(item)
    }

    fn subscribe_sse(&self, session_id: &str) -> Receiver<String> {
        let (tx, rx) = mpsc::channel();
        self.sse
            .lock()
            .expect("mcp sse")
            .entry(session_id.to_string())
            .or_default()
            .push(tx);
        rx
    }

    fn notify_inbox(&self, session_id: &str, pending: usize) {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": "notifications/message",
            "params": {
                "level": "info",
                "logger": "utsuwa",
                "data": format!("{pending} message(s) waiting. Call take_user_message.")
            }
        });
        let line = payload.to_string();
        let mut sinks = self.sse.lock().expect("mcp sse");
        if let Some(list) = sinks.get_mut(session_id) {
            list.retain(|tx| tx.send(line.clone()).is_ok());
            if list.is_empty() {
                sinks.remove(session_id);
            }
        }
    }

    fn take(&self, id: &str) -> Option<InboxItem> {
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
            .filter(|s| s.claimed)
            .map(|s| {
                json!({
                    "id": s.id,
                    "name": s.name,
                    "topic": s.topic,
                    "modelId": s.model_id,
                    "voiceId": s.voice_id,
                    "resumeId": s.resume_id,
                    "userAgent": s.user_agent,
                    "pending": s.inbox.len()
                })
            })
            .collect();
        json!(list)
    }

    fn list_debug(&self) -> Value {
        let sessions = self.sessions.lock().expect("mcp sessions");
        let list: Vec<Value> = sessions
            .values()
            .map(|s| {
                json!({
                    "id": s.id,
                    "name": s.name,
                    "topic": s.topic,
                    "claimed": s.claimed,
                    "resumeId": s.resume_id,
                    "userAgent": s.user_agent,
                    "modelId": s.model_id,
                    "pending": s.inbox.len(),
                    "idleSecs": s.last_seen.elapsed().as_secs()
                })
            })
            .collect();
        json!(list)
    }

    fn prune(&self) {
        let mut sessions = self.sessions.lock().expect("mcp sessions");
        let before = sessions.len();
        let live_sse: HashSet<String> = self
            .sse
            .lock()
            .expect("mcp sse")
            .iter()
            .filter(|(_, txs)| !txs.is_empty())
            .map(|(id, _)| id.clone())
            .collect();
        sessions.retain(|id, s| {
            live_sse.contains(id) || s.last_seen.elapsed() < session_ttl(s)
        });
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
    let prune_state = state.clone();
    thread::Builder::new()
        .name("utsuwa-mcp-prune".into())
        .spawn(move || loop {
            thread::sleep(PRUNE_INTERVAL);
            prune_state.prune();
        })
        .expect("spawn MCP prune thread");
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
            "pollIntervalSeconds": POLL_INTERVAL_SECS,
            "sessions": state.list_public()
        })
        .to_string();
        request.respond(json_response(StatusCode(200), body, None))?;
        return Ok(());
    }

    if method == Method::Get && path == "/mcp" {
        let sid = header_value(&request, "Mcp-Session-Id");
        let Some(sid) = sid else {
            request.respond(text_response(StatusCode(400), "missing Mcp-Session-Id"))?;
            return Ok(());
        };
        if state.get(&sid).is_none() {
            request.respond(text_response(StatusCode(404), "unknown session"))?;
            return Ok(());
        }
        let rx = state.subscribe_sse(&sid);
        let sid_header = sid.clone();
        let sse_state = state.clone();
        thread::Builder::new()
            .name("utsuwa-mcp-sse".into())
            .spawn(move || {
                let mut headers = Vec::new();
                if let Ok(h) = Header::from_bytes(b"Content-Type", b"text/event-stream") {
                    headers.push(h);
                }
                if let Ok(h) = Header::from_bytes(b"Cache-Control", b"no-cache") {
                    headers.push(h);
                }
                if let Ok(h) = Header::from_bytes(b"Mcp-Session-Id", sid_header.as_bytes()) {
                    headers.push(h);
                }
                let response = Response::new(
                    StatusCode(200),
                    headers,
                    SseStream::new(rx, sse_state, sid_header),
                    None,
                    None,
                );
                let _ = request.respond(response);
            })?;
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
    let header_resume = header_value(&request, "X-Utsuwa-Resume-Id")
        .and_then(|s| sanitize_resume_id(&s));
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
        let resume = header_resume.or_else(|| resume_id_from_value(params));
        let user_agent = header_value(&request, "X-Utsuwa-User-Agent")
            .map(|s| pretty_user_agent(&s))
            .or_else(|| client_info_user_agent(params))
            .unwrap_or_else(|| "MCP client".into());
        let session = state.claim_session(name, topic, resume, user_agent);
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
                        obj.insert("pollIntervalSeconds".into(), json!(POLL_INTERVAL_SECS));
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
            "set_character" => handle_set_character(state, &session_id, id, arguments),
            "set_voice" => handle_set_voice(state, &session_id, id, arguments),
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
    let resume = resume_id_from_value(Some(&arguments));
    if let Some(rid) = resume {
        if state.bind_resume(session_id, rid).is_none() {
            return jsonrpc_error(id, -32000, "session gone".into());
        }
    }
    let user_agent = arguments
        .get("userAgent")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    match state.set_identity(session_id, name, topic, user_agent) {
        Some(session) => jsonrpc_result(
            id,
            wrap_tool_result(
                json!({
                    "id": session.id,
                    "name": session.name,
                    "topic": session.topic,
                    "sessionId": session.resume_id,
                    "resumeId": session.resume_id,
                    "userAgent": session.user_agent
                }),
                false,
            ),
        ),
        None => jsonrpc_error(id, -32000, "session gone".into()),
    }
}

fn handle_set_character(state: &McpState, session_id: &str, id: Value, arguments: Value) -> Value {
    let model_id = arguments
        .get("modelId")
        .or_else(|| arguments.get("model"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    let Some(model_id) = model_id else {
        return jsonrpc_error(id, -32602, "modelId is required".into());
    };
    match state.set_appearance(session_id, Some(model_id), None) {
        Some(session) => jsonrpc_result(
            id,
            wrap_tool_result(
                json!({
                    "id": session.id,
                    "modelId": session.model_id,
                    "voiceId": session.voice_id
                }),
                false,
            ),
        ),
        None => jsonrpc_error(id, -32000, "session gone".into()),
    }
}

fn handle_set_voice(state: &McpState, session_id: &str, id: Value, arguments: Value) -> Value {
    let voice_id = arguments
        .get("voiceId")
        .or_else(|| arguments.get("voice"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    let Some(voice_id) = voice_id else {
        return jsonrpc_error(id, -32602, "voiceId is required".into());
    };
    match state.set_appearance(session_id, None, Some(voice_id)) {
        Some(session) => jsonrpc_result(
            id,
            wrap_tool_result(
                json!({
                    "id": session.id,
                    "modelId": session.model_id,
                    "voiceId": session.voice_id
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
        speaker_voice: session.map(|s| s.voice_id.clone()).unwrap_or_default(),
        arguments,
    };

    if let Err(err) = app.emit_to(&target, "mcp:command", cmd) {
        state.complete(&req_id, JsReply { ok: false, payload: json!({}) });
        return jsonrpc_error(id, -32603, format!("failed to reach webview: {err}"));
    }

    match rx.recv_timeout(JS_TIMEOUT) {
        Ok(reply) => {
            let mut payload = reply.payload;
            if name == "get_status" || name == "debug_state" {
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
                    obj.insert("pollIntervalSeconds".into(), json!(POLL_INTERVAL_SECS));
                    if name == "get_status" {
                        obj.insert("instructions".into(), json!(state.instructions_text()));
                    }
                    if name == "debug_state" {
                        let overlay_visible = app
                            .get_webview_window("overlay")
                            .and_then(|w| w.is_visible().ok())
                            .unwrap_or(false);
                        obj.insert(
                            "server".into(),
                            json!({
                                "targetWindow": target,
                                "overlayVisible": overlay_visible,
                                "allSessions": state.list_debug(),
                                "claimedSessions": state.list_public()
                            }),
                        );
                    }
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

struct SseStream {
    rx: Receiver<String>,
    leftover: Vec<u8>,
    state: McpState,
    session_id: String,
}

impl SseStream {
    fn new(rx: Receiver<String>, state: McpState, session_id: String) -> Self {
        Self {
            rx,
            leftover: Vec::new(),
            state,
            session_id,
        }
    }
}

impl Read for SseStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.leftover.is_empty() {
            match self.rx.recv_timeout(Duration::from_secs(15)) {
                Ok(data) => {
                    self.state.touch(&self.session_id);
                    self.leftover = format!("event: message\ndata: {data}\n\n").into_bytes();
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.state.touch(&self.session_id);
                    self.leftover = b": ping\n\n".to_vec();
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(0),
            }
        }
        let n = buf.len().min(self.leftover.len());
        buf[..n].copy_from_slice(&self.leftover[..n]);
        self.leftover.drain(..n);
        Ok(n)
    }
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
        "instructions": compose_instructions(""),
        "pollIntervalSeconds": POLL_INTERVAL_SECS
    })
}

fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "set_session",
                "description": "Call immediately after connect. This is what makes your avatar appear. Pass name (defaults to hostname), topic (1-7 words), sessionId (env AGENT_SESSION_ID), and userAgent (your client product name).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Display name. Defaults to the hostname (shizuku.local → Shizuku)." },
                        "topic": { "type": "string", "description": "1-7 words describing this terminal's work." },
                        "sessionId": { "type": "string", "description": "This agent's session id. Use env AGENT_SESSION_ID if set, otherwise the uuid your runtime already has." },
                        "resumeId": { "type": "string", "description": "Alias of sessionId." },
                        "agentSessionId": { "type": "string", "description": "Alias of sessionId." },
                        "userAgent": { "type": "string", "description": "Client product name, e.g. Cursor, Claude Code, Hermes, Grok Build." }
                    },
                }
            },
            {
                "name": "take_user_message",
                "description": "Take the next chat-bar message for THIS session. Returns {empty:true} if none. Poll every pollIntervalSeconds (default 3), including empty takes — those keep the session alive. `prompt` is a user utterance.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "set_character",
                "description": "Pick a VRM from the user's gallery for this session's avatar (modelId or name, e.g. Tsuki). Applied when multi-character MCP is active; stored even in single-character mode.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "modelId": { "type": "string", "description": "Gallery model id or display name." }
                    },
                    "required": ["modelId"]
                }
            },
            {
                "name": "set_voice",
                "description": "TTS voice id for this session's speak() calls (OmniVoice voice profile id, etc.).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "voiceId": { "type": "string" }
                    },
                    "required": ["voiceId"]
                }
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
            },
            {
                "name": "debug_state",
                "description": "Temporary dump of MCP + scene state (window, sessions, VRM instances). For debugging; safe to remove later.",
                "inputSchema": { "type": "object", "properties": {} }
            },
            {
                "name": "debug_chat_bar",
                "description": "Temporary: get/set the MCP chat-bar dropdown and send a test line as the user. action: get | select | set_draft | send. send queues take_user_message for the selected (or given) session.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "action": { "type": "string", "description": "get (default), select, set_draft, or send" },
                        "sessionId": { "type": "string" },
                        "text": { "type": "string", "description": "Draft or message body for set_draft / send" }
                    }
                }
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
        "speak" | "stop_speech" | "get_status" | "debug_state" | "debug_chat_bar" | "set_session"
        | "set_character" | "set_voice" | "take_user_message" => {
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
    fn sanitize_resume_id_accepts_agent_ids() {
        assert_eq!(
            sanitize_resume_id("01a0979b-f70d-7d80-9fa1-3dadf1b25338").as_deref(),
            Some("01a0979b-f70d-7d80-9fa1-3dadf1b25338")
        );
        assert_eq!(sanitize_resume_id("short"), None);
        assert_eq!(sanitize_resume_id("../etc/passwd"), None);
    }

    #[test]
    fn unclaimed_sessions_expire_faster() {
        let claimed = ClientSession {
            id: "a".into(),
            name: "Shizuku".into(),
            topic: "demo".into(),
            model_id: String::new(),
            voice_id: String::new(),
            resume_id: "01a0979b-f70d-7d80-9fa1-3dadf1b25338".into(),
            user_agent: "Grok Build".into(),
            claimed: true,
            last_seen: Instant::now(),
            inbox: VecDeque::new(),
        };
        let unclaimed = ClientSession {
            resume_id: String::new(),
            topic: String::new(),
            claimed: false,
            ..claimed.clone()
        };
        assert_eq!(session_ttl(&claimed), SESSION_TTL);
        assert_eq!(session_ttl(&unclaimed), UNCLAIMED_TTL);
    }

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
                assert!(instructions.contains("take_user_message"));
                assert!(instructions.contains("idle poller"));
                assert_eq!(v["result"]["pollIntervalSeconds"], 3);
                assert!(instructions.contains("does not replace speak"));
                assert!(instructions.contains("AGENT_SESSION_ID"));
                assert!(instructions.contains("First action after connect"));
                assert!(instructions.contains("user utterance"));
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
                assert!(names.contains(&"set_character"));
                assert!(names.contains(&"set_voice"));
                assert!(names.contains(&"take_user_message"));
                assert!(names.contains(&"speak"));
                assert!(names.contains(&"debug_state"));
                assert!(names.contains(&"debug_chat_bar"));
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
        assert!(default.contains("take_user_message"));
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
