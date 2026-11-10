use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Provider {
    OpenAI,
    Anthropic,
    Gemini,
}

impl Provider {
    pub fn id(self) -> &'static str {
        match self {
            Provider::OpenAI => "openai",
            Provider::Anthropic => "anthropic",
            Provider::Gemini => "gemini",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "openai" | "openrouter" | "compatible" => Some(Provider::OpenAI),
            "anthropic" => Some(Provider::Anthropic),
            "gemini" | "google" => Some(Provider::Gemini),
            _ => None,
        }
    }

    pub fn default_url(self) -> &'static str {
        match self {
            Provider::OpenAI => "https://api.openai.com/v1",
            Provider::Anthropic => "https://api.anthropic.com/v1",
            Provider::Gemini => "https://generativelanguage.googleapis.com/v1beta",
        }
    }

    fn env_keys(self) -> &'static [&'static str] {
        match self {
            Provider::OpenAI => &["OPENAI_API_KEY", "OPENROUTER_API_KEY"],
            Provider::Anthropic => &["ANTHROPIC_API_KEY"],
            Provider::Gemini => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone, Debug)]
pub struct Ctx {
    pub path: Option<PathBuf>,
    pub label: String,
    pub lang: String,
    pub content: String,
    pub sel: Option<(usize, usize)>,
}

#[derive(Clone, Debug)]
pub struct Msg {
    pub role: Role,
    pub text: String,
    pub ctx: Option<Ctx>,
    pub error: bool,
}

pub enum AiEvent {
    Delta(u64, String),
    Done(u64),
    Error(u64, String),
    Models(Vec<String>),
    ModelsError(String),
}

pub struct ChatInfo {
    pub id: String,
    pub title: String,
    pub updated: u64,
}

pub struct Ai {
    pub visible: bool,
    pub width: u16,
    pub provider: Provider,
    pub model: String,
    pub base_url: String,
    pub msgs: Vec<Msg>,
    pub input: Vec<char>,
    pub input_x: usize,
    pub scroll: usize,
    pub streaming: Option<(u64, Arc<AtomicBool>)>,
    pub attach: bool,
    pub sel_ctx: Option<Ctx>,
    pub models: Vec<String>,
    pub loading_models: bool,
    pub chat_id: String,
    next_id: u64,
    tx: Sender<AiEvent>,
    rx: Receiver<AiEvent>,
}

const SYSTEM_PROMPT: &str = "You are a programming assistant embedded in zetsu, a terminal code editor. \
Answer concisely and accurately. Reply in the language the user writes in. \
When you provide code that should replace the user's selection or be inserted into their file, \
put it in a single fenced code block with the language tag and no extra commentary inside the block.";

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn chats_dir() -> Option<PathBuf> {
    Some(crate::config::path()?.parent()?.join("chats"))
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .try_proxy_from_env(true)
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(180))
        .user_agent(concat!("zetsu/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn http_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let msg = v["error"]["message"]
                .as_str()
                .or_else(|| v["error"].as_str())
                .or_else(|| v["message"].as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| body.chars().take(200).collect());
            format!("HTTP {}: {}", code, msg)
        }
        ureq::Error::Transport(t) => t.to_string(),
    }
}

pub fn code_blocks(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cur: Option<(String, Vec<&str>)> = None;
    for line in text.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("```") {
            match cur.take() {
                Some((lang, lines)) => out.push((lang, lines.join("\n"))),
                None => cur = Some((rest.trim().to_string(), Vec::new())),
            }
            continue;
        }
        if let Some((_, lines)) = cur.as_mut() {
            lines.push(line);
        }
    }
    if let Some((lang, lines)) = cur {
        out.push((lang, lines.join("\n")));
    }
    out
}

fn user_content(m: &Msg) -> String {
    match &m.ctx {
        Some(c) => format!(
            "Context ({}):\n```{}\n{}\n```\n\n{}",
            c.label, c.lang, c.content, m.text
        ),
        None => m.text.clone(),
    }
}

fn stream_lines(
    resp: ureq::Response,
    cancel: &AtomicBool,
    mut on_data: impl FnMut(&str) -> Result<bool, String>,
) -> Result<(), String> {
    let reader = BufReader::new(resp.into_reader());
    for line in reader.lines() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let line = line.map_err(|e| e.to_string())?;
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() {
            continue;
        }
        if data == "[DONE]" {
            break;
        }
        if !on_data(data)? {
            break;
        }
    }
    Ok(())
}

struct ChatReq {
    provider: Provider,
    base: String,
    key: String,
    model: String,
    history: Vec<Msg>,
}

fn run_chat(req: ChatReq, id: u64, tx: Sender<AiEvent>, cancel: Arc<AtomicBool>) {
    let ChatReq {
        provider,
        base,
        key,
        model,
        history,
    } = req;
    let base = base.trim_end_matches('/').to_string();
    let send = |s: String| {
        let _ = tx.send(AiEvent::Delta(id, s));
    };
    let result: Result<(), String> = (|| match provider {
        Provider::OpenAI => {
            let mut messages = vec![json!({ "role": "system", "content": SYSTEM_PROMPT })];
            for m in &history {
                let role = if m.role == Role::User { "user" } else { "assistant" };
                messages.push(json!({ "role": role, "content": user_content(m) }));
            }
            let body = json!({ "model": model, "messages": messages, "stream": true });
            let mut req = agent()
                .post(&format!("{}/chat/completions", base))
                .set("Content-Type", "application/json")
                .set("X-Title", "zetsu");
            if !key.is_empty() {
                req = req.set("Authorization", &format!("Bearer {}", key));
            }
            let resp = req.send_string(&body.to_string()).map_err(http_error)?;
            stream_lines(resp, &cancel, |d| {
                let v: Value = serde_json::from_str(d).map_err(|e| e.to_string())?;
                if let Some(e) = v["error"]["message"].as_str() {
                    return Err(e.to_string());
                }
                if let Some(t) = v["choices"][0]["delta"]["content"].as_str() {
                    if !t.is_empty() {
                        send(t.to_string());
                    }
                }
                Ok(true)
            })
        }
        Provider::Anthropic => {
            let messages: Vec<Value> = history
                .iter()
                .map(|m| {
                    let role = if m.role == Role::User { "user" } else { "assistant" };
                    json!({ "role": role, "content": user_content(m) })
                })
                .collect();
            let body = json!({
                "model": model,
                "max_tokens": 8192,
                "system": SYSTEM_PROMPT,
                "messages": messages,
                "stream": true,
            });
            let resp = agent()
                .post(&format!("{}/messages", base))
                .set("Content-Type", "application/json")
                .set("x-api-key", &key)
                .set("anthropic-version", "2023-06-01")
                .send_string(&body.to_string())
                .map_err(http_error)?;
            stream_lines(resp, &cancel, |d| {
                let v: Value = serde_json::from_str(d).map_err(|e| e.to_string())?;
                match v["type"].as_str() {
                    Some("content_block_delta") => {
                        if let Some(t) = v["delta"]["text"].as_str() {
                            send(t.to_string());
                        }
                        Ok(true)
                    }
                    Some("message_stop") => Ok(false),
                    Some("error") => Err(v["error"]["message"].as_str().unwrap_or("error").to_string()),
                    _ => Ok(true),
                }
            })
        }
        Provider::Gemini => {
            let contents: Vec<Value> = history
                .iter()
                .map(|m| {
                    let role = if m.role == Role::User { "user" } else { "model" };
                    json!({ "role": role, "parts": [{ "text": user_content(m) }] })
                })
                .collect();
            let body = json!({
                "contents": contents,
                "systemInstruction": { "parts": [{ "text": SYSTEM_PROMPT }] },
            });
            let model = model.trim_start_matches("models/");
            let resp = agent()
                .post(&format!("{}/models/{}:streamGenerateContent?alt=sse", base, model))
                .set("Content-Type", "application/json")
                .set("x-goog-api-key", &key)
                .send_string(&body.to_string())
                .map_err(http_error)?;
            stream_lines(resp, &cancel, |d| {
                let v: Value = serde_json::from_str(d).map_err(|e| e.to_string())?;
                if let Some(e) = v["error"]["message"].as_str() {
                    return Err(e.to_string());
                }
                if let Some(parts) = v["candidates"][0]["content"]["parts"].as_array() {
                    for p in parts {
                        if let Some(t) = p["text"].as_str() {
                            send(t.to_string());
                        }
                    }
                }
                Ok(true)
            })
        }
    })();
    let _ = match result {
        Ok(()) => tx.send(AiEvent::Done(id)),
        Err(e) => tx.send(AiEvent::Error(id, e)),
    };
}

#[derive(Clone)]
pub struct AiCfg {
    pub provider: Provider,
    pub base: String,
    pub key: String,
    pub model: String,
}

pub fn complete(c: &AiCfg, system: &str, user: &str) -> Result<String, String> {
    let base = c.base.trim_end_matches('/');
    let v: Value = match c.provider {
        Provider::OpenAI => {
            let body = json!({
                "model": c.model,
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": user },
                ],
            });
            let mut req = agent()
                .post(&format!("{}/chat/completions", base))
                .set("Content-Type", "application/json")
                .set("X-Title", "zetsu");
            if !c.key.is_empty() {
                req = req.set("Authorization", &format!("Bearer {}", c.key));
            }
            let s = req
                .send_string(&body.to_string())
                .map_err(http_error)?
                .into_string()
                .map_err(|e| e.to_string())?;
            let v: Value = serde_json::from_str(&s).map_err(|e| e.to_string())?;
            v["choices"][0]["message"]["content"].clone()
        }
        Provider::Anthropic => {
            let body = json!({
                "model": c.model,
                "max_tokens": 4096,
                "system": system,
                "messages": [{ "role": "user", "content": user }],
            });
            let s = agent()
                .post(&format!("{}/messages", base))
                .set("Content-Type", "application/json")
                .set("x-api-key", &c.key)
                .set("anthropic-version", "2023-06-01")
                .send_string(&body.to_string())
                .map_err(http_error)?
                .into_string()
                .map_err(|e| e.to_string())?;
            let v: Value = serde_json::from_str(&s).map_err(|e| e.to_string())?;
            let text: String = v["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|b| b["text"].as_str())
                .collect();
            Value::String(text)
        }
        Provider::Gemini => {
            let body = json!({
                "contents": [{ "role": "user", "parts": [{ "text": user }] }],
                "systemInstruction": { "parts": [{ "text": system }] },
            });
            let model = c.model.trim_start_matches("models/");
            let s = agent()
                .post(&format!("{}/models/{}:generateContent", base, model))
                .set("Content-Type", "application/json")
                .set("x-goog-api-key", &c.key)
                .send_string(&body.to_string())
                .map_err(http_error)?
                .into_string()
                .map_err(|e| e.to_string())?;
            let v: Value = serde_json::from_str(&s).map_err(|e| e.to_string())?;
            let text: String = v["candidates"][0]["content"]["parts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p["text"].as_str())
                .collect();
            Value::String(text)
        }
    };
    match v.as_str() {
        Some(t) if !t.trim().is_empty() => Ok(t.to_string()),
        _ => Err("empty response".into()),
    }
}

fn run_models(provider: Provider, base: String, key: String, tx: Sender<AiEvent>) {
    let base = base.trim_end_matches('/').to_string();
    let result: Result<Vec<String>, String> = (|| {
        let req = match provider {
            Provider::OpenAI => {
                let r = agent().get(&format!("{}/models", base));
                if key.is_empty() {
                    r
                } else {
                    r.set("Authorization", &format!("Bearer {}", key))
                }
            }
            Provider::Anthropic => agent()
                .get(&format!("{}/models?limit=1000", base))
                .set("x-api-key", &key)
                .set("anthropic-version", "2023-06-01"),
            Provider::Gemini => agent()
                .get(&format!("{}/models?pageSize=1000", base))
                .set("x-goog-api-key", &key),
        };
        let body = req
            .call()
            .map_err(http_error)?
            .into_string()
            .map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let mut names: Vec<String> = match provider {
            Provider::Gemini => v["models"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter(|m| {
                    m["supportedGenerationMethods"]
                        .as_array()
                        .is_none_or(|a| a.iter().any(|x| x == "generateContent"))
                })
                .filter_map(|m| m["name"].as_str().map(|n| n.trim_start_matches("models/").to_string()))
                .collect(),
            _ => v["data"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                .collect(),
        };
        names.sort();
        names.dedup();
        Ok(names)
    })();
    let _ = match result {
        Ok(n) => tx.send(AiEvent::Models(n)),
        Err(e) => tx.send(AiEvent::ModelsError(e)),
    };
}

impl Ai {
    pub fn new() -> Self {
        let cfg = crate::config::load();
        let provider = cfg
            .get("ai_provider")
            .and_then(|p| Provider::parse(p))
            .unwrap_or(Provider::OpenAI);
        let (tx, rx) = channel();
        let mut ai = Ai {
            visible: false,
            width: 56,
            provider,
            model: String::new(),
            base_url: String::new(),
            msgs: Vec::new(),
            input: Vec::new(),
            input_x: 0,
            scroll: 0,
            streaming: None,
            attach: true,
            sel_ctx: None,
            models: Vec::new(),
            loading_models: false,
            chat_id: String::new(),
            next_id: 1,
            tx,
            rx,
        };
        ai.load_provider_settings();
        if let Some(w) = cfg.get("ai_width").and_then(|w| w.parse().ok()) {
            ai.width = w;
        }
        ai.load_latest();
        ai
    }

    pub fn load_provider_settings(&mut self) {
        let cfg = crate::config::load();
        let p = self.provider.id();
        self.model = cfg.get(&format!("ai_model_{}", p)).cloned().unwrap_or_default();
        self.base_url = cfg
            .get(&format!("ai_url_{}", p))
            .cloned()
            .unwrap_or_else(|| self.provider.default_url().to_string());
        self.models.clear();
    }

    pub fn key(&self) -> String {
        let cfg = crate::config::load();
        if let Some(k) = cfg
            .get(&format!("ai_key_{}", self.provider.id()))
            .filter(|k| !k.is_empty())
        {
            return k.clone();
        }
        self.provider
            .env_keys()
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .find(|k| !k.is_empty())
            .unwrap_or_default()
    }

    pub fn cfg(&self) -> Option<AiCfg> {
        let key = self.key();
        if self.model.is_empty() || (self.needs_key() && key.is_empty()) {
            return None;
        }
        Some(AiCfg {
            provider: self.provider,
            base: self.base_url.clone(),
            key,
            model: self.model.clone(),
        })
    }

    pub fn needs_key(&self) -> bool {
        let local = self.base_url.contains("localhost") || self.base_url.contains("127.0.0.1");
        !(self.provider == Provider::OpenAI && local)
    }

    pub fn set_provider(&mut self, p: Provider) {
        self.provider = p;
        let _ = crate::config::set("ai_provider", p.id());
        self.load_provider_settings();
    }

    pub fn set_model(&mut self, m: &str) {
        self.model = m.to_string();
        let _ = crate::config::set(&format!("ai_model_{}", self.provider.id()), m);
    }

    pub fn set_url(&mut self, u: &str) {
        self.base_url = u.trim_end_matches('/').to_string();
        let _ = crate::config::set(&format!("ai_url_{}", self.provider.id()), &self.base_url);
        self.models.clear();
    }

    pub fn set_key(&mut self, k: &str) -> std::io::Result<()> {
        self.models.clear();
        crate::config::set(&format!("ai_key_{}", self.provider.id()), k)
    }

    pub fn save_width(&self) {
        let _ = crate::config::set("ai_width", &self.width.to_string());
    }

    pub fn send(&mut self, history: Vec<Msg>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.streaming = Some((id, cancel.clone()));
        let req = ChatReq {
            provider: self.provider,
            base: self.base_url.clone(),
            key: self.key(),
            model: self.model.clone(),
            history,
        };
        let tx = self.tx.clone();
        thread::spawn(move || run_chat(req, id, tx, cancel));
        id
    }

    pub fn cancel(&mut self) {
        if let Some((_, c)) = self.streaming.take() {
            c.store(true, Ordering::Relaxed);
        }
    }

    pub fn fetch_models(&mut self) {
        if self.loading_models {
            return;
        }
        self.loading_models = true;
        let (p, base, key, tx) = (self.provider, self.base_url.clone(), self.key(), self.tx.clone());
        thread::spawn(move || run_models(p, base, key, tx));
    }

    pub fn poll(&mut self) -> Vec<AiEvent> {
        let mut out = Vec::new();
        while let Ok(e) = self.rx.try_recv() {
            out.push(e);
        }
        out
    }

    pub fn new_chat(&mut self) {
        self.cancel();
        self.msgs.clear();
        self.scroll = 0;
        self.chat_id = String::new();
    }

    pub fn save_chat(&mut self) {
        if self.msgs.is_empty() {
            return;
        }
        let Some(dir) = chats_dir() else { return };
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        if self.chat_id.is_empty() {
            self.chat_id = format!("{}", now());
        }
        let title: String = self
            .msgs
            .iter()
            .find(|m| m.role == Role::User)
            .map(|m| m.text.lines().next().unwrap_or("").chars().take(60).collect())
            .unwrap_or_default();
        let msgs: Vec<Value> = self
            .msgs
            .iter()
            .filter(|m| !m.error)
            .map(|m| {
                let mut v = json!({
                    "role": if m.role == Role::User { "user" } else { "assistant" },
                    "text": m.text,
                });
                if let Some(c) = &m.ctx {
                    v["ctx"] = json!({
                        "path": c.path.as_ref().map(|p| p.display().to_string()),
                        "label": c.label,
                        "lang": c.lang,
                        "content": c.content,
                        "sel": c.sel.map(|(a, b)| vec![a, b]),
                    });
                }
                v
            })
            .collect();
        let doc = json!({
            "title": title,
            "updated": now(),
            "provider": self.provider.id(),
            "model": self.model,
            "messages": msgs,
        });
        let _ = fs::write(dir.join(format!("{}.json", self.chat_id)), doc.to_string());
    }

    pub fn list_chats(&self) -> Vec<ChatInfo> {
        let Some(dir) = chats_dir() else { return Vec::new() };
        let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
        let mut v: Vec<ChatInfo> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let p = e.path();
                if p.extension().is_none_or(|x| x != "json") {
                    return None;
                }
                let doc: Value = serde_json::from_str(&fs::read_to_string(&p).ok()?).ok()?;
                Some(ChatInfo {
                    id: p.file_stem()?.to_string_lossy().into_owned(),
                    title: doc["title"].as_str().unwrap_or("").to_string(),
                    updated: doc["updated"].as_u64().unwrap_or(0),
                })
            })
            .collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.updated));
        v
    }

    pub fn load_chat(&mut self, id: &str) -> bool {
        let Some(dir) = chats_dir() else { return false };
        let Ok(text) = fs::read_to_string(dir.join(format!("{}.json", id))) else {
            return false;
        };
        let Ok(doc) = serde_json::from_str::<Value>(&text) else {
            return false;
        };
        self.cancel();
        self.msgs = doc["messages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|m| Msg {
                role: if m["role"] == "user" {
                    Role::User
                } else {
                    Role::Assistant
                },
                text: m["text"].as_str().unwrap_or("").to_string(),
                ctx: m["ctx"].as_object().map(|c| Ctx {
                    path: c.get("path").and_then(|p| p.as_str()).map(PathBuf::from),
                    label: c.get("label").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    lang: c.get("lang").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    content: c.get("content").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    sel: c
                        .get("sel")
                        .and_then(|s| s.as_array())
                        .and_then(|a| Some((a.first()?.as_u64()? as usize, a.get(1)?.as_u64()? as usize))),
                }),
                error: false,
            })
            .collect();
        self.chat_id = id.to_string();
        self.scroll = 0;
        true
    }

    pub fn delete_chat(&self, id: &str) {
        if let Some(dir) = chats_dir() {
            let _ = fs::remove_file(dir.join(format!("{}.json", id)));
        }
    }

    fn load_latest(&mut self) {
        if let Some(c) = self.list_chats().first() {
            let id = c.id.clone();
            self.load_chat(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks() {
        let t = "hi\n```rust\nfn a() {}\nlet x = 1;\n```\ntext\n```\nplain\n```";
        let b = code_blocks(t);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].0, "rust");
        assert_eq!(b[0].1, "fn a() {}\nlet x = 1;");
        assert_eq!(b[1].1, "plain");
    }
}
