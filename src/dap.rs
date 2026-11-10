use crate::lsp::{find_root, which};
use crate::tools::{self, Tool};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    CodeLldb,
    Lldb,
    Gdb,
    Debugpy,
    Delve,
}

#[derive(Clone, Debug)]
pub struct Adapter {
    pub kind: Kind,
    pub name: String,
    pub cmd: Vec<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Debug)]
pub enum PlanErr {
    Missing(Tool),
    Other(String),
}

impl From<String> for PlanErr {
    fn from(s: String) -> Self {
        PlanErr::Other(s)
    }
}

impl From<&str> for PlanErr {
    fn from(s: &str) -> Self {
        PlanErr::Other(s.to_string())
    }
}

#[derive(Clone, Debug)]
pub enum Req {
    Initialize,
    Launch,
    SetBreakpoints(PathBuf),
    ConfigDone,
    StackTrace,
    Scopes,
    Variables(i64),
    Evaluate(EvalKind),
    Step,
    Disconnect,
    Other,
}

#[derive(Clone, Debug)]
pub enum EvalKind {
    Repl,
    Watch(usize),
    Hover,
}

pub enum DapMsg {
    Response {
        req: Req,
        success: bool,
        message: String,
        body: Value,
    },
    Event {
        event: String,
        body: Value,
    },
    Request {
        seq: i64,
        command: String,
    },
    Closed,
}

enum Raw {
    Msg(Value),
    Eof,
}

pub struct Client {
    child: Child,
    writer: Box<dyn Write + Send>,
    seq: i64,
    pending: HashMap<i64, Req>,
    rx: Receiver<Raw>,
    pub caps: Value,
    pub adapter: Adapter,
    pub closed: bool,
}

fn reader(r: impl Read, tx: Sender<Raw>) {
    let mut r = BufReader::new(r);
    loop {
        let mut len: Option<usize> = None;
        loop {
            let mut line = String::new();
            match r.read_line(&mut line) {
                Ok(0) | Err(_) => {
                    let _ = tx.send(Raw::Eof);
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
            let _ = tx.send(Raw::Eof);
            return;
        }
        crate::lsp::log("<< dap", &String::from_utf8_lossy(&buf));
        if let Ok(v) = serde_json::from_slice::<Value>(&buf) {
            if tx.send(Raw::Msg(v)).is_err() {
                return;
            }
        }
    }
}

fn free_port() -> Option<u16> {
    let l = TcpListener::bind("127.0.0.1:0").ok()?;
    Some(l.local_addr().ok()?.port())
}

#[cfg(unix)]
fn detach(c: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
}

#[cfg(windows)]
fn detach(c: &mut Command) {
    use std::os::windows::process::CommandExt;
    c.creation_flags(0x0800_0000);
}

fn spawn(cmd: &[String], env: &[(String, String)], cwd: &Path, piped: bool) -> std::io::Result<Child> {
    let exe = PathBuf::from(&cmd[0]);
    let ext = exe
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut c = if ext == "cmd" || ext == "bat" {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(&exe);
        c
    } else {
        Command::new(&exe)
    };
    c.args(&cmd[1..]).current_dir(cwd).stderr(Stdio::null());
    for (k, v) in env {
        c.env(k, v);
    }
    detach(&mut c);
    if piped {
        c.stdin(Stdio::piped()).stdout(Stdio::piped());
    } else {
        c.stdin(Stdio::null()).stdout(Stdio::null());
    }
    c.spawn()
}

impl Client {
    pub fn start(adapter: Adapter, cwd: &Path) -> Result<Client, String> {
        let (tx, rx) = channel();
        let (child, writer): (Child, Box<dyn Write + Send>) = if adapter.kind == Kind::Delve {
            let port = free_port().ok_or("no free port")?;
            let mut cmd = adapter.cmd.clone();
            cmd.push(format!("--listen=127.0.0.1:{}", port));
            let child = spawn(&cmd, &adapter.env, cwd, false).map_err(|e| e.to_string())?;
            let deadline = Instant::now() + Duration::from_secs(10);
            let stream = loop {
                match TcpStream::connect(("127.0.0.1", port)) {
                    Ok(s) => break s,
                    Err(e) if Instant::now() > deadline => return Err(e.to_string()),
                    Err(_) => thread::sleep(Duration::from_millis(100)),
                }
            };
            let rs = stream.try_clone().map_err(|e| e.to_string())?;
            thread::spawn(move || reader(rs, tx));
            (child, Box::new(stream))
        } else {
            let mut child = spawn(&adapter.cmd, &adapter.env, cwd, true).map_err(|e| e.to_string())?;
            let stdout = child.stdout.take().ok_or("no stdout")?;
            let stdin = child.stdin.take().ok_or("no stdin")?;
            thread::spawn(move || reader(stdout, tx));
            (child, Box::new(stdin))
        };
        Ok(Client {
            child,
            writer,
            seq: 1,
            pending: HashMap::new(),
            rx,
            caps: Value::Null,
            adapter,
            closed: false,
        })
    }

    fn write(&mut self, v: &Value) {
        let body = v.to_string();
        crate::lsp::log(">> dap", &body);
        let msg = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        if self
            .writer
            .write_all(msg.as_bytes())
            .and_then(|_| self.writer.flush())
            .is_err()
        {
            self.closed = true;
        }
    }

    pub fn request(&mut self, command: &str, args: Value, req: Req) -> i64 {
        let seq = self.seq;
        self.seq += 1;
        self.pending.insert(seq, req);
        self.write(&json!({ "seq": seq, "type": "request", "command": command, "arguments": args }));
        seq
    }

    pub fn respond(&mut self, request_seq: i64, command: &str, body: Value) {
        let seq = self.seq;
        self.seq += 1;
        self.write(&json!({
            "seq": seq,
            "type": "response",
            "request_seq": request_seq,
            "success": true,
            "command": command,
            "body": body,
        }));
    }

    pub fn poll(&mut self) -> Vec<DapMsg> {
        let mut out = Vec::new();
        while let Ok(raw) = self.rx.try_recv() {
            match raw {
                Raw::Eof => {
                    self.closed = true;
                    out.push(DapMsg::Closed);
                }
                Raw::Msg(v) => match v["type"].as_str() {
                    Some("response") => {
                        let seq = v["request_seq"].as_i64().unwrap_or(-1);
                        let req = self.pending.remove(&seq).unwrap_or(Req::Other);
                        out.push(DapMsg::Response {
                            req,
                            success: v["success"].as_bool().unwrap_or(false),
                            message: v["message"]
                                .as_str()
                                .or_else(|| v["body"]["error"]["format"].as_str())
                                .unwrap_or("")
                                .to_string(),
                            body: v["body"].clone(),
                        });
                    }
                    Some("event") => out.push(DapMsg::Event {
                        event: v["event"].as_str().unwrap_or("").to_string(),
                        body: v["body"].clone(),
                    }),
                    Some("request") => out.push(DapMsg::Request {
                        seq: v["seq"].as_i64().unwrap_or(0),
                        command: v["command"].as_str().unwrap_or("").to_string(),
                    }),
                    _ => {}
                },
            }
        }
        out
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.closed = true;
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn gdb_has_dap() -> bool {
    let Ok(o) = Command::new("gdb").arg("--version").stdin(Stdio::null()).output() else {
        return false;
    };
    let s = String::from_utf8_lossy(&o.stdout);
    let first = s.lines().next().unwrap_or("");
    first
        .split_whitespace()
        .filter_map(|w| w.split('.').next().and_then(|m| m.parse::<u32>().ok()))
        .next_back()
        .is_some_and(|major| major >= 14)
}

fn quiet(p: &Path, args: &[&str]) -> bool {
    Command::new(p)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn find_python() -> Option<PathBuf> {
    ["python3", "python", "py"].iter().filter_map(|n| which(n)).find(|p| {
        quiet(
            p,
            &["-c", "import sys; sys.exit(0 if sys.version_info >= (3, 8) else 1)"],
        )
    })
}

fn lldb_dap() -> Option<PathBuf> {
    let names = ["lldb-dap", "lldb-vscode"];
    if let Some(p) = names.iter().find_map(|n| which(n)) {
        return Some(p);
    }
    for v in (14..=24).rev() {
        if let Some(p) = which(&format!("lldb-dap-{}", v)) {
            return Some(p);
        }
    }
    if cfg!(target_os = "macos") {
        let o = Command::new("xcrun").args(["-f", "lldb-dap"]).output().ok()?;
        let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
        if o.status.success() && !s.is_empty() {
            return Some(PathBuf::from(s));
        }
    }
    None
}

fn adapter(kind: Kind, name: &str, cmd: Vec<String>) -> Adapter {
    Adapter {
        kind,
        name: name.into(),
        cmd,
        env: Vec::new(),
    }
}

pub fn codelldb_adapter() -> Option<Adapter> {
    tools::codelldb_path()
        .or_else(|| which("codelldb"))
        .map(|p| adapter(Kind::CodeLldb, "codelldb", vec![p.display().to_string()]))
}

fn lldb_adapter() -> Option<Adapter> {
    lldb_dap().map(|p| adapter(Kind::Lldb, "lldb-dap", vec![p.display().to_string()]))
}

fn gdb_adapter() -> Option<Adapter> {
    if !gdb_has_dap() {
        return None;
    }
    which("gdb").map(|p| {
        adapter(
            Kind::Gdb,
            "gdb",
            vec![p.display().to_string(), "-i=dap".into(), "-q".into()],
        )
    })
}

pub fn native_adapter(prefer: Option<Kind>, allow_install: bool) -> Result<Adapter, PlanErr> {
    let found = match prefer {
        Some(Kind::Gdb) => gdb_adapter().or_else(lldb_adapter).or_else(codelldb_adapter),
        Some(Kind::Lldb) => lldb_adapter().or_else(codelldb_adapter).or_else(gdb_adapter),
        Some(Kind::CodeLldb) if allow_install => codelldb_adapter(),
        _ => codelldb_adapter().or_else(lldb_adapter).or_else(gdb_adapter),
    };
    match found {
        Some(a) => Ok(a),
        None if allow_install => Err(PlanErr::Missing(Tool::CodeLldb)),
        None => codelldb_adapter()
            .or_else(lldb_adapter)
            .or_else(gdb_adapter)
            .ok_or_else(|| PlanErr::Other("no native debug adapter available".into())),
    }
}

pub fn python_adapter() -> Result<Adapter, PlanErr> {
    let py = find_python().ok_or("Python 3 was not found — install it from https://www.python.org/downloads")?;
    let cmd = vec![py.display().to_string(), "-m".into(), "debugpy.adapter".into()];
    if quiet(&py, &["-c", "import debugpy"]) {
        return Ok(adapter(Kind::Debugpy, "debugpy", cmd));
    }
    let Some(dir) = tools::debugpy_dir() else {
        return Err(PlanErr::Missing(Tool::Debugpy));
    };
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut pp = dir.display().to_string();
    if let Ok(old) = std::env::var("PYTHONPATH") {
        if !old.is_empty() {
            pp = format!("{}{}{}", pp, sep, old);
        }
    }
    let mut a = adapter(Kind::Debugpy, "debugpy", cmd);
    a.env.push(("PYTHONPATH".into(), pp));
    Ok(a)
}

pub fn go_adapter() -> Result<Adapter, PlanErr> {
    which("dlv")
        .or_else(tools::dlv_path)
        .map(|p| adapter(Kind::Delve, "dlv", vec![p.display().to_string(), "dap".into()]))
        .ok_or(PlanErr::Missing(Tool::Delve))
}

#[derive(Clone, Debug)]
pub struct LaunchPlan {
    pub adapter: Adapter,
    pub args: Value,
    pub cwd: PathBuf,
    pub cargo: Option<Vec<String>>,
}

pub fn launch_args(kind: Kind, program: &str, args: &[String], cwd: &Path, rust: bool) -> Value {
    let cwd = cwd.display().to_string();
    match kind {
        Kind::CodeLldb => {
            let mut v = json!({ "type": "lldb", "request": "launch", "program": program, "args": args, "cwd": cwd, "terminal": "console" });
            if rust {
                v["sourceLanguages"] = json!(["rust"]);
            }
            v
        }
        Kind::Lldb => json!({ "program": program, "args": args, "cwd": cwd, "stopOnEntry": false }),
        Kind::Gdb => json!({ "program": program, "args": args, "cwd": cwd, "stopAtBeginningOfMainSubprogram": true }),
        Kind::Debugpy => json!({
            "type": "python",
            "request": "launch",
            "program": program,
            "args": args,
            "cwd": cwd,
            "console": "internalConsole",
            "redirectOutput": true,
            "justMyCode": true,
        }),
        Kind::Delve => json!({ "mode": "debug", "program": program, "args": args, "cwd": cwd }),
    }
}

fn strip_jsonc(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut in_str = false;
    while i < c.len() {
        if in_str {
            out.push(c[i]);
            if c[i] == '\\' && i + 1 < c.len() {
                out.push(c[i + 1]);
                i += 2;
                continue;
            }
            if c[i] == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c[i] == '"' {
            in_str = true;
            out.push('"');
            i += 1;
        } else if c[i] == '/' && c.get(i + 1) == Some(&'/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if c[i] == '/' && c.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < c.len() && !(c[i] == '*' && c[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    let re = regex::Regex::new(r",(\s*[}\]])").unwrap();
    re.replace_all(&out, "$1").into_owned()
}

fn substitute(v: &Value, vars: &[(&str, String)]) -> Value {
    match v {
        Value::String(s) => {
            let mut s = s.clone();
            for (k, val) in vars {
                s = s.replace(&format!("${{{}}}", k), val);
            }
            Value::String(s)
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| substitute(x, vars)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), substitute(x, vars))).collect()),
        other => other.clone(),
    }
}

pub fn from_launch_json(root: &Path, file: Option<&Path>, allow_install: bool) -> Option<Result<LaunchPlan, PlanErr>> {
    let text = std::fs::read_to_string(root.join(".vscode").join("launch.json")).ok()?;
    let doc: Value = serde_json::from_str(&strip_jsonc(&text)).ok()?;
    let file_s = file.map(|f| f.display().to_string()).unwrap_or_default();
    let dir_s = file
        .and_then(|f| f.parent())
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    let stem = file
        .and_then(|f| f.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let base = file
        .and_then(|f| f.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let vars = [
        ("workspaceFolder", root.display().to_string()),
        ("workspaceRoot", root.display().to_string()),
        ("file", file_s),
        ("fileDirname", dir_s),
        ("fileBasenameNoExtension", stem),
        ("fileBasename", base),
        ("pathSeparator", std::path::MAIN_SEPARATOR.to_string()),
    ];
    for cfg in doc["configurations"].as_array()? {
        if cfg["request"].as_str() != Some("launch") {
            continue;
        }
        let cfg = substitute(cfg, &vars);
        let ty = cfg["type"].as_str().unwrap_or("");
        let adapter = match ty {
            "lldb" | "codelldb" => native_adapter(Some(Kind::CodeLldb), allow_install),
            "lldb-dap" | "lldb-vscode" => native_adapter(Some(Kind::Lldb), allow_install),
            "cppdbg" => {
                let prefer = if cfg["MIMode"].as_str() == Some("lldb") {
                    Kind::Lldb
                } else {
                    Kind::Gdb
                };
                native_adapter(Some(prefer), allow_install)
            }
            "gdb" => native_adapter(Some(Kind::Gdb), allow_install),
            "debugpy" | "python" => python_adapter(),
            "go" => go_adapter(),
            _ => continue,
        };
        let adapter = match adapter {
            Ok(a) => a,
            Err(e) => return Some(Err(e)),
        };
        let cwd = cfg["cwd"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or_else(|| root.to_path_buf());
        let program = cfg["program"].as_str().unwrap_or("").to_string();
        let args: Vec<String> = cfg["args"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a.as_str().map(|s| s.to_string()))
            .collect();
        let cargo = cfg["cargo"]["args"].as_array().map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        });
        let mut largs = launch_args(adapter.kind, &program, &args, &cwd, cargo.is_some());
        if let (Some(dst), Some(src)) = (largs.as_object_mut(), cfg.as_object()) {
            for (k, v) in src {
                if !matches!(k.as_str(), "name" | "type" | "request" | "cargo" | "preLaunchTask") {
                    dst.insert(k.clone(), v.clone());
                }
            }
        }
        return Some(Ok(LaunchPlan {
            adapter,
            args: largs,
            cwd,
            cargo,
        }));
    }
    None
}

pub fn auto_plan(
    lang: &str,
    file: &Path,
    workspace: &Path,
    program: Option<&str>,
    args: &[String],
    allow_install: bool,
) -> Result<LaunchPlan, PlanErr> {
    let rust = lang == "Rust";
    match lang {
        "Python" => {
            let a = python_adapter()?;
            let cwd = find_root("Python", file, workspace);
            let prog = program
                .map(|p| p.to_string())
                .unwrap_or_else(|| file.display().to_string());
            Ok(LaunchPlan {
                args: launch_args(a.kind, &prog, args, &cwd, false),
                adapter: a,
                cwd,
                cargo: None,
            })
        }
        "Go" => {
            let a = go_adapter()?;
            let cwd = find_root("Go", file, workspace);
            let prog = program
                .map(|p| p.to_string())
                .unwrap_or_else(|| file.parent().unwrap_or(&cwd).display().to_string());
            Ok(LaunchPlan {
                args: launch_args(a.kind, &prog, args, &cwd, false),
                adapter: a,
                cwd,
                cargo: None,
            })
        }
        "Rust" | "C/C++" => {
            let prefer = if rust { Some(Kind::CodeLldb) } else { None };
            let a = native_adapter(prefer, allow_install)?;
            let cwd = find_root(lang, file, workspace);
            match program {
                Some(p) => Ok(LaunchPlan {
                    args: launch_args(a.kind, p, args, &cwd, rust),
                    adapter: a,
                    cwd,
                    cargo: None,
                }),
                None if rust => Ok(LaunchPlan {
                    args: launch_args(a.kind, "", args, &cwd, true),
                    adapter: a,
                    cwd,
                    cargo: Some(vec!["build".into()]),
                }),
                None => Err("set the program to debug: :debug <program> [args]".into()),
            }
        }
        _ => Err(format!("debugging is not supported for {}", lang).into()),
    }
}

pub enum BuildResult {
    Ok(String),
    Err(String),
}

pub fn cargo_build(cwd: PathBuf, mut args: Vec<String>, current: Option<PathBuf>) -> Receiver<BuildResult> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        if !args.iter().any(|a| a.starts_with("--message-format")) {
            args.push("--message-format=json-render-diagnostics".into());
        }
        let out = Command::new("cargo")
            .args(&args)
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .output();
        let res = match out {
            Err(e) => BuildResult::Err(format!("cargo: {}", e)),
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                let mut exes: Vec<(String, String)> = Vec::new();
                for line in stdout.lines() {
                    let Ok(v) = serde_json::from_str::<Value>(line) else {
                        continue;
                    };
                    if v["reason"] != "compiler-artifact" {
                        continue;
                    }
                    let is_bin = v["target"]["kind"]
                        .as_array()
                        .is_some_and(|k| k.iter().any(|x| x == "bin"));
                    if let (true, Some(exe)) = (is_bin, v["executable"].as_str()) {
                        exes.push((
                            exe.to_string(),
                            v["target"]["src_path"].as_str().unwrap_or("").to_string(),
                        ));
                    }
                }
                if !o.status.success() {
                    let err = String::from_utf8_lossy(&o.stderr);
                    let tail: Vec<&str> = err.lines().rev().take(40).collect();
                    BuildResult::Err(tail.into_iter().rev().collect::<Vec<_>>().join("\n"))
                } else if exes.is_empty() {
                    BuildResult::Err("cargo build produced no binary target".into())
                } else {
                    let cur = current.map(|c| c.display().to_string());
                    let pick = exes
                        .iter()
                        .find(|(_, src)| Some(src) == cur.as_ref())
                        .unwrap_or(&exes[0]);
                    BuildResult::Ok(pick.0.clone())
                }
            }
        };
        let _ = tx.send(res);
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonc() {
        let s = "{\n // c\n \"a\": \"x//y\", /* b */ \"b\": [1,2,],\n}";
        let v: Value = serde_json::from_str(&strip_jsonc(s)).unwrap();
        assert_eq!(v["a"], "x//y");
        assert_eq!(v["b"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn subst() {
        let v = substitute(
            &json!({ "p": "${workspaceFolder}/a" }),
            &[("workspaceFolder", "/w".into())],
        );
        assert_eq!(v["p"], "/w/a");
    }
}
