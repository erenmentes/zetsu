use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::env;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Enc {
    Utf8,
    Utf16,
    Utf32,
}

pub fn to_lsp_col(line: &[char], x: usize, enc: Enc) -> usize {
    let x = x.min(line.len());
    match enc {
        Enc::Utf32 => x,
        Enc::Utf16 => line[..x].iter().map(|c| c.len_utf16()).sum(),
        Enc::Utf8 => line[..x].iter().map(|c| c.len_utf8()).sum(),
    }
}

pub fn from_lsp_col(line: &[char], col: usize, enc: Enc) -> usize {
    if enc == Enc::Utf32 {
        return col.min(line.len());
    }
    let mut acc = 0;
    for (i, c) in line.iter().enumerate() {
        if acc >= col {
            return i;
        }
        acc += if enc == Enc::Utf16 { c.len_utf16() } else { c.len_utf8() };
    }
    line.len()
}

pub fn path_to_uri(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut dec = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                dec.push(v);
                i += 3;
                continue;
            }
        }
        dec.push(bytes[i]);
        i += 1;
    }
    let mut s = String::from_utf8_lossy(&dec).into_owned();
    if cfg!(windows) {
        if s.starts_with('/') && s.len() > 2 && s.as_bytes()[2] == b':' {
            s.remove(0);
        }
        s = s.replace('/', "\\");
    }
    Some(PathBuf::from(s))
}

pub fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

pub fn candidates(lang: &str) -> &'static [&'static [&'static str]] {
    match lang {
        "Rust" => &[&["rust-analyzer"]],
        "C/C++" => &[&["clangd"]],
        "Python" => &[&["pyright-langserver", "--stdio"], &["basedpyright-langserver", "--stdio"], &["pylsp"]],
        "JS/TS" => &[&["typescript-language-server", "--stdio"]],
        "Go" => &[&["gopls"]],
        "Lua" => &[&["lua-language-server"]],
        "Shell" => &[&["bash-language-server", "start"]],
        _ => &[],
    }
}

fn root_markers(lang: &str) -> &'static [&'static str] {
    match lang {
        "Rust" => &["Cargo.toml"],
        "JS/TS" => &["package.json", "tsconfig.json", "jsconfig.json"],
        "Go" => &["go.mod"],
        "Python" => &["pyproject.toml", "setup.py", "setup.cfg", "requirements.txt"],
        "C/C++" => &["compile_commands.json", "CMakeLists.txt", ".clangd", "Makefile"],
        _ => &[],
    }
}

pub fn find_root(lang: &str, file: &Path, fallback: &Path) -> PathBuf {
    let mut dir = file.parent();
    let markers = root_markers(lang);
    let mut git = None;
    while let Some(d) = dir {
        if markers.iter().any(|m| d.join(m).exists()) {
            return d.to_path_buf();
        }
        if git.is_none() && d.join(".git").exists() {
            git = Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    git.unwrap_or_else(|| fallback.to_path_buf())
}

pub fn language_id(lang: &str, path: &Path) -> &'static str {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match lang {
        "Rust" => "rust",
        "C/C++" => {
            if ext == "c" || ext == "h" {
                "c"
            } else {
                "cpp"
            }
        }
        "Python" => "python",
        "JS/TS" => match ext.as_str() {
            "ts" | "mts" => "typescript",
            "tsx" => "typescriptreact",
            "jsx" => "javascriptreact",
            _ => "javascript",
        },
        "Go" => "go",
        "Lua" => "lua",
        "Shell" => "shellscript",
        _ => "plaintext",
    }
}

pub fn which(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        let pe = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        pe.split(';').filter(|s| !s.is_empty()).map(|s| s.to_lowercase()).collect()
    } else {
        vec![String::new()]
    };
    for dir in env::split_paths(&path) {
        for e in &exts {
            let p = dir.join(format!("{}{}", name, e));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

#[derive(Clone, Debug)]
pub enum Pending {
    Init,
    Completion,
    CompResolve(usize),
    Hover,
    Definition,
    References,
    CodeAction,
    Resolve,
    Signature,
    Format(PathBuf),
    Rename,
    Execute,
}

pub struct Diag {
    pub l0: usize,
    pub c0: usize,
    pub l1: usize,
    pub c1: usize,
    pub sev: u8,
    pub msg: String,
    pub raw: Value,
    pub enc: Enc,
}

pub enum LspEvent {
    Ready(String),
    Response { lang: &'static str, id: i64, kind: Pending, result: Value, error: Option<String> },
    Notification { lang: &'static str, method: String, params: Value },
    ApplyEdit { lang: &'static str, id: Value, params: Value },
    Exited(&'static str, String),
}

enum Raw {
    Msg(&'static str, Value),
    Eof(&'static str),
}

pub struct Server {
    pub name: String,
    child: Child,
    stdin: ChildStdin,
    next_id: i64,
    pending: HashMap<i64, Pending>,
    pub ready: bool,
    pub caps: Value,
    pub enc: Enc,
    queue: Vec<Value>,
    pub root: PathBuf,
    pub progress: HashMap<String, (String, String)>,
    pub dead: bool,
}

pub fn log(dir: &str, body: &str) {
    if let Some(p) = env::var_os("ZETSU_LSP_LOG") {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let short: String = body.chars().take(200000).collect();
            let _ = writeln!(f, "{} {}", dir, short);
        }
    }
}

impl Server {
    fn write(&mut self, v: &Value) {
        if self.dead {
            return;
        }
        let body = v.to_string();
        log(">>", &body);
        let msg = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        if self.stdin.write_all(msg.as_bytes()).and_then(|_| self.stdin.flush()).is_err() {
            self.dead = true;
        }
    }
}

pub struct Lsp {
    pub servers: HashMap<&'static str, Server>,
    pub failed: HashSet<&'static str>,
    pub diags: HashMap<String, Vec<Diag>>,
    tx: Sender<Raw>,
    rx: Receiver<Raw>,
}

fn reader(stdout: impl Read, lang: &'static str, tx: Sender<Raw>) {
    let mut r = BufReader::new(stdout);
    loop {
        let mut len: Option<usize> = None;
        loop {
            let mut line = String::new();
            match r.read_line(&mut line) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(Raw::Eof(lang));
                    return;
                }
                Ok(_) => {}
            }
            let l = line.trim_end();
            if l.is_empty() {
                if len.is_some() {
                    break;
                }
                continue;
            }
            if let Some(v) = l.strip_prefix("Content-Length:") {
                len = v.trim().parse().ok();
            }
        }
        let mut buf = vec![0u8; len.unwrap_or(0)];
        if r.read_exact(&mut buf).is_err() {
            let _ = tx.send(Raw::Eof(lang));
            return;
        }
        log("<<", &String::from_utf8_lossy(&buf));
        if let Ok(v) = serde_json::from_slice::<Value>(&buf) {
            if tx.send(Raw::Msg(lang, v)).is_err() {
                return;
            }
        }
    }
}

fn client_capabilities() -> Value {
    json!({
        "general": { "positionEncodings": ["utf-32", "utf-16"] },
        "textDocument": {
            "synchronization": { "didSave": true, "dynamicRegistration": false },
            "completion": {
                "completionItem": {
                    "snippetSupport": true,
                    "documentationFormat": ["plaintext", "markdown"],
                    "resolveSupport": { "properties": ["documentation", "detail", "additionalTextEdits"] },
                    "labelDetailsSupport": true,
                    "insertReplaceSupport": false
                },
                "contextSupport": true
            },
            "hover": { "contentFormat": ["plaintext", "markdown"] },
            "signatureHelp": {
                "signatureInformation": {
                    "documentationFormat": ["plaintext"],
                    "parameterInformation": { "labelOffsetSupport": true }
                }
            },
            "definition": { "linkSupport": true },
            "references": {},
            "publishDiagnostics": { "relatedInformation": false },
            "codeAction": {
                "codeActionLiteralSupport": {
                    "codeActionKind": { "valueSet": [
                        "", "quickfix", "refactor", "refactor.extract", "refactor.inline",
                        "refactor.rewrite", "source", "source.organizeImports"
                    ] }
                },
                "resolveSupport": { "properties": ["edit"] },
                "dataSupport": true
            },
            "formatting": {},
            "rename": { "prepareSupport": false }
        },
        "workspace": {
            "applyEdit": true,
            "workspaceEdit": { "documentChanges": true },
            "configuration": true,
            "workspaceFolders": true
        },
        "window": { "workDoneProgress": true }
    })
}

impl Lsp {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Lsp { servers: HashMap::new(), failed: HashSet::new(), diags: HashMap::new(), tx, rx }
    }

    pub fn alive(&self, lang: &str) -> bool {
        self.servers.get(lang).map_or(false, |s| !s.dead)
    }

    pub fn ready(&self, lang: &str) -> bool {
        self.servers.get(lang).map_or(false, |s| s.ready && !s.dead)
    }

    pub fn enc(&self, lang: &str) -> Enc {
        self.servers.get(lang).map_or(Enc::Utf16, |s| s.enc)
    }

    pub fn cap(&self, lang: &str, name: &str) -> bool {
        self.servers.get(lang).map_or(false, |s| {
            let c = &s.caps[name];
            !c.is_null() && c != &Value::Bool(false)
        })
    }

    pub fn trigger_chars(&self, lang: &str, provider: &str) -> Vec<char> {
        self.servers
            .get(lang)
            .and_then(|s| s.caps[provider]["triggerCharacters"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_str().and_then(|s| s.chars().next()))
            .collect()
    }

    pub fn start(&mut self, lang: &'static str, file: &Path, cwd: &Path) -> Result<String, String> {
        if self.alive(lang) {
            return Ok(self.servers[lang].name.clone());
        }
        if self.failed.contains(lang) {
            return Err(String::new());
        }
        let cands = candidates(lang);
        let found = cands.iter().find_map(|c| which(c[0]).map(|p| (p, *c)));
        let Some((exe, spec)) = found else {
            self.failed.insert(lang);
            let names: Vec<&str> = cands.iter().map(|c| c[0]).collect();
            return Err(tf!("LSP bulunamadı: {} (PATH'e ekleyin)", "Language server not found: {} (add it to PATH)", names.join(" / ")));
        };
        let root = find_root(lang, file, cwd);
        let ext = exe.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let mut cmd = if ext == "cmd" || ext == "bat" {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&exe);
            c
        } else {
            Command::new(&exe)
        };
        cmd.args(&spec[1..])
            .current_dir(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.failed.insert(lang);
                return Err(tf!("{} başlatılamadı: {}", "could not start {}: {}", spec[0], e));
            }
        };
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let tx = self.tx.clone();
        thread::spawn(move || reader(stdout, lang, tx));
        let name = spec[0].to_string();
        let mut srv = Server {
            name: name.clone(),
            child,
            stdin,
            next_id: 1,
            pending: HashMap::new(),
            ready: false,
            caps: Value::Null,
            enc: Enc::Utf16,
            queue: Vec::new(),
            root: root.clone(),
            progress: HashMap::new(),
            dead: false,
        };
        let root_name = root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": { "name": "zetsu", "version": env!("CARGO_PKG_VERSION") },
            "rootUri": path_to_uri(&root),
            "rootPath": root.to_string_lossy(),
            "workspaceFolders": [{ "uri": path_to_uri(&root), "name": root_name }],
            "capabilities": client_capabilities(),
        });
        srv.pending.insert(0, Pending::Init);
        srv.write(&json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": params }));
        self.servers.insert(lang, srv);
        Ok(name)
    }

    pub fn notify(&mut self, lang: &str, method: &str, params: Value) {
        let Some(s) = self.servers.get_mut(lang) else { return };
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        if s.ready {
            s.write(&msg);
        } else {
            s.queue.push(msg);
        }
    }

    pub fn request(&mut self, lang: &str, method: &str, params: Value, kind: Pending) -> Option<i64> {
        let s = self.servers.get_mut(lang)?;
        if !s.ready || s.dead {
            return None;
        }
        let id = s.next_id;
        s.next_id += 1;
        s.pending.insert(id, kind);
        s.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        Some(id)
    }

    pub fn cancel(&mut self, lang: &str, id: i64) {
        if let Some(s) = self.servers.get_mut(lang) {
            if s.pending.remove(&id).is_some() {
                s.write(&json!({ "jsonrpc": "2.0", "method": "$/cancelRequest", "params": { "id": id } }));
            }
        }
    }

    pub fn reply(&mut self, lang: &str, id: Value, result: Value) {
        if let Some(s) = self.servers.get_mut(lang) {
            s.write(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        }
    }

    pub fn stop(&mut self, lang: &str) {
        if let Some(mut s) = self.servers.remove(lang) {
            s.write(&json!({ "jsonrpc": "2.0", "id": 999_999, "method": "shutdown" }));
            s.write(&json!({ "jsonrpc": "2.0", "method": "exit" }));
            let _ = s.child.kill();
            let _ = s.child.wait();
        }
        self.diags.retain(|_, _| true);
    }

    pub fn stop_all(&mut self) {
        let langs: Vec<&'static str> = self.servers.keys().copied().collect();
        for l in langs {
            self.stop(l);
        }
    }

    pub fn poll(&mut self) -> Vec<LspEvent> {
        let mut out = Vec::new();
        while let Ok(raw) = self.rx.try_recv() {
            match raw {
                Raw::Eof(lang) => {
                    if let Some(s) = self.servers.get_mut(lang) {
                        if !s.dead || s.ready {
                            let name = s.name.clone();
                            s.dead = true;
                            s.ready = false;
                            self.servers.remove(lang);
                            self.failed.insert(lang);
                            out.push(LspEvent::Exited(lang, name));
                        }
                    }
                }
                Raw::Msg(lang, v) => self.dispatch(lang, v, &mut out),
            }
        }
        out
    }

    fn dispatch(&mut self, lang: &'static str, v: Value, out: &mut Vec<LspEvent>) {
        let Some(s) = self.servers.get_mut(lang) else { return };
        let method = v["method"].as_str().map(|m| m.to_string());
        let has_id = !v["id"].is_null();
        match (method, has_id) {
            (Some(m), true) => {
                let id = v["id"].clone();
                match m.as_str() {
                    "workspace/applyEdit" => {
                        out.push(LspEvent::ApplyEdit { lang, id, params: v["params"].clone() });
                    }
                    "workspace/configuration" => {
                        let n = v["params"]["items"].as_array().map_or(0, |a| a.len());
                        let res = Value::Array(vec![Value::Null; n]);
                        s.write(&json!({ "jsonrpc": "2.0", "id": id, "result": res }));
                    }
                    "workspace/workspaceFolders" => {
                        let root = s.root.clone();
                        let name = root.file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
                        s.write(&json!({ "jsonrpc": "2.0", "id": id, "result": [{ "uri": path_to_uri(&root), "name": name }] }));
                    }
                    _ => s.write(&json!({ "jsonrpc": "2.0", "id": id, "result": Value::Null })),
                }
            }
            (Some(m), false) => {
                if m == "$/progress" {
                    let token = v["params"]["token"].to_string();
                    let val = &v["params"]["value"];
                    let mut text = String::new();
                    if let Some(msg) = val["message"].as_str() {
                        text.push_str(msg);
                    }
                    if let Some(p) = val["percentage"].as_u64() {
                        text = format!("{} {}%", text, p);
                    }
                    match val["kind"].as_str() {
                        Some("begin") => {
                            let title = val["title"].as_str().unwrap_or("").to_string();
                            s.progress.insert(token, (title, text));
                        }
                        Some("report") => {
                            if let Some(e) = s.progress.get_mut(&token) {
                                e.1 = text;
                            }
                        }
                        _ => {
                            s.progress.remove(&token);
                        }
                    }
                } else {
                    out.push(LspEvent::Notification { lang, method: m, params: v["params"].clone() });
                }
            }
            (None, true) => {
                let id = v["id"].as_i64().unwrap_or(-1);
                let Some(kind) = s.pending.remove(&id) else { return };
                let error = v["error"]["message"].as_str().map(|e| e.to_string());
                if let Pending::Init = kind {
                    s.caps = v["result"]["capabilities"].clone();
                    s.enc = match s.caps["positionEncoding"].as_str() {
                        Some("utf-32") => Enc::Utf32,
                        Some("utf-8") => Enc::Utf8,
                        _ => Enc::Utf16,
                    };
                    s.ready = true;
                    s.write(&json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }));
                    for m in std::mem::take(&mut s.queue) {
                        s.write(&m);
                    }
                    out.push(LspEvent::Ready(s.name.clone()));
                    return;
                }
                out.push(LspEvent::Response { lang, id, kind, result: v["result"].clone(), error });
            }
            _ => {}
        }
    }
}

pub fn expand_snippet(s: &str) -> (String, Option<usize>) {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut best: Option<(u32, usize)> = None;
    expand_into(&chars, &mut 0, &mut out, &mut best, false);
    (out.clone(), best.map(|b| b.1))
}

fn note_stop(best: &mut Option<(u32, usize)>, n: u32, pos: usize) {
    let key = if n == 0 { u32::MAX } else { n };
    if best.map_or(true, |(k, _)| key < k) {
        *best = Some((key, pos));
    }
}

fn expand_into(c: &[char], i: &mut usize, out: &mut String, best: &mut Option<(u32, usize)>, nested: bool) {
    while *i < c.len() {
        let ch = c[*i];
        if ch == '\\' && *i + 1 < c.len() {
            out.push(c[*i + 1]);
            *i += 2;
            continue;
        }
        if nested && ch == '}' {
            *i += 1;
            return;
        }
        if ch == '$' && *i + 1 < c.len() {
            let nx = c[*i + 1];
            if nx.is_ascii_digit() {
                let mut j = *i + 1;
                let mut n = 0u32;
                while j < c.len() && c[j].is_ascii_digit() {
                    n = n * 10 + c[j].to_digit(10).unwrap();
                    j += 1;
                }
                note_stop(best, n, out.chars().count());
                *i = j;
                continue;
            }
            if nx == '{' {
                let mut j = *i + 2;
                let mut n = 0u32;
                let mut is_num = false;
                while j < c.len() && c[j].is_ascii_digit() {
                    n = n * 10 + c[j].to_digit(10).unwrap();
                    j += 1;
                    is_num = true;
                }
                if is_num {
                    note_stop(best, n, out.chars().count());
                }
                if j < c.len() && c[j] == ':' {
                    *i = j + 1;
                    expand_into(c, i, out, best, true);
                    continue;
                }
                if j < c.len() && c[j] == '|' {
                    j += 1;
                    while j < c.len() && c[j] != ',' && c[j] != '|' {
                        out.push(c[j]);
                        j += 1;
                    }
                    while j < c.len() && c[j] != '}' {
                        j += 1;
                    }
                    *i = j + 1;
                    continue;
                }
                while j < c.len() && c[j] != '}' {
                    j += 1;
                }
                *i = j + 1;
                continue;
            }
        }
        out.push(ch);
        *i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets() {
        assert_eq!(expand_snippet("foo($0)"), ("foo()".into(), Some(4)));
        assert_eq!(expand_snippet("foo(${1:a}, ${2:b})$0"), ("foo(a, b)".into(), Some(4)));
        assert_eq!(expand_snippet("plain"), ("plain".into(), None));
        assert_eq!(expand_snippet("x\\$y"), ("x$y".into(), None));
    }

    #[test]
    fn cols() {
        let l: Vec<char> = "a😀b".chars().collect();
        assert_eq!(to_lsp_col(&l, 2, Enc::Utf16), 3);
        assert_eq!(from_lsp_col(&l, 3, Enc::Utf16), 2);
        assert_eq!(to_lsp_col(&l, 2, Enc::Utf8), 5);
    }

    #[test]
    fn uris() {
        let p = Path::new("/tmp/a b/c.rs");
        let u = path_to_uri(p);
        assert_eq!(u, "file:///tmp/a%20b/c.rs");
        assert_eq!(uri_to_path(&u).unwrap(), p);
    }
}
