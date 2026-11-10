use super::*;
use crate::flow::{self, BoxKind, Canvas, FnInfo, Graph, GridItem, Link, Role, BRAILLE_BITS, NONE};
use crate::render::{put_clip, text_w, BAR, BRIGHT, DIM, ERR, LOGO_GREEN, MACRO, POP, SEL, TYPE};
use crate::screen::{Screen, Style};
use crossterm::style::Color;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{channel, Receiver};
use std::time::Instant;

type AiBatch = Result<Vec<(u64, String)>, String>;

const ZOOMS: [f32; 4] = [1.0, 0.5, 0.25, 0.125];
const NOTE: Color = Color::Rgb {
    r: 0xb4,
    g: 0x9c,
    b: 0xe0,
};
const AI_BATCH_FNS: usize = 24;
const AI_BATCH_CHARS: usize = 24_000;
const AI_FN_CHARS: usize = 4_000;
const AI_CACHE_MAX: usize = 4_000;
const EXEC_BG: Color = Color::Rgb {
    r: 0x2e,
    g: 0x3b,
    b: 0x26,
};
const BP_FG: Color = Color::Rgb {
    r: 0xe5,
    g: 0x48,
    b: 0x4d,
};
const FUNC_FG: Color = Color::Rgb {
    r: 0x61,
    g: 0xaf,
    b: 0xef,
};
const EDGE: Color = Color::Rgb {
    r: 0x4b,
    g: 0x52,
    b: 0x63,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Program,
    File,
    Func,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FlowHit {
    Close,
    ZoomIn,
    ZoomOut,
    Fit,
    Ai,
    Crumb(Level),
}

#[derive(PartialEq, Clone)]
struct GraphKey {
    level: Level,
    path: Option<PathBuf>,
    func: Option<usize>,
    rev: u64,
    width: i32,
}

pub struct FlowView {
    pub visible: bool,
    pub width: u16,
    pub level: Level,
    pub path: Option<PathBuf>,
    pub fns: Vec<FnInfo>,
    parsed: Option<(PathBuf, u64, u64)>,
    rev: u64,
    pub func: Option<usize>,
    follow: Option<(PathBuf, String, usize)>,
    exec_seen: Option<(PathBuf, usize)>,
    graph: Graph,
    key: Option<GraphKey>,
    canvas: Option<Canvas>,
    canvas_line: Option<Option<usize>>,
    pub z: f32,
    pub px: f32,
    pub py: f32,
    pub sel: Option<usize>,
    pub hover: Option<usize>,
    active: Option<usize>,
    active_line: Option<usize>,
    exec: bool,
    recenter: bool,
    program: Vec<(PathBuf, usize)>,
    program_root: Option<PathBuf>,
    pan: Option<(u16, u16, f32, f32)>,
    hits: RefCell<Vec<(u16, u16, u16, FlowHit)>>,
    pub ai_on: bool,
    notes: HashMap<u64, String>,
    notes_loaded: bool,
    ai_job: Option<Receiver<AiBatch>>,
    ai_err: Option<(String, Instant)>,
    ai_wait: Option<(u64, Instant)>,
    caption: Vec<(String, Style)>,
}

impl FlowView {
    pub fn new() -> Self {
        let width = crate::config::load()
            .get("flow_width")
            .and_then(|w| w.parse().ok())
            .unwrap_or(48);
        FlowView {
            visible: false,
            width,
            level: Level::Func,
            path: None,
            fns: Vec::new(),
            parsed: None,
            rev: 0,
            func: None,
            follow: None,
            exec_seen: None,
            graph: Graph::default(),
            key: None,
            canvas: None,
            canvas_line: None,
            z: 1.0,
            px: 0.0,
            py: 0.0,
            sel: None,
            hover: None,
            active: None,
            active_line: None,
            exec: false,
            recenter: true,
            program: Vec::new(),
            program_root: None,
            pan: None,
            hits: RefCell::new(Vec::new()),
            ai_on: crate::config::load().get("flow_ai").is_some_and(|v| v == "on"),
            notes: HashMap::new(),
            notes_loaded: false,
            ai_job: None,
            ai_err: None,
            ai_wait: None,
            caption: Vec::new(),
        }
    }

    pub fn save_width(&self) {
        let _ = crate::config::set("flow_width", &self.width.to_string());
    }

    fn view(&self, l: &Layout) -> (u16, u16, u16, u16) {
        let cap = (self.caption.len() as u16).min(l.text_h.saturating_sub(6));
        (
            l.flow_x + 1,
            l.top + 1 + cap,
            l.flow_w,
            l.text_h.saturating_sub(2 + cap),
        )
    }

    fn to_layout(&self, sx: f32, sy: f32) -> (f32, f32) {
        (self.px + sx / self.z, self.py + sy / self.z)
    }

    fn zoom_index(&self) -> usize {
        ZOOMS.iter().position(|z| (*z - self.z).abs() < 1e-3).unwrap_or(0)
    }

    fn set_zoom(&mut self, z: f32, ax: f32, ay: f32) {
        let (lx, ly) = self.to_layout(ax, ay);
        self.z = z;
        self.px = lx - ax / z;
        self.py = ly - ay / z;
        self.canvas_line = None;
    }

    fn fit(&mut self, vw: u16, vh: u16) {
        let (gw, gh) = (self.graph.w.max(1) as f32, self.graph.h.max(1) as f32);
        self.z = *ZOOMS
            .iter()
            .find(|z| gw * **z <= vw as f32 && gh * **z <= vh as f32)
            .unwrap_or(&ZOOMS[ZOOMS.len() - 1]);
        let (lw, lh) = (vw as f32 / self.z, vh as f32 / self.z);
        self.px = ((gw - lw) / 2.0).min(0.0);
        self.py = if gh <= lh { 0.0 } else { 0.0f32.min((gh - lh) / 2.0) };
    }

    fn reveal(&mut self, i: usize, vw: u16, vh: u16, force: bool) {
        let Some(b) = self.graph.boxes.get(i) else { return };
        let (lw, lh) = (vw as f32 / self.z, vh as f32 / self.z);
        let (bx, by, bw, bh) = (b.x as f32, b.y as f32, b.w as f32, b.h as f32);
        let inside = bx >= self.px && by >= self.py && bx + bw <= self.px + lw && by + bh <= self.py + lh;
        if inside && !force {
            return;
        }
        if bw <= lw && (bx < self.px || bx + bw > self.px + lw || force) {
            self.px = bx + bw / 2.0 - lw / 2.0;
        }
        if by < self.py || by + bh > self.py + lh || force {
            self.py = by + bh / 2.0 - lh / 3.0;
        }
        self.clamp(vw, vh);
    }

    fn clamp(&mut self, vw: u16, vh: u16) {
        let (lw, lh) = (vw as f32 / self.z, vh as f32 / self.z);
        let gw = self.graph.w as f32;
        let gh = self.graph.h as f32;
        self.px = self.px.clamp(-lw / 2.0, (gw - lw / 2.0).max(0.0));
        self.py = self.py.clamp(-lh / 2.0, (gh - lh / 2.0).max(0.0));
    }
}

fn notes_path() -> Option<PathBuf> {
    Some(crate::config::path()?.parent()?.join("flow_ai.json"))
}

fn fn_hash(f: &FnInfo, en: bool) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    en.hash(&mut h);
    f.name.hash(&mut h);
    f.code.hash(&mut h);
    h.finish()
}

fn wrap(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cut = false;
    for word in text.split_whitespace() {
        let need = text_w(&cur) + usize::from(!cur.is_empty()) + text_w(word);
        if !cur.is_empty() && need > width {
            lines.push(std::mem::take(&mut cur));
            if lines.len() == max_lines {
                cut = true;
                break;
            }
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() && !cut {
        lines.push(cur);
    }
    if cut {
        if let Some(last) = lines.last_mut() {
            *last = flow::trunc(&format!("{} …", last), width as i32);
        }
    }
    lines
}

fn parse_notes(reply: &str, ids: &[(String, u64)]) -> Vec<(u64, String)> {
    let start = reply.find('{');
    let end = reply.rfind('}');
    let obj: Option<serde_json::Value> = match (start, end) {
        (Some(a), Some(b)) if b > a => serde_json::from_str(&reply[a..=b]).ok(),
        _ => None,
    };
    ids.iter()
        .map(|(id, h)| {
            let t = obj
                .as_ref()
                .and_then(|o| o[id.as_str()].as_str())
                .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
                .unwrap_or_default();
            (*h, t.trim_matches(|c| c == '"' || c == '`').to_string())
        })
        .collect()
}

fn source_files(root: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 8 || out.len() >= 400 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(root) else { return };
    let mut ents: Vec<_> = rd.flatten().collect();
    ents.sort_by_key(|e| e.file_name());
    for e in ents {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            if matches!(
                name.as_str(),
                "target" | "node_modules" | "build" | "dist" | "out" | "venv" | "__pycache__" | "vendor"
            ) {
                continue;
            }
            source_files(&p, out, depth + 1);
        } else if flow::lang_for(&p).is_some() {
            out.push(p);
            if out.len() >= 400 {
                return;
            }
        }
    }
}

fn kind_color(k: BoxKind) -> Color {
    match k {
        BoxKind::Start | BoxKind::End => LOGO_GREEN,
        BoxKind::Cond => TYPE,
        BoxKind::Loop => MACRO,
        BoxKind::Jump => ERR,
        BoxKind::Stmt => BRIGHT,
        BoxKind::Func => FUNC_FG,
        BoxKind::File => TYPE,
    }
}

impl Editor {
    pub(crate) fn flow_toggle(&mut self) {
        if self.flow.visible && self.focus == Focus::Flow {
            self.flow.visible = false;
            self.focus = Focus::Editor;
            return;
        }
        self.back_to_normal();
        if !self.flow.visible {
            self.flow.visible = true;
            self.flow.recenter = true;
            self.flow.follow = None;
            if self.flow.level == Level::Program {
                self.flow.level = Level::Func;
            }
        }
        self.focus = Focus::Flow;
    }

    fn flow_close(&mut self) {
        self.flow.visible = false;
        if self.flow.focus_is(self.focus) {
            self.focus = Focus::Editor;
        }
    }

    fn flow_source(&self, p: &Path) -> Option<(String, u64)> {
        let key = path_key(p);
        if let Some(b) = self
            .bufs
            .iter()
            .find(|b| b.path.as_ref().map(|x| path_key(x)).as_ref() == Some(&key))
        {
            let mut s = String::new();
            for (i, l) in b.lines.iter().enumerate() {
                if i > 0 {
                    s.push('\n');
                }
                s.extend(l.iter());
            }
            return Some((s, b.rev + 1));
        }
        std::fs::read_to_string(p).ok().map(|s| (s, 0))
    }

    fn flow_parse(&mut self, p: &Path) {
        let Some(lang) = flow::lang_for(p) else {
            self.flow.fns.clear();
            self.flow.parsed = Some((p.to_path_buf(), u64::MAX, 0));
            self.flow.path = Some(p.to_path_buf());
            return;
        };
        let Some((src, rev)) = self.flow_source(p) else { return };
        if let Some((pp, r, _)) = &self.flow.parsed {
            if pp == p && *r == rev && rev != 0 {
                return;
            }
            if pp == p && rev == 0 && *r == 0 && self.flow.path.as_deref() == Some(p) {
                return;
            }
        }
        let prev = self
            .flow
            .func
            .and_then(|i| self.flow.fns.get(i))
            .map(|f| (f.name.clone(), f.start));
        self.flow.fns = flow::parse(lang, &src);
        self.flow.rev += 1;
        self.flow.parsed = Some((p.to_path_buf(), rev, self.flow.rev));
        if self.flow.path.as_deref() != Some(p) {
            self.flow.path = Some(p.to_path_buf());
            self.flow.func = None;
        } else if let Some((name, start)) = prev {
            self.flow.func = self
                .flow
                .fns
                .iter()
                .enumerate()
                .filter(|(_, f)| f.name == name)
                .min_by_key(|(_, f)| f.start.abs_diff(start))
                .map(|(i, _)| i);
        }
    }

    fn flow_target(&self) -> Option<(PathBuf, usize, bool)> {
        if self.dbg.state == debug_ui::DbgState::Stopped {
            if let Some((p, l)) = &self.dbg.at {
                return Some((p.clone(), *l, true));
            }
        }
        let b = self.buf();
        b.path.as_ref().map(|p| (absolute(p), b.cy, false))
    }

    fn flow_program(&mut self) {
        let root = self.tree.root.clone();
        if self.flow.program_root.as_ref() == Some(&root) && !self.flow.program.is_empty() {
            return;
        }
        let mut files = Vec::new();
        source_files(&root, &mut files, 0);
        self.flow.program = files
            .into_iter()
            .map(|p| {
                let n = match (flow::lang_for(&p), self.flow_source(&p)) {
                    (Some(lang), Some((src, _))) if src.len() < 2_000_000 => flow::parse(lang, &src).len(),
                    _ => 0,
                };
                (p, n)
            })
            .collect();
        self.flow.program_root = Some(root);
        self.flow.rev += 1;
    }

    pub(crate) fn flow_sync(&mut self) {
        if !self.flow.visible {
            return;
        }
        let l = self.layout();
        let (_, _, vw, vh) = self.flow.view(&l);
        if vw < 4 || vh < 2 {
            return;
        }
        let target = self.flow_target();
        let mut exec_jump = false;
        if let Some((p, line, true)) = &target {
            let seen = Some((p.clone(), *line));
            if self.flow.exec_seen != seen {
                self.flow.exec_seen = seen;
                exec_jump = true;
            }
        } else {
            self.flow.exec_seen = None;
        }
        if exec_jump && self.flow.level != Level::Func {
            self.flow.level = Level::Func;
        }
        match self.flow.level {
            Level::Program => self.flow_program(),
            _ => {
                let want = match &target {
                    Some((p, _, _)) if flow::lang_for(p).is_some() || self.flow.path.is_none() => Some(p.clone()),
                    _ => self.flow.path.clone(),
                };
                if let Some(p) = want {
                    self.flow_parse(&p);
                }
            }
        }
        let tline = target
            .as_ref()
            .filter(|(p, _, _)| self.flow.level == Level::Program || self.flow.path.as_ref() == Some(p))
            .map(|(_, l, _)| *l);
        self.flow.exec = target.as_ref().is_some_and(|t| t.2);
        if self.flow.level != Level::Program {
            if let (Some(line), Some(p)) = (tline, self.flow.path.clone()) {
                let f = flow::innermost(&self.flow.fns, line);
                let key = f.map(|i| (p.clone(), self.flow.fns[i].name.clone(), self.flow.fns[i].start));
                if key.is_some() && (key != self.flow.follow || exec_jump) {
                    let changed = key != self.flow.follow;
                    self.flow.follow = key;
                    if self.flow.level == Level::Func && (changed || self.flow.func.is_none()) {
                        self.flow.func = f;
                        self.flow.recenter = true;
                    }
                }
            }
            if self.flow.level == Level::Func && self.flow.func.is_none() && !self.flow.fns.is_empty() {
                self.flow.func = Some(0);
            }
        }
        let cap = self.flow_caption(vw as usize);
        if cap.len() != self.flow.caption.len() && (self.flow.caption.is_empty() || cap.is_empty()) {
            self.flow.recenter = true;
        }
        self.flow.caption = cap;
        let (_, _, vw, vh) = self.flow.view(&self.layout());
        let key = GraphKey {
            level: self.flow.level,
            path: self.flow.path.clone(),
            func: self.flow.func,
            rev: self.flow.rev,
            width: if self.flow.level == Level::Func { 0 } else { vw as i32 },
        };
        if self.flow.key.as_ref() != Some(&key) {
            let fresh = self
                .flow
                .key
                .as_ref()
                .is_none_or(|k| k.level != key.level || k.func != key.func || k.path != key.path);
            self.flow.graph = self.flow_build(vw as i32);
            self.flow.key = Some(key);
            self.flow.canvas = None;
            self.flow.sel = None;
            self.flow.hover = None;
            self.flow.active = None;
            if fresh {
                self.flow.recenter = true;
            }
        }
        let active_line = if self.flow.level == Level::Program { None } else { tline };
        let active = match self.flow.level {
            Level::Program => self.flow.path.as_ref().and_then(|p| {
                self.flow
                    .graph
                    .boxes
                    .iter()
                    .position(|b| b.link == Link::File(p.clone()))
            }),
            _ => active_line.and_then(|l| self.flow.graph.find_line(l)),
        };
        if self.flow.canvas.is_none() || self.flow.canvas_line != Some(active_line) {
            self.flow.canvas = Some(flow::rasterize(&self.flow.graph, active_line));
            self.flow.canvas_line = Some(active_line);
        }
        self.flow.active_line = active_line;
        if std::mem::take(&mut self.flow.recenter) {
            self.flow.fit(vw, vh);
            if self.flow.z < 1.0 && self.flow.level == Level::Func {
                self.flow.z = 1.0;
                self.flow.px = 0.0;
                self.flow.py = 0.0;
            }
            if let Some(a) = active {
                self.flow.reveal(a, vw, vh, false);
            }
            self.flow.clamp(vw, vh);
        } else if active != self.flow.active || exec_jump {
            if let Some(a) = active {
                self.flow.reveal(a, vw, vh, false);
            }
        }
        self.flow.active = active;
    }

    pub(crate) fn flow_ai_toggle(&mut self) {
        self.flow.ai_on = !self.flow.ai_on;
        self.flow.ai_err = None;
        self.flow.ai_wait = None;
        let _ = crate::config::set("flow_ai", if self.flow.ai_on { "on" } else { "off" });
        self.flow.key = None;
        if self.flow.ai_on && self.ai.cfg().is_none() {
            self.info(t!(
                "AI açıklamaları için önce AI panelinden (Ctrl+L) bir model seçin",
                "Pick a model in the AI panel (Ctrl+L) to get explanations"
            ));
        }
    }

    fn flow_load_notes(&mut self) {
        if self.flow.notes_loaded {
            return;
        }
        self.flow.notes_loaded = true;
        let Some(p) = notes_path() else { return };
        let Ok(s) = std::fs::read_to_string(p) else { return };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) else {
            return;
        };
        if let Some(o) = v.as_object() {
            for (k, t) in o {
                if let (Ok(h), Some(t)) = (u64::from_str_radix(k, 16), t.as_str()) {
                    self.flow.notes.insert(h, t.to_string());
                }
            }
        }
    }

    fn flow_save_notes(&mut self) {
        if self.flow.notes.len() > AI_CACHE_MAX {
            let extra = self.flow.notes.len() - AI_CACHE_MAX;
            let drop: Vec<u64> = self.flow.notes.keys().take(extra).copied().collect();
            for k in drop {
                self.flow.notes.remove(&k);
            }
        }
        let Some(p) = notes_path() else { return };
        let map: serde_json::Map<String, serde_json::Value> = self
            .flow
            .notes
            .iter()
            .map(|(k, v)| (format!("{:016x}", k), serde_json::Value::String(v.clone())))
            .collect();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, serde_json::Value::Object(map).to_string());
    }

    fn flow_caption(&self, width: usize) -> Vec<(String, Style)> {
        if !self.flow.ai_on || self.flow.level != Level::Func || width < 12 {
            return Vec::new();
        }
        let Some(f) = self.flow.func.and_then(|i| self.flow.fns.get(i)) else {
            return Vec::new();
        };
        let w = width.saturating_sub(2);
        let note = self.flow.notes.get(&fn_hash(f, crate::i18n::is_en()));
        let (text, st) = match note {
            Some(n) if !n.is_empty() => (format!("✦ {}", n), Style::fg(NOTE).italic()),
            Some(_) => return Vec::new(),
            None => {
                if self.ai.cfg().is_none() {
                    (
                        t!(
                            "✦ AI açıklaması için Ctrl+L ile bir model seçin",
                            "✦ Pick a model with Ctrl+L to get explanations"
                        )
                        .to_string(),
                        Style::fg(DIM).italic(),
                    )
                } else if let Some((e, _)) = &self.flow.ai_err {
                    (format!("✦ {}", e), Style::fg(ERR))
                } else {
                    (
                        t!("✦ açıklanıyor…", "✦ explaining…").to_string(),
                        Style::fg(DIM).italic(),
                    )
                }
            }
        };
        wrap(&text, w, 3).into_iter().map(|l| (l, st)).collect()
    }

    pub fn flow_tick(&mut self) -> bool {
        if !self.flow.visible || !self.flow.ai_on {
            return false;
        }
        self.flow_load_notes();
        if let Some(rx) = &self.flow.ai_job {
            return match rx.try_recv() {
                Ok(Ok(v)) => {
                    self.flow.ai_job = None;
                    self.flow.ai_err = None;
                    for (h, t) in v {
                        self.flow.notes.insert(h, t);
                    }
                    self.flow_save_notes();
                    self.flow.key = None;
                    true
                }
                Ok(Err(e)) => {
                    self.flow.ai_job = None;
                    let e = flow::trunc(&e.replace('\n', " "), 160);
                    self.flow.ai_err = Some((tf!("AI hatası: {}", "AI error: {}", e), Instant::now()));
                    true
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.flow.ai_job = None;
                    true
                }
                Err(_) => false,
            };
        }
        if self.flow.level == Level::Program || self.flow.fns.is_empty() {
            return false;
        }
        if self
            .flow
            .ai_err
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed().as_secs() < 30)
        {
            return false;
        }
        let Some(cfg) = self.ai.cfg() else { return false };
        let en = crate::i18n::is_en();
        let missing: Vec<usize> = (0..self.flow.fns.len())
            .filter(|i| !self.flow.notes.contains_key(&fn_hash(&self.flow.fns[*i], en)))
            .collect();
        if missing.is_empty() {
            self.flow.ai_wait = None;
            return false;
        }
        let rev = self.flow.rev;
        match self.flow.ai_wait {
            Some((r, t)) if r == rev => {
                if t.elapsed().as_millis() < 1200 {
                    return false;
                }
            }
            _ => {
                self.flow.ai_wait = Some((rev, Instant::now()));
                return false;
            }
        }
        let cur = self.flow.func;
        let mut order = missing;
        order.sort_by_key(|i| (Some(*i) != cur, *i));
        let mut ids = Vec::new();
        let mut body = String::new();
        let file = self
            .flow
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        body.push_str(&format!("File: {}\n\n", file));
        for i in order {
            let f = &self.flow.fns[i];
            let code: String = f.code.chars().take(AI_FN_CHARS).collect();
            if !ids.is_empty() && (ids.len() >= AI_BATCH_FNS || body.len() + code.len() > AI_BATCH_CHARS) {
                break;
            }
            let id = format!("f{}", ids.len() + 1);
            body.push_str(&format!("### {} — {}\n```\n{}\n```\n\n", id, f.name, code));
            ids.push((id, fn_hash(f, en)));
        }
        let lang = if en { "English" } else { "Turkish" };
        let system = format!(
            "You explain functions for a code flowchart viewer. For each function write exactly one short, plain sentence \
             (at most 18 words) saying what it does and what it returns or changes. Do not repeat the function name, \
             do not use markdown or code. Write in {}. Reply with only a JSON object that maps each id (f1, f2, ...) \
             to its sentence.",
            lang
        );
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = crate::ai::complete(&cfg, &system, &body).map(|reply| parse_notes(&reply, &ids));
            let _ = tx.send(r);
        });
        self.flow.ai_job = Some(rx);
        true
    }

    fn flow_build(&self, vw: i32) -> Graph {
        match self.flow.level {
            Level::Func => match self.flow.func.and_then(|i| self.flow.fns.get(i)) {
                Some(f) => flow::function_graph(f),
                None => Graph::default(),
            },
            Level::File => {
                let items = self
                    .flow
                    .fns
                    .iter()
                    .enumerate()
                    .map(|(i, f)| GridItem {
                        label: format!("{}()", f.name),
                        sub: tf!(
                            "{} satır · {} dal",
                            "{} lines · {} branches",
                            f.end - f.start + 1,
                            f.branches
                        ),
                        note: if self.flow.ai_on {
                            self.flow
                                .notes
                                .get(&fn_hash(f, crate::i18n::is_en()))
                                .cloned()
                                .unwrap_or_default()
                        } else {
                            String::new()
                        },
                        lines: (f.start, f.end),
                        link: Link::Func(i),
                        kind: BoxKind::Func,
                    })
                    .collect();
                flow::grid(items, vw)
            }
            Level::Program => {
                let root = self.tree.root.clone();
                let items = self
                    .flow
                    .program
                    .iter()
                    .map(|(p, n)| GridItem {
                        label: p.strip_prefix(&root).unwrap_or(p).display().to_string(),
                        sub: tf!("{} fonksiyon", "{} functions", n),
                        note: String::new(),
                        lines: (usize::MAX, 0),
                        link: Link::File(p.clone()),
                        kind: BoxKind::File,
                    })
                    .collect();
                flow::grid(items, vw)
            }
        }
    }

    fn flow_set_level(&mut self, lv: Level) {
        if self.flow.level == lv {
            return;
        }
        if lv == Level::Func && self.flow.func.is_none() {
            let line = self.flow_target().map(|t| t.1).unwrap_or(0);
            self.flow.func =
                flow::innermost(&self.flow.fns, line).or(if self.flow.fns.is_empty() { None } else { Some(0) });
        }
        self.flow.level = lv;
        self.flow.recenter = true;
        self.flow.z = 1.0;
    }

    fn flow_zoom(&mut self, dir: i32, ax: f32, ay: f32) {
        let i = self.flow.zoom_index();
        if dir > 0 {
            match self.flow.level {
                Level::Program => {
                    let pick = self.flow.hover.or(self.flow.sel).or(self.flow.active);
                    if let Some(Link::File(p)) = pick.and_then(|i| self.flow.graph.boxes.get(i)).map(|b| b.link.clone())
                    {
                        self.flow_open_file(&p);
                    } else {
                        self.flow_set_level(Level::File);
                    }
                }
                Level::File => {
                    let pick = self.flow.hover.or(self.flow.sel).or(self.flow.active);
                    match pick.and_then(|i| self.flow.graph.boxes.get(i)).map(|b| b.link.clone()) {
                        Some(Link::Func(f)) => self.flow_open_func(f, false),
                        _ => self.flow_set_level(Level::Func),
                    }
                }
                Level::Func if i > 0 => self.flow.set_zoom(ZOOMS[i - 1], ax, ay),
                Level::Func => {}
            }
        } else {
            match self.flow.level {
                Level::Func if i + 1 < ZOOMS.len() && self.flow.graph.h as f32 * self.flow.z > 4.0 => {
                    self.flow.set_zoom(ZOOMS[i + 1], ax, ay)
                }
                Level::Func => self.flow_set_level(Level::File),
                Level::File => self.flow_set_level(Level::Program),
                Level::Program => {}
            }
        }
    }

    fn flow_open_file(&mut self, p: &Path) {
        self.flow.path = Some(p.to_path_buf());
        self.flow.parsed = None;
        self.flow.func = None;
        self.flow.follow = None;
        self.push_jump();
        self.open_path(p);
        self.focus = Focus::Flow;
        self.flow_set_level(Level::File);
    }

    fn flow_open_func(&mut self, i: usize, jump: bool) {
        let Some(f) = self.flow.fns.get(i) else { return };
        let start = f.start;
        self.flow.func = Some(i);
        self.flow.follow = self.flow.path.clone().map(|p| (p, f.name.clone(), f.start));
        self.flow.level = Level::Func;
        self.flow.recenter = true;
        self.flow.z = 1.0;
        if jump {
            self.flow_jump(start);
        }
    }

    fn flow_jump(&mut self, line: usize) {
        let Some(p) = self.flow.path.clone() else { return };
        self.push_jump();
        self.open_path(&p);
        let b = self.buf_mut();
        b.cy = line.min(b.last());
        b.cx = b.first_nb(b.cy);
        b.sync_want();
        self.focus = Focus::Flow;
    }

    fn flow_activate(&mut self, i: usize) {
        let Some(b) = self.flow.graph.boxes.get(i).cloned() else {
            return;
        };
        self.flow.sel = Some(i);
        match b.link {
            Link::File(p) => self.flow_open_file(&p),
            Link::Func(f) => self.flow_open_func(f, true),
            Link::None => {
                let line = match self.flow.active_line {
                    Some(l) if b.contains_line(l) => l,
                    _ => b.lines.0,
                };
                self.flow_jump(line);
            }
        }
    }

    fn flow_breakpoint(&mut self, i: usize) {
        let Some(b) = self.flow.graph.boxes.get(i).cloned() else {
            return;
        };
        if !matches!(b.link, Link::None) {
            return;
        }
        let line = b.items.first().map_or(b.lines.0, |it| it.line);
        let Some(p) = self.flow.path.clone() else { return };
        if self.buf().path.as_ref().map(|x| path_key(x)) != Some(path_key(&p)) {
            self.open_path(&p);
            self.focus = Focus::Flow;
        }
        self.flow.sel = Some(i);
        self.toggle_breakpoint(line);
    }

    pub(crate) fn flow_command(&mut self, arg: &str) {
        match arg.trim() {
            "" => self.flow_toggle(),
            "on" | "open" => {
                if !self.flow.visible {
                    self.flow_toggle();
                }
            }
            "off" | "close" => self.flow_close(),
            "fn" | "func" | "function" => {
                self.flow.visible = true;
                self.flow_set_level(Level::Func);
            }
            "file" => {
                self.flow.visible = true;
                self.flow_set_level(Level::File);
            }
            "program" | "project" | "all" => {
                self.flow.visible = true;
                self.flow.program.clear();
                self.flow_set_level(Level::Program);
            }
            "fit" => self.flow.recenter = true,
            other => self.error(tf!(
                "Bilinmeyen :flow argümanı: {}",
                "Unknown :flow argument: {}",
                other
            )),
        }
    }

    fn flow_view_size(&self) -> (u16, u16) {
        let l = self.layout();
        let v = self.flow.view(&l);
        (v.2, v.3)
    }

    pub(crate) fn flow_key(&mut self, k: KeyEvent) {
        let (vw, vh) = self.flow_view_size();
        let step = 4.0 / self.flow.z;
        let n = self.flow.graph.boxes.len();
        match k.code {
            KeyCode::Esc => self.focus = Focus::Editor,
            KeyCode::Char(':') => self.start_cmd(Mode::Command),
            KeyCode::Char('+') | KeyCode::Char('=') => self.flow_zoom(1, vw as f32 / 2.0, vh as f32 / 2.0),
            KeyCode::Char('-') | KeyCode::Char('_') => self.flow_zoom(-1, vw as f32 / 2.0, vh as f32 / 2.0),
            KeyCode::Char('0') | KeyCode::Char('f') => {
                self.flow.fit(vw, vh);
                self.flow.clamp(vw, vh);
            }
            KeyCode::Char('c') => {
                if let Some(a) = self.flow.active.or(self.flow.sel) {
                    self.flow.reveal(a, vw, vh, true);
                }
            }
            KeyCode::Left | KeyCode::Char('h') => self.flow.px -= step,
            KeyCode::Right | KeyCode::Char('l') => self.flow.px += step,
            KeyCode::Up | KeyCode::Char('k') => self.flow.py -= step / 2.0,
            KeyCode::Down | KeyCode::Char('j') => self.flow.py += step / 2.0,
            KeyCode::PageDown => self.flow.py += vh as f32 / self.flow.z,
            KeyCode::PageUp => self.flow.py -= vh as f32 / self.flow.z,
            KeyCode::Tab | KeyCode::Char('n') if n > 0 => {
                let i = self.flow.sel.or(self.flow.active).map_or(0, |i| (i + 1) % n);
                self.flow.sel = Some(i);
                self.flow.reveal(i, vw, vh, false);
            }
            KeyCode::BackTab | KeyCode::Char('N') if n > 0 => {
                let i = self.flow.sel.or(self.flow.active).map_or(n - 1, |i| (i + n - 1) % n);
                self.flow.sel = Some(i);
                self.flow.reveal(i, vw, vh, false);
            }
            KeyCode::Enter => {
                if let Some(i) = self.flow.sel.or(self.flow.active) {
                    self.flow_activate(i);
                }
            }
            KeyCode::Char('b') => {
                if let Some(i) = self.flow.sel.or(self.flow.active) {
                    self.flow_breakpoint(i);
                }
            }
            KeyCode::Backspace | KeyCode::Char('u') => match self.flow.level {
                Level::Func => self.flow_set_level(Level::File),
                Level::File => self.flow_set_level(Level::Program),
                Level::Program => {}
            },
            KeyCode::Char('q') => self.flow_close(),
            _ => {}
        }
        self.flow.clamp(vw, vh);
    }

    pub(crate) fn flow_mouse(&mut self, m: &MouseEvent, l: &Layout) -> bool {
        if l.flow_w == 0 && self.drag != Drag::FlowSep {
            return false;
        }
        let (x, y) = (m.column, m.row);
        let (vx, vy, vw, vh) = self.flow.view(l);
        let in_rows = y >= l.top && y < l.top + l.text_h;
        let over = in_rows && x >= l.flow_x && x < l.flow_x + 1 + l.flow_w;
        match m.kind {
            MouseEventKind::Drag(MouseButton::Left) if self.drag == Drag::FlowSep => {
                let right = l.flow_x + 1 + l.flow_w;
                let max = right.saturating_sub(l.ed_x + 30).max(24);
                self.flow.width = right.saturating_sub(x + 1).clamp(24.min(max), max);
                self.flow.key = None;
                return true;
            }
            MouseEventKind::Up(_) if self.drag == Drag::FlowSep => {
                self.drag = Drag::None;
                self.flow.save_width();
                return true;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.drag == Drag::FlowPan => {
                if let Some((x0, y0, px0, py0)) = self.flow.pan {
                    self.flow.px = px0 - (x as f32 - x0 as f32) / self.flow.z;
                    self.flow.py = py0 - (y as f32 - y0 as f32) / self.flow.z;
                    self.flow.clamp(vw, vh);
                }
                return true;
            }
            MouseEventKind::Up(_) if self.drag == Drag::FlowPan => {
                self.drag = Drag::None;
                self.flow.pan = None;
                return true;
            }
            _ => {}
        }
        if !over {
            if matches!(m.kind, MouseEventKind::Moved) {
                self.flow.hover = None;
            }
            return false;
        }
        let in_view = x >= vx && y >= vy && x < vx + vw && y < vy + vh;
        let sx = x.saturating_sub(vx) as f32 + 0.5;
        let sy = y.saturating_sub(vy) as f32 + 0.5;
        let boxhit = if in_view {
            let (lx, ly) = self.flow.to_layout(sx, sy);
            self.flow.graph.hit(lx.floor() as i32, ly.floor() as i32)
        } else {
            None
        };
        let zoom_mod = m.modifiers.contains(KeyModifiers::ALT) || m.modifiers.contains(KeyModifiers::CONTROL);
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if matches!(self.mode, Mode::Command | Mode::Search) {
                    self.mode = Mode::Normal;
                }
                if x == l.flow_x {
                    self.drag = Drag::FlowSep;
                    return true;
                }
                self.focus = Focus::Flow;
                let hit = self
                    .flow
                    .hits
                    .borrow()
                    .iter()
                    .rev()
                    .find(|h| h.0 == y && x >= h.1 && x < h.2)
                    .map(|h| h.3);
                if let Some(h) = hit {
                    match h {
                        FlowHit::Ai => self.flow_ai_toggle(),
                        FlowHit::Close => self.flow_close(),
                        FlowHit::ZoomIn => self.flow_zoom(1, vw as f32 / 2.0, vh as f32 / 2.0),
                        FlowHit::ZoomOut => self.flow_zoom(-1, vw as f32 / 2.0, vh as f32 / 2.0),
                        FlowHit::Fit => {
                            self.flow.fit(vw, vh);
                            self.flow.clamp(vw, vh);
                        }
                        FlowHit::Crumb(lv) => {
                            if lv == Level::Program {
                                self.flow.program.clear();
                            }
                            self.flow_set_level(lv);
                        }
                    }
                    return true;
                }
                if let Some(i) = boxhit {
                    self.flow_activate(i);
                } else if in_view {
                    self.drag = Drag::FlowPan;
                    self.flow.pan = Some((x, y, self.flow.px, self.flow.py));
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                self.focus = Focus::Flow;
                if let Some(i) = boxhit {
                    self.flow_breakpoint(i);
                }
            }
            MouseEventKind::Down(MouseButton::Middle) if in_view => {
                self.drag = Drag::FlowPan;
                self.flow.pan = Some((x, y, self.flow.px, self.flow.py));
            }
            MouseEventKind::Moved => {
                self.flow.hover = boxhit;
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if in_view => {
                let up = m.kind == MouseEventKind::ScrollUp;
                if zoom_mod {
                    self.flow.hover = boxhit;
                    self.flow_zoom(if up { 1 } else { -1 }, sx, sy);
                } else if m.modifiers.contains(KeyModifiers::SHIFT) {
                    self.flow.px += if up { -6.0 } else { 6.0 } / self.flow.z;
                } else {
                    self.flow.py += if up { -3.0 } else { 3.0 } / self.flow.z;
                }
                self.flow.clamp(vw, vh);
            }
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight if in_view => {
                let left = m.kind == MouseEventKind::ScrollLeft;
                self.flow.px += if left { -6.0 } else { 6.0 } / self.flow.z;
                self.flow.clamp(vw, vh);
            }
            _ => {}
        }
        true
    }

    fn flow_bp_lines(&self) -> Option<&std::collections::BTreeSet<usize>> {
        let p = self.flow.path.as_ref()?;
        self.dbg.bps.get(&path_key(p)).map(|e| &e.1)
    }

    fn flow_cell_style(&self, c: &crate::flow::Cell) -> Style {
        let f = &self.flow;
        let b = (c.node != NONE).then(|| &f.graph.boxes[c.node as usize]);
        let mut st = match (c.role, b) {
            (Role::Line, _) | (Role::Arrow, _) => Style::fg(EDGE),
            (Role::Label, _) => Style::fg(DIM).italic(),
            (Role::Border, Some(b)) => Style::fg(kind_color(b.kind)),
            (Role::Sub, _) => Style::fg(DIM),
            (Role::Note, _) => Style::fg(NOTE).italic(),
            (Role::Text, Some(b)) if b.kind == BoxKind::Stmt => Style::fg(BRIGHT),
            (Role::Text, Some(b)) => Style::fg(kind_color(b.kind)),
            _ => Style::new(),
        };
        if c.node != NONE {
            let n = Some(c.node as usize);
            if n == f.active {
                st = st.on(if f.exec { EXEC_BG } else { SEL });
                if c.role == Role::Text {
                    st = st.bold();
                }
            } else if n == f.sel || n == f.hover {
                st = st.on(POP);
            }
        }
        st
    }

    pub(crate) fn draw_flow(&self, s: &mut Screen, l: &Layout) {
        let mut hits = self.flow.hits.borrow_mut();
        hits.clear();
        if l.flow_w == 0 {
            return;
        }
        let focused = self.focus == Focus::Flow;
        let sep = Style::fg(if focused { LOGO_GREEN } else { DIM });
        for r in 0..l.text_h {
            s.put_char(l.flow_x, l.top + r, '│', sep);
        }
        let (vx, vy, vw, vh) = self.flow.view(l);
        let right = vx + vw;
        s.fill(vx, l.top, vw, Style::new().on(BAR));
        let title_st = Style::fg(if focused { LOGO_GREEN } else { BRIGHT }).on(BAR).bold();
        let mut x = s.put(vx + 1, l.top, "◇ FLOW", title_st) + 1;
        let crumbs: Vec<(String, Level)> = {
            let mut v = vec![(
                self.tree
                    .root
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "/".into()),
                Level::Program,
            )];
            if self.flow.level != Level::Program {
                if let Some(p) = &self.flow.path {
                    v.push((
                        p.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default(),
                        Level::File,
                    ));
                }
            }
            if self.flow.level == Level::Func {
                if let Some(f) = self.flow.func.and_then(|i| self.flow.fns.get(i)) {
                    v.push((format!("{}()", f.name), Level::Func));
                }
            }
            v
        };
        let tools_w: u16 = 17;
        let limit = right.saturating_sub(tools_w + 1);
        for (i, (name, lv)) in crumbs.iter().enumerate() {
            if x >= limit {
                break;
            }
            if i > 0 {
                x = s.put(x, l.top, " › ", Style::fg(DIM).on(BAR));
            }
            let cur = *lv == self.flow.level;
            let st = if cur {
                Style::fg(BRIGHT).on(BAR).bold()
            } else {
                Style::fg(DIM).on(BAR)
            };
            let room = limit.saturating_sub(x) as usize;
            let nx = put_clip(s, x, l.top, name, st, room);
            hits.push((l.top, x, nx, FlowHit::Crumb(*lv)));
            x = nx;
        }
        let mut tx = right.saturating_sub(tools_w);
        let ai_st = if self.flow.ai_job.is_some() {
            Style::fg(NOTE).on(BAR).italic()
        } else if self.flow.ai_on {
            Style::fg(NOTE).on(BAR).bold()
        } else {
            Style::fg(DIM).on(BAR)
        };
        s.put(tx + 1, l.top, if self.flow.ai_on { "✦AI" } else { "·AI" }, ai_st);
        hits.push((l.top, tx, tx + 4, FlowHit::Ai));
        tx += 4;
        for (lab, h) in [
            ("−", FlowHit::ZoomOut),
            ("+", FlowHit::ZoomIn),
            ("⤢", FlowHit::Fit),
            ("✕", FlowHit::Close),
        ] {
            s.put(tx + 1, l.top, lab, Style::fg(BRIGHT).on(BAR));
            hits.push((l.top, tx, tx + 3, h));
            tx += 3;
        }
        for (i, (line, st)) in self.flow.caption.iter().enumerate() {
            let y = l.top + 1 + i as u16;
            if y >= vy {
                break;
            }
            s.fill(vx, y, vw, Style::new().on(POP));
            put_clip(s, vx + 1, y, line, st.on(POP), vw.saturating_sub(2) as usize);
        }
        let fy = l.top + l.text_h.saturating_sub(1);
        s.fill(vx, fy, vw, Style::new().on(BAR));
        let Some(cv) = self.flow.canvas.as_ref() else { return };
        if self.flow.graph.boxes.is_empty() {
            let msg = match self.flow.level {
                Level::Func if self.flow.path.as_ref().and_then(|p| flow::lang_for(p)).is_none() => t!(
                    "Bu dosya türü için akış şeması yok (Rust, Python, Go, C/C++, JS/TS)",
                    "No flowchart for this file type (Rust, Python, Go, C/C++, JS/TS)"
                ),
                _ => t!("Burada fonksiyon yok", "No functions here"),
            };
            put_clip(s, vx + 2, vy + 1, msg, Style::fg(DIM), vw.saturating_sub(3) as usize);
            return;
        }
        if self.flow.z >= 1.0 {
            let ox = self.flow.px.round() as i32;
            let oy = self.flow.py.round() as i32;
            for sy in 0..vh {
                let mut sx = 0;
                while sx < vw {
                    let c = cv.get(ox + sx as i32, oy + sy as i32);
                    match c {
                        Some(c) if c.role != Role::Empty && c.ch != '\0' => {
                            let w = text_w(&c.ch.to_string()).max(1) as u16;
                            if sx + w <= vw {
                                s.put_char(vx + sx, vy + sy, c.ch, self.flow_cell_style(c));
                            }
                            sx += w;
                        }
                        _ => sx += 1,
                    }
                }
            }
        } else {
            self.draw_flow_braille(s, cv, vx, vy, vw, vh);
        }
        if let Some(bps) = self.flow_bp_lines() {
            if self.flow.level == Level::Func {
                for b in &self.flow.graph.boxes {
                    if b.kind == BoxKind::Start || b.kind == BoxKind::End {
                        continue;
                    }
                    let first = b.items.first().map_or(b.lines.0, |i| i.line);
                    let has = bps.range(b.lines.0..=b.lines.1).next().is_some()
                        && (b.items.len() <= 1 || bps.range(first..=b.lines.1).next().is_some());
                    if !has {
                        continue;
                    }
                    let lx = b.x as f32 + 0.5;
                    let ly = b.y as f32 + if self.flow.z >= 1.0 { 1.0 } else { b.h as f32 / 2.0 };
                    let sx = ((lx - self.flow.px) * self.flow.z).floor();
                    let sy = ((ly - self.flow.py) * self.flow.z).floor();
                    if sx >= 0.0 && sy >= 0.0 && (sx as u16) < vw && (sy as u16) < vh {
                        s.put_char(vx + sx as u16, vy + sy as u16, '●', Style::fg(BP_FG).bold());
                    }
                }
            }
        }
        let info_box = self.flow.hover.or(self.flow.sel).or(self.flow.active);
        let zoom = format!(" {}% ", (self.flow.z * 100.0).round() as i32);
        let zw = text_w(&zoom) as u16;
        let info = match info_box.and_then(|i| self.flow.graph.boxes.get(i)) {
            Some(b) => match &b.link {
                Link::File(_) => format!("{}  ·  {}", b.label, b.sub),
                Link::Func(_) if !b.note.is_empty() => format!("{}  {}", b.label, b.note),
                Link::Func(_) => format!("{}  ·  {}:{}", b.label, t!("satır", "line"), b.lines.0 + 1),
                Link::None => {
                    let act = self.flow.active_line.filter(|l| b.contains_line(*l));
                    let line = act.unwrap_or(b.items.first().map_or(b.lines.0, |i| i.line));
                    format!("{}  {}", line + 1, b.text_for(act))
                }
            },
            None => t!(
                "+/− yakınlaştır · sürükle: kaydır · tık: git · sağ tık: breakpoint",
                "+/− zoom · drag: pan · click: go to · right-click: breakpoint"
            )
            .to_string(),
        };
        put_clip(
            s,
            vx + 1,
            fy,
            &info,
            Style::fg(BRIGHT).on(BAR),
            vw.saturating_sub(zw + 2) as usize,
        );
        s.put(right.saturating_sub(zw), fy, &zoom, Style::fg(DIM).on(BAR));
    }

    fn draw_flow_braille(&self, s: &mut Screen, cv: &Canvas, vx: u16, vy: u16, vw: u16, vh: u16) {
        let z = self.flow.z;
        let f = &self.flow;
        let dxs = 1.0 / (2.0 * z);
        let dys = 1.0 / (4.0 * z);
        for sy in 0..vh {
            for sx in 0..vw {
                let mut bits: u16 = 0;
                let mut best: Option<(u8, Style)> = None;
                for di in 0..2 {
                    for dj in 0..4 {
                        let lx0 = f.px + (2 * sx as u32 + di) as f32 * dxs;
                        let ly0 = f.py + (4 * sy as u32 + dj) as f32 * dys;
                        let (x0, y0) = (lx0.floor() as i32, ly0.floor() as i32);
                        let (x1, y1) = (
                            ((lx0 + dxs).ceil() as i32).max(x0 + 1),
                            ((ly0 + dys).ceil() as i32).max(y0 + 1),
                        );
                        let mut on = false;
                        for yy in y0..y1 {
                            for xx in x0..x1 {
                                let Some(c) = cv.get(xx, yy) else { continue };
                                let node = (c.node != NONE).then_some(c.node as usize);
                                let (lit, pri, st) = match c.role {
                                    Role::Line | Role::Arrow => (true, 1, Style::fg(EDGE)),
                                    Role::Border => {
                                        let b = &f.graph.boxes[c.node as usize];
                                        (true, 2, Style::fg(kind_color(b.kind)))
                                    }
                                    Role::Text | Role::Sub
                                        if node.is_some() && (node == f.active || node == f.hover || node == f.sel) =>
                                    {
                                        (true, 3, Style::fg(BRIGHT))
                                    }
                                    _ => (false, 0, Style::new()),
                                };
                                let (pri, st) = if node.is_some() && node == f.active {
                                    (4, Style::fg(if f.exec { LOGO_GREEN } else { BRIGHT }).bold())
                                } else if node.is_some() && (node == f.hover || node == f.sel) {
                                    (3, Style::fg(BRIGHT))
                                } else {
                                    (pri, st)
                                };
                                if lit {
                                    on = true;
                                    if best.is_none_or(|b| b.0 < pri) {
                                        best = Some((pri, st));
                                    }
                                }
                            }
                        }
                        if on {
                            bits |= BRAILLE_BITS[di as usize][dj as usize];
                        }
                    }
                }
                if bits != 0 {
                    let ch = char::from_u32(0x2800 + bits as u32).unwrap_or(' ');
                    let st = best.map_or(Style::fg(EDGE), |b| b.1);
                    s.put_char(vx + sx, vy + sy, ch, st);
                }
            }
        }
    }
}

impl FlowView {
    fn focus_is(&self, f: Focus) -> bool {
        f == Focus::Flow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_from_fenced_reply() {
        let reply = "Sure:\n```json\n{\"f1\": \"Adds the numbers.\", \"f2\": \"  Picks   a label \"}\n```";
        let ids = vec![("f1".to_string(), 1u64), ("f2".to_string(), 2), ("f3".to_string(), 3)];
        let n = parse_notes(reply, &ids);
        assert_eq!(n[0], (1, "Adds the numbers.".to_string()));
        assert_eq!(n[1], (2, "Picks a label".to_string()));
        assert_eq!(n[2], (3, String::new()));
        assert!(parse_notes("no json", &ids).iter().all(|x| x.1.is_empty()));
    }

    #[test]
    fn caption_wrap() {
        let w = wrap("one two three four five six seven eight nine ten", 12, 2);
        assert_eq!(w.len(), 2);
        assert!(w.iter().all(|l| text_w(l) <= 12));
        assert!(w[1].ends_with('…'));
        assert_eq!(wrap("short", 20, 3), vec!["short".to_string()]);
    }
}
