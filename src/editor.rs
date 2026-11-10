use crate::buffer::{cls, disp_col, idx_at_col, leading_ws, Buffer, Pos, TAB_W};
use crate::discord::Presence;
use crate::highlight;
use crate::lsp::{path_key, Lsp};
use crate::tree::Tree;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::cell::{Cell, RefCell};
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

#[path = "ai_ui.rs"]
mod ai_ui;
#[path = "debug_ui.rs"]
mod debug_ui;
#[path = "find.rs"]
mod find;
#[path = "flow_ui.rs"]
mod flow_ui;
pub use find::{FindBar, Matcher};
#[path = "ide.rs"]
mod ide;
pub use ide::{Menu, Popup, MENU_ROWS, POPUP_ROWS};

pub const NONAME: &str = "[isimsiz]";
const HELP_NAME: &str = "[yardım]";

pub fn help_text() -> &'static str {
    t!(include_str!("help_tr.txt"), include_str!("help_en.txt"))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
    Command,
    Search,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Tree,
    Ai,
    Find,
    Debug,
    Flow,
}

#[derive(Clone)]
pub enum Reg {
    Chars(Vec<char>),
    Lines(Vec<Vec<char>>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MKind {
    Excl,
    Incl,
    Line,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drag {
    None,
    Text,
    Sep,
    AiSep,
    DbgSep,
    FlowSep,
    FlowPan,
}

pub struct Layout {
    pub tree_w: u16,
    pub ed_x: u16,
    pub gutter: u16,
    pub text_x: u16,
    pub text_w: u16,
    pub top: u16,
    pub text_h: u16,
    pub ed_w: u16,
    pub ai_x: u16,
    pub ai_w: u16,
    pub flow_x: u16,
    pub flow_w: u16,
    pub ed_r: u16,
    pub dbg_y: u16,
    pub dbg_h: u16,
    pub dbg_cols: (u16, u16, u16),
}

pub struct Editor {
    pub bufs: Vec<Buffer>,
    pub cur: usize,
    pub tree: Tree,
    pub focus: Focus,
    pub mode: Mode,
    pub cmd: Vec<char>,
    pub cmd_x: usize,
    pub msg: Option<(String, bool)>,
    pub reg: Reg,
    pub search: Option<Matcher>,
    pub hl_search: bool,
    pub numbers: bool,
    pub relative: bool,
    pub w: u16,
    pub h: u16,
    pub quit: bool,
    pub burst: bool,
    pub anchor: Pos,
    pub tab_spans: Vec<(u16, u16, usize)>,
    op: Option<char>,
    prefix: Option<char>,
    count: Option<usize>,
    op_count: Option<usize>,
    history: Vec<String>,
    hist_i: usize,
    last_click: Option<(Instant, u16, u16)>,
    clicks: u8,
    drag: Drag,
    drag_anchor: Pos,
    pub lsp: Lsp,
    pub popup: Option<Popup>,
    pub popup_rect: Cell<Option<(u16, u16, u16, u16)>>,
    pub menu: Option<Menu>,
    pub menu_rect: Cell<Option<(u16, u16, u16, u16)>>,
    pub hover: Option<Vec<String>>,
    pub sig: Option<(String, Option<(usize, usize)>)>,
    comp_id: Option<i64>,
    menu_anchor: Option<(u16, u16)>,
    ctx_pos: Option<(u16, u16)>,
    jumps: Vec<(PathBuf, Pos)>,
    clip: Option<arboard::Clipboard>,
    clip_last: String,
    pub clip_on: bool,
    pub presence: Presence,
    pub recent: Vec<PathBuf>,
    pub ai: crate::ai::Ai,
    ai_pick_pending: bool,
    pub ai_hits: RefCell<Vec<(u16, u16, u16, ai_ui::AiHit)>>,
    pub ai_cursor: Cell<Option<(u16, u16)>>,
    pub find: Option<FindBar>,
    pub find_hits: RefCell<Vec<(u16, u16, u16, find::FindHit)>>,
    pub find_cursor: Cell<Option<(u16, u16)>>,
    find_cache: RefCell<find::MatchCache>,
    insert_cp_pending: bool,
    ws_sym_id: Option<i64>,
    pub dbg: debug_ui::Debugger,
    pub flow: flow_ui::FlowView,
    pub dbg_hits: RefCell<Vec<(u16, u16, u16, debug_ui::DbgHit)>>,
    pub dbg_cursor: Cell<Option<(u16, u16)>>,
    pub splash_hits: RefCell<Vec<(u16, usize)>>,
}

pub const SPLASH_OPEN: usize = 1000;
pub const SPLASH_HELP: usize = 1001;

fn is_ctrl(k: &KeyEvent) -> bool {
    k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT)
}

fn is_motion(c: char) -> bool {
    "hjklwbe0^$G{}%".contains(c)
}

pub fn absolute(p: &Path) -> PathBuf {
    let base = if p.is_absolute() {
        PathBuf::new()
    } else {
        env::current_dir().unwrap_or_default()
    };
    let mut out = base;
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn toggle_case(c: char) -> char {
    if c.is_uppercase() {
        c.to_lowercase().next().unwrap_or(c)
    } else {
        c.to_uppercase().next().unwrap_or(c)
    }
}

impl Editor {
    pub fn new(arg: Option<String>, w: u16, h: u16) -> Self {
        let mut bufs = Vec::new();
        let mut tree_visible = arg.is_some();
        let mut msg = None;
        let mut opened_dir = false;
        if let Some(a) = arg {
            let p = PathBuf::from(&a);
            if p.is_dir() {
                let _ = env::set_current_dir(&p);
                opened_dir = true;
            } else {
                let abs = absolute(&p);
                match Buffer::open(&abs) {
                    Ok(b) => {
                        if !abs.exists() {
                            msg = Some((tf!("\"{}\" [yeni dosya]", "\"{}\" [new file]", a), false));
                        }
                        bufs.push(b);
                    }
                    Err(e) => msg = Some((format!("{}: {}", a, e), true)),
                }
                tree_visible = false;
            }
        }
        if bufs.is_empty() {
            bufs.push(Buffer::scratch(NONAME));
        }
        let root = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut tree = Tree::new(root.clone());
        tree.visible = tree_visible;
        let recent: Vec<PathBuf> = crate::config::get_list("recent")
            .into_iter()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .collect();
        let mut ed = Editor {
            bufs,
            cur: 0,
            tree,
            focus: Focus::Editor,
            mode: Mode::Normal,
            cmd: Vec::new(),
            cmd_x: 0,
            msg,
            reg: Reg::Chars(Vec::new()),
            search: None,
            hl_search: false,
            numbers: true,
            relative: false,
            w,
            h,
            quit: false,
            burst: false,
            anchor: (0, 0),
            tab_spans: Vec::new(),
            op: None,
            prefix: None,
            count: None,
            op_count: None,
            history: Vec::new(),
            hist_i: 0,
            last_click: None,
            clicks: 0,
            drag: Drag::None,
            drag_anchor: (0, 0),
            lsp: Lsp::new(),
            popup: None,
            popup_rect: Cell::new(None),
            menu: None,
            menu_rect: Cell::new(None),
            hover: None,
            sig: None,
            comp_id: None,
            menu_anchor: None,
            ctx_pos: None,
            jumps: Vec::new(),
            clip: arboard::Clipboard::new().ok(),
            clip_last: String::new(),
            clip_on: true,
            presence: {
                let cfg = crate::config::load();
                let id = env::var("ZETSU_DISCORD_ID")
                    .ok()
                    .or_else(|| cfg.get("discord_client_id").cloned())
                    .unwrap_or_default();
                let on = cfg.get("discord").is_none_or(|v| v != "off");
                Presence::start(id, on)
            },
            recent,
            splash_hits: RefCell::new(Vec::new()),
            ai: crate::ai::Ai::new(),
            ai_pick_pending: false,
            ai_hits: RefCell::new(Vec::new()),
            ai_cursor: Cell::new(None),
            find: None,
            find_hits: RefCell::new(Vec::new()),
            find_cursor: Cell::new(None),
            find_cache: RefCell::new(None),
            insert_cp_pending: false,
            ws_sym_id: None,
            dbg: debug_ui::Debugger::new(),
            flow: flow_ui::FlowView::new(),
            dbg_hits: RefCell::new(Vec::new()),
            dbg_cursor: Cell::new(None),
        };
        if opened_dir {
            ed.push_recent(&root);
        }
        ed
    }

    pub(crate) fn push_recent(&mut self, p: &Path) {
        let key = path_key(p);
        self.recent.retain(|r| path_key(r) != key);
        self.recent.insert(0, p.to_path_buf());
        self.recent.truncate(9);
        let joined = self
            .recent
            .iter()
            .map(|r| r.display().to_string())
            .collect::<Vec<_>>()
            .join("|");
        let _ = crate::config::set("recent", &joined);
    }

    pub fn buf(&self) -> &Buffer {
        &self.bufs[self.cur]
    }

    pub fn buf_mut(&mut self) -> &mut Buffer {
        &mut self.bufs[self.cur]
    }

    fn info(&mut self, s: impl Into<String>) {
        self.msg = Some((s.into(), false));
    }

    fn error(&mut self, s: impl Into<String>) {
        self.msg = Some((s.into(), true));
    }

    pub fn layout(&self) -> Layout {
        let tree_w = if self.tree.visible {
            self.tree.width.min(self.w.saturating_sub(20))
        } else {
            0
        };
        let ed_x = if self.tree.visible { tree_w + 1 } else { 0 };
        let b = self.buf();
        let gutter = if self.numbers {
            (b.lines.len().to_string().len().max(3) + 3) as u16
        } else {
            2
        };
        let text_x = ed_x + gutter;
        let ai_w = if self.ai.visible {
            let max = self.w.saturating_sub(ed_x + 31).max(24);
            self.ai.width.clamp(24.min(max), max)
        } else {
            0
        };
        let ai_x = if ai_w > 0 {
            self.w.saturating_sub(ai_w + 1)
        } else {
            self.w
        };
        let (flow_x, flow_w) = if self.flow.visible && ai_x > ed_x + 30 + 25 {
            let room = ai_x.saturating_sub(ed_x + 30 + 1);
            let fw = self.flow.width.clamp(24.min(room), room);
            (ai_x - fw - 1, fw)
        } else {
            (ai_x, 0)
        };
        let ed_r = flow_x;
        let text_w = ed_r.saturating_sub(text_x).max(1);
        let total_h = self.h.saturating_sub(3).max(1);
        let dbg_h = if self.dbg.visible && total_h >= 14 {
            self.dbg.height.clamp(6, total_h - 6)
        } else {
            0
        };
        let text_h = total_h - dbg_h;
        let dbg_y = 1 + text_h;
        let vw = self.w * 40 / 100;
        let sw = self.w * 25 / 100;
        let dbg_cols = (vw, sw, self.w.saturating_sub(vw + sw));
        let ed_w = ed_r.saturating_sub(ed_x);
        Layout {
            tree_w,
            ed_x,
            gutter,
            text_x,
            text_w,
            top: 1,
            text_h,
            ed_w,
            ai_x,
            ai_w,
            flow_x,
            flow_w,
            ed_r,
            dbg_y,
            dbg_h,
            dbg_cols,
        }
    }

    pub fn is_splash(&self) -> bool {
        self.bufs.len() == 1 && self.is_pristine(0) && matches!(self.mode, Mode::Normal | Mode::Command)
    }

    fn is_pristine(&self, i: usize) -> bool {
        let b = &self.bufs[i];
        b.path.is_none() && !b.dirty && b.name == NONAME && b.lines.len() == 1 && b.lines[0].is_empty()
    }

    pub fn rel_path(&self, b: &Buffer) -> String {
        match &b.path {
            Some(p) => p.strip_prefix(&self.tree.root).unwrap_or(p).display().to_string(),
            None => Self::display_name(b),
        }
    }

    pub fn display_name(b: &Buffer) -> String {
        match b.name.as_str() {
            NONAME => t!("[isimsiz]", "[untitled]").to_string(),
            HELP_NAME => t!("[yardım]", "[help]").to_string(),
            n => n.to_string(),
        }
    }

    fn set_lang(&mut self, arg: &str) {
        let en = match arg.to_lowercase().as_str() {
            "en" | "english" | "ingilizce" => true,
            "tr" | "turkish" | "türkçe" | "turkce" => false,
            "" => {
                let cur = crate::i18n::code();
                return self.info(tf!("Dil: {}  (:lang en | tr)", "Language: {}  (:lang en | tr)", cur));
            }
            _ => return self.error(t!("Kullanım: :lang en | tr", "Usage: :lang en | tr")),
        };
        crate::i18n::set_en(en);
        let _ = crate::config::set("lang", crate::i18n::code());
        if let Some(i) = self.bufs.iter().position(|b| b.name == HELP_NAME) {
            let (cy, sy) = (self.bufs[i].cy, self.bufs[i].scroll_y);
            let mut nb = Buffer::from_text(HELP_NAME, help_text());
            nb.cy = cy.min(nb.last());
            nb.scroll_y = sy;
            self.bufs[i] = nb;
        }
        self.info(t!("Arayüz dili: Türkçe", "Interface language: English"));
    }

    pub fn pending_str(&self) -> String {
        let mut s = String::new();
        if let Some(n) = self.op_count {
            s += &n.to_string();
        }
        if let Some(o) = self.op {
            s.push(o);
        }
        if let Some(n) = self.count {
            s += &n.to_string();
        }
        if let Some(p) = self.prefix {
            s.push(p);
        }
        s
    }

    pub fn selection(&self) -> (Pos, Pos) {
        let b = self.buf();
        let c = (b.cx, b.cy);
        let a = self.anchor;
        if (a.1, a.0) <= (c.1, c.0) {
            (a, c)
        } else {
            (c, a)
        }
    }

    pub(crate) fn set_reg(&mut self, r: Reg) {
        if self.clip_on {
            let text = match &r {
                Reg::Chars(c) => c.iter().collect::<String>(),
                Reg::Lines(ls) => ls.iter().map(|l| l.iter().collect::<String>() + "\n").collect(),
            };
            if let Some(c) = self.clip.as_mut() {
                if c.set_text(text.clone()).is_ok() {
                    self.clip_last = text;
                }
            }
        }
        self.reg = r;
    }

    fn sync_from_clip(&mut self) {
        if !self.clip_on {
            return;
        }
        let Some(c) = self.clip.as_mut() else { return };
        let Ok(t) = c.get_text() else { return };
        let t = t.replace("\r\n", "\n");
        if t.is_empty() || t == self.clip_last.replace("\r\n", "\n") {
            return;
        }
        self.clip_last = t.clone();
        self.reg = if let Some(body) = t.strip_suffix('\n') {
            Reg::Lines(body.split('\n').map(|l| l.chars().collect()).collect())
        } else {
            Reg::Chars(t.chars().collect())
        };
    }

    fn clipboard_text(&mut self) -> Option<String> {
        self.clip.as_mut()?.get_text().ok()
    }

    fn indent_unit(&self) -> Vec<char> {
        if self.buf().use_tabs {
            vec!['\t']
        } else {
            vec![' '; TAB_W]
        }
    }

    pub fn handle(&mut self, ev: Event) {
        match ev {
            Event::Key(k) => {
                if k.kind == KeyEventKind::Release {
                    return;
                }
                self.on_key(k);
                self.after(3);
            }
            Event::Mouse(m) => {
                self.on_mouse(m);
                self.after(0);
            }
            Event::Paste(s) => {
                self.on_paste(&s);
                self.after(3);
            }
            Event::Resize(w, h) => {
                self.w = w;
                self.h = h;
                self.after(0);
            }
            _ => {}
        }
    }

    fn after(&mut self, so: usize) {
        if self.bufs.is_empty() {
            self.bufs.push(Buffer::scratch(NONAME));
        }
        self.cur = self.cur.min(self.bufs.len() - 1);
        let allow = matches!(self.mode, Mode::Insert | Mode::Visual | Mode::VisualLine);
        self.buf_mut().clamp(allow);
        self.scroll_to_cursor(so);
        let rows = self.tree_rows();
        let t = &mut self.tree;
        t.sel = t.sel.min(t.nodes.len().saturating_sub(1));
        t.scroll = t.scroll.min(t.nodes.len().saturating_sub(rows.max(1)));
    }

    fn scroll_to_cursor(&mut self, so: usize) {
        let l = self.layout();
        let h = l.text_h as usize;
        let w = l.text_w as usize;
        let so = so.min(h.saturating_sub(1) / 2);
        let b = self.buf_mut();
        if b.cy < b.scroll_y + so {
            b.scroll_y = b.cy.saturating_sub(so);
        } else if b.cy + so >= b.scroll_y + h {
            b.scroll_y = b.cy + so + 1 - h;
        }
        let dc = disp_col(&b.lines[b.cy], b.cx);
        if dc < b.scroll_x {
            b.scroll_x = dc;
        } else if dc >= b.scroll_x + w {
            b.scroll_x = dc + 1 - w;
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        if self.mode != Mode::Command {
            self.msg = None;
        }
        if self.hover.take().is_some() && k.code == KeyCode::Esc {
            return;
        }
        if self.menu.is_some() {
            return self.menu_key(k);
        }
        if is_ctrl(&k) && k.code == KeyCode::Char('l') {
            return self.ai_toggle();
        }
        if is_ctrl(&k) && k.code == KeyCode::Char('g') {
            return self.flow_toggle();
        }
        if let KeyCode::F(n) = k.code {
            let shift = k.modifiers.contains(KeyModifiers::SHIFT);
            let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
            match (n, shift, ctrl) {
                (5, true, true) => return self.debug_restart(),
                (5, true, false) => return self.debug_stop(),
                (5, false, _) => return self.debug_start(),
                (6, _, _) => return self.debug_step("pause"),
                (9, _, _) => {
                    let y = self.buf().cy;
                    return self.toggle_breakpoint(y);
                }
                (10, _, _) => return self.debug_step("next"),
                (11, true, _) => return self.debug_step("stepOut"),
                (11, false, _) => return self.debug_step("stepIn"),
                _ => {}
            }
        }
        if self.focus == Focus::Ai && !matches!(self.mode, Mode::Command | Mode::Search) {
            let global = is_ctrl(&k)
                && matches!(
                    k.code,
                    KeyCode::Char('w')
                        | KeyCode::Char('b')
                        | KeyCode::Char('k')
                        | KeyCode::Char('s')
                        | KeyCode::Char('f')
                );
            if !global {
                return self.ai_key(k);
            }
        }
        if self.focus == Focus::Debug && !matches!(self.mode, Mode::Command | Mode::Search) {
            let global = is_ctrl(&k)
                && matches!(
                    k.code,
                    KeyCode::Char('w')
                        | KeyCode::Char('b')
                        | KeyCode::Char('k')
                        | KeyCode::Char('s')
                        | KeyCode::Char('f')
                );
            if !global {
                return self.dbg_key(k);
            }
        }
        if self.focus == Focus::Flow && !matches!(self.mode, Mode::Command | Mode::Search) {
            let global = is_ctrl(&k)
                && matches!(
                    k.code,
                    KeyCode::Char('w')
                        | KeyCode::Char('b')
                        | KeyCode::Char('k')
                        | KeyCode::Char('s')
                        | KeyCode::Char('f')
                        | KeyCode::Char('z')
                        | KeyCode::Char('y')
                );
            if !global {
                return self.flow_key(k);
            }
        }
        if self.focus == Focus::Find && !matches!(self.mode, Mode::Command | Mode::Search) {
            let global = is_ctrl(&k)
                && matches!(
                    k.code,
                    KeyCode::Char('w')
                        | KeyCode::Char('b')
                        | KeyCode::Char('k')
                        | KeyCode::Char('s')
                        | KeyCode::Char('z')
                        | KeyCode::Char('y')
                        | KeyCode::Char('t')
                );
            if !global {
                return self.find_key(k);
            }
        }
        if is_ctrl(&k) {
            match k.code {
                KeyCode::Char('s') => {
                    self.save(None);
                    return;
                }
                KeyCode::Char('b') => {
                    self.toggle_tree();
                    return;
                }
                KeyCode::Char('k') => {
                    let d = self.tree.root.clone();
                    self.folder_picker(d, None);
                    return;
                }
                KeyCode::Char('f') => return self.open_find(false),
                KeyCode::Char('h') => return self.open_find(true),
                KeyCode::Char('z') if k.modifiers.contains(KeyModifiers::SHIFT) => return self.redo_any(),
                KeyCode::Char('Z') => return self.redo_any(),
                KeyCode::Char('z') => return self.undo_any(),
                KeyCode::Char('y') => return self.redo_any(),
                KeyCode::Char('t') => return self.workspace_symbols(),
                KeyCode::Char('O') => return self.lsp_document_symbols(),
                KeyCode::Char('o') if k.modifiers.contains(KeyModifiers::SHIFT) => return self.lsp_document_symbols(),
                KeyCode::Char('.') => {
                    self.back_to_normal();
                    return self.lsp_code_actions();
                }
                KeyCode::Char('w') => {
                    self.back_to_normal();
                    self.focus = match self.focus {
                        Focus::Editor if self.tree.visible => Focus::Tree,
                        Focus::Editor | Focus::Tree if self.flow.visible => Focus::Flow,
                        Focus::Editor | Focus::Tree | Focus::Flow if self.ai.visible => Focus::Ai,
                        Focus::Editor | Focus::Tree | Focus::Flow | Focus::Ai if self.dbg.visible => Focus::Debug,
                        _ => Focus::Editor,
                    };
                    return;
                }
                KeyCode::Char('c') | KeyCode::Char('x') if matches!(self.mode, Mode::Visual | Mode::VisualLine) => {
                    let cut = k.code == KeyCode::Char('x');
                    self.visual_op(if cut { 'd' } else { 'y' });
                    self.info(if cut {
                        t!("Kesildi (panoda)", "Cut (in clipboard)")
                    } else {
                        t!("Panoya kopyalandı", "Copied to clipboard")
                    });
                    return;
                }
                KeyCode::Char('c') => {
                    if self.mode == Mode::Normal && self.focus == Focus::Editor {
                        self.info(t!("Çıkmak için :q yazın", "Type :q to quit"));
                    } else {
                        self.back_to_normal();
                    }
                    return;
                }
                _ => {}
            }
        }
        match self.mode {
            Mode::Command | Mode::Search => self.cmd_key(k),
            _ if self.focus == Focus::Tree => self.tree_key(k),
            Mode::Normal => self.normal_key(k),
            Mode::Insert => self.insert_key(k),
            Mode::Visual | Mode::VisualLine => self.visual_key(k),
        }
    }

    fn back_to_normal(&mut self) {
        match self.mode {
            Mode::Insert => self.leave_insert(),
            _ => self.mode = Mode::Normal,
        }
        self.reset_pending();
    }

    fn reset_pending(&mut self) {
        self.op = None;
        self.prefix = None;
        self.count = None;
        self.op_count = None;
    }

    fn take_n(&mut self) -> (usize, bool) {
        let given = self.count.is_some() || self.op_count.is_some();
        let n = self.count.take().unwrap_or(1) * self.op_count.take().unwrap_or(1);
        (n.max(1), given)
    }

    fn count_digit(&mut self, c: char) -> bool {
        if let Some(d) = c.to_digit(10) {
            if d != 0 || self.count.is_some() {
                self.count = Some(
                    self.count
                        .unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(d as usize)
                        .min(1_000_000),
                );
                return true;
            }
        }
        false
    }

    fn key_char(k: &KeyEvent) -> Option<char> {
        Some(match k.code {
            KeyCode::Char(c) => c,
            KeyCode::Left | KeyCode::Backspace => 'h',
            KeyCode::Right => 'l',
            KeyCode::Up => 'k',
            KeyCode::Down | KeyCode::Enter => 'j',
            KeyCode::Home => '0',
            KeyCode::End => '$',
            KeyCode::Delete => 'x',
            _ => return None,
        })
    }

    fn toggle_tree(&mut self) {
        self.back_to_normal();
        self.tree.visible = !self.tree.visible;
        self.focus = if self.tree.visible { Focus::Tree } else { Focus::Editor };
        if self.tree.visible {
            self.tree.refresh();
        }
    }

    fn tree_rows(&self) -> usize {
        self.layout().text_h.saturating_sub(1) as usize
    }

    fn tree_follow(&mut self) {
        let r = self.tree_rows().max(1);
        let t = &mut self.tree;
        if t.sel < t.scroll {
            t.scroll = t.sel;
        } else if t.sel >= t.scroll + r {
            t.scroll = t.sel + 1 - r;
        }
    }

    fn tree_key(&mut self, k: KeyEvent) {
        let n = self.tree.nodes.len();
        match k.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.tree.sel + 1 < n {
                    self.tree.sel += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => self.tree.sel = self.tree.sel.saturating_sub(1),
            KeyCode::Char('g') | KeyCode::Home => self.tree.sel = 0,
            KeyCode::Char('G') | KeyCode::End => self.tree.sel = n.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Char('o') | KeyCode::Right => {
                self.tree_activate(self.tree.sel)
            }
            KeyCode::Char('h') | KeyCode::Left => {
                let i = self.tree.sel;
                if i < n {
                    if self.tree.nodes[i].is_dir && self.tree.nodes[i].expanded {
                        self.tree.toggle(i);
                    } else if let Some(p) = self.tree.parent_of(i) {
                        self.tree.sel = p;
                    }
                }
            }
            KeyCode::Char('r') => {
                self.tree.refresh();
                self.info(t!("Dosya ağacı yenilendi", "File tree refreshed"));
            }
            KeyCode::Char(':') => self.start_cmd(Mode::Command),
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Tab => self.focus = Focus::Editor,
            _ => {}
        }
        self.tree_follow();
    }

    fn tree_activate(&mut self, i: usize) {
        if i >= self.tree.nodes.len() {
            return;
        }
        if self.tree.nodes[i].is_dir {
            self.tree.toggle(i);
        } else {
            let p = self.tree.nodes[i].path.clone();
            self.open_path(&p);
            self.focus = Focus::Editor;
        }
    }

    pub fn open_path(&mut self, p: &Path) {
        let p = absolute(p);
        if p.is_dir() {
            let _ = env::set_current_dir(&p);
            self.push_recent(&p);
            self.tree.set_root(p);
            self.tree.visible = true;
            self.focus = Focus::Tree;
            return;
        }
        self.back_to_normal();
        let key = path_key(&p);
        if let Some(i) = self
            .bufs
            .iter()
            .position(|b| b.path.as_ref().map(|x| path_key(x)).as_ref() == Some(&key))
        {
            self.cur = i;
            return;
        }
        match Buffer::open(&p) {
            Ok(b) => {
                let is_new = !p.exists();
                if self.is_pristine(self.cur) {
                    self.bufs[self.cur] = b;
                } else {
                    self.bufs.push(b);
                    self.cur = self.bufs.len() - 1;
                }
                let name = self.rel_path(self.buf());
                if is_new {
                    self.info(tf!("\"{}\" [yeni dosya]", "\"{}\" [new file]", name));
                } else {
                    let n = self.buf().lines.len();
                    self.info(tf!("\"{}\" {} satır", "\"{}\" {} lines", name, n));
                }
            }
            Err(e) => self.error(format!("{}: {}", p.display(), e)),
        }
    }

    fn normal_key(&mut self, k: KeyEvent) {
        if is_ctrl(&k) {
            if let KeyCode::Char(c) = k.code {
                match c {
                    'r' => self.redo(),
                    'd' => self.half_page(true),
                    'u' => self.half_page(false),
                    'f' => self.full_page(true),
                    'o' => self.jump_back(),
                    ' ' => self.lsp_hover(),
                    _ => {}
                }
            }
            self.reset_pending();
            return;
        }
        match k.code {
            KeyCode::F(2) => return self.start_rename(),
            KeyCode::F(12) if k.modifiers.contains(KeyModifiers::SHIFT) => return self.lsp_references(),
            KeyCode::F(12) => return self.lsp_definition(),
            KeyCode::PageDown => return self.full_page(true),
            KeyCode::PageUp => return self.full_page(false),
            KeyCode::Esc => return self.reset_pending(),
            KeyCode::Tab => {
                if self.tree.visible {
                    self.focus = Focus::Tree;
                }
                return;
            }
            _ => {}
        }
        let Some(c) = Self::key_char(&k) else { return };
        if self.is_splash() && self.prefix.is_none() && self.op.is_none() && self.count.is_none() {
            if let Some(d) = c.to_digit(10).filter(|d| *d >= 1) {
                if let Some(p) = self.recent.get(d as usize - 1).cloned() {
                    self.open_path(&p);
                    return;
                }
            }
        }
        if let Some(p) = self.prefix.take() {
            return self.prefix_arg(p, c);
        }
        if self.count_digit(c) {
            return;
        }
        if let Some(op) = self.op {
            if c == op {
                return self.op_line_n(op);
            }
            match c {
                'f' | 'F' | 't' | 'T' | 'g' => self.prefix = Some(c),
                _ if is_motion(c) => self.apply_op_motion(op, c, None),
                _ => self.reset_pending(),
            }
            return;
        }
        if is_motion(c) {
            return self.do_motion(c, None);
        }
        match c {
            'f' | 'F' | 't' | 'T' | 'g' | 'r' | ']' | '[' => self.prefix = Some(c),
            'd' | 'y' | 'c' | '>' | '<' => {
                self.op = Some(c);
                self.op_count = self.count.take();
            }
            'i' => self.enter_insert(true),
            'a' => {
                let b = self.buf_mut();
                if b.line_len(b.cy) > 0 {
                    b.cx += 1;
                }
                self.enter_insert(true);
            }
            'I' => {
                let b = self.buf_mut();
                b.cx = b.first_nb(b.cy);
                self.enter_insert(true);
            }
            'A' => {
                let b = self.buf_mut();
                b.cx = b.line_len(b.cy);
                self.enter_insert(true);
            }
            'o' => self.open_line(true),
            'O' => self.open_line(false),
            'x' => self.op_with('d', 'l'),
            'X' => self.op_with('d', 'h'),
            'D' => self.op_with('d', '$'),
            'C' => self.op_with('c', '$'),
            's' => self.op_with('c', 'l'),
            'S' => self.op_line_n('c'),
            'Y' => self.op_line_n('y'),
            'p' => self.paste(true),
            'P' => self.paste(false),
            'u' => self.undo(),
            'J' => self.join_lines(),
            '~' => self.toggle_case_n(),
            'v' | 'V' => {
                self.anchor = (self.buf().cx, self.buf().cy);
                self.mode = if c == 'v' { Mode::Visual } else { Mode::VisualLine };
                self.count = None;
            }
            ':' => self.start_cmd(Mode::Command),
            '/' => self.start_cmd(Mode::Search),
            'n' => self.find_next(true),
            'N' => self.find_next(false),
            '*' => self.search_word(),
            'K' => {
                if !self.debug_hover() {
                    self.lsp_hover();
                }
            }
            _ => self.reset_pending(),
        }
    }

    fn prefix_arg(&mut self, p: char, c: char) {
        match p {
            'g' => match c {
                'g' => self.motion_or_op('g', None),
                't' => {
                    self.reset_pending();
                    self.cycle_buf(1);
                }
                'T' => {
                    self.reset_pending();
                    self.cycle_buf(-1);
                }
                'd' if self.op.is_none() => {
                    self.reset_pending();
                    self.lsp_definition();
                }
                'r' if self.op.is_none() => {
                    self.reset_pending();
                    self.lsp_references();
                }
                'a' if self.op.is_none() => {
                    self.reset_pending();
                    self.lsp_code_actions();
                }
                's' if self.op.is_none() => {
                    self.reset_pending();
                    self.lsp_document_symbols();
                }
                'S' if self.op.is_none() => {
                    self.reset_pending();
                    self.workspace_symbols();
                }
                _ => self.reset_pending(),
            },
            ']' | '[' => {
                self.reset_pending();
                if c == 'd' {
                    self.goto_diag(p == ']');
                }
            }
            'f' | 'F' | 't' | 'T' => self.motion_or_op(p, Some(c)),
            'r' => self.replace_chars(c),
            _ => self.reset_pending(),
        }
    }

    fn motion_or_op(&mut self, m: char, arg: Option<char>) {
        match self.op {
            Some(op) => self.apply_op_motion(op, m, arg),
            None => self.do_motion(m, arg),
        }
    }

    fn op_with(&mut self, op: char, m: char) {
        self.op_count = self.count.take();
        self.apply_op_motion(op, m, None);
    }

    fn op_line_n(&mut self, op: char) {
        let (n, _) = self.take_n();
        self.op = None;
        let b = self.buf();
        let y0 = b.cy;
        let y1 = (b.cy + n - 1).min(b.last());
        self.op_lines(op, y0, y1);
    }

    fn motion_target(&self, m: char, arg: Option<char>, n: usize, given: bool) -> Option<(Pos, MKind)> {
        use MKind::*;
        let b = self.buf();
        let (x, y) = (b.cx, b.cy);
        let last = b.last();
        Some(match m {
            'h' => ((x.saturating_sub(n), y), Excl),
            'l' => (((x + n).min(b.line_len(y)), y), Excl),
            'j' => ((x, (y + n).min(last)), Line),
            'k' => ((x, y.saturating_sub(n)), Line),
            '0' => ((0, y), Excl),
            '^' => ((b.first_nb(y), y), Excl),
            '$' => {
                let yy = (y + n - 1).min(last);
                ((b.line_len(yy).saturating_sub(1), yy), Incl)
            }
            'w' => {
                let mut p = (x, y);
                for _ in 0..n {
                    p = b.next_word(p);
                }
                (p, Excl)
            }
            'b' => {
                let mut p = (x, y);
                for _ in 0..n {
                    p = b.prev_word(p);
                }
                (p, Excl)
            }
            'e' => {
                let mut p = (x, y);
                for _ in 0..n {
                    p = b.end_word(p);
                }
                (p, Incl)
            }
            'G' => {
                let yy = if given { (n - 1).min(last) } else { last };
                ((b.first_nb(yy), yy), Line)
            }
            'g' => {
                let yy = if given { (n - 1).min(last) } else { 0 };
                ((b.first_nb(yy), yy), Line)
            }
            '}' => {
                let mut yy = y;
                for _ in 0..n {
                    if yy < last {
                        yy += 1;
                    }
                    while yy < last && !b.lines[yy].is_empty() {
                        yy += 1;
                    }
                }
                let xx = if yy == last { b.line_len(yy) } else { 0 };
                ((xx, yy), Excl)
            }
            '{' => {
                let mut yy = y;
                for _ in 0..n {
                    yy = yy.saturating_sub(1);
                    while yy > 0 && !b.lines[yy].is_empty() {
                        yy -= 1;
                    }
                }
                ((0, yy), Excl)
            }
            '%' => (b.match_bracket((x, y))?, Incl),
            'f' | 't' => {
                let c = arg?;
                let line = &b.lines[y];
                let mut pos = x;
                for _ in 0..n {
                    pos = (pos + 1..line.len()).find(|&i| line[i] == c)?;
                }
                let xx = if m == 't' { pos - 1 } else { pos };
                ((xx, y), Incl)
            }
            'F' | 'T' => {
                let c = arg?;
                let line = &b.lines[y];
                let mut pos = x;
                for _ in 0..n {
                    pos = (0..pos).rev().find(|&i| line[i] == c)?;
                }
                let xx = if m == 'T' { pos + 1 } else { pos };
                ((xx, y), Excl)
            }
            _ => return None,
        })
    }

    fn do_motion(&mut self, m: char, arg: Option<char>) {
        let (n, given) = self.take_n();
        self.prefix = None;
        let Some(((x, y), _)) = self.motion_target(m, arg, n, given) else {
            return;
        };
        let allow = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        let b = self.buf_mut();
        b.cy = y;
        if m == 'j' || m == 'k' {
            b.cx = idx_at_col(&b.lines[y], b.want_col);
        } else {
            b.cx = x;
        }
        b.clamp(allow);
        if m == '$' {
            b.want_col = usize::MAX;
        } else if m != 'j' && m != 'k' {
            b.sync_want();
        }
    }

    fn apply_op_motion(&mut self, op: char, m: char, arg: Option<char>) {
        let (n, given) = self.take_n();
        self.op = None;
        self.prefix = None;
        let cur = (self.buf().cx, self.buf().cy);
        let under = self.buf().lines[cur.1].get(cur.0).copied();
        let (target, kind) = if op == 'c' && m == 'w' && n == 1 && under.is_some_and(|c| !c.is_whitespace()) {
            let line = &self.buf().lines[cur.1];
            let k = cls(line[cur.0]);
            let mut x = cur.0;
            while x + 1 < line.len() && cls(line[x + 1]) == k {
                x += 1;
            }
            ((x, cur.1), MKind::Incl)
        } else {
            let m2 = if op == 'c' && m == 'w' { 'e' } else { m };
            match self.motion_target(m2, arg, n, given) {
                Some(t) => t,
                None => return,
            }
        };
        match kind {
            MKind::Line => self.op_lines(op, cur.1.min(target.1), cur.1.max(target.1)),
            _ => {
                let (a, mut e) = if (target.1, target.0) < (cur.1, cur.0) {
                    (target, cur)
                } else {
                    (cur, target)
                };
                if kind == MKind::Incl {
                    e.0 += 1;
                }
                if kind == MKind::Excl && e.0 == 0 && e.1 > a.1 {
                    let yy = e.1 - 1;
                    e = (self.buf().line_len(yy), yy);
                }
                let len = self.buf().line_len(e.1);
                e.0 = e.0.min(len);
                if a == e {
                    if op == 'c' {
                        self.enter_insert(true);
                    }
                    return;
                }
                self.op_chars(op, a, e);
            }
        }
    }

    fn op_lines(&mut self, op: char, y0: usize, y1: usize) {
        let unit = self.indent_unit();
        match op {
            'y' => {
                let ls = self.buf().lines[y0..=y1].to_vec();
                let n = ls.len();
                self.set_reg(Reg::Lines(ls));
                self.buf_mut().cy = y0;
                if n > 2 {
                    self.info(tf!("{} satır kopyalandı", "{} lines yanked", n));
                }
            }
            'd' => {
                let b = self.buf_mut();
                b.checkpoint();
                let ls = b.delete_lines(y0, y1);
                b.cy = y0.min(b.last());
                b.cx = b.first_nb(b.cy);
                let n = ls.len();
                self.set_reg(Reg::Lines(ls));
                if n > 2 {
                    self.info(tf!("{} satır silindi", "{} lines deleted", n));
                }
            }
            'c' => {
                let b = self.buf_mut();
                b.checkpoint();
                let indent = leading_ws(&b.lines[y0]);
                let ls: Vec<Vec<char>> = b.lines.splice(y0..=y1, [indent.clone()]).collect();
                b.touch();
                b.cy = y0;
                b.cx = indent.len();
                self.set_reg(Reg::Lines(ls));
                self.enter_insert(false);
            }
            '>' | '<' => {
                let b = self.buf_mut();
                b.checkpoint();
                for y in y0..=y1 {
                    let line = &mut b.lines[y];
                    if op == '>' {
                        if !line.is_empty() {
                            line.splice(0..0, unit.iter().copied());
                        }
                    } else if line.first() == Some(&'\t') {
                        line.remove(0);
                    } else {
                        let n = line.iter().take(TAB_W).take_while(|c| **c == ' ').count();
                        line.drain(..n);
                    }
                }
                b.touch();
                b.cy = y0;
                b.cx = b.first_nb(y0);
            }
            _ => {}
        }
    }

    fn op_chars(&mut self, op: char, a: Pos, e: Pos) {
        match op {
            'y' => {
                let t = self.buf().get_text(a, e);
                self.set_reg(Reg::Chars(t));
                let b = self.buf_mut();
                b.cx = a.0;
                b.cy = a.1;
            }
            'd' | 'c' => {
                let b = self.buf_mut();
                b.checkpoint();
                let t = b.delete_text(a, e);
                b.cx = a.0;
                b.cy = a.1;
                self.set_reg(Reg::Chars(t));
                if op == 'c' {
                    self.enter_insert(false);
                }
            }
            '>' | '<' => self.op_lines(op, a.1, e.1),
            '~' => {
                let b = self.buf_mut();
                b.checkpoint();
                for y in a.1..=e.1 {
                    let len = b.line_len(y);
                    let x0 = if y == a.1 { a.0 } else { 0 };
                    let x1 = if y == e.1 { e.0.min(len) } else { len };
                    for x in x0..x1 {
                        b.lines[y][x] = toggle_case(b.lines[y][x]);
                    }
                }
                b.touch();
                b.cx = a.0;
                b.cy = a.1;
            }
            _ => {}
        }
    }

    fn replace_chars(&mut self, c: char) {
        let (n, _) = self.take_n();
        let b = self.buf_mut();
        let len = b.line_len(b.cy);
        if b.cx + n > len {
            return;
        }
        b.checkpoint();
        for i in 0..n {
            let x = b.cx + i;
            b.lines[b.cy][x] = c;
        }
        b.cx += n - 1;
        b.touch();
    }

    fn toggle_case_n(&mut self) {
        let (n, _) = self.take_n();
        let b = self.buf_mut();
        let len = b.line_len(b.cy);
        if len == 0 {
            return;
        }
        b.checkpoint();
        let end = (b.cx + n).min(len);
        for x in b.cx..end {
            let y = b.cy;
            b.lines[y][x] = toggle_case(b.lines[y][x]);
        }
        b.cx = end.min(len - 1);
        b.touch();
    }

    fn join_lines(&mut self) {
        let (n, _) = self.take_n();
        let b = self.buf_mut();
        let times = n.max(2) - 1;
        if b.cy >= b.last() {
            return;
        }
        b.checkpoint();
        for _ in 0..times {
            if b.cy >= b.last() {
                break;
            }
            let next = b.lines.remove(b.cy + 1);
            let trimmed: Vec<char> = next.iter().skip_while(|c| c.is_whitespace()).copied().collect();
            let line = &mut b.lines[b.cy];
            while line.last().is_some_and(|c| c.is_whitespace()) {
                line.pop();
            }
            b.cx = line.len();
            if !trimmed.is_empty() && !line.is_empty() && trimmed[0] != ')' {
                line.push(' ');
            }
            line.extend(trimmed);
        }
        b.touch();
    }

    fn paste(&mut self, after: bool) {
        self.sync_from_clip();
        let (n, _) = self.take_n();
        let reg = self.reg.clone();
        let b = self.buf_mut();
        match reg {
            Reg::Lines(ls) => {
                if ls.is_empty() {
                    return;
                }
                b.checkpoint();
                let at = if after { b.cy + 1 } else { b.cy };
                let all: Vec<Vec<char>> = (0..n).flat_map(|_| ls.iter().cloned()).collect();
                b.insert_lines(at, &all);
                b.cy = at;
                b.cx = b.first_nb(at);
            }
            Reg::Chars(cs) => {
                if cs.is_empty() {
                    return;
                }
                b.checkpoint();
                let mut x = b.cx;
                if after && b.line_len(b.cy) > 0 {
                    x += 1;
                }
                let text: Vec<char> = (0..n).flat_map(|_| cs.iter().copied()).collect();
                let y = b.cy;
                let end = b.insert_text((x, y), &text);
                if text.contains(&'\n') {
                    b.cx = x;
                    b.cy = y;
                } else {
                    b.cx = end.0.saturating_sub(1);
                }
            }
        }
        self.buf_mut().sync_want();
    }

    pub(crate) fn undo_any(&mut self) {
        match self.mode {
            Mode::Insert => {
                self.popup = None;
                self.sig = None;
                if !self.insert_cp_pending {
                    self.buf_mut().drop_noop_checkpoint();
                }
                self.undo();
                self.insert_cp_pending = true;
            }
            Mode::Visual | Mode::VisualLine => {
                self.mode = Mode::Normal;
                self.undo();
            }
            _ => self.undo(),
        }
        let allow = self.mode == Mode::Insert;
        self.buf_mut().clamp(allow);
    }

    pub(crate) fn redo_any(&mut self) {
        if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
            self.mode = Mode::Normal;
        }
        if self.mode == Mode::Insert {
            self.popup = None;
            self.insert_cp_pending = true;
        }
        self.redo();
        let allow = self.mode == Mode::Insert;
        self.buf_mut().clamp(allow);
    }

    fn undo(&mut self) {
        if !self.buf_mut().undo() {
            self.info(t!("Geri alınacak bir şey yok", "Already at oldest change"));
        }
    }

    fn redo(&mut self) {
        if !self.buf_mut().redo() {
            self.info(t!("Yinelenecek bir şey yok", "Already at newest change"));
        }
    }

    fn half_page(&mut self, down: bool) {
        let h = (self.layout().text_h as usize / 2).max(1);
        self.move_page(down, h);
    }

    fn full_page(&mut self, down: bool) {
        let h = (self.layout().text_h as usize).saturating_sub(2).max(1);
        self.move_page(down, h);
    }

    fn move_page(&mut self, down: bool, h: usize) {
        let b = self.buf_mut();
        let last = b.last();
        if down {
            b.cy = (b.cy + h).min(last);
            b.scroll_y = (b.scroll_y + h).min(last);
        } else {
            b.cy = b.cy.saturating_sub(h);
            b.scroll_y = b.scroll_y.saturating_sub(h);
        }
        b.cx = idx_at_col(&b.lines[b.cy], b.want_col);
    }

    fn enter_insert(&mut self, checkpoint: bool) {
        self.insert_cp_pending = false;
        if checkpoint {
            self.buf_mut().checkpoint();
        }
        self.mode = Mode::Insert;
        self.msg = None;
        self.reset_pending();
    }

    fn leave_insert(&mut self) {
        self.mode = Mode::Normal;
        self.popup = None;
        self.sig = None;
        self.comp_id = None;
        let pending = std::mem::take(&mut self.insert_cp_pending);
        let b = self.buf_mut();
        if !pending {
            b.drop_noop_checkpoint();
        }
        if b.cx > 0 {
            b.cx -= 1;
        }
        b.clamp(false);
        b.sync_want();
    }

    fn open_line(&mut self, below: bool) {
        let unit = self.indent_unit();
        let py = self.buf().lang.name == "Python";
        let b = self.buf_mut();
        b.checkpoint();
        let line = &b.lines[b.cy];
        let mut ind = leading_ws(line);
        let last_nb = line.iter().rev().find(|c| !c.is_whitespace()).copied();
        if below && (matches!(last_nb, Some('{' | '[' | '(')) || (py && last_nb == Some(':'))) {
            ind.extend(&unit);
        }
        let at = if below { b.cy + 1 } else { b.cy };
        b.lines.insert(at, ind.clone());
        b.cy = at;
        b.cx = ind.len();
        b.touch();
        self.enter_insert(false);
    }

    fn insert_key(&mut self, k: KeyEvent) {
        let ctrl = is_ctrl(&k);
        if self.insert_cp_pending {
            let modifies = match k.code {
                KeyCode::Char('v') => true,
                KeyCode::Char(_) => !ctrl,
                KeyCode::Enter | KeyCode::Backspace | KeyCode::Delete | KeyCode::Tab => true,
                _ => false,
            };
            if modifies {
                self.insert_cp_pending = false;
                self.buf_mut().checkpoint();
            }
        }
        if self.popup.is_some() {
            match k.code {
                KeyCode::Down => return self.popup_move(1),
                KeyCode::Up | KeyCode::BackTab => return self.popup_move(-1),
                KeyCode::Char('n') if ctrl => return self.popup_move(1),
                KeyCode::Char('p') if ctrl => return self.popup_move(-1),
                KeyCode::PageDown => return self.popup_move(POPUP_ROWS as isize),
                KeyCode::PageUp => return self.popup_move(-(POPUP_ROWS as isize)),
                KeyCode::Tab => return self.accept_completion(),
                KeyCode::Enter => {
                    if self.popup_fully_typed() {
                        self.popup = None;
                    } else {
                        return self.accept_completion();
                    }
                }
                KeyCode::Esc => self.popup = None,
                _ => {}
            }
        }
        match k.code {
            KeyCode::Esc => return self.leave_insert(),
            KeyCode::Char(c) if !ctrl => {
                self.insert_char(c);
                self.buf_mut().sync_want();
                return self.after_insert_char(c);
            }
            KeyCode::Char(' ') | KeyCode::Null if ctrl || k.code == KeyCode::Null => {
                return self.trigger_completion(true, None);
            }
            KeyCode::Char('n') | KeyCode::Char('p') if ctrl => return self.trigger_completion(true, None),
            KeyCode::Char('v') if ctrl => {
                if let Some(t) = self.clipboard_text() {
                    let t: Vec<char> = t.replace("\r\n", "\n").chars().collect();
                    let b = self.buf_mut();
                    let p = b.insert_text((b.cx, b.cy), &t);
                    b.cx = p.0;
                    b.cy = p.1;
                }
            }
            KeyCode::Enter => {
                self.sig = None;
                self.insert_newline();
            }
            KeyCode::Backspace => {
                self.backspace();
                if self.popup.is_some() {
                    self.refilter();
                }
            }
            KeyCode::Delete => self.delete_forward(),
            KeyCode::Tab => self.insert_tab(),
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Home | KeyCode::End
                if self.popup.is_some() || self.sig.is_some() =>
            {
                self.popup = None;
                self.sig = None;
                return self.insert_key(k);
            }
            KeyCode::Left => {
                let b = self.buf_mut();
                b.cx = b.cx.saturating_sub(1);
            }
            KeyCode::Right => {
                let b = self.buf_mut();
                if b.cx < b.line_len(b.cy) {
                    b.cx += 1;
                }
            }
            KeyCode::Up | KeyCode::Down => {
                let b = self.buf_mut();
                if k.code == KeyCode::Up {
                    b.cy = b.cy.saturating_sub(1);
                } else if b.cy < b.last() {
                    b.cy += 1;
                }
                b.cx = idx_at_col(&b.lines[b.cy], b.want_col);
                return;
            }
            KeyCode::Home => self.buf_mut().cx = 0,
            KeyCode::End => {
                let b = self.buf_mut();
                b.cx = b.line_len(b.cy);
            }
            KeyCode::PageDown => return self.full_page(true),
            KeyCode::PageUp => return self.full_page(false),
            _ => return,
        }
        self.buf_mut().sync_want();
    }

    fn insert_char(&mut self, c: char) {
        let unit_len = self.indent_unit().len();
        let burst = self.burst;
        let b = self.buf_mut();
        let (x, y) = (b.cx.min(b.line_len(b.cy)), b.cy);
        b.lines[y].insert(x, c);
        b.cx = x + 1;
        b.touch();
        if !burst && matches!(c, '}' | ']' | ')') {
            let line = &b.lines[y];
            if x > 0 && line[..x].iter().all(|c| *c == ' ' || *c == '\t') {
                let rm = if line[0] == '\t' { 1 } else { unit_len.min(x) };
                b.lines[y].drain(..rm);
                b.cx -= rm;
            }
        }
    }

    fn insert_newline(&mut self) {
        let unit = self.indent_unit();
        let py = self.buf().lang.name == "Python";
        let burst = self.burst;
        let b = self.buf_mut();
        let (cx, cy) = (b.cx.min(b.line_len(b.cy)), b.cy);
        if burst {
            let p = b.insert_text((cx, cy), &['\n']);
            b.cx = p.0;
            b.cy = p.1;
            return;
        }
        let line = &b.lines[cy];
        let base = leading_ws(&line[..cx]);
        let before = line[..cx].iter().rev().find(|c| !c.is_whitespace()).copied();
        let after = line[cx..].iter().find(|c| !c.is_whitespace()).copied();
        let opener = matches!(before, Some('{' | '[' | '(')) || (py && before == Some(':'));
        let ws_after = line[cx..].iter().take_while(|c| **c == ' ' || **c == '\t').count();
        b.lines[cy].drain(cx..cx + ws_after);
        let mut ins = vec!['\n'];
        ins.extend(&base);
        if opener {
            ins.extend(&unit);
        }
        let pos = b.insert_text((cx, cy), &ins);
        let closes = matches!(
            (before, after),
            (Some('{'), Some('}')) | (Some('['), Some(']')) | (Some('('), Some(')'))
        );
        if closes {
            let mut tail = vec!['\n'];
            tail.extend(&base);
            b.insert_text(pos, &tail);
        }
        b.cx = pos.0;
        b.cy = pos.1;
    }

    fn backspace(&mut self) {
        let b = self.buf_mut();
        if b.cx > 0 {
            let x = b.cx.min(b.line_len(b.cy));
            let line = &b.lines[b.cy];
            let all_sp = line[..x].iter().all(|c| *c == ' ');
            let n = if all_sp && !b.use_tabs {
                let r = x % TAB_W;
                if r == 0 {
                    TAB_W.min(x)
                } else {
                    r
                }
            } else {
                1
            };
            let y = b.cy;
            b.lines[y].drain(x - n..x);
            b.cx = x - n;
            b.touch();
        } else if b.cy > 0 {
            let line = b.lines.remove(b.cy);
            b.cy -= 1;
            b.cx = b.lines[b.cy].len();
            let y = b.cy;
            b.lines[y].extend(line);
            b.touch();
        }
    }

    fn delete_forward(&mut self) {
        let b = self.buf_mut();
        let len = b.line_len(b.cy);
        let y = b.cy;
        if b.cx < len {
            let x = b.cx;
            b.lines[y].remove(x);
            b.touch();
        } else if y < b.last() {
            let next = b.lines.remove(y + 1);
            b.lines[y].extend(next);
            b.touch();
        }
    }

    fn insert_tab(&mut self) {
        let b = self.buf_mut();
        let y = b.cy;
        let x = b.cx.min(b.line_len(y));
        if b.use_tabs {
            b.lines[y].insert(x, '\t');
            b.cx = x + 1;
        } else {
            let col = disp_col(&b.lines[y], x);
            let n = TAB_W - col % TAB_W;
            for _ in 0..n {
                b.lines[y].insert(x, ' ');
            }
            b.cx = x + n;
        }
        b.touch();
    }

    fn on_paste(&mut self, s: &str) {
        if self.focus == Focus::Ai && !matches!(self.mode, Mode::Command | Mode::Search) {
            return self.ai_paste(s);
        }
        if self.focus == Focus::Find && !matches!(self.mode, Mode::Command | Mode::Search) {
            return self.find_paste(s);
        }
        if self.mode == Mode::Insert && self.insert_cp_pending {
            self.insert_cp_pending = false;
            self.buf_mut().checkpoint();
        }
        let text: Vec<char> = s.replace("\r\n", "\n").replace('\r', "\n").chars().collect();
        match self.mode {
            Mode::Command | Mode::Search => {
                for c in text.into_iter().take_while(|c| *c != '\n') {
                    self.cmd.insert(self.cmd_x, c);
                    self.cmd_x += 1;
                }
            }
            Mode::Normal | Mode::Insert => {
                if self.focus == Focus::Tree {
                    return;
                }
                let normal = self.mode == Mode::Normal;
                let b = self.buf_mut();
                if normal {
                    b.checkpoint();
                }
                let p = b.insert_text((b.cx, b.cy), &text);
                b.cx = p.0;
                b.cy = p.1;
            }
            _ => self.mode = Mode::Normal,
        }
    }

    fn visual_key(&mut self, k: KeyEvent) {
        if is_ctrl(&k) {
            if let KeyCode::Char(c) = k.code {
                match c {
                    'd' => self.half_page(true),
                    'u' => self.half_page(false),
                    _ => {}
                }
            }
            return;
        }
        if k.code == KeyCode::Esc {
            self.mode = Mode::Normal;
            self.reset_pending();
            return;
        }
        let Some(c) = Self::key_char(&k) else { return };
        if let Some(p) = self.prefix.take() {
            return self.prefix_arg(p, c);
        }
        if self.count_digit(c) {
            return;
        }
        if is_motion(c) {
            return self.do_motion(c, None);
        }
        match c {
            'f' | 'F' | 't' | 'T' | 'g' => self.prefix = Some(c),
            'v' => {
                self.mode = if self.mode == Mode::Visual {
                    Mode::Normal
                } else {
                    Mode::Visual
                }
            }
            'V' => {
                self.mode = if self.mode == Mode::VisualLine {
                    Mode::Normal
                } else {
                    Mode::VisualLine
                }
            }
            'o' => {
                let a = self.anchor;
                self.anchor = (self.buf().cx, self.buf().cy);
                let b = self.buf_mut();
                b.cx = a.0;
                b.cy = a.1;
            }
            'd' | 'x' => self.visual_op('d'),
            'y' => self.visual_op('y'),
            'c' | 's' => self.visual_op('c'),
            '>' | '<' | '~' => self.visual_op(c),
            'J' => {
                let (a, e) = self.selection();
                self.mode = Mode::Normal;
                self.buf_mut().cy = a.1;
                self.count = Some(e.1 - a.1 + 1);
                self.join_lines();
            }
            'p' | 'P' => self.visual_paste(),
            ':' => {
                self.mode = Mode::Normal;
                self.start_cmd(Mode::Command);
            }
            _ => {}
        }
    }

    fn visual_paste(&mut self) {
        self.sync_from_clip();
        let saved = self.reg.clone();
        self.visual_op('d');
        self.set_reg(saved);
        self.paste(false);
    }

    fn visual_op(&mut self, op: char) {
        let (a, e) = self.selection();
        let line_mode = self.mode == Mode::VisualLine;
        self.mode = Mode::Normal;
        self.reset_pending();
        if line_mode {
            self.op_lines(op, a.1, e.1);
        } else {
            let b = self.buf();
            let len = b.line_len(e.1);
            let e2 = if e.0 < len {
                (e.0 + 1, e.1)
            } else if e.1 < b.last() {
                (0, e.1 + 1)
            } else {
                (len, e.1)
            };
            if a == e2 {
                return;
            }
            self.op_chars(op, a, e2);
        }
        if op == 'y' {
            let b = self.buf_mut();
            b.cx = a.0;
            b.cy = a.1;
        }
    }

    fn start_cmd(&mut self, m: Mode) {
        self.mode = m;
        self.cmd.clear();
        self.cmd_x = 0;
        self.hist_i = self.history.len();
        self.msg = None;
        self.reset_pending();
    }

    fn cmd_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => self.mode = Mode::Normal,
            KeyCode::Enter => {
                let s: String = self.cmd.iter().collect();
                let m = self.mode;
                self.mode = Mode::Normal;
                if !s.is_empty() && self.history.last() != Some(&s) {
                    self.history.push(s.clone());
                }
                if m == Mode::Command {
                    self.run_command(&s);
                } else {
                    self.do_search(&s);
                }
            }
            KeyCode::Backspace => {
                if self.cmd.is_empty() {
                    self.mode = Mode::Normal;
                } else if self.cmd_x > 0 {
                    self.cmd_x -= 1;
                    self.cmd.remove(self.cmd_x);
                }
            }
            KeyCode::Delete => {
                if self.cmd_x < self.cmd.len() {
                    self.cmd.remove(self.cmd_x);
                }
            }
            KeyCode::Left => self.cmd_x = self.cmd_x.saturating_sub(1),
            KeyCode::Right => self.cmd_x = (self.cmd_x + 1).min(self.cmd.len()),
            KeyCode::Home => self.cmd_x = 0,
            KeyCode::End => self.cmd_x = self.cmd.len(),
            KeyCode::Up => {
                if self.hist_i > 0 {
                    self.hist_i -= 1;
                    self.cmd = self.history[self.hist_i].chars().collect();
                    self.cmd_x = self.cmd.len();
                }
            }
            KeyCode::Down => {
                if self.hist_i + 1 < self.history.len() {
                    self.hist_i += 1;
                    self.cmd = self.history[self.hist_i].chars().collect();
                } else {
                    self.hist_i = self.history.len();
                    self.cmd.clear();
                }
                self.cmd_x = self.cmd.len();
            }
            KeyCode::Tab => {
                if self.mode == Mode::Command {
                    self.complete();
                }
            }
            KeyCode::Char(c) => {
                if is_ctrl(&k) {
                    if c == 'u' {
                        self.cmd.drain(..self.cmd_x);
                        self.cmd_x = 0;
                    }
                } else {
                    self.cmd.insert(self.cmd_x, c);
                    self.cmd_x += 1;
                }
            }
            _ => {}
        }
    }

    fn complete(&mut self) {
        let s: String = self.cmd.iter().collect();
        let Some(sp) = s.find(' ') else { return };
        let (head, arg) = (&s[..=sp], &s[sp + 1..]);
        let (dir, prefix) = match arg.rfind(['/', '\\']) {
            Some(i) => (&arg[..=i], &arg[i + 1..]),
            None => ("", arg),
        };
        let read = if dir.is_empty() {
            PathBuf::from(".")
        } else {
            PathBuf::from(dir)
        };
        let Ok(rd) = fs::read_dir(&read) else { return };
        let mut names: Vec<(String, bool)> = rd
            .filter_map(|e| e.ok())
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path().is_dir()))
            .filter(|(n, _)| n.starts_with(prefix))
            .collect();
        if names.is_empty() {
            return;
        }
        names.sort();
        let first: Vec<char> = names[0].0.chars().collect();
        let mut common = first.len();
        for (n, _) in &names[1..] {
            let nc: Vec<char> = n.chars().collect();
            common = common.min(first.iter().zip(nc.iter()).take_while(|(a, b)| a == b).count());
        }
        let mut done: String = first[..common].iter().collect();
        if names.len() == 1 && names[0].1 {
            done.push('/');
        }
        let new = format!("{}{}{}", head, dir, done);
        self.cmd = new.chars().collect();
        self.cmd_x = self.cmd.len();
    }

    fn run_command(&mut self, s: &str) {
        let s = s.trim();
        if s.is_empty() {
            return;
        }
        if let Ok(n) = s.parse::<usize>() {
            let b = self.buf_mut();
            b.cy = n.saturating_sub(1).min(b.last());
            b.cx = b.first_nb(b.cy);
            b.sync_want();
            return;
        }
        if let Some(rest) = s.strip_prefix('!') {
            return self.run_shell(rest.trim());
        }
        if s.starts_with("s/") || s.starts_with("%s/") {
            return self.substitute(s);
        }
        let (cmd, arg) = match s.find(char::is_whitespace) {
            Some(i) => (&s[..i], s[i..].trim()),
            None => (s, ""),
        };
        let arg_opt = if arg.is_empty() { None } else { Some(arg) };
        match cmd {
            "w" | "w!" | "write" => {
                self.save(arg_opt);
            }
            "q" | "quit" | "close" => self.close_buffer(false, true),
            "q!" | "quit!" => self.close_buffer(true, true),
            "wq" | "x" | "wq!" | "x!" => {
                if self.save(arg_opt) {
                    self.close_buffer(false, true);
                }
            }
            "wa" | "wall" => self.save_all(),
            "qa" | "qall" => self.quit_all(false),
            "qa!" | "qall!" => self.quit_all(true),
            "wqa" | "xa" | "xall" => {
                self.save_all();
                self.quit_all(false);
            }
            "e" | "edit" => match arg_opt {
                Some(p) => self.open_path(Path::new(p)),
                None => self.error(t!("Kullanım: :e <dosya>", "Usage: :e <file>")),
            },
            "e!" => self.reload(),
            "enew" | "new" => {
                self.bufs.push(Buffer::scratch(NONAME));
                self.cur = self.bufs.len() - 1;
            }
            "bn" | "bnext" => self.cycle_buf(1),
            "bp" | "bprev" | "bN" => self.cycle_buf(-1),
            "bd" | "bdelete" => self.close_buffer(false, false),
            "bd!" | "bdelete!" => self.close_buffer(true, false),
            "b" | "buffer" => match arg.parse::<usize>() {
                Ok(n) if n >= 1 && n <= self.bufs.len() => self.cur = n - 1,
                _ => self.error(t!("Geçersiz sekme numarası", "Invalid tab number")),
            },
            "ls" | "buffers" => {
                let s = self
                    .bufs
                    .iter()
                    .enumerate()
                    .map(|(i, b)| {
                        format!(
                            "{}{}:{}{}",
                            if i == self.cur { "%" } else { "" },
                            i + 1,
                            b.name,
                            if b.dirty { "+" } else { "" }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("  ");
                self.info(s);
            }
            "tree" | "Tree" | "ex" | "Ex" => self.toggle_tree(),
            "cd" => {
                let target = arg_opt.map(PathBuf::from).unwrap_or_else(|| self.tree.root.clone());
                match env::set_current_dir(&target) {
                    Ok(()) => {
                        let root = env::current_dir().unwrap_or(target);
                        self.info(format!("{}", root.display()));
                        self.push_recent(&root);
                        self.tree.set_root(root);
                    }
                    Err(e) => self.error(format!("cd: {}", e)),
                }
            }
            "pwd" => {
                let d = env::current_dir().map(|d| d.display().to_string()).unwrap_or_default();
                self.info(d);
            }
            "set" | "se" => self.set_option(arg),
            "noh" | "nohlsearch" => self.hl_search = false,
            "h" | "help" => self.open_help(),
            "rename" | "rn" => match arg_opt {
                Some(n) => self.lsp_rename(n),
                None => self.start_rename(),
            },
            "format" | "fmt" => self.lsp_format(),
            "actions" | "ca" => self.lsp_code_actions(),
            "diag" | "diagnostics" => self.diag_menu(),
            "def" | "definition" => self.lsp_definition(),
            "refs" | "references" => self.lsp_references(),
            "symbols" | "sym" => self.lsp_document_symbols(),
            "wsymbols" | "wsym" => self.workspace_symbols(),
            "find" => self.open_find(false),
            "replace" => self.open_find(true),
            "hover" => self.lsp_hover(),
            "lsp" | "LspInfo" => self.lsp_command(arg),
            "discord" => self.discord_command(arg),
            "debug" | "dbg" => self.debug_command(arg),
            "flow" => self.flow_command(arg),
            "install" => self.install_command(arg),
            "uninstall" => self.uninstall_command(arg),
            "break" | "breakpoint" => {
                let y = self.buf().cy;
                self.toggle_breakpoint(y);
            }
            "breakclear" => {
                let keys: Vec<String> = self.dbg.bps.keys().cloned().collect();
                for k in keys {
                    if let Some(e) = self.dbg.bps.get_mut(&k) {
                        e.1.clear();
                    }
                }
                let y = self.buf().cy;
                self.toggle_breakpoint(y);
                self.toggle_breakpoint(y);
                self.dbg.bps.clear();
                self.info(t!("Tüm breakpoint'ler silindi", "All breakpoints cleared"));
            }
            "watch" => match arg_opt {
                Some(e) => self.add_watch(e),
                None => self.error(t!("Kullanım: :watch <ifade>", "Usage: :watch <expression>")),
            },
            "unwatch" => match arg.parse::<usize>() {
                Ok(n) if n >= 1 && n <= self.dbg.watches.len() => {
                    self.dbg.watches.remove(n - 1);
                }
                _ => self.dbg.watches.clear(),
            },
            "ai" => self.ai_command(arg),
            "lang" | "language" | "dil" => self.set_lang(arg),
            "open" | "o" | "folder" => match arg_opt {
                Some(p) => self.open_path(Path::new(p)),
                None => {
                    let d = self.tree.root.clone();
                    self.folder_picker(d, None);
                }
            },
            _ => self.error(tf!("Bilinmeyen komut: {}", "Unknown command: {}", cmd)),
        }
    }

    pub fn presence_tick(&mut self) {
        let b = self.buf();
        let details = match &b.path {
            Some(_) if self.dbg.active() => format!("Debugging {}", b.name),
            Some(_) => format!("Editing {}", b.name),
            None if b.name == HELP_NAME => "Reading help".to_string(),
            None => "Idling".to_string(),
        };
        let state = format!("Workspace: {}", self.tree.root_name());
        self.presence.update(details, state);
    }

    fn discord_command(&mut self, arg: &str) {
        let (sub, val) = match arg.split_once(char::is_whitespace) {
            Some((a, b)) => (a, b.trim()),
            None => (arg, ""),
        };
        match sub {
            "on" | "off" => {
                let on = sub == "on";
                self.presence.enable(on);
                let _ = crate::config::set("discord", sub);
                self.info(if on {
                    t!("Discord durumu açık", "Discord presence on")
                } else {
                    t!("Discord durumu kapalı", "Discord presence off")
                });
            }
            "id" if !val.is_empty() => {
                if !val.chars().all(|c| c.is_ascii_digit()) {
                    return self.error(t!(
                        "Uygulama kimliği yalnızca rakamlardan oluşmalı",
                        "Application ID must contain only digits"
                    ));
                }
                self.presence.set_id(val.to_string());
                match crate::config::set("discord_client_id", val) {
                    Ok(()) => self.info(t!(
                        "Discord uygulama kimliği kaydedildi",
                        "Discord application ID saved"
                    )),
                    Err(e) => self.error(tf!("Ayar yazılamadı: {}", "Could not write config: {}", e)),
                }
            }
            _ => {
                let st = self.presence.status();
                let on = if self.presence.enabled {
                    t!("açık", "on")
                } else {
                    t!("kapalı", "off")
                };
                if self.presence.client_id.is_empty() {
                    self.info(t!(
                        "Discord: kimlik yok — :discord id <APPLICATION_ID> ile ayarlayın",
                        "Discord: no ID — set it with :discord id <APPLICATION_ID>"
                    ));
                } else {
                    self.info(format!("Discord: {} ({})", on, st));
                }
            }
        }
    }

    fn set_option(&mut self, arg: &str) {
        match arg {
            "nu" | "number" => self.numbers = true,
            "nonu" | "nonumber" => self.numbers = false,
            "rnu" | "relativenumber" => {
                self.relative = true;
                self.numbers = true;
            }
            "nornu" | "norelativenumber" => self.relative = false,
            "et" | "expandtab" => self.buf_mut().use_tabs = false,
            "noet" | "noexpandtab" => self.buf_mut().use_tabs = true,
            "crlf" => {
                self.buf_mut().crlf = true;
                self.buf_mut().touch();
            }
            "clip" | "clipboard" => {
                self.clip_on = true;
                if self.clip.is_none() {
                    self.clip = arboard::Clipboard::new().ok();
                }
                if self.clip.is_none() {
                    self.error(t!("Sistem panosuna erişilemiyor", "System clipboard is not available"));
                }
            }
            "noclip" | "noclipboard" => self.clip_on = false,
            "lf" => {
                self.buf_mut().crlf = false;
                self.buf_mut().touch();
            }
            _ => self.error(tf!("Bilinmeyen seçenek: {}", "Unknown option: {}", arg)),
        }
    }

    fn open_help(&mut self) {
        if let Some(i) = self.bufs.iter().position(|b| b.name == HELP_NAME) {
            self.cur = i;
            return;
        }
        let b = Buffer::from_text(HELP_NAME, help_text());
        if self.is_pristine(self.cur) {
            self.bufs[self.cur] = b;
        } else {
            self.bufs.push(b);
            self.cur = self.bufs.len() - 1;
        }
    }

    fn reload(&mut self) {
        let Some(p) = self.buf().path.clone() else {
            return self.error(t!("Dosya adı yok", "No file name"));
        };
        match Buffer::open(&p) {
            Ok(mut nb) => {
                let b = self.buf();
                nb.cy = b.cy;
                nb.cx = b.cx;
                nb.scroll_y = b.scroll_y;
                self.lsp_close_buf(self.cur);
                self.bufs[self.cur] = nb;
                self.info(t!("Dosya diskten yeniden yüklendi", "Reloaded from disk"));
            }
            Err(e) => self.error(e.to_string()),
        }
    }

    pub fn save(&mut self, path: Option<&str>) -> bool {
        if let Some(p) = path {
            let abs = absolute(Path::new(p));
            self.lsp_close_buf(self.cur);
            let b = self.buf_mut();
            b.name = abs
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            b.lang = highlight::detect(&abs);
            b.path = Some(abs);
        }
        if self.buf().path.is_none() {
            self.error(t!(
                "Dosya adı yok — :w <dosya> kullanın",
                "No file name — use :w <file>"
            ));
            return false;
        }
        match self.buf_mut().save() {
            Ok(bytes) => {
                let name = self.rel_path(self.buf());
                let n = self.buf().lines.len();
                self.info(tf!("\"{}\" {}S {}B yazıldı", "\"{}\" {}L {}B written", name, n, bytes));
                self.tree.refresh();
                self.lsp_did_save();
                true
            }
            Err(e) => {
                self.error(tf!("Yazılamadı: {}", "Write failed: {}", e));
                false
            }
        }
    }

    fn save_all(&mut self) {
        let mut n = 0;
        let mut errs = Vec::new();
        for b in self.bufs.iter_mut() {
            if b.dirty && b.path.is_some() {
                match b.save() {
                    Ok(_) => n += 1,
                    Err(e) => errs.push(format!("{}: {}", b.name, e)),
                }
            }
        }
        if errs.is_empty() {
            self.info(tf!("{} dosya yazıldı", "{} files written", n));
        } else {
            self.error(errs.join("; "));
        }
        self.tree.refresh();
    }

    fn close_buffer(&mut self, force: bool, quit_if_last: bool) {
        if self.buf().dirty && !force && !self.is_scratch_output() {
            return self.error(t!(
                "Kaydedilmemiş değişiklikler var (zorlamak için ! ekleyin)",
                "Unsaved changes (add ! to override)"
            ));
        }
        if self.bufs.len() == 1 {
            if quit_if_last {
                self.quit = true;
            } else {
                self.lsp_close_buf(0);
                self.bufs[0] = Buffer::scratch(NONAME);
            }
            return;
        }
        self.lsp_close_buf(self.cur);
        self.bufs.remove(self.cur);
        if self.cur >= self.bufs.len() {
            self.cur = self.bufs.len() - 1;
        }
        self.mode = Mode::Normal;
    }

    fn is_scratch_output(&self) -> bool {
        let b = self.buf();
        b.path.is_none() && (b.name.starts_with("[!") || b.name == HELP_NAME)
    }

    fn quit_all(&mut self, force: bool) {
        if !force {
            if let Some(i) = self
                .bufs
                .iter()
                .position(|b| b.dirty && (b.path.is_some() || b.name == NONAME))
            {
                self.cur = i;
                let name = self.bufs[i].name.clone();
                return self.error(tf!(
                    "\"{}\" kaydedilmedi (:qa! ile zorla)",
                    "\"{}\" is not saved (:qa! to force)",
                    name
                ));
            }
        }
        self.quit = true;
    }

    fn cycle_buf(&mut self, d: isize) {
        let n = self.bufs.len() as isize;
        self.cur = ((self.cur as isize + d).rem_euclid(n)) as usize;
        self.mode = Mode::Normal;
    }

    fn run_shell(&mut self, cmd: &str) {
        if cmd.is_empty() {
            return self.error(t!("Kullanım: :!komut", "Usage: :!command"));
        }
        let file = self
            .buf()
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let full = cmd.replace('%', &format!("\"{}\"", file));
        let mut c = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", &full]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-c", &full]);
            c
        };
        c.current_dir(&self.tree.root).stdin(Stdio::null());
        match c.output() {
            Ok(o) => {
                let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
                let err = String::from_utf8_lossy(&o.stderr);
                if !err.is_empty() {
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text += &err;
                }
                let code = o.status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into());
                let mut short: String = cmd.chars().take(24).collect();
                if cmd.chars().count() > 24 {
                    short.push('…');
                }
                let name = format!("[!{}]", short);
                let b = Buffer::from_text(&name, text.trim_end_matches('\n'));
                if let Some(i) = self.bufs.iter().position(|b| b.name == name) {
                    self.bufs[i] = b;
                    self.cur = i;
                } else if self.is_pristine(self.cur) {
                    self.bufs[self.cur] = b;
                } else {
                    self.bufs.push(b);
                    self.cur = self.bufs.len() - 1;
                }
                let msg = tf!("!{} → çıkış kodu {}", "!{} → exit code {}", cmd, code);
                if o.status.success() {
                    self.info(msg);
                } else {
                    self.error(msg);
                }
                self.tree.refresh();
            }
            Err(e) => self.error(tf!("Çalıştırılamadı: {}", "Failed to run: {}", e)),
        }
    }

    fn substitute(&mut self, s: &str) {
        let (all, rest) = match s.strip_prefix('%') {
            Some(r) => (true, r),
            None => (false, s),
        };
        let rest = &rest[2..];
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() < 2 || parts[0].is_empty() {
            return self.error(t!(
                "Kullanım: :s/aranan/yeni/g  veya  :%s/aranan/yeni/g",
                "Usage: :s/old/new/g  or  :%s/old/new/g"
            ));
        }
        let pat: Vec<char> = parts[0].chars().collect();
        let rep: Vec<char> = parts[1].chars().collect();
        let global = parts.get(2).is_some_and(|f| f.contains('g'));
        let b = self.buf_mut();
        b.checkpoint();
        let (y0, y1) = if all { (0, b.last()) } else { (b.cy, b.cy) };
        let mut count = 0;
        for y in y0..=y1 {
            let out = {
                let line = &b.lines[y];
                let mut out = Vec::with_capacity(line.len());
                let mut i = 0;
                let mut done = false;
                while i < line.len() {
                    if !done && line[i..].starts_with(&pat) {
                        out.extend(&rep);
                        i += pat.len();
                        count += 1;
                        if !global {
                            done = true;
                        }
                    } else {
                        out.push(line[i]);
                        i += 1;
                    }
                }
                out
            };
            b.lines[y] = out;
        }
        if count == 0 {
            b.drop_noop_checkpoint();
            let p: String = pat.iter().collect();
            self.error(tf!("Bulunamadı: {}", "Not found: {}", p));
        } else {
            b.touch();
            self.info(tf!("{} değişiklik yapıldı", "{} substitutions", count));
        }
    }

    fn do_search(&mut self, s: &str) {
        if !s.is_empty() {
            self.search = Some(Matcher::smart(s, false));
        }
        self.find_next(true);
    }

    fn search_word(&mut self) {
        let b = self.buf();
        let line = &b.lines[b.cy];
        if b.cx >= line.len() || cls(line[b.cx]) != 1 {
            return;
        }
        let mut s = b.cx;
        while s > 0 && cls(line[s - 1]) == 1 {
            s -= 1;
        }
        let mut e = b.cx;
        while e < line.len() && cls(line[e]) == 1 {
            e += 1;
        }
        let word: String = line[s..e].iter().collect();
        self.search = Some(Matcher::new(&word, true, true, false).unwrap_or_else(|_| Matcher::smart(&word, true)));
        self.find_next(true);
    }

    fn find_next(&mut self, fwd: bool) {
        let Some(m) = self.search.clone() else {
            return self.error(t!("Arama deseni yok", "No previous search pattern"));
        };
        self.hl_search = true;
        let b = self.buf();
        let n = b.lines.len();
        let (cx, cy) = (b.cx, b.cy);
        let mut found = None;
        for i in 0..=n {
            let y = if fwd { (cy + i) % n } else { (cy + n - i % n) % n };
            let ms: Vec<usize> = m.find_line(&b.lines[y]).into_iter().map(|r| r.0).collect();
            let pick = if fwd {
                if i == 0 {
                    ms.iter().find(|&&x| x > cx)
                } else if i == n {
                    ms.iter().find(|&&x| x <= cx)
                } else {
                    ms.first()
                }
            } else if i == 0 {
                ms.iter().rev().find(|&&x| x < cx)
            } else if i == n {
                ms.iter().rev().find(|&&x| x >= cx)
            } else {
                ms.last()
            };
            if let Some(&x) = pick {
                found = Some((x, y));
                break;
            }
        }
        let p = m.pattern.clone();
        match found {
            Some((x, y)) => {
                let wrapped = if fwd { (y, x) <= (cy, cx) } else { (y, x) >= (cy, cx) };
                let b = self.buf_mut();
                b.cx = x;
                b.cy = y;
                b.sync_want();
                if wrapped {
                    self.info(tf!("/{}  (başa sarıldı)", "/{}  (wrapped)", p));
                } else {
                    self.info(format!("/{}", p));
                }
            }
            None => self.error(tf!("Bulunamadı: {}", "Not found: {}", p)),
        }
    }

    fn pos_from_mouse(&self, x: u16, y: u16, l: &Layout) -> Pos {
        let b = self.buf();
        let ly = if y < l.top {
            b.scroll_y.saturating_sub(1)
        } else {
            b.scroll_y + (y - l.top) as usize
        };
        let ly = ly.min(b.last());
        let dcol = b.scroll_x + x.saturating_sub(l.text_x) as usize;
        (idx_at_col(&b.lines[ly], dcol), ly)
    }

    fn in_rect(r: Option<(u16, u16, u16, u16)>, x: u16, y: u16) -> Option<(u16, u16)> {
        let (rx, ry, rw, rh) = r?;
        (x >= rx && x < rx + rw && y >= ry && y < ry + rh).then(|| (x - rx, y - ry))
    }

    fn mouse_overlays(&mut self, m: &MouseEvent) -> bool {
        let (x, y) = (m.column, m.row);
        let is_down = matches!(m.kind, MouseEventKind::Down(_));
        let wheel = match m.kind {
            MouseEventKind::ScrollDown => 1,
            MouseEventKind::ScrollUp => -1,
            _ => 0,
        };
        if self.menu.is_some() {
            let hit = Self::in_rect(self.menu_rect.get(), x, y);
            if wheel != 0 {
                if let Some(mm) = self.menu.as_mut() {
                    let n = mm.items.len();
                    mm.sel = (mm.sel as isize + wheel).clamp(0, n as isize - 1) as usize;
                }
                self.menu_follow();
                return true;
            }
            if is_down {
                match hit {
                    Some((_, ry)) if ry >= 1 => {
                        let i = self.menu.as_ref().map_or(0, |mm| mm.scroll) + ry as usize - 1;
                        if i < self.menu.as_ref().map_or(0, |mm| mm.items.len()) {
                            self.menu_choose(i);
                        }
                    }
                    Some(_) => {}
                    None => self.menu = None,
                }
                return true;
            }
            return matches!(
                m.kind,
                MouseEventKind::Moved | MouseEventKind::Up(_) | MouseEventKind::Drag(_)
            );
        }
        if self.popup.is_some() {
            let hit = Self::in_rect(self.popup_rect.get(), x, y);
            if let Some((_, ry)) = hit {
                if wheel != 0 {
                    self.popup_move(wheel);
                    return true;
                }
                if is_down {
                    let i = self.popup.as_ref().map_or(0, |p| p.scroll) + ry as usize;
                    if let Some(p) = self.popup.as_mut() {
                        if i < p.shown.len() {
                            p.sel = i;
                        }
                    }
                    self.accept_completion();
                    return true;
                }
            } else if is_down {
                self.popup = None;
            }
        }
        false
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        if matches!(
            m.kind,
            MouseEventKind::Down(_) | MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
        ) {
            self.hover = None;
        }
        if self.mouse_overlays(&m) {
            return;
        }
        let l = self.layout();
        if self.flow_mouse(&m, &l) {
            return;
        }
        let (x, y) = (m.column, m.row);
        let in_text_rows = y >= l.top && y < l.top + l.text_h;
        let over_tree = self.tree.visible && x < l.tree_w && in_text_rows;
        let over_ai = self.ai.visible && x >= l.ai_x && in_text_rows;
        let over_dbg = l.dbg_h > 0 && y >= l.dbg_y && y < l.dbg_y + l.dbg_h;
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) if over_dbg => {
                if matches!(self.mode, Mode::Command | Mode::Search) {
                    self.mode = Mode::Normal;
                }
                let now = Instant::now();
                let double = matches!(self.last_click, Some((t, lx, ly)) if now.duration_since(t).as_millis() < 400 && lx == x && ly == y);
                self.last_click = Some((now, x, y));
                if !self.dbg_click(x, y, double) && y == l.dbg_y {
                    self.drag = Drag::DbgSep;
                }
                return;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.drag == Drag::DbgSep => {
                let total = self.h.saturating_sub(3);
                self.dbg.height = (total + 1).saturating_sub(y).clamp(6, total.saturating_sub(6).max(6));
                return;
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown if over_dbg => {
                let d: isize = if m.kind == MouseEventKind::ScrollDown { 3 } else { -3 };
                self.dbg_scroll(x, d);
                return;
            }
            MouseEventKind::Down(MouseButton::Left)
                if in_text_rows && x >= l.ed_x && x < l.text_x && !over_ai && !self.is_splash() =>
            {
                let ly = self.buf().scroll_y + (y - l.top) as usize;
                let on_find = self.find.is_some() && self.find_click(x, y);
                if !on_find && ly < self.buf().lines.len() {
                    self.toggle_breakpoint(ly);
                }
                return;
            }
            MouseEventKind::Down(MouseButton::Left) if over_ai => {
                if matches!(self.mode, Mode::Command | Mode::Search) {
                    self.mode = Mode::Normal;
                }
                if x == l.ai_x {
                    self.drag = Drag::AiSep;
                } else {
                    self.back_to_normal();
                    self.ai_click(x, y);
                }
                return;
            }
            MouseEventKind::ScrollUp if over_ai => {
                self.ai.scroll += 3;
                return;
            }
            MouseEventKind::ScrollDown if over_ai => {
                self.ai.scroll = self.ai.scroll.saturating_sub(3);
                return;
            }
            MouseEventKind::Down(_) if over_ai => return,
            MouseEventKind::Drag(MouseButton::Left) if self.drag == Drag::AiSep => {
                let max = self.w.saturating_sub(l.ed_x + 31).max(24);
                self.ai.width = self.w.saturating_sub(x + 1).clamp(24.min(max), max);
                return;
            }
            MouseEventKind::Up(_) if self.drag == Drag::AiSep => {
                self.drag = Drag::None;
                self.ai.save_width();
                return;
            }
            _ => {}
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let now = Instant::now();
                let same = matches!(self.last_click, Some((t, lx, ly)) if now.duration_since(t).as_millis() < 400 && lx == x && ly == y);
                self.clicks = if same { self.clicks % 3 + 1 } else { 1 };
                self.last_click = Some((now, x, y));

                if matches!(self.mode, Mode::Command | Mode::Search) {
                    self.mode = Mode::Normal;
                }
                if y == 0 {
                    if let Some(&(_, _, i)) = self.tab_spans.iter().find(|(a, b, _)| x >= *a && x < *b) {
                        self.back_to_normal();
                        self.cur = i;
                        self.focus = Focus::Editor;
                    }
                    return;
                }
                if !in_text_rows {
                    return;
                }
                if self.tree.visible && x == l.tree_w {
                    self.drag = Drag::Sep;
                    return;
                }
                if over_tree {
                    self.back_to_normal();
                    self.focus = Focus::Tree;
                    let row = (y - l.top) as usize;
                    if row == 0 {
                        return;
                    }
                    let i = self.tree.scroll + row - 1;
                    if i < self.tree.nodes.len() {
                        self.tree.sel = i;
                        self.tree_activate(i);
                    }
                    return;
                }
                if self.find.is_some() && self.find_click(x, y) {
                    return;
                }
                if x >= l.ed_x && x < l.ed_r && self.is_splash() {
                    let hit = self.splash_hits.borrow().iter().find(|(hy, _)| *hy == y).map(|h| h.1);
                    match hit {
                        Some(SPLASH_OPEN) => {
                            let d = self.tree.root.clone();
                            self.folder_picker(d, None);
                        }
                        Some(SPLASH_HELP) => self.open_help(),
                        Some(i) => {
                            if let Some(p) = self.recent.get(i).cloned() {
                                self.open_path(&p);
                            }
                        }
                        None => {}
                    }
                    return;
                }
                if x >= l.ed_x && x < l.ed_r {
                    self.focus = Focus::Editor;
                    if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
                        self.mode = Mode::Normal;
                    }
                    self.reset_pending();
                    let (cx, cy) = self.pos_from_mouse(x, y, &l);
                    self.drag = Drag::Text;
                    self.drag_anchor = (cx, cy);
                    let word = {
                        let line = &self.buf().lines[cy];
                        if cx < line.len() {
                            let k = cls(line[cx]);
                            let mut s = cx;
                            while s > 0 && cls(line[s - 1]) == k {
                                s -= 1;
                            }
                            let mut e = cx;
                            while e + 1 < line.len() && cls(line[e + 1]) == k {
                                e += 1;
                            }
                            Some((s, e))
                        } else {
                            None
                        }
                    };
                    if self.clicks >= 2 && self.mode == Mode::Insert {
                        self.leave_insert();
                    }
                    let b = self.buf_mut();
                    b.cx = cx;
                    b.cy = cy;
                    match (self.clicks, word) {
                        (2, Some((s, e))) => {
                            self.buf_mut().cx = e;
                            self.anchor = (s, cy);
                            self.drag_anchor = (s, cy);
                            self.mode = Mode::Visual;
                        }
                        (3, _) => {
                            self.anchor = (0, cy);
                            self.mode = Mode::VisualLine;
                        }
                        _ => {}
                    }
                    self.buf_mut().sync_want();
                    if m.modifiers.contains(KeyModifiers::CONTROL) && self.clicks == 1 {
                        self.drag = Drag::None;
                        self.lsp_definition();
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if in_text_rows && x >= l.ed_x && !over_tree {
                    self.focus = Focus::Editor;
                    if matches!(self.mode, Mode::Command | Mode::Search) {
                        self.mode = Mode::Normal;
                    }
                    if !matches!(self.mode, Mode::Visual | Mode::VisualLine) {
                        if self.mode == Mode::Insert {
                            self.leave_insert();
                        }
                        let (cx, cy) = self.pos_from_mouse(x, y, &l);
                        let b = self.buf_mut();
                        b.cx = cx;
                        b.cy = cy;
                        b.sync_want();
                    }
                    self.open_context_menu(x, y + 1);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => match self.drag {
                Drag::Sep => {
                    let max = self.w.saturating_sub(20).max(12);
                    self.tree.width = x.clamp(12, max);
                }
                Drag::Text => {
                    let p = self.pos_from_mouse(x, y, &l);
                    let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
                    let b = self.buf();
                    if !visual && p == (b.cx, b.cy) {
                        return;
                    }
                    if !visual {
                        if self.mode == Mode::Insert {
                            self.leave_insert();
                        }
                        self.anchor = self.drag_anchor;
                        self.mode = Mode::Visual;
                    }
                    let b = self.buf_mut();
                    b.cx = p.0;
                    b.cy = p.1;
                    b.sync_want();
                }
                Drag::None | Drag::AiSep | Drag::DbgSep | Drag::FlowSep | Drag::FlowPan => {}
            },
            MouseEventKind::Up(_) => self.drag = Drag::None,
            MouseEventKind::Down(MouseButton::Middle) => {
                if y == 0 {
                    if let Some(&(_, _, i)) = self.tab_spans.iter().find(|(a, b, _)| x >= *a && x < *b) {
                        self.back_to_normal();
                        self.cur = i;
                        self.close_buffer(false, false);
                    }
                }
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let d: isize = if m.kind == MouseEventKind::ScrollDown { 3 } else { -3 };
                if over_tree {
                    let max = self.tree.nodes.len().saturating_sub(1) as isize;
                    self.tree.scroll = (self.tree.scroll as isize + d).clamp(0, max.max(0)) as usize;
                } else {
                    self.wheel(d, &l);
                }
            }
            MouseEventKind::ScrollRight | MouseEventKind::ScrollLeft => {
                let b = self.buf_mut();
                if m.kind == MouseEventKind::ScrollRight {
                    b.scroll_x += 4;
                } else {
                    b.scroll_x = b.scroll_x.saturating_sub(4);
                }
                let col = disp_col(&b.lines[b.cy], b.cx);
                let w = l.text_w as usize;
                if col < b.scroll_x || col >= b.scroll_x + w {
                    let target = if col < b.scroll_x {
                        b.scroll_x
                    } else {
                        b.scroll_x + w - 1
                    };
                    b.cx = idx_at_col(&b.lines[b.cy], target);
                }
            }
            _ => {}
        }
    }

    fn wheel(&mut self, d: isize, l: &Layout) {
        let h = l.text_h as usize;
        let b = self.buf_mut();
        let max = b.last() as isize;
        b.scroll_y = (b.scroll_y as isize + d).clamp(0, max) as usize;
        if b.cy < b.scroll_y {
            b.cy = b.scroll_y;
        } else if b.cy >= b.scroll_y + h {
            b.cy = b.scroll_y + h - 1;
        }
        b.cy = b.cy.min(b.last());
        b.cx = idx_at_col(&b.lines[b.cy], b.want_col);
    }
}
