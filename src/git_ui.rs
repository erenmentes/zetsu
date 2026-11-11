use super::*;
use crate::buffer::char_width;
use crate::git::{self, Blame, Commit, Conflict, Hunk, Status};
use crate::render::{put_clip, text_w, BRIGHT, DIM, ERR, LOGO_GREEN, MACRO, POP, SEL, STR, TYPE};
use crate::screen::{Screen, Style};
use crossterm::style::Color;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

pub(crate) const GIT_ADD: Color = STR;
pub(crate) const GIT_MOD: Color = Color::Rgb {
    r: 0x61,
    g: 0xaf,
    b: 0xef,
};
pub(crate) const GIT_DEL: Color = ERR;
pub(crate) const OURS_BG: Color = Color::Rgb {
    r: 0x1f,
    g: 0x36,
    b: 0x2a,
};
pub(crate) const THEIRS_BG: Color = Color::Rgb {
    r: 0x1f,
    g: 0x2c,
    b: 0x45,
};
pub(crate) const MARK_BG: Color = Color::Rgb {
    r: 0x33,
    g: 0x3a,
    b: 0x48,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GitHit {
    TabFiles,
    TabGit,
    Branch,
    Msg,
    Commit,
    Ai,
    Push,
    Pull,
    Fetch,
    Row(usize),
}

#[derive(Clone, Debug)]
enum Row {
    Head(String, usize),
    File(usize, bool),
    Commit(usize),
    Note(String),
}

struct BaseInfo {
    lines: Option<Vec<String>>,
    ends_nl: bool,
    gen: u64,
}

type OpJob = (String, Receiver<Result<String, String>>);
type BaseJob = (String, u64, Receiver<Option<(Vec<String>, bool)>>);
type BlameKey = (String, usize, u64);
type HunkCache = HashMap<String, (u64, u64, Rc<Vec<Hunk>>)>;
type ConfCache = Option<(usize, u64, Rc<Vec<Conflict>>)>;
type TreeColors = Option<(u64, Rc<HashMap<String, Color>>)>;
type DiffMap = (PathBuf, Vec<Option<(String, usize)>>);
pub(crate) type ConfHit = (u16, u16, u16, usize, u8);

pub struct GitState {
    pub tab: bool,
    pub root: Option<PathBuf>,
    root_src: Option<PathBuf>,
    pub status: Option<Status>,
    status_job: Option<Receiver<Result<Status, String>>>,
    status_at: Option<Instant>,
    status_now: bool,
    gen: u64,
    bases: HashMap<String, BaseInfo>,
    base_jobs: Vec<BaseJob>,
    hunk_cache: RefCell<HunkCache>,
    conf_cache: RefCell<ConfCache>,
    tree_cache: RefCell<TreeColors>,
    pub blame_on: bool,
    blame: Option<(BlameKey, Result<Blame, String>)>,
    blame_job: Option<(BlameKey, Receiver<Result<Blame, String>>)>,
    blame_want: Option<(BlameKey, Instant)>,
    pub sel: usize,
    scroll: usize,
    pub msg: Vec<char>,
    msg_x: usize,
    pub editing: bool,
    op: Option<OpJob>,
    out: Option<(String, bool)>,
    log: Vec<Commit>,
    log_job: Option<Receiver<Result<Vec<Commit>, String>>>,
    log_gen: u64,
    ai_job: Option<Receiver<Result<String, String>>>,
    discard: Option<(usize, Instant)>,
    diff_maps: HashMap<String, DiffMap>,
    pub hits: RefCell<Vec<(u16, u16, u16, GitHit)>>,
    pub conf_hits: RefCell<Vec<ConfHit>>,
    pub cursor: Cell<Option<(u16, u16)>>,
    available: bool,
}

impl GitState {
    pub fn new() -> Self {
        GitState {
            tab: false,
            root: None,
            root_src: None,
            status: None,
            status_job: None,
            status_at: None,
            status_now: true,
            gen: 0,
            bases: HashMap::new(),
            base_jobs: Vec::new(),
            hunk_cache: RefCell::new(HashMap::new()),
            conf_cache: RefCell::new(None),
            tree_cache: RefCell::new(None),
            blame_on: crate::config::load().get("git_blame").is_none_or(|v| v != "off"),
            blame: None,
            blame_job: None,
            blame_want: None,
            sel: 0,
            scroll: 0,
            msg: Vec::new(),
            msg_x: 0,
            editing: false,
            op: None,
            out: None,
            log: Vec::new(),
            log_job: None,
            log_gen: u64::MAX,
            ai_job: None,
            discard: None,
            diff_maps: HashMap::new(),
            hits: RefCell::new(Vec::new()),
            conf_hits: RefCell::new(Vec::new()),
            cursor: Cell::new(None),
            available: git::available(),
        }
    }

    fn reset(&mut self) {
        let keep_blame = self.blame_on;
        let avail = self.available;
        *self = GitState::new();
        self.blame_on = keep_blame;
        self.available = avail;
    }

    fn rows(&self) -> Vec<Row> {
        let mut v = Vec::new();
        let Some(st) = &self.status else { return v };
        let conf: Vec<usize> = (0..st.entries.len()).filter(|i| st.entries[*i].conflict()).collect();
        let staged: Vec<usize> = (0..st.entries.len()).filter(|i| st.entries[*i].staged()).collect();
        let changed: Vec<usize> = (0..st.entries.len()).filter(|i| st.entries[*i].unstaged()).collect();
        if !conf.is_empty() {
            v.push(Row::Head(t!("ÇAKIŞMALAR", "CONFLICTS").into(), conf.len()));
            v.extend(conf.into_iter().map(|i| Row::File(i, false)));
        }
        if !staged.is_empty() {
            v.push(Row::Head(t!("HAZIR (STAGED)", "STAGED").into(), staged.len()));
            v.extend(staged.into_iter().map(|i| Row::File(i, true)));
        }
        v.push(Row::Head(t!("DEĞİŞİKLİKLER", "CHANGES").into(), changed.len()));
        if changed.is_empty() {
            v.push(Row::Note(t!("Değişiklik yok", "No changes").into()));
        }
        v.extend(changed.into_iter().map(|i| Row::File(i, false)));
        v.push(Row::Head(t!("GEÇMİŞ", "HISTORY").into(), self.log.len()));
        v.extend((0..self.log.len()).map(Row::Commit));
        v
    }

    pub fn status_now_set(&mut self) {
        self.status_now = true;
    }

    pub fn change_count(&self) -> usize {
        self.status.as_ref().map_or(0, |s| s.entries.len())
    }
}

fn chars_to_lines(lines: &[Vec<char>]) -> Vec<String> {
    lines.iter().map(|l| l.iter().collect()).collect()
}

fn clean_ai_message(s: &str) -> String {
    let t = s.trim();
    let t = t
        .strip_prefix("```")
        .map(|r| r.split_once('\n').map_or("", |x| x.1))
        .unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t);
    t.trim().trim_matches('"').trim().to_string()
}

impl Editor {
    fn git_root(&self) -> Option<PathBuf> {
        self.git.root.clone()
    }

    fn git_buf_rel(&self, bi: usize) -> Option<(PathBuf, String)> {
        let root = self.git.root.as_ref()?;
        let p = self.bufs.get(bi)?.path.as_ref()?;
        git::rel(root, &absolute(p)).map(|r| (root.clone(), r))
    }

    fn git_refresh(&mut self) {
        self.git.status_now = true;
        self.git.gen += 1;
    }

    pub fn git_tick(&mut self) -> bool {
        if !self.git.available {
            return false;
        }
        let mut changed = false;
        let src = self.tree.root.clone();
        if self.git.root_src.as_ref() != Some(&src) {
            self.git.reset();
            self.git.root = git::find_root(&src);
            self.git.root_src = Some(src);
            changed = true;
        }
        let Some(root) = self.git.root.clone() else {
            return changed;
        };
        if let Some(rx) = &self.git.status_job {
            if let Ok(r) = rx.try_recv() {
                self.git.status_job = None;
                self.git.status_at = Some(Instant::now());
                if let Ok(st) = r {
                    if self.git.status.as_ref() != Some(&st) {
                        self.git.status = Some(st);
                        self.git.gen += 1;
                        changed = true;
                    }
                }
            }
        }
        let side = self.tree.visible && self.git.tab;
        let every = if side { 2 } else { 5 };
        if self.git.status_job.is_none()
            && (self.git.status_now || self.git.status_at.is_none_or(|t| t.elapsed().as_secs() >= every))
        {
            self.git.status_now = false;
            let r = root.clone();
            self.git.status_job = Some(git::spawn(move || git::status(&r)));
        }
        if self.git.status.is_some() {
            changed |= self.git_bases(&root);
        }
        changed |= self.git_blame_tick(&root);
        if let Some((label, rx)) = &self.git.op {
            if let Ok(r) = rx.try_recv() {
                let label = label.clone();
                self.git.op = None;
                match r {
                    Ok(o) => {
                        let line = o
                            .lines()
                            .rev()
                            .find(|l| !l.trim().is_empty())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let text = if line.is_empty() {
                            format!("✓ {}", label)
                        } else {
                            format!("✓ {}: {}", label, line)
                        };
                        self.git.out = Some((text, false));
                    }
                    Err(e) => {
                        let line = e
                            .lines()
                            .find(|l| !l.trim().is_empty())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        self.git.out = Some((format!("✖ {}: {}", label, line), true));
                        self.error(format!("git {}: {}", label, line));
                    }
                }
                self.git_refresh();
                self.git.log_gen = u64::MAX;
                if ["switch", "pull", "restore", "clean"]
                    .iter()
                    .any(|p| label.starts_with(p))
                {
                    self.git_reload_after_checkout();
                }
                changed = true;
            }
        }
        if side && self.git.log_job.is_none() && self.git.log_gen != self.git.gen {
            self.git.log_gen = self.git.gen;
            let r = root.clone();
            self.git.log_job = Some(git::spawn(move || git::log(&r, 100)));
        }
        if let Some(rx) = &self.git.log_job {
            if let Ok(r) = rx.try_recv() {
                self.git.log_job = None;
                self.git.log = r.unwrap_or_default();
                changed = true;
            }
        }
        if let Some(rx) = &self.git.ai_job {
            if let Ok(r) = rx.try_recv() {
                self.git.ai_job = None;
                match r {
                    Ok(m) => {
                        self.git.msg = clean_ai_message(&m).chars().collect();
                        self.git.msg_x = self.git.msg.len();
                        self.git.out = Some((t!("✦ mesaj önerildi", "✦ message suggested").into(), false));
                    }
                    Err(e) => self.git.out = Some((format!("✖ AI: {}", e.lines().next().unwrap_or("")), true)),
                }
                changed = true;
            }
        }
        changed
    }

    fn git_bases(&mut self, root: &Path) -> bool {
        let mut changed = false;
        let mut i = 0;
        while i < self.git.base_jobs.len() {
            if let Ok(r) = self.git.base_jobs[i].2.try_recv() {
                let (key, gen, _) = self.git.base_jobs.remove(i);
                let (lines, ends_nl) = match r {
                    Some((l, e)) => (Some(l), e),
                    None => (None, true),
                };
                self.git.bases.insert(key, BaseInfo { lines, ends_nl, gen });
                changed = true;
            } else {
                i += 1;
            }
        }
        let gen = self.git.gen;
        let want: Vec<usize> = (0..self.bufs.len())
            .filter(|&b| b == self.cur || self.bufs[b].path.is_some())
            .collect();
        for bi in want {
            let Some(p) = self.bufs[bi].path.clone() else { continue };
            let key = path_key(&p);
            if self.git.bases.get(&key).is_some_and(|b| b.gen == gen) || self.git.base_jobs.iter().any(|j| j.0 == key) {
                continue;
            }
            if bi != self.cur && !self.git.bases.contains_key(&key) {
                continue;
            }
            let Some(rel) = git::rel(root, &absolute(&p)) else {
                self.git.bases.insert(
                    key,
                    BaseInfo {
                        lines: None,
                        ends_nl: true,
                        gen,
                    },
                );
                continue;
            };
            let untracked = self
                .git
                .status
                .as_ref()
                .is_some_and(|s| s.entries.iter().any(|e| e.untracked() && e.rel == rel));
            let r = root.to_path_buf();
            let rx = git::spawn(move || {
                if untracked {
                    return Some((Vec::new(), true));
                }
                git::base_text(&r, &rel).map(|t| (git::split_lines(&t), t.is_empty() || t.ends_with('\n')))
            });
            self.git.base_jobs.push((key, gen, rx));
        }
        changed
    }

    fn git_blame_tick(&mut self, root: &Path) -> bool {
        let mut changed = false;
        if let Some((k, rx)) = &self.git.blame_job {
            if let Ok(r) = rx.try_recv() {
                self.git.blame = Some((k.clone(), r));
                self.git.blame_job = None;
                changed = true;
            }
        }
        if !self.git.blame_on || self.focus != Focus::Editor || self.mode != Mode::Normal {
            return changed;
        }
        let b = self.buf();
        let Some(p) = b.path.as_ref() else { return changed };
        let key = path_key(p);
        let tracked = self
            .git
            .bases
            .get(&key)
            .is_some_and(|x| x.lines.as_ref().is_some_and(|l| !l.is_empty()));
        if !tracked {
            return changed;
        }
        let want = (key, b.cy, b.rev);
        if self.git.blame.as_ref().is_some_and(|(k, _)| *k == want)
            || self.git.blame_job.as_ref().is_some_and(|(k, _)| *k == want)
        {
            return changed;
        }
        match &self.git.blame_want {
            Some((k, t)) if *k == want => {
                if t.elapsed().as_millis() < 350 || self.git.blame_job.is_some() {
                    return changed;
                }
            }
            _ => {
                self.git.blame_want = Some((want, Instant::now()));
                return changed;
            }
        }
        let Some(rel) = git::rel(root, &absolute(p)) else {
            return changed;
        };
        let contents = b.text();
        let line = b.cy;
        let r = root.to_path_buf();
        self.git.blame_job = Some((want, git::spawn(move || git::blame(&r, &rel, line, &contents))));
        changed
    }

    pub(crate) fn git_hunks(&self, bi: usize) -> Option<Rc<Vec<Hunk>>> {
        let b = self.bufs.get(bi)?;
        let key = path_key(b.path.as_ref()?);
        let base = self.git.bases.get(&key)?;
        let lines = base.lines.as_ref()?;
        if let Some((rev, gen, h)) = self.git.hunk_cache.borrow().get(&key) {
            if *rev == b.rev && *gen == base.gen {
                return Some(h.clone());
            }
        }
        let cur = chars_to_lines(&b.lines);
        let cur = if cur.len() == 1 && cur[0].is_empty() && lines.is_empty() {
            Vec::new()
        } else {
            cur
        };
        let h = Rc::new(git::hunks(lines, &cur));
        self.git
            .hunk_cache
            .borrow_mut()
            .insert(key, (b.rev, base.gen, h.clone()));
        Some(h)
    }

    pub(crate) fn git_signs(&self, from: usize, count: usize) -> Vec<u8> {
        let mut v = vec![0u8; count];
        let Some(hs) = self.git_hunks(self.cur) else { return v };
        let mut set = |line: usize, s: u8| {
            if line >= from && line < from + count {
                let slot = &mut v[line - from];
                if *slot == 0 || s < 3 {
                    *slot = s;
                }
            }
        };
        for h in hs.iter() {
            if h.new_len > 0 {
                let s = if h.old_len == 0 { 1 } else { 2 };
                for l in h.new_start..h.new_start + h.new_len {
                    set(l, s);
                }
            } else if h.new_start == 0 {
                set(0, 4);
            } else {
                set(h.new_start - 1, 3);
            }
        }
        v
    }

    pub(crate) fn git_conflicts(&self) -> Rc<Vec<Conflict>> {
        let b = self.buf();
        if let Some((bi, rev, c)) = self.git.conf_cache.borrow().as_ref() {
            if *bi == self.cur && *rev == b.rev {
                return c.clone();
            }
        }
        let c = Rc::new(git::conflicts(&b.lines));
        *self.git.conf_cache.borrow_mut() = Some((self.cur, b.rev, c.clone()));
        c
    }

    pub(crate) fn git_line_bg(&self, cs: &[Conflict], ly: usize) -> Option<Color> {
        let c = cs.iter().find(|c| ly >= c.start && ly <= c.end)?;
        let ours_end = c.base.unwrap_or(c.mid);
        Some(if ly == c.start || ly == c.mid || ly == c.end || Some(ly) == c.base {
            MARK_BG
        } else if ly < ours_end {
            OURS_BG
        } else if ly > c.mid {
            THEIRS_BG
        } else {
            MARK_BG
        })
    }

    pub(crate) fn git_draw_conflict_buttons(&self, s: &mut Screen, x: u16, y: u16, right: u16, idx: usize) {
        let mut hits = self.git.conf_hits.borrow_mut();
        let mut cx = x + 2;
        for (k, label) in [
            (0u8, t!("✓ Bizimki", "✓ Accept ours")),
            (1u8, t!("✓ Gelen", "✓ Accept theirs")),
            (2u8, t!("✓ İkisi", "✓ Accept both")),
        ] {
            let w = text_w(label) as u16 + 2;
            if cx + w > right {
                break;
            }
            s.put(cx, y, &format!(" {} ", label), Style::fg(BRIGHT).on(SEL));
            hits.push((y, cx, cx + w, idx, k));
            cx += w + 1;
        }
    }

    pub(crate) fn git_blame_text(&self) -> Option<String> {
        if !self.git.blame_on || self.focus != Focus::Editor || self.mode != Mode::Normal {
            return None;
        }
        let b = self.buf();
        let key = path_key(b.path.as_ref()?);
        let ((k, line, rev), r) = self.git.blame.as_ref()?;
        if *k != key || *line != b.cy || *rev != b.rev {
            return None;
        }
        let bl = r.as_ref().ok()?;
        if bl.uncommitted {
            return Some(t!("Siz · henüz commit edilmedi", "You · not committed yet").to_string());
        }
        Some(format!("{}, {} · {}", bl.author, git::ago(bl.time), bl.summary))
    }

    pub(crate) fn git_tree_colors(&self) -> Option<Rc<HashMap<String, Color>>> {
        let st = self.git.status.as_ref()?;
        let root = self.git.root.as_ref()?;
        if let Some((g, m)) = self.git.tree_cache.borrow().as_ref() {
            if *g == self.git.gen {
                return Some(m.clone());
            }
        }
        let mut m: HashMap<String, Color> = HashMap::new();
        let mut dirs: HashSet<String> = HashSet::new();
        let rk = path_key(root);
        for e in &st.entries {
            let c = if e.conflict() {
                MACRO
            } else if e.untracked() || e.x == 'A' {
                GIT_ADD
            } else if e.x == 'D' || e.y == 'D' {
                GIT_DEL
            } else {
                TYPE
            };
            m.insert(path_key(&e.path), c);
            let mut p = e.path.parent();
            while let Some(d) = p {
                let k = path_key(d);
                if k.len() <= rk.len() {
                    break;
                }
                dirs.insert(k);
                p = d.parent();
            }
        }
        for d in dirs {
            m.entry(d).or_insert(Color::Rgb {
                r: 0xb8,
                g: 0xa0,
                b: 0x6a,
            });
        }
        let m = Rc::new(m);
        *self.git.tree_cache.borrow_mut() = Some((self.git.gen, m.clone()));
        Some(m)
    }

    pub(crate) fn git_status_label(&self) -> Option<String> {
        let st = self.git.status.as_ref()?;
        let mut s = format!("⎇ {}", st.branch);
        if st.ahead > 0 {
            s.push_str(&format!(" ↑{}", st.ahead));
        }
        if st.behind > 0 {
            s.push_str(&format!(" ↓{}", st.behind));
        }
        Some(s)
    }

    pub(crate) fn git_toggle(&mut self) {
        if !self.git.available {
            return self.error(t!("git bulunamadı (PATH)", "git was not found on PATH"));
        }
        if self.git.root.is_none() {
            return self.error(t!(
                "Bu klasör bir git deposu değil",
                "This folder is not a git repository"
            ));
        }
        if self.tree.visible && self.git.tab && self.focus == Focus::Tree {
            self.git.tab = false;
            self.git.editing = false;
            self.focus = Focus::Editor;
            return;
        }
        self.back_to_normal();
        self.tree.visible = true;
        self.git.tab = true;
        self.focus = Focus::Tree;
        self.git_refresh();
    }

    fn git_run(&mut self, label: &str, f: impl FnOnce(&Path) -> Result<String, String> + Send + 'static) {
        let Some(root) = self.git_root() else {
            return self.error(t!("Git deposu yok", "No git repository"));
        };
        if self.git.op.is_some() {
            return self.error(t!("Bir git işlemi zaten sürüyor", "A git operation is already running"));
        }
        self.git.out = Some((format!("… {}", label), false));
        self.git.op = Some((label.to_string(), git::spawn(move || f(&root))));
    }

    fn git_stage(&mut self, rel: String, stage: bool) {
        if stage {
            self.git_run("add", move |r| git::run(r, &["add", "--", &rel], None));
        } else {
            self.git_run("restore --staged", move |r| {
                git::run(r, &["restore", "--staged", "--", &rel], None)
                    .or_else(|_| git::run(r, &["rm", "--cached", "-q", "--", &rel], None))
            });
        }
    }

    fn git_commit(&mut self) {
        let msg: String = self.git.msg.iter().collect();
        if msg.trim().is_empty() {
            return self.error(t!(
                "Commit mesajı boş (a: AI ile öner)",
                "Commit message is empty (a: suggest with AI)"
            ));
        }
        let Some(st) = self.git.status.clone() else { return };
        let any_staged = st.entries.iter().any(|e| e.staged());
        if !any_staged && st.entries.is_empty() && !st.merging {
            return self.error(t!("Commit edilecek değişiklik yok", "Nothing to commit"));
        }
        if st.entries.iter().any(|e| e.conflict()) {
            return self.error(t!(
                "Önce çakışmaları çözüp stage'leyin",
                "Resolve and stage the conflicts first"
            ));
        }
        if self.bufs.iter().any(|b| b.dirty && b.path.is_some()) {
            self.info(t!("Kaydedilmemiş dosyalar var", "There are unsaved files"));
        }
        let stage_all = !any_staged && !st.merging;
        self.git.msg.clear();
        self.git.msg_x = 0;
        self.git.editing = false;
        self.git_run("commit", move |r| {
            if stage_all {
                git::run(r, &["add", "-A"], None)?;
            }
            git::run(r, &["commit", "-q", "-F", "-"], Some(&msg))?;
            git::run(r, &["log", "-1", "--format=%h %s"], None)
        });
    }

    fn git_push(&mut self) {
        let Some(st) = &self.git.status else { return };
        let has_up = st.upstream.is_some();
        self.git_run("push", move |r| {
            if has_up {
                return git::run(r, &["push"], None).map(|o| if o.trim().is_empty() { "ok".into() } else { o });
            }
            let rem = git::remotes(r);
            let name = if rem.iter().any(|x| x == "origin") {
                "origin".to_string()
            } else {
                rem.first().cloned().ok_or_else(|| "no remote configured".to_string())?
            };
            git::run(r, &["push", "-u", &name, "HEAD"], None).map(|o| if o.trim().is_empty() { "ok".into() } else { o })
        });
    }

    fn git_ai_message(&mut self) {
        let Some(cfg) = self.ai.cfg() else {
            return self.error(t!(
                "AI için önce Ctrl+L ile bir model seçin",
                "Pick a model with Ctrl+L first"
            ));
        };
        let Some(root) = self.git_root() else { return };
        if self.git.ai_job.is_some() {
            return;
        }
        self.git.out = Some((t!("✦ mesaj yazılıyor…", "✦ writing message…").into(), false));
        self.git.ai_job = Some(git::spawn(move || {
            let mut diff = git::run(&root, &["diff", "--cached"], None)?;
            if diff.trim().is_empty() {
                diff = git::run(&root, &["diff"], None)?;
                let untracked = git::run(&root, &["ls-files", "--others", "--exclude-standard"], None)?;
                if !untracked.trim().is_empty() {
                    diff.push_str("\nNew files:\n");
                    diff.push_str(&untracked);
                }
            }
            if diff.trim().is_empty() {
                return Err("no changes".into());
            }
            let diff: String = diff.chars().take(16_000).collect();
            let recent = git::run(&root, &["log", "-n8", "--format=%s"], None).unwrap_or_default();
            let system = "You write git commit messages. Reply with only the message, no quotes or markdown. \
                          First line: a concise imperative summary of at most 72 characters without a trailing period. \
                          Follow the style of the recent commit subjects you are given (for example whether they use \
                          prefixes like 'feat:'). Add a blank line and up to three short '- ' lines only when the change \
                          needs explanation. Write in English.";
            let user = format!("Recent commit subjects:\n{}\n\nDiff:\n{}", recent, diff);
            crate::ai::complete(&cfg, system, &user)
        }));
    }

    fn git_branch_menu(&mut self) {
        let Some(root) = self.git_root() else { return };
        let list = match git::branches(&root) {
            Ok(l) => l,
            Err(e) => return self.error(e),
        };
        let cur = self.git.status.as_ref().map(|s| s.branch.clone()).unwrap_or_default();
        let mut items = vec![(
            format!("+ {}", t!("Yeni branch…", "New branch…")),
            ide::MenuAction::Git("new".into()),
        )];
        for b in list {
            let mark = if b == cur { "● " } else { "  " };
            items.push((format!("{}{}", mark, b), ide::MenuAction::Git(format!("switch:{}", b))));
        }
        let sel = items.iter().position(|i| i.0.starts_with('●')).unwrap_or(0);
        self.menu = Some(ide::Menu::filterable(t!("Branch", "Branch").into(), items, sel, false));
    }

    pub(crate) fn git_menu_action(&mut self, a: &str) {
        if a == "new" {
            self.start_cmd(Mode::Command);
            self.cmd = "branch ".chars().collect();
            self.cmd_x = self.cmd.len();
            return;
        }
        if let Some(b) = a.strip_prefix("switch:") {
            let b = b.to_string();
            self.git_switch(b, false);
        }
    }

    fn git_switch(&mut self, b: String, create: bool) {
        if self.bufs.iter().any(|x| x.dirty && x.path.is_some()) {
            return self.error(t!(
                "Branch değiştirmeden önce dosyaları kaydedin",
                "Save your files before switching branches"
            ));
        }
        let label = format!("switch {}", b);
        self.git_run(&label, move |r| {
            if create {
                git::run(r, &["switch", "-c", &b], None)
            } else {
                git::run(r, &["switch", &b], None)
            }
            .map(|_| b.clone())
        });
        self.git.base_jobs.clear();
        self.git.bases.clear();
    }

    pub(crate) fn git_reload_after_checkout(&mut self) {
        for i in 0..self.bufs.len() {
            let Some(p) = self.bufs[i].path.clone() else { continue };
            if self.bufs[i].dirty {
                continue;
            }
            if let Ok(nb) = Buffer::open(&p) {
                if nb.lines != self.bufs[i].lines {
                    let (cy, cx) = (self.bufs[i].cy, self.bufs[i].cx);
                    self.bufs[i].lines = nb.lines;
                    self.bufs[i].rev += 1;
                    self.bufs[i].cy = cy.min(self.bufs[i].lines.len().saturating_sub(1));
                    self.bufs[i].cx = cx;
                    self.bufs[i].clamp(false);
                }
            }
        }
    }

    pub(crate) fn git_open_diff(&mut self, name: String, text: String) {
        if text.trim().is_empty() {
            return self.info(t!("Fark yok", "No differences"));
        }
        let Some(root) = self.git_root() else { return };
        let map = git::diff_targets(&text);
        if let Some(i) = self.bufs.iter().position(|b| b.name == name) {
            self.bufs[i].set_text(&text);
            self.bufs[i].cy = 0;
            self.bufs[i].cx = 0;
            self.bufs[i].scroll_y = 0;
            self.bufs[i].dirty = false;
            self.cur = i;
        } else {
            let mut b = Buffer::from_text(&name, &text);
            b.lang = crate::highlight::diff();
            if self.is_pristine(self.cur) {
                self.bufs[self.cur] = b;
            } else {
                self.bufs.push(b);
                self.cur = self.bufs.len() - 1;
            }
        }
        self.git.diff_maps.insert(name, (root, map));
        self.focus = Focus::Editor;
        self.mode = Mode::Normal;
    }

    pub(crate) fn git_diff_jump(&mut self) -> bool {
        let b = self.buf();
        let Some((root, map)) = self.git.diff_maps.get(&b.name) else {
            return false;
        };
        let Some(Some((rel, line))) = map.get(b.cy).cloned() else {
            return true;
        };
        let p = root.join(&rel);
        if !p.exists() {
            self.error(t!("Dosya artık yok", "File no longer exists"));
            return true;
        }
        self.push_jump();
        self.open_path(&p);
        let b = self.buf_mut();
        b.cy = line.min(b.last());
        b.cx = b.first_nb(b.cy);
        b.sync_want();
        true
    }

    fn git_file_diff(&mut self, idx: usize, staged: bool) {
        let Some(root) = self.git_root() else { return };
        let Some(e) = self.git.status.as_ref().and_then(|s| s.entries.get(idx)).cloned() else {
            return;
        };
        let name = format!("[± {}{}]", if staged { "staged " } else { "" }, e.rel);
        if staged {
            match git::run(&root, &["diff", "--cached", "--", &e.rel], None) {
                Ok(t) => self.git_open_diff(name, t),
                Err(er) => self.error(er),
            }
            return;
        }
        let key = path_key(&e.path);
        let cur: Vec<String> = match self
            .bufs
            .iter()
            .find(|b| b.path.as_ref().map(|p| path_key(p)) == Some(key.clone()))
        {
            Some(b) => chars_to_lines(&b.lines),
            None => std::fs::read_to_string(&e.path)
                .map(|t| git::split_lines(&t))
                .unwrap_or_default(),
        };
        let old: Vec<String> = if e.untracked() {
            Vec::new()
        } else {
            git::base_text(&root, &e.rel)
                .map(|t| git::split_lines(&t))
                .unwrap_or_default()
        };
        let text = if e.conflict() {
            git::run(&root, &["diff", "--", &e.rel], None).unwrap_or_default()
        } else {
            git::unified(&old, &cur, &e.rel, &e.rel)
        };
        self.git_open_diff(name, text);
    }

    pub(crate) fn git_current_diff(&mut self) {
        let Some((root, rel)) = self.git_buf_rel(self.cur) else {
            return self.error(t!(
                "Bu dosya git deposunda değil",
                "This file is not in the git repository"
            ));
        };
        let key = path_key(self.buf().path.as_ref().unwrap_or(&root));
        let old = self
            .git
            .bases
            .get(&key)
            .and_then(|b| b.lines.clone())
            .unwrap_or_default();
        let cur = chars_to_lines(&self.buf().lines);
        let text = git::unified(&old, &cur, &rel, &rel);
        self.git_open_diff(format!("[± {}]", rel), text);
    }

    fn git_show(&mut self, i: usize) {
        let Some(c) = self.git.log.get(i).cloned() else { return };
        let Some(root) = self.git_root() else { return };
        match git::run(&root, &["show", "--format=fuller", "--stat", "--patch", &c.hash], None) {
            Ok(t) => self.git_open_diff(format!("[± {}]", c.short), t),
            Err(e) => self.error(e),
        }
    }

    pub(crate) fn git_hunk_at_cursor(&self) -> Option<Hunk> {
        let hs = self.git_hunks(self.cur)?;
        let cy = self.buf().cy;
        hs.iter().find(|h| h.covers(cy)).cloned()
    }

    pub(crate) fn git_conflict_at_cursor(&self) -> Option<usize> {
        let cy = self.buf().cy;
        self.git_conflicts().iter().position(|c| cy >= c.start && cy <= c.end)
    }

    pub(crate) fn git_next(&mut self, fwd: bool) {
        let cy = self.buf().cy;
        let cs = self.git_conflicts();
        let targets: Vec<usize> = if !cs.is_empty() {
            cs.iter().map(|c| c.start).collect()
        } else {
            match self.git_hunks(self.cur) {
                Some(hs) => hs
                    .iter()
                    .map(|h| {
                        if h.new_len == 0 {
                            h.new_start.saturating_sub(1)
                        } else {
                            h.new_start
                        }
                    })
                    .collect(),
                None => Vec::new(),
            }
        };
        let t = if fwd {
            targets
                .iter()
                .copied()
                .find(|&l| l > cy)
                .or_else(|| targets.first().copied())
        } else {
            targets
                .iter()
                .rev()
                .copied()
                .find(|&l| l < cy)
                .or_else(|| targets.last().copied())
        };
        match t {
            Some(l) => {
                self.push_jump();
                let b = self.buf_mut();
                b.cy = l.min(b.last());
                b.cx = b.first_nb(b.cy);
                b.sync_want();
            }
            None => self.info(t!("Değişiklik yok", "No changes")),
        }
    }

    pub(crate) fn git_hunk_cmd(&mut self, what: &str) {
        match what {
            "diff" | "preview" => return self.git_current_diff(),
            "next" => return self.git_next(true),
            "prev" => return self.git_next(false),
            _ => {}
        }
        let Some(h) = self.git_hunk_at_cursor() else {
            return self.error(t!(
                "İmleç bir değişikliğin üzerinde değil",
                "The cursor is not on a change"
            ));
        };
        match what {
            "revert" | "undo" => {
                let b = self.buf_mut();
                b.checkpoint();
                if h.new_len > 0 {
                    b.delete_lines(h.new_start, h.new_start + h.new_len - 1);
                    if b.lines.len() == 1 && b.lines[0].is_empty() && h.new_start == 0 && !h.old_lines.is_empty() {
                        b.lines.clear();
                    }
                }
                let ins: Vec<Vec<char>> = h.old_lines.iter().map(|l| l.chars().collect()).collect();
                if !ins.is_empty() {
                    b.insert_lines(h.new_start, &ins);
                }
                if b.lines.is_empty() {
                    b.lines.push(Vec::new());
                }
                b.cy = h.new_start.min(b.last());
                b.cx = b.first_nb(b.cy);
                b.sync_want();
                b.touch();
                self.info(t!("Değişiklik geri alındı", "Change reverted"));
            }
            "stage" => {
                let Some((_, rel)) = self.git_buf_rel(self.cur) else {
                    return;
                };
                if self.buf().dirty && !self.save(None) {
                    return;
                }
                let key = path_key(self.buf().path.as_ref().unwrap_or(&PathBuf::new()));
                let Some(base) = self.git.bases.get(&key) else { return };
                let total = base.lines.as_ref().map_or(0, |l| l.len());
                let patch = git::hunk_patch(&rel, &h, base.ends_nl, total);
                let untracked = base.lines.as_ref().is_some_and(|l| l.is_empty())
                    && self
                        .git
                        .status
                        .as_ref()
                        .is_some_and(|s| s.entries.iter().any(|e| e.untracked() && e.rel == rel));
                if untracked {
                    return self.git_stage(rel, true);
                }
                self.git_run("stage hunk", move |r| {
                    git::run(
                        r,
                        &["apply", "--cached", "--unidiff-zero", "--whitespace=nowarn", "-"],
                        Some(&patch),
                    )
                });
            }
            _ => self.error(t!(
                "Kullanım: :hunk stage|revert|diff|next|prev",
                "Usage: :hunk stage|revert|diff|next|prev"
            )),
        }
    }

    pub(crate) fn git_resolve(&mut self, idx: usize, kind: u8) {
        let cs = self.git_conflicts();
        let Some(c) = cs.get(idx).copied() else { return };
        let b = self.buf_mut();
        let rep = git::resolve(&b.lines, &c, kind != 1, kind != 0);
        b.checkpoint();
        b.delete_lines(c.start, c.end);
        if !rep.is_empty() {
            b.insert_lines(c.start, &rep);
        }
        b.cy = c.start.min(b.last());
        b.cx = b.first_nb(b.cy);
        b.sync_want();
        let left = git::conflicts(&self.buf().lines).len();
        if left == 0 {
            self.info(t!(
                "Tüm çakışmalar çözüldü: kaydedip Git panelinde stage'leyin",
                "All conflicts resolved: save and stage the file in the Git panel"
            ));
        }
    }

    pub(crate) fn git_conflict_cmd(&mut self, what: &str) {
        let Some(i) = self.git_conflict_at_cursor() else {
            return self.error(t!(
                "İmleç bir çakışmanın içinde değil",
                "The cursor is not inside a conflict"
            ));
        };
        let k = match what {
            "ours" | "mine" => 0,
            "theirs" => 1,
            "both" => 2,
            _ => {
                return self.error(t!(
                    "Kullanım: :conflict ours|theirs|both",
                    "Usage: :conflict ours|theirs|both"
                ))
            }
        };
        self.git_resolve(i, k);
    }

    pub(crate) fn git_command(&mut self, arg: &str) {
        let arg = arg.trim();
        let (sub, rest) = arg.split_once(' ').map_or((arg, ""), |(a, b)| (a, b.trim()));
        match sub {
            "" | "panel" => self.git_toggle(),
            "blame" => {
                self.git.blame_on = match rest {
                    "on" => true,
                    "off" => false,
                    _ => !self.git.blame_on,
                };
                let _ = crate::config::set("git_blame", if self.git.blame_on { "on" } else { "off" });
                self.info(if self.git.blame_on {
                    t!("Blame açık", "Blame on")
                } else {
                    t!("Blame kapalı", "Blame off")
                });
            }
            "diff" => self.git_current_diff(),
            "stage" | "add" => match self.git_buf_rel(self.cur) {
                Some((_, rel)) => self.git_stage(rel, true),
                None => self.error(t!(
                    "Bu dosya git deposunda değil",
                    "This file is not in the git repository"
                )),
            },
            "unstage" | "reset" => match self.git_buf_rel(self.cur) {
                Some((_, rel)) => self.git_stage(rel, false),
                None => self.error(t!(
                    "Bu dosya git deposunda değil",
                    "This file is not in the git repository"
                )),
            },
            "commit" => {
                if !rest.is_empty() {
                    self.git.msg = rest.chars().collect();
                }
                self.git_commit();
            }
            "push" => self.git_push(),
            "pull" => self.git_run("pull", |r| git::run(r, &["pull"], None)),
            "fetch" => self.git_run("fetch", |r| git::run(r, &["fetch", "--all", "--prune"], None)),
            "log" | "history" => {
                self.git_toggle_on();
                let rows = self.git.rows();
                if let Some(i) = rows.iter().position(|r| matches!(r, Row::Commit(_))) {
                    self.git.sel = i;
                }
            }
            "branch" | "branches" | "checkout" | "switch" => {
                if rest.is_empty() {
                    self.git_branch_menu()
                } else {
                    self.git_branch_cmd(rest)
                }
            }
            "refresh" => self.git_refresh(),
            other => self.error(tf!("Bilinmeyen :git komutu: {}", "Unknown :git command: {}", other)),
        }
    }

    fn git_toggle_on(&mut self) {
        if !(self.tree.visible && self.git.tab) {
            self.git_toggle();
        }
    }

    pub(crate) fn git_branch_cmd(&mut self, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return self.git_branch_menu();
        }
        let Some(root) = self.git_root() else { return };
        let exists = git::branches(&root)
            .map(|l| l.iter().any(|b| b == name))
            .unwrap_or(false);
        self.git_switch(name.to_string(), !exists);
    }

    pub(crate) fn git_context_cmd(&mut self, c: &str) -> bool {
        match c {
            "hunk_stage" => self.git_hunk_cmd("stage"),
            "hunk_revert" => self.git_hunk_cmd("revert"),
            "hunk_diff" => self.git_hunk_cmd("diff"),
            "conf_ours" => self.git_conflict_cmd("ours"),
            "conf_theirs" => self.git_conflict_cmd("theirs"),
            "conf_both" => self.git_conflict_cmd("both"),
            _ => return false,
        }
        true
    }

    pub(crate) fn git_context_items(&self) -> Vec<(String, ide::MenuAction)> {
        let mut v = Vec::new();
        if self.git_conflict_at_cursor().is_some() {
            v.push((
                t!("Çakışma: bizimkini al", "Conflict: accept ours").into(),
                ide::MenuAction::Cmd("conf_ours"),
            ));
            v.push((
                t!("Çakışma: geleni al", "Conflict: accept theirs").into(),
                ide::MenuAction::Cmd("conf_theirs"),
            ));
            v.push((
                t!("Çakışma: ikisini de al", "Conflict: accept both").into(),
                ide::MenuAction::Cmd("conf_both"),
            ));
        } else if self.git_hunk_at_cursor().is_some() {
            v.push((
                t!("Değişikliği stage'le", "Stage change").into(),
                ide::MenuAction::Cmd("hunk_stage"),
            ));
            v.push((
                t!("Değişikliği geri al", "Revert change").into(),
                ide::MenuAction::Cmd("hunk_revert"),
            ));
            v.push((
                t!("Dosya farkını göster", "Show file diff").into(),
                ide::MenuAction::Cmd("hunk_diff"),
            ));
        }
        v
    }

    pub(crate) fn git_gutter_menu(&mut self, x: u16, y: u16) {
        let items = self.git_context_items();
        if items.is_empty() {
            return;
        }
        self.ctx_pos = Some((x, y));
        let mut m = ide::Menu::filterable(String::new(), items, 0, false);
        m.filter = None;
        m.anchor = Some((x, y));
        self.menu = Some(m);
    }

    pub(crate) fn git_editor_click(&mut self, x: u16, y: u16) -> bool {
        let hit = self
            .git
            .conf_hits
            .borrow()
            .iter()
            .find(|h| h.0 == y && x >= h.1 && x < h.2)
            .map(|h| (h.3, h.4));
        if let Some((i, k)) = hit {
            self.git_resolve(i, k);
            return true;
        }
        false
    }

    fn git_list_top(&self, l: &Layout) -> u16 {
        l.top + 6
    }

    fn git_list_rows(&self, l: &Layout) -> usize {
        (l.top + l.text_h).saturating_sub(self.git_list_top(l) + 1) as usize
    }

    fn git_follow(&mut self, n: usize) {
        let l = self.layout();
        let r = self.git_list_rows(&l).max(1);
        if self.git.sel >= n {
            self.git.sel = n.saturating_sub(1);
        }
        if self.git.sel < self.git.scroll {
            self.git.scroll = self.git.sel;
        } else if self.git.sel >= self.git.scroll + r {
            self.git.scroll = self.git.sel + 1 - r;
        }
    }

    fn git_activate(&mut self, i: usize) {
        let rows = self.git.rows();
        match rows.get(i) {
            Some(Row::File(e, staged)) => self.git_file_diff(*e, *staged),
            Some(Row::Commit(c)) => self.git_show(*c),
            _ => {}
        }
    }

    fn git_toggle_stage_row(&mut self, i: usize) {
        let rows = self.git.rows();
        let Some(Row::File(e, staged)) = rows.get(i) else {
            return;
        };
        let Some(en) = self.git.status.as_ref().and_then(|s| s.entries.get(*e)).cloned() else {
            return;
        };
        if en.conflict() && !git::conflicts(&self.git_file_lines(&en.path)).is_empty() {
            return self.error(t!(
                "Dosyada hâlâ çakışma işaretleri var",
                "The file still contains conflict markers"
            ));
        }
        self.git_stage(en.rel, !staged);
    }

    fn git_file_lines(&self, p: &Path) -> Vec<Vec<char>> {
        let key = path_key(p);
        match self
            .bufs
            .iter()
            .find(|b| b.path.as_ref().map(|x| path_key(x)) == Some(key.clone()))
        {
            Some(b) => b.lines.clone(),
            None => std::fs::read_to_string(p)
                .map(|t| t.lines().map(|l| l.chars().collect()).collect())
                .unwrap_or_default(),
        }
    }

    fn git_discard_row(&mut self, i: usize) {
        let rows = self.git.rows();
        let Some(Row::File(e, staged)) = rows.get(i) else {
            return;
        };
        if *staged {
            return self.error(t!(
                "Önce unstage edin (s), sonra silin",
                "Unstage it first (s), then discard"
            ));
        }
        let Some(en) = self.git.status.as_ref().and_then(|s| s.entries.get(*e)).cloned() else {
            return;
        };
        match self.git.discard {
            Some((j, t)) if j == i && t.elapsed().as_secs() < 4 => {
                self.git.discard = None;
                let rel = en.rel.clone();
                if en.untracked() {
                    self.git_run("clean", move |r| git::run(r, &["clean", "-f", "-q", "--", &rel], None));
                } else {
                    self.git_run("restore", move |r| git::run(r, &["restore", "--", &rel], None));
                }
                if let Some(bi) = self
                    .bufs
                    .iter()
                    .position(|b| b.path.as_ref().map(|p| path_key(p)) == Some(path_key(&en.path)))
                {
                    self.bufs[bi].dirty = false;
                }
                self.git.status_now = true;
            }
            _ => {
                self.git.discard = Some((i, Instant::now()));
                self.error(tf!(
                    "{} içindeki değişiklikler silinecek: onaylamak için tekrar d",
                    "Changes in {} will be lost: press d again to confirm",
                    en.rel
                ));
            }
        }
    }

    fn git_msg_key(&mut self, k: KeyEvent) {
        let g = &mut self.git;
        match k.code {
            KeyCode::Esc => g.editing = false,
            KeyCode::Enter => {
                g.editing = false;
                self.git_commit();
            }
            KeyCode::Backspace => {
                if g.msg_x > 0 {
                    g.msg_x -= 1;
                    g.msg.remove(g.msg_x);
                }
            }
            KeyCode::Delete => {
                if g.msg_x < g.msg.len() {
                    g.msg.remove(g.msg_x);
                }
            }
            KeyCode::Left => g.msg_x = g.msg_x.saturating_sub(1),
            KeyCode::Right => g.msg_x = (g.msg_x + 1).min(g.msg.len()),
            KeyCode::Home => g.msg_x = 0,
            KeyCode::End => g.msg_x = g.msg.len(),
            KeyCode::Char('u') if is_ctrl(&k) => {
                g.msg.clear();
                g.msg_x = 0;
            }
            KeyCode::Char(c) if !is_ctrl(&k) => {
                g.msg.insert(g.msg_x, c);
                g.msg_x += 1;
            }
            _ => {}
        }
    }

    pub(crate) fn git_paste(&mut self, text: &str) -> bool {
        if !(self.focus == Focus::Tree && self.git.tab && self.git.editing) {
            return false;
        }
        for c in text.chars().filter(|c| *c != '\r') {
            self.git.msg.insert(self.git.msg_x, c);
            self.git.msg_x += 1;
        }
        true
    }

    pub(crate) fn git_key(&mut self, k: KeyEvent) {
        if self.git.editing {
            return self.git_msg_key(k);
        }
        let rows = self.git.rows();
        let n = rows.len();
        let page = self.git_list_rows(&self.layout()).max(1);
        match k.code {
            KeyCode::Char('j') | KeyCode::Down => self.git.sel = (self.git.sel + 1).min(n.saturating_sub(1)),
            KeyCode::Char('k') | KeyCode::Up => self.git.sel = self.git.sel.saturating_sub(1),
            KeyCode::PageDown => self.git.sel = (self.git.sel + page).min(n.saturating_sub(1)),
            KeyCode::PageUp => self.git.sel = self.git.sel.saturating_sub(page),
            KeyCode::Char('g') | KeyCode::Home => self.git.sel = 0,
            KeyCode::Char('G') | KeyCode::End => self.git.sel = n.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char('l') => self.git_activate(self.git.sel),
            KeyCode::Char('o') => {
                if let Some(Row::File(e, _)) = rows.get(self.git.sel) {
                    if let Some(p) = self
                        .git
                        .status
                        .as_ref()
                        .and_then(|s| s.entries.get(*e))
                        .map(|e| e.path.clone())
                    {
                        if p.exists() {
                            self.open_path(&p);
                            self.focus = Focus::Editor;
                        }
                    }
                }
            }
            KeyCode::Char('s') | KeyCode::Char(' ') => self.git_toggle_stage_row(self.git.sel),
            KeyCode::Char('S') => self.git_run("add -A", |r| git::run(r, &["add", "-A"], None)),
            KeyCode::Char('U') => self.git_run("unstage", |r| {
                git::run(r, &["reset", "-q"], None).or_else(|_| git::run(r, &["rm", "-r", "--cached", "-q", "."], None))
            }),
            KeyCode::Char('d') => self.git_discard_row(self.git.sel),
            KeyCode::Char('c') | KeyCode::Char('i') => {
                self.git.editing = true;
                self.git.msg_x = self.git.msg.len();
            }
            KeyCode::Char('a') => self.git_ai_message(),
            KeyCode::Char('C') => self.git_commit(),
            KeyCode::Char('P') => self.git_push(),
            KeyCode::Char('p') => self.git_run("pull", |r| git::run(r, &["pull"], None)),
            KeyCode::Char('f') => self.git_run("fetch", |r| git::run(r, &["fetch", "--all", "--prune"], None)),
            KeyCode::Char('b') => self.git_branch_menu(),
            KeyCode::Char('r') => {
                self.git_refresh();
                self.git.log_gen = u64::MAX;
            }
            KeyCode::Char('1') => self.git.tab = false,
            KeyCode::Char(':') => self.start_cmd(Mode::Command),
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Tab => self.focus = Focus::Editor,
            _ => {}
        }
        self.git_follow(n);
    }

    pub(crate) fn git_sidebar_mouse(&mut self, m: &MouseEvent, l: &Layout) -> bool {
        let (x, y) = (m.column, m.row);
        if !self.tree.visible || x >= l.tree_w || y < l.top || y >= l.top + l.text_h || self.git.root.is_none() {
            return false;
        }
        if y == l.top {
            if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                let hit = self
                    .git
                    .hits
                    .borrow()
                    .iter()
                    .find(|h| h.0 == y && x >= h.1 && x < h.2)
                    .map(|h| h.3);
                match hit {
                    Some(GitHit::TabFiles) => {
                        self.git.tab = false;
                        self.git.editing = false;
                        self.focus = Focus::Tree;
                        return true;
                    }
                    Some(GitHit::TabGit) => {
                        self.git.tab = true;
                        self.focus = Focus::Tree;
                        self.git_refresh();
                        return true;
                    }
                    _ => return true,
                }
            }
            return matches!(m.kind, MouseEventKind::Down(_));
        }
        if !self.git.tab {
            return false;
        }
        let rows = self.git.rows();
        match m.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let r = self.git_list_rows(l);
                let max = rows.len().saturating_sub(r);
                self.git.scroll = if m.kind == MouseEventKind::ScrollDown {
                    (self.git.scroll + 3).min(max)
                } else {
                    self.git.scroll.saturating_sub(3)
                };
                return true;
            }
            MouseEventKind::Down(btn) => {
                if matches!(self.mode, Mode::Command | Mode::Search) {
                    self.mode = Mode::Normal;
                }
                self.focus = Focus::Tree;
                let hit = self
                    .git
                    .hits
                    .borrow()
                    .iter()
                    .find(|h| h.0 == y && x >= h.1 && x < h.2)
                    .map(|h| h.3);
                self.git.editing = hit == Some(GitHit::Msg);
                match hit {
                    Some(GitHit::Branch) => self.git_branch_menu(),
                    Some(GitHit::Msg) => self.git.msg_x = self.git.msg.len(),
                    Some(GitHit::Commit) => self.git_commit(),
                    Some(GitHit::Ai) => self.git_ai_message(),
                    Some(GitHit::Push) => self.git_push(),
                    Some(GitHit::Pull) => self.git_run("pull", |r| git::run(r, &["pull"], None)),
                    Some(GitHit::Fetch) => self.git_run("fetch", |r| git::run(r, &["fetch", "--all", "--prune"], None)),
                    Some(GitHit::Row(i)) => {
                        self.git.sel = i;
                        let stage_click = matches!(rows.get(i), Some(Row::File(..))) && x + 4 >= l.tree_w;
                        if btn == MouseButton::Right || stage_click {
                            self.git_toggle_stage_row(i);
                        } else {
                            self.git_activate(i);
                        }
                    }
                    _ => {}
                }
                return true;
            }
            _ => {}
        }
        true
    }

    pub(crate) fn draw_sidebar_tabs(&self, s: &mut Screen, l: &Layout) -> bool {
        let mut hits = self.git.hits.borrow_mut();
        hits.clear();
        if self.git.root.is_none() {
            return false;
        }
        let focused = self.focus == Focus::Tree;
        let w = l.tree_w;
        s.fill(0, l.top, w, Style::new());
        let files = format!(" {} ", self.tree.root_name().to_uppercase());
        let n = self.git.change_count();
        let gitl = if n > 0 {
            format!(" GIT {} ", n)
        } else {
            " GIT ".to_string()
        };
        let on = |a: bool| {
            if a {
                Style::fg(if focused { BRIGHT } else { DIM })
                    .bold()
                    .underline(LOGO_GREEN)
            } else {
                Style::fg(DIM)
            }
        };
        let fx = put_clip(
            s,
            0,
            l.top,
            &files,
            on(!self.git.tab),
            (w as usize).saturating_sub(text_w(&gitl) + 1),
        );
        hits.push((l.top, 0, fx, GitHit::TabFiles));
        let gx = s.put(fx, l.top, &gitl, on(self.git.tab));
        hits.push((l.top, fx, gx, GitHit::TabGit));
        true
    }

    pub(crate) fn draw_git_panel(&self, s: &mut Screen, l: &Layout) {
        let mut hits = self.git.hits.borrow_mut();
        let focused = self.focus == Focus::Tree;
        let w = l.tree_w;
        let wu = w as usize;
        self.git.cursor.set(None);
        let mut y = l.top + 1;
        let Some(st) = &self.git.status else {
            put_clip(s, 1, y, t!("yükleniyor…", "loading…"), Style::fg(DIM), wu - 1);
            return;
        };
        let mut x = s.put(1, y, "⎇ ", Style::fg(MACRO));
        let bx = x;
        x = put_clip(
            s,
            x,
            y,
            &st.branch,
            Style::fg(BRIGHT).bold(),
            wu.saturating_sub(x as usize + 8),
        );
        hits.push((y, bx.saturating_sub(2), x, GitHit::Branch));
        if st.ahead > 0 {
            x = s.put(x, y, &format!(" ↑{}", st.ahead), Style::fg(STR));
        }
        if st.behind > 0 {
            x = s.put(x, y, &format!(" ↓{}", st.behind), Style::fg(TYPE));
        }
        if st.merging {
            put_clip(
                s,
                x + 1,
                y,
                "MERGE",
                Style::fg(MACRO).bold(),
                wu.saturating_sub(x as usize + 1),
            );
        }
        y += 1;
        s.fill(1, y, w.saturating_sub(2), Style::new().on(POP));
        hits.push((y, 1, w.saturating_sub(1), GitHit::Msg));
        let inner = wu.saturating_sub(4);
        if self.git.msg.is_empty() && !self.git.editing {
            put_clip(
                s,
                2,
                y,
                t!("Commit mesajı… (c)", "Commit message… (c)"),
                Style::fg(DIM).italic().on(POP),
                inner,
            );
        } else {
            let shown: Vec<char> = self.git.msg.iter().map(|c| if *c == '\n' { '⏎' } else { *c }).collect();
            let before: usize = shown[..self.git.msg_x.min(shown.len())]
                .iter()
                .map(|c| char_width(*c, 0))
                .sum();
            let skip_w = if self.git.editing {
                before.saturating_sub(inner.saturating_sub(1))
            } else {
                0
            };
            let mut acc = 0;
            let mut start = 0;
            while start < shown.len() && acc < skip_w {
                acc += char_width(shown[start], 0);
                start += 1;
            }
            let text: String = shown[start..].iter().collect();
            put_clip(s, 2, y, &text, Style::fg(BRIGHT).on(POP), inner);
            if self.git.editing && focused {
                self.git.cursor.set(Some((2 + (before - acc).min(inner) as u16, y)));
            }
        }
        y += 1;
        let mut bx = 1u16;
        let ai_busy = self.git.ai_job.is_some();
        let buttons: [(&str, GitHit, Color); 5] = [
            (t!("✓ Commit", "✓ Commit"), GitHit::Commit, LOGO_GREEN),
            (
                if ai_busy { "✦ …" } else { "✦ AI" },
                GitHit::Ai,
                Color::Rgb {
                    r: 0xb4,
                    g: 0x9c,
                    b: 0xe0,
                },
            ),
            ("↑ Push", GitHit::Push, BRIGHT),
            ("↓ Pull", GitHit::Pull, BRIGHT),
            ("⟳", GitHit::Fetch, BRIGHT),
        ];
        let mut row_y = y;
        for (label, hit, c) in buttons {
            let bw = text_w(label) as u16 + 2;
            if bx + bw > w {
                row_y += 1;
                bx = 1;
                if row_y > y + 1 {
                    break;
                }
            }
            s.put(bx, row_y, &format!(" {} ", label), Style::fg(c).on(SEL));
            hits.push((row_y, bx, bx + bw, hit));
            bx += bw + 1;
        }
        y += 2;
        match (&self.git.op, &self.git.out) {
            (Some((label, _)), _) => {
                put_clip(s, 1, y, &format!("… {}", label), Style::fg(TYPE).italic(), wu - 1);
            }
            (None, Some((t, err))) => {
                put_clip(s, 1, y, t, Style::fg(if *err { ERR } else { DIM }), wu - 1);
            }
            _ => {}
        }
        let top = self.git_list_top(l);
        let rows = self.git.rows();
        let nrows = self.git_list_rows(l);
        let scroll = self.git.scroll.min(rows.len().saturating_sub(1));
        for (k, row) in rows.iter().enumerate().skip(scroll).take(nrows) {
            let ry = top + (k - scroll) as u16;
            let selected = k == self.git.sel && focused;
            if selected {
                s.fill(0, ry, w, Style::new().on(SEL));
            }
            let bg = |st: Style| if selected { st.on(SEL) } else { st };
            match row {
                Row::Head(t, n) => {
                    let x = s.put(1, ry, t, bg(Style::fg(DIM).bold()));
                    s.put(x + 1, ry, &n.to_string(), bg(Style::fg(DIM)));
                }
                Row::Note(t) => {
                    put_clip(s, 3, ry, t, bg(Style::fg(DIM).italic()), wu.saturating_sub(3));
                }
                Row::File(i, staged) => {
                    let Some(e) = st.entries.get(*i) else { continue };
                    let code = e.code(*staged);
                    let cc = match code {
                        'A' | 'U' => GIT_ADD,
                        'D' => GIT_DEL,
                        '!' => MACRO,
                        _ => TYPE,
                    };
                    let (dir, name) = match e.rel.rsplit_once('/') {
                        Some((d, n)) => (d, n),
                        None => ("", e.rel.as_str()),
                    };
                    let nx = put_clip(s, 2, ry, name, bg(Style::fg(BRIGHT)), wu.saturating_sub(6));
                    if !dir.is_empty() && (nx as usize) + 3 < wu.saturating_sub(4) {
                        put_clip(
                            s,
                            nx + 1,
                            ry,
                            dir,
                            bg(Style::fg(DIM)),
                            wu.saturating_sub(nx as usize + 6),
                        );
                    }
                    s.put(w.saturating_sub(3), ry, &code.to_string(), bg(Style::fg(cc).bold()));
                }
                Row::Commit(i) => {
                    let Some(c) = self.git.log.get(*i) else { continue };
                    let x = s.put(2, ry, &c.short, bg(Style::fg(MACRO)));
                    put_clip(
                        s,
                        x + 1,
                        ry,
                        &c.subject,
                        bg(Style::fg(BRIGHT)),
                        wu.saturating_sub(x as usize + 2),
                    );
                }
            }
            hits.push((ry, 0, w, GitHit::Row(k)));
        }
        let fy = l.top + l.text_h - 1;
        let sel_commit = match rows.get(self.git.sel) {
            Some(Row::Commit(i)) if focused => self.git.log.get(*i),
            _ => None,
        };
        if let (Some(c), false) = (sel_commit, self.git.editing) {
            s.fill(0, fy, w, Style::new());
            put_clip(
                s,
                1,
                fy,
                &format!("{} · {}", c.author, git::ago(c.time)),
                Style::fg(DIM),
                wu - 1,
            );
        } else if focused && !self.git.editing {
            let hint = t!(
                "s stage · d sil · c mesaj · a AI · C commit · P push · p pull · b branch",
                "s stage · d discard · c message · a AI · C commit · P push · p pull · b branch"
            );
            s.fill(0, fy, w, Style::new());
            put_clip(s, 1, fy, hint, Style::fg(DIM), wu - 1);
        } else if self.git.editing {
            s.fill(0, fy, w, Style::new());
            put_clip(
                s,
                1,
                fy,
                t!("Enter: commit · Esc: bitti", "Enter: commit · Esc: done"),
                Style::fg(DIM),
                wu - 1,
            );
        }
    }
}

pub(crate) fn sign_style(sign: u8) -> (char, Style) {
    match sign {
        1 => ('▎', Style::fg(GIT_ADD)),
        2 => ('▎', Style::fg(GIT_MOD)),
        3 => ('▁', Style::fg(GIT_DEL)),
        _ => ('▔', Style::fg(GIT_DEL)),
    }
}

pub(crate) fn diff_line_style(line: &[char]) -> Option<Style> {
    let starts =
        |p: &str| line.len() >= p.chars().count() && line.iter().take(p.chars().count()).copied().eq(p.chars());
    if starts("+++ ") || starts("--- ") || starts("diff ") || starts("index ") {
        return Some(Style::fg(BRIGHT).bold());
    }
    if starts("commit ") || starts("Author:") || starts("AuthorDate:") || starts("Commit:") || starts("CommitDate:") {
        return Some(Style::fg(TYPE));
    }
    match line.first() {
        Some('+') => Some(Style::fg(GIT_ADD)),
        Some('-') => Some(Style::fg(GIT_DEL)),
        Some('@') => Some(Style::fg(MACRO)),
        _ => None,
    }
}
