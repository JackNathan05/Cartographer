//! v0.7 Ask Claude: questions about your PC in plain English ("what can I delete to free 50 GB?").
//!
//! Two ways to reach Claude, same read-only tools (ask_tools.rs):
//! - **Claude Code** (default when installed): runs `claude -p` with every built-in tool switched off
//!   and Cartographer itself as the only MCP server (`cartographer --mcp`). Uses your Claude login.
//! - **Anthropic API**: your own API key (stored encrypted with Windows DPAPI), a tool-use loop here.
//!
//! Claude only ever sees metadata (names, paths, sizes, dates) that the tools return, never file
//! contents, and it cannot change anything: archive / delete stay buttons you press yourself.

use crate::ask_tools;
use crate::helper::data_dir;
use serde::Serialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const MODEL: &str = "claude-opus-5-5";
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const NO_WINDOW: u32 = 0x0800_0000;

pub const SYSTEM: &str = "You are Cartographer's assistant, built into a Windows app that maps the user's PC: drives, folders, projects, games, apps and backups. Answer questions about this PC using the tools, which read Cartographer's local index (names, paths, sizes and dates only; you cannot see file contents).

How to work:
- Look things up before answering; don't guess sizes or paths. Start with `overview` when you need the big picture.
- Be concrete: name the folders, give sizes and dates, and add up totals when the user asks how much space they could free.
- You cannot change anything. When something could be cleaned up, say what you'd suggest and point the user to Cartographer's Cleanup tab (archive is reversible: a verified zip; delete goes to the Recycle Bin; apps and games should be uninstalled, never deleted).
- Be careful with deletion advice: never suggest removing Windows, Program Files, AppData of installed apps, or anything that isn't backed up without saying so plainly.
- Write full Windows paths exactly as the tools give them, inside backticks (`C:\\Users\\...`), so the app can turn them into links.
- Keep answers short and easy to scan: a one-line answer first, then a short list if needed. Use plain language.";

// ---------------------------------------------------------------- API key (DPAPI)

fn key_path() -> PathBuf {
    data_dir().join("anthropic-key.bin")
}

fn dpapi(data: &[u8], protect: bool) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = unsafe {
        if protect {
            CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0, &mut out)
        } else {
            CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), 0, &mut out)
        }
    };
    if ok == 0 || out.pbData.is_null() {
        return None;
    }
    let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe { LocalFree(out.pbData as _) };
    Some(v)
}

pub fn save_key(key: &str) -> Result<(), String> {
    let k = key.trim();
    if k.is_empty() {
        let _ = std::fs::remove_file(key_path());
        return Ok(());
    }
    if !k.starts_with("sk-ant-") {
        return Err("That doesn't look like an Anthropic API key (they start with sk-ant-).".into());
    }
    let enc = dpapi(k.as_bytes(), true).ok_or("Windows couldn't encrypt the key")?;
    std::fs::create_dir_all(data_dir()).map_err(|e| e.to_string())?;
    std::fs::write(key_path(), enc).map_err(|e| e.to_string())
}

fn load_key() -> Option<String> {
    if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
        if !k.trim().is_empty() {
            return Some(k.trim().to_string());
        }
    }
    let enc = std::fs::read(key_path()).ok()?;
    String::from_utf8(dpapi(&enc, false)?).ok()
}

/// "sk-ant-…a1b2" for the UI; the key itself never leaves this module.
pub fn key_hint() -> Option<String> {
    load_key().map(|k| format!("sk-ant-…{}", &k[k.len().saturating_sub(4)..]))
}

// ---------------------------------------------------------------- Claude Code

pub fn claude_code_path() -> Option<PathBuf> {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let local = PathBuf::from(&home).join(r".local\bin\claude.exe");
    if local.exists() {
        return Some(local);
    }
    let out = Command::new("where.exe").arg("claude").creation_flags(NO_WINDOW).output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| PathBuf::from(l.trim()))
        .find(|p| p.extension().map(|e| e.eq_ignore_ascii_case("exe") || e.eq_ignore_ascii_case("cmd")).unwrap_or(false) && p.exists())
}

#[derive(Serialize)]
pub struct Status {
    pub claude_code: bool,
    pub api_key: Option<String>,
    pub model: &'static str,
}

pub fn status() -> Status {
    Status { claude_code: claude_code_path().is_some(), api_key: key_hint(), model: MODEL }
}

// ---------------------------------------------------------------- one question

#[derive(Serialize, Clone)]
pub struct Step {
    pub tool: String,
    pub input: Value,
}

#[derive(Serialize)]
pub struct Answer {
    pub text: String,
    pub via: String, // "Claude Code" | "Anthropic API"
    pub steps: Vec<Step>,
    pub seconds: f64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: Option<f64>,
}

/// One conversation. The API keeps the full message history (append-only, thinking blocks kept
/// as returned); Claude Code gets the recent Q&A as text.
#[derive(Default)]
pub struct Conversation {
    pub messages: Vec<Value>,
    pub turns: Vec<(String, String)>,
}

pub fn ask(
    db: &Path,
    convo: &mut Conversation,
    question: &str,
    backend: &str,
    on_step: &dyn Fn(&Step),
) -> Result<Answer, String> {
    let use_cc = match backend {
        "claude_code" => true,
        "api" => false,
        _ => claude_code_path().is_some(), // "auto": Claude Code when installed, else the API key
    };
    let t0 = Instant::now();
    let mut a = if use_cc {
        ask_claude_code(convo, question, on_step)?
    } else {
        let key = load_key().ok_or("Add an Anthropic API key first (or install Claude Code).")?;
        ask_api(db, &key, convo, question, on_step)?
    };
    a.seconds = t0.elapsed().as_secs_f64();
    convo.turns.push((question.to_string(), a.text.clone()));
    Ok(a)
}

fn post(key: &str, body: &Value) -> Result<Value, String> {
    let mut tries = 0;
    loop {
        tries += 1;
        let r = ureq::post(API_URL)
            .timeout(Duration::from_secs(600))
            .set("x-api-key", key)
            .set("anthropic-version", "2026-10-21")
            .set("anthropic-beta", "server-side-fallback-2026-07-01")
            .set("content-type", "application/json")
            .send_string(&body.to_string());
        match r {
            Ok(resp) => {
                let s = resp.into_string().map_err(|e| e.to_string())?;
                return serde_json::from_str(&s).map_err(|e| e.to_string());
            }
            Err(ureq::Error::Status(code, resp)) => {
                let retry_after = resp.header("retry-after").and_then(|v| v.parse::<u64>().ok());
                let text = resp.into_string().unwrap_or_default();
                let msg = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v["error"]["message"].as_str().map(String::from))
                    .unwrap_or(text);
                let retryable = code == 429 || code == 408 || code == 409 || code >= 500;
                if retryable && tries <= 3 {
                    std::thread::sleep(Duration::from_secs(retry_after.unwrap_or(2u64.pow(tries)).min(30)));
                    continue;
                }
                return Err(match code {
                    401 => "The API key was rejected. Check it in Ask settings.".into(),
                    403 => format!("The API key isn't allowed to do this: {msg}"),
                    429 => "Rate limited by the Anthropic API. Try again in a minute.".into(),
                    _ => format!("Anthropic API error {code}: {msg}"),
                });
            }
            Err(e) => {
                if tries <= 2 {
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
                return Err(format!("Couldn't reach the Anthropic API: {e}"));
            }
        }
    }
}

fn ask_api(db: &Path, key: &str, convo: &mut Conversation, question: &str, on_step: &dyn Fn(&Step)) -> Result<Answer, String> {
    let mut conn = crate::db::open(db).map_err(|e| e.to_string())?;
    let tools = ask_tools::specs();
    convo.messages.push(json!({ "role": "user", "content": question }));
    let (mut tin, mut tout) = (0u64, 0u64);
    let mut steps = Vec::new();
    for _ in 0..16 {
        let body = json!({
            "model": MODEL,
            "max_tokens": 16000,
            "system": [{ "type": "text", "text": SYSTEM, "cache_control": { "type": "ephemeral" } }],
            "tools": tools,
            "thinking": { "type": "adaptive" },
            "output_config": { "effort": "medium" },
            "fallbacks": "default",
            "messages": convo.messages,
        });
        let r = post(key, &body)?;
        tin += r["usage"]["input_tokens"].as_u64().unwrap_or(0)
            + r["usage"]["cache_read_input_tokens"].as_u64().unwrap_or(0)
            + r["usage"]["cache_creation_input_tokens"].as_u64().unwrap_or(0);
        tout += r["usage"]["output_tokens"].as_u64().unwrap_or(0);
        let content = r["content"].clone();
        let stop = r["stop_reason"].as_str().unwrap_or("").to_string();
        // keep the assistant turn exactly as returned (thinking blocks included)
        convo.messages.push(json!({ "role": "assistant", "content": content }));
        if stop == "refusal" {
            return Ok(answer("Claude declined to answer this one. Try asking it a different way.".into(), "Anthropic API", steps, tin, tout));
        }
        if stop != "tool_use" {
            let text: Vec<&str> = content.as_array().map(|v| v.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect()).unwrap_or_default();
            let mut t = text.join("\n\n");
            if stop == "max_tokens" {
                t.push_str("\n\n(The answer was cut off.)");
            }
            return Ok(answer(t, "Anthropic API", steps, tin, tout));
        }
        let mut results = Vec::new();
        for b in content.as_array().into_iter().flatten().filter(|b| b["type"] == "tool_use") {
            let name = b["name"].as_str().unwrap_or("");
            let step = Step { tool: name.to_string(), input: b["input"].clone() };
            on_step(&step);
            steps.push(step);
            let (text, err) = match ask_tools::run(&mut conn, name, &b["input"]) {
                Ok(v) => (v.to_string(), false),
                Err(e) => (e, true),
            };
            results.push(json!({ "type": "tool_result", "tool_use_id": b["id"], "content": text, "is_error": err }));
        }
        convo.messages.push(json!({ "role": "user", "content": results }));
    }
    Err("Claude used too many steps on this question. Try a narrower question.".into())
}

fn answer(text: String, via: &str, steps: Vec<Step>, tin: u64, tout: u64) -> Answer {
    // Claude Opus 5.5: $4 / $20 per million tokens (cache reads are cheaper, so this is an upper bound)
    let cost = (tin as f64 * 4.0 + tout as f64 * 20.0) / 1e6;
    Answer { text, via: via.into(), steps, seconds: 0.0, tokens_in: tin, tokens_out: tout, cost_usd: Some(cost) }
}

fn ask_claude_code(convo: &Conversation, question: &str, on_step: &dyn Fn(&Step)) -> Result<Answer, String> {
    let exe = claude_code_path().ok_or("Claude Code isn't installed.")?;
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = data_dir().join("ask");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let cfg = dir.join("mcp.json");
    let mcp = json!({ "mcpServers": { "cartographer": { "type": "stdio", "command": me.to_string_lossy(), "args": ["--mcp"] } } });
    std::fs::write(&cfg, mcp.to_string()).map_err(|e| e.to_string())?;

    let mut prompt = String::new();
    let recent = &convo.turns[convo.turns.len().saturating_sub(6)..];
    if !recent.is_empty() {
        prompt.push_str("Earlier in this conversation:\n");
        for (q, a) in recent {
            prompt.push_str(&format!("User: {q}\nYou: {a}\n\n"));
        }
        prompt.push_str("New question:\n");
    }
    prompt.push_str(question);

    let mut child = Command::new(exe)
        .args(["-p", "--output-format", "stream-json", "--verbose", "--no-session-persistence"])
        .args(["--tools", ""])
        .args(["--strict-mcp-config", "--mcp-config"])
        .arg(&cfg)
        .args(["--allowedTools", "mcp__cartographer"])
        .args(["--system-prompt", SYSTEM])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(NO_WINDOW)
        .spawn()
        .map_err(|e| format!("Couldn't start Claude Code: {e}"))?;
    child.stdin.take().map(|mut s| s.write_all(prompt.as_bytes()));
    let stdout = child.stdout.take().ok_or("no output")?;
    let t0 = Instant::now();
    let mut steps = Vec::new();
    let mut result: Option<Value> = None;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v["type"].as_str() {
            Some("assistant") => {
                for b in v["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_use") {
                    let name = b["name"].as_str().unwrap_or("").trim_start_matches("mcp__cartographer__").to_string();
                    let step = Step { tool: name, input: b["input"].clone() };
                    on_step(&step);
                    steps.push(step);
                }
            }
            Some("result") => result = Some(v),
            _ => {}
        }
        if t0.elapsed() > Duration::from_secs(600) {
            let _ = child.kill();
            return Err("Claude Code took too long.".into());
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let Some(r) = result else {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            use std::io::Read;
            let _ = e.read_to_string(&mut err);
        }
        let err = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
        return Err(format!("Claude Code stopped without an answer ({status}). {err}"));
    };
    if r["is_error"].as_bool().unwrap_or(false) {
        let msg = r["result"].as_str().unwrap_or("error");
        if msg.to_lowercase().contains("authenticat") || msg.to_lowercase().contains("login") {
            return Err(format!("Claude Code needs you to sign in again ({msg}). Open a terminal, run `claude`, log in, then ask again. Or add an API key in Ask settings."));
        }
        return Err(format!("Claude Code: {msg}"));
    }
    Ok(Answer {
        text: r["result"].as_str().unwrap_or("").to_string(),
        via: "Claude Code".into(),
        steps,
        seconds: 0.0,
        tokens_in: r["usage"]["input_tokens"].as_u64().unwrap_or(0)
            + r["usage"]["cache_read_input_tokens"].as_u64().unwrap_or(0)
            + r["usage"]["cache_creation_input_tokens"].as_u64().unwrap_or(0),
        tokens_out: r["usage"]["output_tokens"].as_u64().unwrap_or(0),
        cost_usd: None, // billed to the Claude plan, not per question
    })
}

// ---------------------------------------------------------------- MCP server (for Claude Code)

/// `cartographer --mcp`: a minimal stdio MCP server exposing the read-only tools.
pub fn mcp_server() -> i32 {
    let db = data_dir().join("index.db");
    let mut conn = match crate::db::open(&db) {
        Ok(c) => c,
        Err(_) => return 1,
    };
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(req) = serde_json::from_str::<Value>(&line) else { continue };
        let id = req.get("id").cloned();
        let method = req["method"].as_str().unwrap_or("");
        let result = match method {
            "initialize" => Some(json!({
                "protocolVersion": req["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "cartographer", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Read-only access to Cartographer's index of this PC (names, paths, sizes, dates).",
            })),
            "ping" => Some(json!({})),
            "tools/list" => Some(json!({
                "tools": ask_tools::specs().into_iter().map(|t| json!({
                    "name": t["name"], "description": t["description"], "inputSchema": t["input_schema"],
                    "annotations": { "readOnlyHint": true },
                })).collect::<Vec<_>>()
            })),
            "tools/call" => {
                let name = req["params"]["name"].as_str().unwrap_or("");
                let args = req["params"].get("arguments").cloned().unwrap_or(json!({}));
                Some(match ask_tools::run(&mut conn, name, &args) {
                    Ok(v) => json!({ "content": [{ "type": "text", "text": v.to_string() }], "isError": false }),
                    Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
                })
            }
            _ => None,
        };
        let Some(id) = id else { continue }; // notifications get no reply
        let msg = match result {
            Some(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
            None => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("unknown method {method}") } }),
        };
        if writeln!(out, "{msg}").is_err() || out.flush().is_err() {
            break;
        }
    }
    0
}
