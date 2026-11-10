use super::*;
use crate::dap::{
    auto_plan, cargo_build, from_launch_json, BuildResult, Client, DapMsg, EvalKind, LaunchPlan, PlanErr, Req,
};
use crate::render::{put_clip, text_w, BAR, BRIGHT, DIM, ERR, LOGO_GREEN, MACRO, SEL, STR, TYPE};
use crate::screen::{Screen, Style};
use crate::tools::{self, Progress, Tool};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::mpsc::Receiver;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DbgState {
    Idle,
    Installing,
    Building,
    Starting,
    Running,
    Stopped,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Vars,
    Stack,
    Console,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DbgHit {
    Continue,
    Pause,
    Next,
    StepIn,
    StepOut,
    Restart,
    Stop,
    Close,
    Var(usize),
    Frame(usize),
    Console,
    Resize,
}

pub struct Frame {
    pub id: i64,
    pub name: String,
    pub path: Option<PathBuf>,
    pub line: usize,
}

pub struct VarNode {
    pub name: String,
    pub value: String,
    pub var_ref: i64,
    pub depth: usize,
    pub expanded: bool,
}

pub struct Debugger {
    pub client: Option<Client>,
    pub state: DbgState,
    pub bps: BTreeMap<String, (PathBuf, BTreeSet<usize>)>,
    pub unverified: HashSet<(String, usize)>,
    pub thread: Option<i64>,
    pub frames: Vec<Frame>,
    pub frame_sel: usize,
    pub vars: Vec<VarNode>,
    pub var_sel: usize,
    pub var_scroll: usize,
    pub console: Vec<(String, u8)>,
    pub con_scroll: usize,
    pub input: Vec<char>,
    pub input_x: usize,
    pub watches: Vec<(String, String)>,
    pub stop_info: String,
    pub at: Option<(PathBuf, usize)>,
    pub visible: bool,
    pub height: u16,
    pub section: Section,
    plan: Option<LaunchPlan>,
    build: Option<Receiver<BuildResult>>,
    stopping: bool,
    restart: bool,
    pub program: Option<String>,
    pub args: Vec<String>,
    partial: bool,
    skip_entry: bool,
    bp_ids: HashSet<i64>,
    generation: u64,
    install: Option<(Tool, Receiver<Progress>, bool)>,
    pub progress: String,
    install_failed: HashSet<Tool>,
    keep_console: bool,
}

impl Debugger {
    pub fn new() -> Self {
        Debugger {
            client: None,
            state: DbgState::Idle,
            bps: BTreeMap::new(),
            unverified: HashSet::new(),
            thread: None,
            frames: Vec::new(),
            frame_sel: 0,
            vars: Vec::new(),
            var_sel: 0,
            var_scroll: 0,
            console: Vec::new(),
            con_scroll: 0,
            input: Vec::new(),
            input_x: 0,
            watches: Vec::new(),
            stop_info: String::new(),
            at: None,
            visible: false,
            height: 12,
            section: Section::Vars,
            plan: None,
            build: None,
            stopping: false,
            restart: false,
            program: None,
            args: Vec::new(),
            partial: false,
            skip_entry: false,
            bp_ids: HashSet::new(),
            generation: 0,
            install: None,
            progress: String::new(),
            install_failed: HashSet::new(),
            keep_console: false,
        }
    }

    pub fn active(&self) -> bool {
        self.state != DbgState::Idle
    }

    fn log(&mut self, s: &str, kind: u8) {
        let mut parts = s.split('\n').peekable();
        if self.partial && kind < 2 {
            if let (Some(first), Some(last)) = (parts.peek(), self.console.last_mut()) {
                if last.1 == kind {
                    last.0.push_str(first.trim_end_matches('\r'));
                    parts.next();
                }
            }
        }
        let parts: Vec<&str> = parts.collect();
        let n = parts.len();
        for (i, line) in parts.into_iter().enumerate() {
            if i + 1 == n && line.is_empty() {
                break;
            }
            self.console.push((line.trim_end_matches('\r').to_string(), kind));
        }
        self.partial = kind < 2 && !s.ends_with('\n');
        if self.console.len() > 5000 {
            let n = self.console.len() - 5000;
            self.console.drain(..n);
        }
        self.con_scroll = 0;
    }

    fn req(&mut self, command: &str, args: Value, r: Req) {
        if let Some(c) = self.client.as_mut() {
            c.request(command, args, r);
        }
    }

    pub fn has_bp(&self, path: &Path, line: usize) -> Option<bool> {
        let key = path_key(path);
        let set = &self.bps.get(&key)?.1;
        if set.contains(&line) {
            Some(!self.unverified.contains(&(key, line)))
        } else {
            None
        }
    }

    fn send_bps(&mut self, key: &str) {
        let Some((path, lines)) = self.bps.get(key).cloned() else {
            return;
        };
        let bps: Vec<Value> = lines.iter().map(|l| json!({ "line": l + 1 })).collect();
        let args = json!({
            "source": { "path": path.display().to_string(), "name": path.file_name().map(|n| n.to_string_lossy().into_owned()) },
            "breakpoints": bps,
            "lines": lines.iter().map(|l| l + 1).collect::<Vec<_>>(),
        });
        self.req("setBreakpoints", args, Req::SetBreakpoints(path));
    }
}

impl Editor {
    pub(crate) fn toggle_breakpoint(&mut self, line: usize) {
        let Some(path) = self.buf().path.clone() else {
            return self.error(t!(
                "Breakpoint için dosyayı kaydedin",
                "Save the file to set breakpoints"
            ));
        };
        let key = path_key(&path);
        let entry = self
            .dbg
            .bps
            .entry(key.clone())
            .or_insert_with(|| (path.clone(), BTreeSet::new()));
        if !entry.1.remove(&line) {
            entry.1.insert(line);
        }
        if entry.1.is_empty() {
            self.dbg.bps.remove(&key);
        }
        self.dbg.unverified.remove(&(key.clone(), line));
        if self.dbg.client.is_some() && matches!(self.dbg.state, DbgState::Running | DbgState::Stopped) {
            if !self.dbg.bps.contains_key(&key) {
                self.dbg.bps.insert(key.clone(), (path, BTreeSet::new()));
                self.dbg.send_bps(&key);
                self.dbg.bps.remove(&key);
            } else {
                self.dbg.send_bps(&key);
            }
        }
    }

    pub(crate) fn debug_start(&mut self) {
        match self.dbg.state {
            DbgState::Stopped => return self.debug_step("continue"),
            DbgState::Running | DbgState::Building | DbgState::Starting | DbgState::Installing => return,
            DbgState::Idle => {}
        }
        let Some(file) = self.buf().path.clone() else {
            return self.error(t!("Debug için bir dosya açın", "Open a file to debug"));
        };
        let root = self.tree.root.clone();
        let lang = self.buf().lang.name;
        let mut allow = true;
        let plan = loop {
            let r = match from_launch_json(&root, Some(&file), allow) {
                Some(r) => r,
                None => auto_plan(lang, &file, &root, self.dbg.program.as_deref(), &self.dbg.args, allow),
            };
            match r {
                Ok(p) => break p,
                Err(PlanErr::Missing(t)) if allow && t == Tool::CodeLldb && self.dbg.install_failed.contains(&t) => {
                    allow = false;
                }
                Err(PlanErr::Missing(t)) if !self.dbg.install_failed.contains(&t) => {
                    return self.start_install(t, true);
                }
                Err(PlanErr::Missing(t)) => {
                    return self.error(tf!(
                        "{} kurulamadı; :install {} ile tekrar deneyin",
                        "{} could not be installed; retry with :install {}",
                        t.name(),
                        t.name().to_lowercase()
                    ));
                }
                Err(PlanErr::Other(e)) => return self.error(e),
            }
        };
        if !std::mem::take(&mut self.dbg.keep_console) {
            self.dbg.console.clear();
        }
        self.dbg.visible = true;
        self.dbg.frames.clear();
        self.dbg.vars.clear();
        self.dbg.at = None;
        self.dbg.stopping = false;
        if let Some(cargo) = plan.cargo.clone() {
            self.dbg.log(&format!("$ cargo {}", cargo.join(" ")), 2);
            self.dbg.state = DbgState::Building;
            self.dbg.build = Some(cargo_build(plan.cwd.clone(), cargo, Some(file)));
            self.dbg.plan = Some(plan);
            self.info(t!("Derleniyor…", "Building…"));
        } else {
            self.launch_plan(plan);
        }
    }

    pub(crate) fn start_install(&mut self, t: Tool, then_start: bool) {
        if self.dbg.install.is_some() {
            return self.error(t!("Zaten bir kurulum sürüyor", "An installation is already running"));
        }
        self.dbg.console.clear();
        self.dbg.visible = true;
        self.dbg.state = DbgState::Installing;
        self.dbg.progress.clear();
        self.dbg.log(
            &tf!(
                "{} bulunamadı, indiriliyor (yalnızca bir kez)…",
                "{} not found, downloading (one time only)…",
                t.name()
            ),
            2,
        );
        self.dbg.install = Some((t, tools::install(t), then_start));
        self.info(tf!("{} kuruluyor…", "Installing {}…", t.name()));
    }

    fn install_tick(&mut self) -> bool {
        let Some((t, rx, then_start)) = &self.dbg.install else {
            return false;
        };
        let (t, then_start) = (*t, *then_start);
        let msgs: Vec<Progress> = rx.try_iter().collect();
        let changed = !msgs.is_empty();
        let mut done = None;
        for p in msgs {
            match p {
                Progress::Status(s) => {
                    self.dbg.progress = s.clone();
                    self.dbg.log(&s, 2);
                }
                Progress::Bytes(n, total) => {
                    let mb = |b: u64| b as f64 / 1_048_576.0;
                    self.dbg.progress = match total {
                        Some(tot) if tot > 0 => {
                            format!("{:.1} / {:.1} MB  {}%", mb(n), mb(tot), n * 100 / tot)
                        }
                        _ => format!("{:.1} MB", mb(n)),
                    };
                }
                Progress::Done(r) => {
                    done = Some(r);
                    break;
                }
            }
        }
        if let Some(r) = done {
            self.dbg.install = None;
            self.dbg.progress.clear();
            self.dbg.state = DbgState::Idle;
            match r {
                Ok(()) => {
                    self.dbg.install_failed.remove(&t);
                    self.dbg.log(&tf!("{} kuruldu", "{} installed", t.name()), 2);
                    self.info(tf!("{} kuruldu", "{} installed", t.name()));
                    if then_start {
                        self.dbg.keep_console = true;
                        self.debug_start();
                    }
                }
                Err(e) => {
                    self.dbg.install_failed.insert(t);
                    self.dbg
                        .log(&tf!("{} kurulamadı: {}", "{} install failed: {}", t.name(), e), 1);
                    if then_start {
                        self.dbg.keep_console = true;
                        self.debug_start();
                    } else {
                        self.error(tf!("{} kurulamadı: {}", "{} install failed: {}", t.name(), e));
                    }
                }
            }
        }
        changed
    }

    fn launch_plan(&mut self, plan: LaunchPlan) {
        self.dbg
            .log(&tf!("{} başlatılıyor…", "starting {}…", plan.adapter.name), 2);
        self.dbg.generation += 1;
        match Client::start(plan.adapter.clone(), &plan.cwd) {
            Ok(c) => {
                self.dbg.skip_entry = plan.adapter.kind == crate::dap::Kind::Gdb
                    && plan.args["stopAtBeginningOfMainSubprogram"].as_bool().unwrap_or(false);
                self.dbg.client = Some(c);
                self.dbg.state = DbgState::Starting;
                let args = json!({
                    "clientID": "zetsu",
                    "clientName": "zetsu",
                    "adapterID": plan.adapter.name,
                    "pathFormat": "path",
                    "linesStartAt1": true,
                    "columnsStartAt1": true,
                    "supportsVariableType": true,
                    "supportsVariablePaging": false,
                    "supportsRunInTerminalRequest": false,
                    "locale": crate::i18n::code(),
                });
                self.dbg.plan = Some(plan);
                self.dbg.req("initialize", args, Req::Initialize);
            }
            Err(e) => {
                self.dbg.state = DbgState::Idle;
                self.dbg.log(&e, 1);
                self.error(tf!(
                    "Debug adapter başlatılamadı: {}",
                    "Could not start debug adapter: {}",
                    e
                ));
            }
        }
    }

    fn end_session(&mut self, msg: &str) {
        self.dbg.generation += 1;
        if let Some(mut c) = self.dbg.client.take() {
            c.kill();
        }
        self.dbg.state = DbgState::Idle;
        self.dbg.thread = None;
        self.dbg.frames.clear();
        self.dbg.vars.clear();
        self.dbg.at = None;
        self.dbg.stopping = false;
        self.dbg.unverified.clear();
        self.dbg.bp_ids.clear();
        self.dbg.log(msg, 2);
        if std::mem::take(&mut self.dbg.restart) {
            self.debug_start();
        }
    }

    pub(crate) fn debug_stop(&mut self) {
        match self.dbg.state {
            DbgState::Idle => {}
            DbgState::Installing => {
                self.dbg.install = None;
                self.dbg.progress.clear();
                self.end_session(t!("Kurulum iptal edildi", "Installation cancelled"));
            }
            DbgState::Building => {
                self.dbg.build = None;
                self.dbg.progress.clear();
                self.end_session(t!("Derleme iptal edildi", "Build cancelled"));
            }
            _ if self.dbg.stopping || self.dbg.client.is_none() => {
                self.end_session(t!("Debug oturumu sonlandırıldı", "Debug session ended"));
            }
            _ => {
                self.dbg.stopping = true;
                self.dbg
                    .req("disconnect", json!({ "terminateDebuggee": true }), Req::Disconnect);
            }
        }
    }

    pub(crate) fn debug_restart(&mut self) {
        if self.dbg.active() {
            self.dbg.restart = true;
            self.debug_stop();
        } else {
            self.debug_start();
        }
    }

    pub(crate) fn debug_step(&mut self, cmd: &str) {
        if cmd == "pause" {
            if self.dbg.state == DbgState::Running {
                let t = self.dbg.thread.unwrap_or(0);
                self.dbg.req("pause", json!({ "threadId": t }), Req::Step);
            }
            return;
        }
        if self.dbg.state != DbgState::Stopped {
            if cmd == "continue" && self.dbg.state == DbgState::Idle {
                self.debug_start();
            }
            return;
        }
        let Some(t) = self.dbg.thread else { return };
        self.dbg.req(cmd, json!({ "threadId": t }), Req::Step);
        self.dbg.state = DbgState::Running;
        self.dbg.at = None;
    }

    fn select_frame(&mut self, i: usize) {
        let Some(f) = self.dbg.frames.get(i) else { return };
        self.dbg.frame_sel = i;
        let (id, path, line) = (f.id, f.path.clone(), f.line);
        if let Some(p) = path {
            if self
                .bufs
                .iter()
                .any(|b| b.path.as_ref().map(|x| path_key(x)) == Some(path_key(&p)))
                || p.exists()
            {
                self.open_path(&p);
                let b = self.buf_mut();
                b.cy = line.min(b.last());
                b.cx = b.first_nb(b.cy);
                b.sync_want();
            }
            if i == 0 {
                self.dbg.at = Some((p, line));
            }
        }
        self.dbg.req("scopes", json!({ "frameId": id }), Req::Scopes);
        for (wi, (expr, _)) in self.dbg.watches.clone().into_iter().enumerate() {
            self.dbg.req(
                "evaluate",
                json!({ "expression": expr, "frameId": id, "context": "watch" }),
                Req::Evaluate(EvalKind::Watch(wi)),
            );
        }
    }

    fn frame_id(&self) -> Option<i64> {
        self.dbg.frames.get(self.dbg.frame_sel).map(|f| f.id)
    }

    pub(crate) fn debug_eval(&mut self, expr: &str, kind: EvalKind) {
        if self.dbg.client.is_none() {
            return;
        }
        let ctx = match kind {
            EvalKind::Repl => "repl",
            EvalKind::Watch(_) => "watch",
            EvalKind::Hover => "hover",
        };
        let codelldb = self
            .dbg
            .client
            .as_ref()
            .is_some_and(|c| c.adapter.kind == crate::dap::Kind::CodeLldb);
        let expr = match (codelldb, &kind) {
            (true, EvalKind::Repl) => match expr.strip_prefix('`') {
                Some(cmd) => cmd.to_string(),
                None => format!("?{}", expr),
            },
            _ => expr.to_string(),
        };
        let mut args = json!({ "expression": expr, "context": ctx });
        if let Some(f) = self.frame_id() {
            args["frameId"] = json!(f);
        }
        self.dbg.req("evaluate", args, Req::Evaluate(kind));
    }

    pub(crate) fn debug_hover(&mut self) -> bool {
        if self.dbg.state != DbgState::Stopped {
            return false;
        }
        let w = self.word_under_cursor();
        if w.is_empty() {
            return false;
        }
        self.debug_eval(&w, EvalKind::Hover);
        true
    }

    pub(crate) fn add_watch(&mut self, expr: &str) {
        self.dbg.watches.push((expr.to_string(), String::new()));
        self.dbg.visible = true;
        if self.dbg.state == DbgState::Stopped {
            let i = self.dbg.watches.len() - 1;
            self.debug_eval(expr, EvalKind::Watch(i));
        }
    }

    fn toggle_var(&mut self, i: usize) {
        let Some(n) = self.dbg.vars.get(i) else { return };
        if n.var_ref <= 0 {
            return;
        }
        let (depth, var_ref, expanded) = (n.depth, n.var_ref, n.expanded);
        if expanded {
            let mut j = i + 1;
            while j < self.dbg.vars.len() && self.dbg.vars[j].depth > depth {
                j += 1;
            }
            self.dbg.vars.drain(i + 1..j);
            self.dbg.vars[i].expanded = false;
        } else {
            self.dbg.vars[i].expanded = true;
            self.dbg.req(
                "variables",
                json!({ "variablesReference": var_ref }),
                Req::Variables(var_ref),
            );
        }
    }

    pub fn debug_tick(&mut self) -> bool {
        let mut changed = self.install_tick();
        let results: Vec<BuildResult> = self
            .dbg
            .build
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for res in results {
            changed = true;
            if !matches!(res, BuildResult::Line(_)) {
                self.dbg.build = None;
                self.dbg.progress.clear();
            }
            match res {
                BuildResult::Line(l) => {
                    if l.starts_with("Blocking") {
                        self.dbg.log(
                            t!(
                                "cargo, target klasörünün kilidini bekliyor (rust-analyzer çalışıyor olabilir)…",
                                "cargo is waiting for the target directory lock (rust-analyzer may be running)…"
                            ),
                            2,
                        );
                    } else if !l.starts_with("Compiling") {
                        self.dbg.log(&l, 2);
                    }
                    self.dbg.progress = l;
                }
                BuildResult::Ok(exe) => {
                    self.dbg.log(&tf!("program: {}", "program: {}", exe), 2);
                    if let Some(mut plan) = self.dbg.plan.take() {
                        plan.args["program"] = json!(exe);
                        self.launch_plan(plan);
                    }
                }
                BuildResult::Err(e) => {
                    self.dbg.log(&e, 1);
                    self.dbg.state = DbgState::Idle;
                    self.error(t!("Derleme başarısız (konsola bakın)", "Build failed (see console)"));
                }
            }
        }
        let msgs = match self.dbg.client.as_mut() {
            Some(c) => c.poll(),
            None => return changed,
        };
        let generation = self.dbg.generation;
        for m in msgs {
            changed = true;
            self.on_dap(m);
            if self.dbg.generation != generation {
                break;
            }
        }
        changed
    }

    fn on_dap(&mut self, m: DapMsg) {
        match m {
            DapMsg::Closed => {
                if self.dbg.active() {
                    self.end_session(t!("Debug oturumu sona erdi", "Debug session ended"));
                }
            }
            DapMsg::Request { seq, command } => {
                if let Some(c) = self.dbg.client.as_mut() {
                    c.respond(seq, &command, json!({}));
                }
            }
            DapMsg::Event { event, body } => self.on_dap_event(&event, body),
            DapMsg::Response {
                req,
                success,
                message,
                body,
            } => {
                if !success && !matches!(req, Req::Evaluate(_) | Req::Disconnect | Req::Other | Req::ConfigDone) {
                    let msg = if message.is_empty() {
                        format!("{:?}", req)
                    } else {
                        message.clone()
                    };
                    self.dbg.log(&format!("{}\n", msg), 1);
                    if matches!(req, Req::Launch | Req::Initialize) {
                        self.error(tf!("Debug başlatılamadı: {}", "Debug launch failed: {}", msg));
                        return self.end_session(t!("Oturum başlatılamadı", "Session failed to start"));
                    }
                }
                self.on_dap_response(req, success, message, body);
            }
        }
    }

    fn on_dap_event(&mut self, event: &str, body: Value) {
        match event {
            "initialized" => {
                let keys: Vec<String> = self.dbg.bps.keys().cloned().collect();
                for k in keys {
                    self.dbg.send_bps(&k);
                }
                self.dbg
                    .req("setExceptionBreakpoints", json!({ "filters": [] }), Req::Other);
                let caps = self.dbg.client.as_ref().map(|c| c.caps.clone()).unwrap_or(Value::Null);
                if caps["supportsConfigurationDoneRequest"].as_bool().unwrap_or(true) {
                    self.dbg.req("configurationDone", json!({}), Req::ConfigDone);
                }
                self.dbg.state = DbgState::Running;
            }
            "stopped" => {
                if let Some(t) = body["threadId"].as_i64() {
                    self.dbg.thread = Some(t);
                }
                let user_hit = body["hitBreakpointIds"].as_array().is_some_and(|a| {
                    a.iter()
                        .any(|id| id.as_i64().is_some_and(|i| self.dbg.bp_ids.contains(&i)))
                });
                if std::mem::take(&mut self.dbg.skip_entry) && !user_hit {
                    let t = self.dbg.thread.unwrap_or(1);
                    self.dbg.req("continue", json!({ "threadId": t }), Req::Step);
                    return;
                }
                self.dbg.state = DbgState::Stopped;
                let reason = body["reason"].as_str().unwrap_or("");
                let reason = match reason {
                    "breakpoint" | "function breakpoint" => t!("breakpoint", "breakpoint"),
                    "step" => t!("adım", "step"),
                    "pause" => t!("duraklatıldı", "paused"),
                    "exception" => t!("istisna", "exception"),
                    "entry" => t!("giriş", "entry"),
                    other => other,
                }
                .to_string();
                let detail = body["text"]
                    .as_str()
                    .or_else(|| body["description"].as_str())
                    .unwrap_or("");
                self.dbg.stop_info = if detail.is_empty() || detail == reason {
                    reason
                } else {
                    format!("{}: {}", reason, detail)
                };
                if body["reason"] == "exception" {
                    let d = self.dbg.stop_info.clone();
                    self.dbg.log(&d, 1);
                }
                self.dbg.visible = true;
                let t = self.dbg.thread.unwrap_or(0);
                if self.dbg.thread.is_none() {
                    self.dbg.req("threads", json!({}), Req::Other);
                }
                self.dbg.req(
                    "stackTrace",
                    json!({ "threadId": t, "startFrame": 0, "levels": 64 }),
                    Req::StackTrace,
                );
            }
            "continued" => {
                self.dbg.state = DbgState::Running;
                self.dbg.at = None;
            }
            "output" => {
                let cat = body["category"].as_str().unwrap_or("console");
                if cat == "telemetry" {
                    return;
                }
                let kind = match cat {
                    "stderr" => 1,
                    "stdout" => 0,
                    _ => 2,
                };
                if let Some(o) = body["output"].as_str() {
                    self.dbg.log(o, kind);
                }
            }
            "exited" => {
                let code = body["exitCode"].as_i64().unwrap_or(0);
                self.dbg
                    .log(&tf!("Program çıktı (kod {})", "Program exited (code {})", code), 2);
            }
            "terminated" => {
                if self.dbg.client.is_some() && !self.dbg.stopping {
                    self.dbg.stopping = true;
                    self.dbg
                        .req("disconnect", json!({ "terminateDebuggee": true }), Req::Disconnect);
                } else {
                    self.end_session(t!("Debug oturumu sona erdi", "Debug session ended"));
                }
            }
            "breakpoint" => {
                let bp = &body["breakpoint"];
                if let (Some(path), Some(line)) = (bp["source"]["path"].as_str(), bp["line"].as_u64()) {
                    let key = path_key(Path::new(path));
                    let l = line as usize - 1;
                    if bp["verified"].as_bool().unwrap_or(true) {
                        self.dbg.unverified.remove(&(key, l));
                    } else {
                        self.dbg.unverified.insert((key, l));
                    }
                }
            }
            _ => {}
        }
    }

    fn on_dap_response(&mut self, req: Req, success: bool, message: String, body: Value) {
        match req {
            Req::Initialize => {
                if let Some(c) = self.dbg.client.as_mut() {
                    c.caps = body;
                }
                let args = self.dbg.plan.as_ref().map(|p| p.args.clone()).unwrap_or(Value::Null);
                self.dbg.req("launch", args, Req::Launch);
            }
            Req::SetBreakpoints(path) => {
                let key = path_key(&path);
                let lines: Vec<usize> = self
                    .dbg
                    .bps
                    .get(&key)
                    .map(|e| e.1.iter().copied().collect())
                    .unwrap_or_default();
                for (i, bp) in body["breakpoints"].as_array().into_iter().flatten().enumerate() {
                    if let Some(id) = bp["id"].as_i64() {
                        self.dbg.bp_ids.insert(id);
                    }
                    let line = bp["line"]
                        .as_u64()
                        .map(|l| l as usize - 1)
                        .or_else(|| lines.get(i).copied());
                    if let Some(l) = line {
                        if bp["verified"].as_bool().unwrap_or(true) {
                            self.dbg.unverified.remove(&(key.clone(), l));
                        } else {
                            self.dbg.unverified.insert((key.clone(), l));
                        }
                    }
                }
            }
            Req::StackTrace => {
                self.dbg.frames = body["stackFrames"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|f| Frame {
                        id: f["id"].as_i64().unwrap_or(0),
                        name: f["name"].as_str().unwrap_or("?").to_string(),
                        path: f["source"]["path"].as_str().map(PathBuf::from),
                        line: (f["line"].as_u64().unwrap_or(1) as usize).saturating_sub(1),
                    })
                    .collect();
                let first = self
                    .dbg
                    .frames
                    .iter()
                    .position(|f| f.path.as_ref().is_some_and(|p| p.exists()))
                    .unwrap_or(0);
                if first < self.dbg.frames.len() {
                    if let Some(p) = self.dbg.frames[first].path.clone() {
                        self.dbg.at = Some((p, self.dbg.frames[first].line));
                    }
                    self.select_frame(first);
                    self.focus = Focus::Editor;
                    if self.mode != Mode::Normal {
                        self.back_to_normal();
                    }
                }
            }
            Req::Scopes => {
                self.dbg.vars.clear();
                self.dbg.var_sel = 0;
                self.dbg.var_scroll = 0;
                let scopes: Vec<Value> = body["scopes"].as_array().cloned().unwrap_or_default();
                let mut to_load = Vec::new();
                let mut opened = 0;
                for s in scopes.iter() {
                    let r = s["variablesReference"].as_i64().unwrap_or(0);
                    let expensive = s["expensive"].as_bool().unwrap_or(false);
                    let name = s["name"].as_str().unwrap_or("").to_lowercase();
                    let hint = s["presentationHint"].as_str().unwrap_or("");
                    let minor = hint == "registers" || name.contains("register") || name.contains("global");
                    let expand = !expensive && !minor && opened < 2;
                    if expand {
                        opened += 1;
                    }
                    self.dbg.vars.push(VarNode {
                        name: s["name"].as_str().unwrap_or("").to_string(),
                        value: String::new(),
                        var_ref: r,
                        depth: 0,
                        expanded: expand,
                    });
                    if expand && r > 0 {
                        to_load.push(r);
                    }
                }
                for r in to_load {
                    self.dbg
                        .req("variables", json!({ "variablesReference": r }), Req::Variables(r));
                }
            }
            Req::Variables(r) => {
                let Some(i) = self.dbg.vars.iter().position(|v| v.var_ref == r && v.expanded) else {
                    return;
                };
                let depth = self.dbg.vars[i].depth;
                let mut j = i + 1;
                while j < self.dbg.vars.len() && self.dbg.vars[j].depth > depth {
                    j += 1;
                }
                let kids: Vec<VarNode> = body["variables"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .take(500)
                    .map(|v| VarNode {
                        name: v["name"].as_str().unwrap_or("").to_string(),
                        value: v["value"].as_str().unwrap_or("").replace('\n', " "),
                        var_ref: v["variablesReference"].as_i64().unwrap_or(0),
                        depth: depth + 1,
                        expanded: false,
                    })
                    .collect();
                self.dbg.vars.splice(i + 1..j, kids);
            }
            Req::Evaluate(kind) => {
                let result = if success {
                    body["result"].as_str().unwrap_or("").to_string()
                } else {
                    format!("⚠ {}", message)
                };
                match kind {
                    EvalKind::Repl => self.dbg.log(&result, if success { 0 } else { 1 }),
                    EvalKind::Watch(i) => {
                        if let Some(w) = self.dbg.watches.get_mut(i) {
                            w.1 = result;
                        }
                    }
                    EvalKind::Hover => {
                        if success {
                            let w = self.word_under_cursor();
                            self.hover = Some(vec![format!("{} = {}", w, result)]);
                        }
                    }
                }
            }
            Req::Disconnect => self.end_session(t!("Debug oturumu sona erdi", "Debug session ended")),
            _ => {}
        }
    }

    pub(crate) fn install_command(&mut self, arg: &str) {
        let arg = arg.trim();
        if arg.is_empty() {
            let list: Vec<String> = Tool::ALL
                .iter()
                .map(|t| {
                    let mark = if tools::installed(*t) { "✓" } else { "·" };
                    format!("{} {}", mark, t.name())
                })
                .collect();
            return self.info(tf!(
                "Debug araçları: {}  (:install <ad>)",
                "Debug tools: {}  (:install <name>)",
                list.join("  ")
            ));
        }
        let Some(t) = Tool::parse(arg) else {
            return self.error(tf!("Bilinmeyen araç: {}", "Unknown tool: {}", arg));
        };
        if self.dbg.state != DbgState::Idle {
            return self.error(t!("Önce debug oturumunu durdurun", "Stop the debug session first"));
        }
        self.dbg.install_failed.remove(&t);
        self.start_install(t, false);
    }

    pub(crate) fn uninstall_command(&mut self, arg: &str) {
        let Some(t) = Tool::parse(arg.trim()) else {
            return self.error(t!(
                "Kullanım: :uninstall <codelldb|debugpy|delve>",
                "Usage: :uninstall <codelldb|debugpy|delve>"
            ));
        };
        if self.dbg.state != DbgState::Idle {
            return self.error(t!("Önce debug oturumunu durdurun", "Stop the debug session first"));
        }
        match tools::uninstall(t) {
            Ok(()) => self.info(tf!("{} kaldırıldı", "{} removed", t.name())),
            Err(e) => self.error(tf!("{} kaldırılamadı: {}", "Could not remove {}: {}", t.name(), e)),
        }
    }

    pub(crate) fn debug_command(&mut self, arg: &str) {
        let mut parts = arg.split_whitespace();
        match parts.next() {
            None | Some("start") | Some("run") => self.debug_start(),
            Some("stop") => self.debug_stop(),
            Some("restart") => self.debug_restart(),
            Some("panel") => self.dbg.visible = !self.dbg.visible,
            Some("args") => {
                self.dbg.args = parts.map(|s| s.to_string()).collect();
                let a = self.dbg.args.join(" ");
                self.info(tf!("Program argümanları: {}", "Program arguments: {}", a));
            }
            Some("program") => {
                self.dbg.program = parts.next().map(|p| absolute(Path::new(p)).display().to_string());
                let p = self.dbg.program.clone().unwrap_or_default();
                self.info(tf!("Program: {}", "Program: {}", p));
            }
            Some(p) => {
                self.dbg.program = Some(absolute(Path::new(p)).display().to_string());
                self.dbg.args = parts.map(|s| s.to_string()).collect();
                self.debug_start();
            }
        }
    }

    pub(crate) fn dbg_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT);
        let d = &mut self.dbg;
        if k.code == KeyCode::Esc {
            self.focus = Focus::Editor;
            return;
        }
        if k.code == KeyCode::Char(':') && d.section != Section::Console {
            return self.start_cmd(Mode::Command);
        }
        if matches!(k.code, KeyCode::Tab | KeyCode::BackTab) {
            d.section = match (d.section, k.code == KeyCode::BackTab) {
                (Section::Vars, false) | (Section::Console, true) => Section::Stack,
                (Section::Stack, false) | (Section::Vars, true) => Section::Console,
                _ => Section::Vars,
            };
            return;
        }
        match d.section {
            Section::Vars => match k.code {
                KeyCode::Down | KeyCode::Char('j') => d.var_sel = (d.var_sel + 1).min(d.vars.len().saturating_sub(1)),
                KeyCode::Up | KeyCode::Char('k') => d.var_sel = d.var_sel.saturating_sub(1),
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => {
                    let i = d.var_sel;
                    let collapsed = d.vars.get(i).is_some_and(|v| !v.expanded);
                    if k.code == KeyCode::Enter || collapsed {
                        self.toggle_var(i);
                    }
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    let i = d.var_sel;
                    if d.vars.get(i).is_some_and(|v| v.expanded) {
                        self.toggle_var(i);
                    } else if let Some(depth) = d.vars.get(i).map(|v| v.depth).filter(|x| *x > 0) {
                        if let Some(p) = (0..i).rev().find(|&j| d.vars[j].depth == depth - 1) {
                            d.var_sel = p;
                        }
                    }
                }
                _ => {}
            },
            Section::Stack => match k.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    let i = (d.frame_sel + 1).min(d.frames.len().saturating_sub(1));
                    self.select_frame(i);
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    let i = d.frame_sel.saturating_sub(1);
                    self.select_frame(i);
                }
                _ => {}
            },
            Section::Console => match k.code {
                KeyCode::Enter => {
                    let expr: String = d.input.iter().collect();
                    if !expr.trim().is_empty() {
                        d.input.clear();
                        d.input_x = 0;
                        d.log(&format!("› {}", expr), 3);
                        self.debug_eval(&expr, EvalKind::Repl);
                    }
                }
                KeyCode::Char('u') if ctrl => {
                    d.input.drain(..d.input_x);
                    d.input_x = 0;
                }
                KeyCode::Char(c) if !ctrl => {
                    d.input.insert(d.input_x, c);
                    d.input_x += 1;
                }
                KeyCode::Backspace => {
                    if d.input_x > 0 {
                        d.input_x -= 1;
                        d.input.remove(d.input_x);
                    }
                }
                KeyCode::Left => d.input_x = d.input_x.saturating_sub(1),
                KeyCode::Right => d.input_x = (d.input_x + 1).min(d.input.len()),
                KeyCode::Up | KeyCode::PageUp => d.con_scroll += if k.code == KeyCode::Up { 1 } else { 10 },
                KeyCode::Down | KeyCode::PageDown => {
                    d.con_scroll = d
                        .con_scroll
                        .saturating_sub(if k.code == KeyCode::Down { 1 } else { 10 })
                }
                _ => {}
            },
        }
    }

    pub(crate) fn dbg_click(&mut self, x: u16, y: u16, double: bool) -> bool {
        let hit = self
            .dbg_hits
            .borrow()
            .iter()
            .find(|h| h.0 == y && x >= h.1 && x < h.2)
            .map(|h| h.3);
        let Some(hit) = hit else { return false };
        match hit {
            DbgHit::Continue => self.debug_start(),
            DbgHit::Pause => self.debug_step("pause"),
            DbgHit::Next => self.debug_step("next"),
            DbgHit::StepIn => self.debug_step("stepIn"),
            DbgHit::StepOut => self.debug_step("stepOut"),
            DbgHit::Restart => self.debug_restart(),
            DbgHit::Stop => self.debug_stop(),
            DbgHit::Close => {
                self.dbg.visible = false;
                if self.focus == Focus::Debug {
                    self.focus = Focus::Editor;
                }
            }
            DbgHit::Var(i) => {
                self.focus = Focus::Debug;
                self.dbg.section = Section::Vars;
                let was = self.dbg.var_sel == i;
                self.dbg.var_sel = i;
                if was || double || self.dbg.vars.get(i).is_some_and(|v| v.var_ref > 0) {
                    self.toggle_var(i);
                }
            }
            DbgHit::Frame(i) => {
                self.dbg.section = Section::Stack;
                self.select_frame(i);
            }
            DbgHit::Console => {
                self.focus = Focus::Debug;
                self.dbg.section = Section::Console;
            }
            DbgHit::Resize => return false,
        }
        true
    }

    pub(crate) fn dbg_scroll(&mut self, x: u16, d: isize) {
        let l = self.layout();
        let vw = l.dbg_cols.0;
        let sw = l.dbg_cols.1;
        if x < vw {
            let max = self.dbg.vars.len().saturating_sub(1) as isize;
            self.dbg.var_scroll = (self.dbg.var_scroll as isize + d).clamp(0, max.max(0)) as usize;
        } else if x >= vw + sw {
            self.dbg.con_scroll = (self.dbg.con_scroll as isize - d).max(0) as usize;
        }
    }

    pub(crate) fn draw_debug(&self, s: &mut Screen, l: &Layout) {
        let mut hits = self.dbg_hits.borrow_mut();
        hits.clear();
        self.dbg_cursor.set(None);
        if l.dbg_h == 0 {
            return;
        }
        let d = &self.dbg;
        let y0 = l.dbg_y;
        let w = s.w;
        let focused = self.focus == Focus::Debug;
        s.fill(0, y0, w, Style::new().on(BAR));
        hits.push((y0, 0, w, DbgHit::Resize));
        let (label, color) = match d.state {
            DbgState::Idle => (t!("DEBUG", "DEBUG"), DIM),
            DbgState::Installing => (t!("KURULUYOR", "INSTALLING"), TYPE),
            DbgState::Building => (t!("DERLENİYOR", "BUILDING"), TYPE),
            DbgState::Starting => (t!("BAŞLIYOR", "STARTING"), TYPE),
            DbgState::Running => (t!("ÇALIŞIYOR", "RUNNING"), STR),
            DbgState::Stopped => (t!("DURDU", "PAUSED"), ERR),
        };
        let mut x = s.put(1, y0, &format!(" ● {} ", label), Style::fg(color).on(BAR).bold());
        if let Some(c) = &d.client {
            x = s.put(x, y0, &format!("{}  ", c.adapter.name), Style::fg(DIM).on(BAR));
        }
        if matches!(d.state, DbgState::Installing | DbgState::Building) && !d.progress.is_empty() {
            x = s.put(x, y0, &format!(" {} ", d.progress), Style::fg(BRIGHT).on(BAR));
        }
        if d.state == DbgState::Stopped {
            let loc =
                d.at.as_ref()
                    .map(|(p, line)| {
                        format!(
                            "{}:{}",
                            p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
                            line + 1
                        )
                    })
                    .unwrap_or_default();
            x = s.put(x, y0, &format!(" {}  {} ", d.stop_info, loc), Style::fg(BRIGHT).on(BAR));
        }
        let running = d.state == DbgState::Running;
        let buttons: Vec<(&str, DbgHit, bool)> = vec![
            (
                if running { "⏸ F6" } else { "▶ F5" },
                if running { DbgHit::Pause } else { DbgHit::Continue },
                true,
            ),
            ("⤼ F10", DbgHit::Next, d.state == DbgState::Stopped),
            ("↓ F11", DbgHit::StepIn, d.state == DbgState::Stopped),
            ("↑ ⇧F11", DbgHit::StepOut, d.state == DbgState::Stopped),
            ("⟲", DbgHit::Restart, d.active()),
            ("■ ⇧F5", DbgHit::Stop, d.active()),
            ("✕", DbgHit::Close, true),
        ];
        let total: u16 = buttons.iter().map(|b| text_w(b.0) as u16 + 2).sum();
        let mut bx = w.saturating_sub(total + 1).max(x + 1);
        for (lab, hit, enabled) in buttons {
            let st = if enabled {
                Style::fg(BRIGHT).on(BAR)
            } else {
                Style::fg(DIM).on(BAR)
            };
            let lw = text_w(lab) as u16;
            s.put(bx, y0, lab, st);
            if enabled {
                hits.push((y0, bx, bx + lw, hit));
            }
            bx += lw + 2;
        }

        let (vw, sw, cw) = l.dbg_cols;
        let body_y = y0 + 1;
        let rows = l.dbg_h.saturating_sub(2) as usize;
        let head = |s: &mut Screen, x: u16, wd: u16, t: &str, sec: Section| {
            let active = focused && d.section == sec;
            let st = if active {
                Style::fg(LOGO_GREEN).bold()
            } else {
                Style::fg(DIM).bold()
            };
            put_clip(s, x + 1, body_y, t, st, wd.saturating_sub(2) as usize);
        };
        head(s, 0, vw, t!("DEĞİŞKENLER", "VARIABLES"), Section::Vars);
        head(s, vw, sw, t!("ÇAĞRI YIĞINI", "CALL STACK"), Section::Stack);
        head(s, vw + sw, cw, t!("KONSOL", "CONSOLE"), Section::Console);
        for r in 0..=rows as u16 {
            s.put_char(vw.saturating_sub(1), body_y + r, '│', Style::fg(BAR));
            s.put_char(vw + sw - 1, body_y + r, '│', Style::fg(BAR));
        }

        let mut lines: Vec<(String, Style, Option<usize>)> = Vec::new();
        for (e, v) in &d.watches {
            let val = if v.is_empty() { "…".to_string() } else { v.clone() };
            lines.push((format!("◇ {} = {}", e, val), Style::fg(MACRO), None));
        }
        for (i, v) in d.vars.iter().enumerate() {
            let arrow = if v.var_ref > 0 {
                if v.expanded {
                    "▾ "
                } else {
                    "▸ "
                }
            } else {
                "  "
            };
            let text = if v.depth == 0 {
                format!("{}{}", arrow, v.name)
            } else if v.value.is_empty() {
                format!("{}{}{}", "  ".repeat(v.depth), arrow, v.name)
            } else {
                format!("{}{}{} = {}", "  ".repeat(v.depth), arrow, v.name, v.value)
            };
            let st = if v.depth == 0 {
                Style::fg(BRIGHT).bold()
            } else {
                Style::new()
            };
            lines.push((text, st, Some(i)));
        }
        if lines.is_empty() {
            let hint = if d.active() {
                t!(
                    "Program durduğunda değişkenler burada görünür",
                    "Variables appear when the program pauses"
                )
            } else {
                t!("F9 breakpoint · F5 başlat", "F9 breakpoint · F5 start")
            };
            lines.push((hint.to_string(), Style::fg(DIM), None));
        }
        let sel_line = lines.iter().position(|l| l.2 == Some(d.var_sel));
        let mut start = d.var_scroll.min(lines.len().saturating_sub(1));
        if let Some(sl) = sel_line {
            if sl < start {
                start = sl;
            } else if sl >= start + rows {
                start = sl + 1 - rows;
            }
        }
        for (r, (text, st, idx)) in lines.iter().skip(start).take(rows).enumerate() {
            let y = body_y + 1 + r as u16;
            let selected = idx.is_some() && *idx == Some(d.var_sel) && focused && d.section == Section::Vars;
            let st = if selected { st.on(SEL) } else { *st };
            if selected {
                s.fill(0, y, vw.saturating_sub(1), Style::new().on(SEL));
            }
            put_clip(s, 1, y, text, st, vw.saturating_sub(3) as usize);
            if let Some(i) = idx {
                hits.push((y, 0, vw.saturating_sub(1), DbgHit::Var(*i)));
            }
        }

        for (r, f) in d.frames.iter().take(rows).enumerate() {
            let y = body_y + 1 + r as u16;
            let file = f
                .path
                .as_ref()
                .map(|p| {
                    format!(
                        "{}:{}",
                        p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
                        f.line + 1
                    )
                })
                .unwrap_or_default();
            let sel = r == d.frame_sel;
            let st = if sel {
                Style::fg(BRIGHT).on(SEL)
            } else {
                Style::fg(if f.path.is_some() { BRIGHT } else { DIM })
            };
            if sel {
                s.fill(vw, y, sw.saturating_sub(1), Style::new().on(SEL));
            }
            let nx = put_clip(s, vw + 1, y, &f.name, st, sw.saturating_sub(3) as usize);
            let room = (vw + sw).saturating_sub(nx + 3) as usize;
            if room > 4 {
                put_clip(
                    s,
                    nx + 1,
                    y,
                    &file,
                    if sel { Style::fg(DIM).on(SEL) } else { Style::fg(DIM) },
                    room,
                );
            }
            hits.push((y, vw, vw + sw - 1, DbgHit::Frame(r)));
        }

        let cx0 = vw + sw;
        let con_rows = rows.saturating_sub(1);
        let inner = cw.saturating_sub(2) as usize;
        let mut wrapped: Vec<(String, u8)> = Vec::new();
        for (line, kind) in &d.console {
            let chars: Vec<char> = line.chars().collect();
            if chars.is_empty() {
                wrapped.push((String::new(), *kind));
                continue;
            }
            let mut cur = String::new();
            let mut cwid = 0;
            for c in chars {
                let cw2 = crate::buffer::char_width(c, cwid).max(1);
                if cwid + cw2 > inner {
                    wrapped.push((std::mem::take(&mut cur), *kind));
                    cwid = 0;
                }
                cur.push(if c == '\t' { ' ' } else { c });
                cwid += cw2;
            }
            wrapped.push((cur, *kind));
        }
        let scroll = d.con_scroll.min(wrapped.len().saturating_sub(con_rows));
        let start = wrapped.len().saturating_sub(con_rows + scroll);
        for (r, (text, kind)) in wrapped.iter().skip(start).take(con_rows).enumerate() {
            let st = match kind {
                1 => Style::fg(ERR),
                2 => Style::fg(DIM),
                3 => Style::fg(LOGO_GREEN),
                _ => Style::new(),
            };
            put_clip(s, cx0 + 1, body_y + 1 + r as u16, text, st, inner);
        }
        let iy = body_y + rows as u16;
        let in_focus = focused && d.section == Section::Console;
        s.put(
            cx0 + 1,
            iy,
            "›",
            Style::fg(if in_focus { LOGO_GREEN } else { DIM }).bold(),
        );
        let it: String = d.input.iter().collect();
        if it.is_empty() && !in_focus {
            put_clip(
                s,
                cx0 + 3,
                iy,
                t!("ifade yazın…", "evaluate expression…"),
                Style::fg(DIM),
                inner.saturating_sub(2),
            );
        } else {
            put_clip(s, cx0 + 3, iy, &it, Style::fg(BRIGHT), inner.saturating_sub(2));
        }
        for r in 0..con_rows as u16 + 1 {
            hits.push((body_y + 1 + r, cx0, w, DbgHit::Console));
        }
        if in_focus {
            let cxp: u16 = d.input[..d.input_x]
                .iter()
                .map(|c| crate::buffer::char_width(*c, 0) as u16)
                .sum();
            self.dbg_cursor.set(Some(((cx0 + 3 + cxp).min(w - 1), iy)));
        }
    }
}
