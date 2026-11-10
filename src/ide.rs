use super::*;
use crate::buffer::leading_ws;
use crate::lsp::{
    candidates, expand_snippet, from_lsp_col, language_id, path_key, path_to_uri, to_lsp_col, uri_to_path, Diag, Enc,
    LspEvent, Pending,
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub const POPUP_ROWS: usize = 10;
pub const MENU_ROWS: usize = 14;

pub struct CItem {
    pub label: String,
    pub kind: u64,
    pub detail: String,
    filter: String,
    sort: String,
    text: String,
    snippet: bool,
    start: Option<usize>,
    end: Option<usize>,
    extra: Vec<Value>,
    raw: Value,
}

pub struct Popup {
    pub items: Vec<CItem>,
    pub shown: Vec<usize>,
    pub sel: usize,
    pub scroll: usize,
    pub y: usize,
    pub start: usize,
    pub lsp: bool,
    pub incomplete: bool,
}

#[derive(Clone)]
pub enum MenuAction {
    Code(&'static str, Value),
    Jump(PathBuf, usize, usize, Enc),
    Cmd(&'static str),
    Dir(PathBuf),
    OpenDir(PathBuf),
    Model(String),
    Chat(String),
    AiProvider(crate::ai::Provider),
}

pub struct Menu {
    pub title: String,
    pub items: Vec<(String, MenuAction)>,
    pub sel: usize,
    pub scroll: usize,
    pub anchor: Option<(u16, u16)>,
    pub dir: Option<PathBuf>,
    pub filter: Option<String>,
    pub base: Vec<(String, MenuAction)>,
    pub custom_model: bool,
    pub ws_symbols: bool,
}

impl Menu {
    pub fn filterable(title: String, items: Vec<(String, MenuAction)>, sel: usize, custom_model: bool) -> Self {
        Menu {
            title,
            base: items.clone(),
            items,
            sel,
            scroll: 0,
            anchor: None,
            dir: None,
            filter: Some(String::new()),
            custom_model,
            ws_symbols: false,
        }
    }

    fn apply_filter(&mut self) {
        let f = self.filter.clone().unwrap_or_default().to_lowercase();
        self.items = self
            .base
            .iter()
            .filter(|(t, _)| f.is_empty() || t.to_lowercase().contains(&f))
            .cloned()
            .collect();
        self.sel = 0;
        self.scroll = 0;
    }
}

fn fuzzy(pat: &str, s: &str) -> Option<u8> {
    if pat.is_empty() || s.starts_with(pat) {
        return Some(0);
    }
    if s.contains(pat) {
        return Some(1);
    }
    let mut it = s.chars();
    for p in pat.chars() {
        it.find(|c| *c == p)?;
    }
    Some(2)
}

fn markup_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .map(markup_text)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        Value::Object(o) => o.get("value").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        _ => String::new(),
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Editor {
    pub(crate) fn lsp_lang(&self, i: usize) -> Option<&'static str> {
        let b = self.bufs.get(i)?;
        if b.path.is_some() && !candidates(b.lang.name).is_empty() {
            Some(b.lang.name)
        } else {
            None
        }
    }

    fn cur_ready(&self) -> Option<&'static str> {
        self.lsp_lang(self.cur).filter(|l| self.lsp.ready(l))
    }

    pub(crate) fn lsp_sync(&mut self, i: usize) {
        let Some(lang) = self.lsp_lang(i) else { return };
        let b = &mut self.bufs[i];
        if !b.lsp_open || b.rev == b.lsp_rev {
            return;
        }
        b.lsp_version += 1;
        b.lsp_rev = b.rev;
        let params = json!({
            "textDocument": { "uri": path_to_uri(b.path.as_ref().unwrap()), "version": b.lsp_version },
            "contentChanges": [{ "text": b.text() }]
        });
        self.lsp.notify(lang, "textDocument/didChange", params);
    }

    fn lsp_open_buf(&mut self, i: usize) {
        let Some(lang) = self.lsp_lang(i) else { return };
        if self.bufs[i].lsp_open {
            return;
        }
        if !self.lsp.alive(lang) {
            if self.lsp.failed.contains(lang) {
                return;
            }
            let path = self.bufs[i].path.clone().unwrap();
            match self.lsp.start(lang, &path, &self.tree.root) {
                Ok(name) => self.info(tf!("{} başlatılıyor…", "starting {}…", name)),
                Err(e) => {
                    if !e.is_empty() {
                        self.error(e);
                    }
                    return;
                }
            }
        }
        let b = &mut self.bufs[i];
        b.lsp_open = true;
        b.lsp_rev = b.rev;
        b.lsp_version = 1;
        let path = b.path.clone().unwrap();
        let params = json!({
            "textDocument": {
                "uri": path_to_uri(&path),
                "languageId": language_id(lang, &path),
                "version": 1,
                "text": b.text(),
            }
        });
        self.lsp.notify(lang, "textDocument/didOpen", params);
    }

    pub(crate) fn lsp_close_buf(&mut self, i: usize) {
        let Some(lang) = self.lsp_lang(i) else { return };
        if !self.bufs[i].lsp_open {
            return;
        }
        let path = self.bufs[i].path.clone().unwrap();
        self.lsp.notify(
            lang,
            "textDocument/didClose",
            json!({ "textDocument": { "uri": path_to_uri(&path) } }),
        );
        self.lsp.diags.remove(&path_key(&path));
        self.bufs[i].lsp_open = false;
    }

    pub(crate) fn lsp_did_save(&mut self) {
        let Some(lang) = self.lsp_lang(self.cur) else { return };
        if !self.buf().lsp_open {
            return;
        }
        self.lsp_sync(self.cur);
        let uri = path_to_uri(self.buf().path.as_ref().unwrap());
        self.lsp
            .notify(lang, "textDocument/didSave", json!({ "textDocument": { "uri": uri } }));
    }

    pub fn lsp_tick(&mut self) -> bool {
        let before = self.lsp_status();
        let had_msg = self.msg.is_some();
        for i in 0..self.bufs.len() {
            if self.bufs[i].path.is_some() && !self.bufs[i].lsp_open {
                self.lsp_open_buf(i);
            }
            self.lsp_sync(i);
        }
        let events = self.lsp.poll();
        let mut changed = !events.is_empty();
        for ev in events {
            self.on_lsp_event(ev);
        }
        if self.lsp_status() != before || self.msg.is_some() != had_msg {
            changed = true;
        }
        changed
    }

    pub fn lsp_status(&self) -> Option<String> {
        let lang = self.lsp_lang(self.cur)?;
        let s = self.lsp.servers.get(lang)?;
        if !s.ready {
            return Some(format!("{}…", s.name));
        }
        if let Some((t, m)) = s.progress.values().next() {
            let mut txt = format!("{}: {} {}", s.name, t, m);
            txt = txt.trim().chars().take(48).collect();
            return Some(txt);
        }
        Some(s.name.clone())
    }

    fn on_lsp_event(&mut self, ev: LspEvent) {
        match ev {
            LspEvent::Ready(name) => self.info(tf!("{} hazır", "{} ready", name)),
            LspEvent::Exited(lang, name) => {
                for b in self.bufs.iter_mut() {
                    if b.lang.name == lang {
                        b.lsp_open = false;
                        if let Some(p) = &b.path {
                            self.lsp.diags.remove(&path_key(p));
                        }
                    }
                }
                self.error(tf!(
                    "{} kapandı (:lsp restart ile yeniden başlat)",
                    "{} exited (restart with :lsp restart)",
                    name
                ));
            }
            LspEvent::Notification { lang, method, params } => match method.as_str() {
                "textDocument/publishDiagnostics" => self.on_diagnostics(lang, &params),
                "window/showMessage" => {
                    let m = params["message"]
                        .as_str()
                        .unwrap_or("")
                        .lines()
                        .next()
                        .unwrap_or("")
                        .to_string();
                    if params["type"].as_u64().unwrap_or(4) <= 1 {
                        self.error(m);
                    } else {
                        self.info(m);
                    }
                }
                _ => {}
            },
            LspEvent::ApplyEdit { lang, id, params } => {
                self.apply_workspace_edit(lang, &params["edit"]);
                self.lsp.reply(lang, id, json!({ "applied": true }));
            }
            LspEvent::Response {
                lang,
                id,
                kind,
                result,
                error,
            } => {
                if let Some(e) = error {
                    if !matches!(kind, Pending::Completion | Pending::Signature | Pending::Hover) {
                        self.error(format!("LSP: {}", e));
                    }
                    if matches!(kind, Pending::Completion) && Some(id) == self.comp_id {
                        self.comp_id = None;
                    }
                    return;
                }
                match kind {
                    Pending::Completion => {
                        if Some(id) == self.comp_id {
                            self.comp_id = None;
                            self.on_completion(lang, result);
                        }
                    }
                    Pending::CompResolve(buf) => self.on_comp_resolve(lang, buf, result),
                    Pending::Hover => self.on_hover(result),
                    Pending::DocSymbols => self.on_doc_symbols(lang, result),
                    Pending::WsSymbols => self.on_ws_symbols(lang, id, result),
                    Pending::Definition => self.on_definition(lang, result),
                    Pending::References => self.on_references(lang, result),
                    Pending::CodeAction => self.on_code_actions(lang, result),
                    Pending::Resolve => self.run_action(lang, result, false),
                    Pending::Signature => self.on_signature(result),
                    Pending::Format(path) => self.on_format(lang, &path, result),
                    Pending::Rename => {
                        if result.is_null() {
                            self.error(t!("Yeniden adlandırılamadı", "Rename failed"));
                        } else {
                            let n = self.apply_workspace_edit(lang, &result);
                            self.info(tf!(
                                "{} dosyada yeniden adlandırıldı (:wa ile kaydet)",
                                "Renamed in {} files (:wa to save)",
                                n
                            ));
                        }
                    }
                    Pending::Execute | Pending::Init => {}
                }
            }
        }
    }

    fn on_diagnostics(&mut self, lang: &'static str, params: &Value) {
        let Some(path) = params["uri"].as_str().and_then(uri_to_path) else {
            return;
        };
        let enc = self.lsp.enc(lang);
        let num = |v: &Value| v.as_u64().unwrap_or(0) as usize;
        let ds: Vec<Diag> = params["diagnostics"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|d| {
                        let r = &d["range"];
                        let mut msg = d["message"].as_str().unwrap_or("").to_string();
                        if let Some(src) = d["source"].as_str() {
                            msg = format!("{} [{}]", msg, src);
                        }
                        Diag {
                            l0: num(&r["start"]["line"]),
                            c0: num(&r["start"]["character"]),
                            l1: num(&r["end"]["line"]),
                            c1: num(&r["end"]["character"]),
                            sev: d["severity"].as_u64().unwrap_or(1) as u8,
                            msg,
                            raw: d.clone(),
                            enc,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.lsp.diags.insert(path_key(&path), ds);
    }

    pub fn buf_diags(&self, i: usize) -> Option<&Vec<Diag>> {
        let p = self.bufs.get(i)?.path.as_ref()?;
        self.lsp.diags.get(&path_key(p)).filter(|d| !d.is_empty())
    }

    pub fn diag_at_cursor(&self) -> Option<(u8, String)> {
        let b = self.buf();
        let ds = self.buf_diags(self.cur)?;
        ds.iter()
            .filter(|d| d.l0 <= b.cy && b.cy <= d.l1)
            .min_by_key(|d| d.sev)
            .map(|d| (d.sev, d.msg.lines().next().unwrap_or("").to_string()))
    }

    pub fn diag_counts(&self) -> (usize, usize) {
        self.buf_diags(self.cur).map_or((0, 0), |ds| {
            (
                ds.iter().filter(|d| d.sev == 1).count(),
                ds.iter().filter(|d| d.sev == 2).count(),
            )
        })
    }

    pub(crate) fn goto_diag(&mut self, fwd: bool) {
        let b = self.buf();
        let Some(ds) = self.buf_diags(self.cur) else {
            return self.info(t!("Tanı yok", "No diagnostics"));
        };
        let mut pos: Vec<(usize, usize, String)> = ds
            .iter()
            .map(|d| {
                let l = d.l0.min(b.last());
                (
                    l,
                    from_lsp_col(&b.lines[l], d.c0, d.enc),
                    d.msg.lines().next().unwrap_or("").to_string(),
                )
            })
            .collect();
        pos.sort_by_key(|a| (a.0, a.1));
        let cur = (b.cy, b.cx);
        let pick = if fwd {
            pos.iter().find(|p| (p.0, p.1) > cur).or(pos.first())
        } else {
            pos.iter().rev().find(|p| (p.0, p.1) < cur).or(pos.last())
        };
        if let Some((y, x, m)) = pick.cloned() {
            let b = self.buf_mut();
            b.cy = y;
            b.cx = x;
            b.sync_want();
            self.info(m);
        }
    }

    pub(crate) fn diag_menu(&mut self) {
        let b = self.buf();
        let Some(ds) = self.buf_diags(self.cur) else {
            return self.info(t!("Tanı yok", "No diagnostics"));
        };
        let path = b.path.clone().unwrap();
        let mut items: Vec<(u8, usize, String, MenuAction)> = ds
            .iter()
            .map(|d| {
                let sev = match d.sev {
                    1 => t!("hata", "error"),
                    2 => t!("uyarı", "warn"),
                    3 => t!("bilgi", "info"),
                    _ => t!("ipucu", "hint"),
                };
                let label = format!("{:>4}  {:<5} {}", d.l0 + 1, sev, d.msg.lines().next().unwrap_or(""));
                (d.sev, d.l0, label, MenuAction::Jump(path.clone(), d.l0, d.c0, d.enc))
            })
            .collect();
        items.sort_by_key(|i| (i.0, i.1));
        let n = items.len();
        self.menu = Some(Menu {
            title: tf!("Tanılar ({})", "Diagnostics ({})", n),
            items: items.into_iter().map(|i| (i.2, i.3)).collect(),
            sel: 0,
            scroll: 0,
            anchor: None,
            dir: None,
            filter: None,
            base: Vec::new(),
            custom_model: false,
            ws_symbols: false,
        });
    }

    fn tdpp(&self, lang: &str) -> Value {
        let b = self.buf();
        let enc = self.lsp.enc(lang);
        json!({
            "textDocument": { "uri": path_to_uri(b.path.as_ref().unwrap()) },
            "position": { "line": b.cy, "character": to_lsp_col(&b.lines[b.cy], b.cx, enc) }
        })
    }

    fn ready_lang(&mut self, cap: &str) -> Option<&'static str> {
        let Some(lang) = self.lsp_lang(self.cur) else {
            self.error(t!("Bu dosya için dil sunucusu yok", "No language server for this file"));
            return None;
        };
        if !self.lsp.ready(lang) {
            if self.lsp.alive(lang) {
                self.info(t!(
                    "Dil sunucusu henüz hazır değil…",
                    "Language server is still starting…"
                ));
            } else {
                self.error(t!(
                    "Dil sunucusu çalışmıyor (:lsp)",
                    "Language server is not running (:lsp)"
                ));
            }
            return None;
        }
        if !self.lsp.cap(lang, cap) {
            self.error(t!("Sunucu bu özelliği desteklemiyor", "Server does not support this"));
            return None;
        }
        self.lsp_sync(self.cur);
        Some(lang)
    }

    pub(crate) fn lsp_hover(&mut self) {
        let Some(lang) = self.ready_lang("hoverProvider") else {
            return;
        };
        let p = self.tdpp(lang);
        self.lsp.request(lang, "textDocument/hover", p, Pending::Hover);
    }

    pub(crate) fn lsp_definition(&mut self) {
        let Some(lang) = self.ready_lang("definitionProvider") else {
            return;
        };
        let p = self.tdpp(lang);
        self.lsp
            .request(lang, "textDocument/definition", p, Pending::Definition);
    }

    pub(crate) fn lsp_references(&mut self) {
        let Some(lang) = self.ready_lang("referencesProvider") else {
            return;
        };
        let mut p = self.tdpp(lang);
        p["context"] = json!({ "includeDeclaration": true });
        self.lsp
            .request(lang, "textDocument/references", p, Pending::References);
    }

    pub(crate) fn lsp_format(&mut self) {
        let Some(lang) = self.ready_lang("documentFormattingProvider") else {
            return;
        };
        let b = self.buf();
        let path = b.path.clone().unwrap();
        let p = json!({
            "textDocument": { "uri": path_to_uri(&path) },
            "options": { "tabSize": TAB_W, "insertSpaces": !b.use_tabs }
        });
        self.lsp
            .request(lang, "textDocument/formatting", p, Pending::Format(path));
    }

    pub(crate) fn lsp_rename(&mut self, new_name: &str) {
        let Some(lang) = self.ready_lang("renameProvider") else {
            return;
        };
        let mut p = self.tdpp(lang);
        p["newName"] = json!(new_name);
        self.lsp.request(lang, "textDocument/rename", p, Pending::Rename);
    }

    pub(crate) fn word_under_cursor(&self) -> String {
        let b = self.buf();
        let line = &b.lines[b.cy];
        if b.cx >= line.len() || !is_ident(line[b.cx]) {
            return String::new();
        }
        let mut s = b.cx;
        while s > 0 && is_ident(line[s - 1]) {
            s -= 1;
        }
        let mut e = b.cx;
        while e < line.len() && is_ident(line[e]) {
            e += 1;
        }
        line[s..e].iter().collect()
    }

    pub(crate) fn start_rename(&mut self) {
        let w = self.word_under_cursor();
        self.start_cmd(Mode::Command);
        self.cmd = format!("rename {}", w).chars().collect();
        self.cmd_x = self.cmd.len();
    }

    pub(crate) fn lsp_code_actions(&mut self) {
        let Some(lang) = self.ready_lang("codeActionProvider") else {
            return;
        };
        let enc = self.lsp.enc(lang);
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        let (a, e) = if visual {
            self.selection()
        } else {
            ((self.buf().cx, self.buf().cy), (self.buf().cx, self.buf().cy))
        };
        let b = self.buf();
        let range = json!({
            "start": { "line": a.1, "character": to_lsp_col(&b.lines[a.1], a.0, enc) },
            "end": { "line": e.1, "character": to_lsp_col(&b.lines[e.1], e.0 + visual as usize, enc) },
        });
        let diags: Vec<Value> = self
            .buf_diags(self.cur)
            .map(|ds| {
                ds.iter()
                    .filter(|d| d.l0 <= e.1 && a.1 <= d.l1)
                    .map(|d| d.raw.clone())
                    .collect()
            })
            .unwrap_or_default();
        let p = json!({
            "textDocument": { "uri": path_to_uri(b.path.as_ref().unwrap()) },
            "range": range,
            "context": { "diagnostics": diags, "triggerKind": 1 }
        });
        self.lsp
            .request(lang, "textDocument/codeAction", p, Pending::CodeAction);
    }

    fn on_hover(&mut self, r: Value) {
        let text = markup_text(&r["contents"]);
        let mut lines: Vec<String> = Vec::new();
        for l in text.lines() {
            let l = l.trim_end();
            if l.starts_with("```") {
                continue;
            }
            if l.is_empty() && lines.last().is_none_or(|x| x.is_empty()) {
                continue;
            }
            lines.push(l.replace("&nbsp;", " "));
        }
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        if lines.is_empty() {
            self.info(t!("Bilgi yok", "No information"));
        } else {
            lines.truncate(40);
            self.hover = Some(lines);
        }
    }

    fn on_definition(&mut self, lang: &'static str, r: Value) {
        let loc = if r.is_array() { r[0].clone() } else { r };
        if loc.is_null() {
            return self.info(t!("Tanım bulunamadı", "Definition not found"));
        }
        let (uri, range) = if loc["targetUri"].is_string() {
            (&loc["targetUri"], &loc["targetSelectionRange"])
        } else {
            (&loc["uri"], &loc["range"])
        };
        let Some(path) = uri.as_str().and_then(uri_to_path) else {
            return;
        };
        let line = range["start"]["line"].as_u64().unwrap_or(0) as usize;
        let col = range["start"]["character"].as_u64().unwrap_or(0) as usize;
        let enc = self.lsp.enc(lang);
        self.jump_to(&path, line, col, enc);
    }

    fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.tree.root).unwrap_or(p).display().to_string()
    }

    fn on_references(&mut self, lang: &'static str, r: Value) {
        let arr = r.as_array().cloned().unwrap_or_default();
        if arr.is_empty() {
            return self.info(t!("Referans bulunamadı", "No references found"));
        }
        let enc = self.lsp.enc(lang);
        let mut cache: HashMap<PathBuf, Vec<String>> = HashMap::new();
        let mut items = Vec::new();
        for loc in arr.iter().take(300) {
            let Some(path) = loc["uri"].as_str().and_then(uri_to_path) else {
                continue;
            };
            let line = loc["range"]["start"]["line"].as_u64().unwrap_or(0) as usize;
            let col = loc["range"]["start"]["character"].as_u64().unwrap_or(0) as usize;
            let preview = match self
                .bufs
                .iter()
                .find(|b| b.path.as_ref().map(|p| path_key(p)) == Some(path_key(&path)))
            {
                Some(b) => b
                    .lines
                    .get(line)
                    .map(|l| l.iter().collect::<String>())
                    .unwrap_or_default(),
                None => cache
                    .entry(path.clone())
                    .or_insert_with(|| {
                        fs::read_to_string(&path)
                            .unwrap_or_default()
                            .lines()
                            .map(|s| s.to_string())
                            .collect()
                    })
                    .get(line)
                    .cloned()
                    .unwrap_or_default(),
            };
            let label = format!("{}:{}  {}", self.rel(&path), line + 1, preview.trim());
            items.push((label, MenuAction::Jump(path, line, col, enc)));
        }
        if items.len() == 1 {
            if let Some((_, MenuAction::Jump(p, l, c, e))) = items.pop() {
                self.jump_to(&p, l, c, e);
            }
            return;
        }
        let n = items.len();
        self.menu = Some(Menu {
            title: tf!("Referanslar ({})", "References ({})", n),
            items,
            sel: 0,
            scroll: 0,
            anchor: None,
            dir: None,
            filter: None,
            base: Vec::new(),
            custom_model: false,
            ws_symbols: false,
        });
    }

    fn on_code_actions(&mut self, lang: &'static str, r: Value) {
        let mut arr = r.as_array().cloned().unwrap_or_default();
        if arr.is_empty() {
            return self.info(t!("Kullanılabilir kod eylemi yok", "No code actions available"));
        }
        arr.sort_by_key(|a| {
            let quick = a["kind"].as_str().is_some_and(|k| k.starts_with("quickfix"));
            let pref = a["isPreferred"].as_bool().unwrap_or(false);
            (!pref, !quick)
        });
        let items = arr
            .into_iter()
            .map(|a| {
                let mut t = a["title"].as_str().unwrap_or("?").to_string();
                if a["kind"].as_str().is_some_and(|k| k.starts_with("quickfix")) {
                    t = format!("⚡ {}", t);
                }
                (t, MenuAction::Code(lang, a))
            })
            .collect();
        let anchor = self.menu_anchor.take();
        self.menu = Some(Menu {
            title: t!("Kod eylemleri", "Code actions").into(),
            items,
            sel: 0,
            scroll: 0,
            anchor,
            dir: None,
            filter: None,
            base: Vec::new(),
            custom_model: false,
            ws_symbols: false,
        });
    }

    pub(crate) fn run_action(&mut self, lang: &'static str, a: Value, allow_resolve: bool) {
        let mut did = false;
        if a["edit"].is_object() {
            self.apply_workspace_edit(lang, &a["edit"]);
            did = true;
        }
        let cmd = if a["command"].is_string() {
            Some(a.clone())
        } else if a["command"].is_object() {
            Some(a["command"].clone())
        } else {
            None
        };
        if let Some(c) = cmd {
            let p = json!({ "command": c["command"], "arguments": c["arguments"] });
            self.lsp.request(lang, "workspace/executeCommand", p, Pending::Execute);
            did = true;
        }
        if !did && allow_resolve && !a["data"].is_null() {
            self.lsp.request(lang, "codeAction/resolve", a, Pending::Resolve);
            return;
        }
        if !did {
            self.error(t!("Eylem uygulanamadı", "Could not apply action"));
        }
    }

    fn on_signature(&mut self, r: Value) {
        if self.mode != Mode::Insert {
            return;
        }
        let Some(sigs) = r["signatures"].as_array().filter(|s| !s.is_empty()) else {
            self.sig = None;
            return;
        };
        let idx = (r["activeSignature"].as_u64().unwrap_or(0) as usize).min(sigs.len() - 1);
        let s = &sigs[idx];
        let label = s["label"].as_str().unwrap_or("").to_string();
        let ap = s["activeParameter"]
            .as_u64()
            .or(r["activeParameter"].as_u64())
            .unwrap_or(0) as usize;
        let lc: Vec<char> = label.chars().collect();
        let hl = match &s["parameters"][ap]["label"] {
            Value::String(p) => {
                let pc: Vec<char> = p.chars().collect();
                (0..lc.len().saturating_sub(pc.len()) + 1)
                    .find(|&i| lc[i..].starts_with(&pc))
                    .map(|i| (i, i + pc.len()))
            }
            Value::Array(a) if a.len() == 2 => {
                let s0 = a[0].as_u64().unwrap_or(0) as usize;
                let s1 = a[1].as_u64().unwrap_or(0) as usize;
                Some((from_lsp_col(&lc, s0, Enc::Utf16), from_lsp_col(&lc, s1, Enc::Utf16)))
            }
            _ => None,
        };
        self.sig = Some((label, hl));
    }

    fn on_format(&mut self, lang: &'static str, path: &Path, r: Value) {
        let Some(edits) = r.as_array() else {
            return self.error(t!(
                "Değişiklik yok ya da biçimlendirilemedi (sözdizimi hatası olabilir)",
                "No changes or formatting failed (syntax error?)"
            ));
        };
        let key = path_key(path);
        let Some(i) = self
            .bufs
            .iter()
            .position(|b| b.path.as_ref().map(|p| path_key(p)) == Some(key.clone()))
        else {
            return;
        };
        if edits.is_empty() {
            return self.info(t!("Dosya zaten biçimli", "Already formatted"));
        }
        let enc = self.lsp.enc(lang);
        self.bufs[i].checkpoint();
        self.apply_text_edits(i, edits, enc, None);
        self.bufs[i].clamp(false);
        self.info(t!("Biçimlendirildi", "Formatted"));
    }

    fn buf_index_for(&mut self, path: &Path) -> Option<usize> {
        let key = path_key(path);
        if let Some(i) = self
            .bufs
            .iter()
            .position(|b| b.path.as_ref().map(|p| path_key(p)) == Some(key.clone()))
        {
            return Some(i);
        }
        let b = Buffer::open(path).ok()?;
        self.bufs.push(b);
        Some(self.bufs.len() - 1)
    }

    pub(crate) fn apply_workspace_edit(&mut self, lang: &'static str, edit: &Value) -> usize {
        let enc = self.lsp.enc(lang);
        let mut per: Vec<(PathBuf, Vec<Value>)> = Vec::new();
        if let Some(ch) = edit["changes"].as_object() {
            for (uri, edits) in ch {
                if let (Some(p), Some(e)) = (uri_to_path(uri), edits.as_array()) {
                    per.push((p, e.clone()));
                }
            }
        }
        if let Some(dc) = edit["documentChanges"].as_array() {
            for d in dc {
                if let (Some(p), Some(e)) = (
                    d["textDocument"]["uri"].as_str().and_then(uri_to_path),
                    d["edits"].as_array(),
                ) {
                    per.push((p, e.clone()));
                }
            }
        }
        let mut n = 0;
        for (path, edits) in per {
            if let Some(i) = self.buf_index_for(&path) {
                self.bufs[i].checkpoint();
                self.apply_text_edits(i, &edits, enc, None);
                n += 1;
            }
        }
        n
    }

    fn apply_text_edits(&mut self, i: usize, edits: &[Value], enc: Enc, track: Option<usize>) -> isize {
        let b = &mut self.bufs[i];
        let conv = |b: &Buffer, p: &Value| -> Pos {
            let l = p["line"].as_u64().unwrap_or(0) as usize;
            let c = p["character"].as_u64().unwrap_or(0) as usize;
            if l > b.last() {
                (b.line_len(b.last()), b.last())
            } else {
                (from_lsp_col(&b.lines[l], c, enc), l)
            }
        };
        let mut es: Vec<(Pos, Pos, Vec<char>)> = edits
            .iter()
            .filter_map(|e| {
                let s = conv(b, &e["range"]["start"]);
                let en = conv(b, &e["range"]["end"]);
                let t: Vec<char> = e["newText"].as_str()?.replace("\r\n", "\n").chars().collect();
                Some((s, en, t))
            })
            .collect();
        es.sort_by_key(|e| std::cmp::Reverse((e.0 .1, e.0 .0)));
        let mut delta = 0isize;
        for (s, e, t) in es {
            b.delete_text(s, e);
            b.insert_text(s, &t);
            if let Some(ty) = track {
                if e.1 < ty {
                    delta += t.iter().filter(|c| **c == '\n').count() as isize - (e.1 - s.1) as isize;
                }
            }
        }
        b.touch();
        delta
    }

    pub(crate) fn word_start(&self) -> usize {
        let b = self.buf();
        let line = &b.lines[b.cy];
        let mut s = b.cx.min(line.len());
        while s > 0 && is_ident(line[s - 1]) {
            s -= 1;
        }
        s
    }

    pub(crate) fn trigger_completion(&mut self, manual: bool, trig: Option<char>) {
        let lang = self.cur_ready().filter(|l| self.lsp.cap(l, "completionProvider"));
        let Some(lang) = lang else {
            return self.word_completion(manual);
        };
        self.lsp_sync(self.cur);
        if let Some(old) = self.comp_id.take() {
            self.lsp.cancel(lang, old);
        }
        let mut p = self.tdpp(lang);
        p["context"] = match trig {
            Some(c) => json!({ "triggerKind": 2, "triggerCharacter": c.to_string() }),
            None => json!({ "triggerKind": if manual { 1 } else { 3 } }),
        };
        if trig.is_none() && !manual {
            p["context"] = json!({ "triggerKind": 1 });
        }
        self.comp_id = self
            .lsp
            .request(lang, "textDocument/completion", p, Pending::Completion);
    }

    fn word_completion(&mut self, manual: bool) {
        let start = self.word_start();
        let b = self.buf();
        let prefix: String = b.lines[b.cy][start..b.cx.min(b.line_len(b.cy))].iter().collect();
        if prefix.chars().count() < if manual { 1 } else { 3 } {
            if !manual {
                self.popup = None;
            }
            return;
        }
        let mut words: HashSet<String> = HashSet::new();
        for buf in &self.bufs {
            for line in buf.lines.iter().take(20_000) {
                let mut cur = String::new();
                for &c in line.iter().chain(std::iter::once(&' ')) {
                    if is_ident(c) {
                        cur.push(c);
                    } else {
                        if cur.chars().count() >= 3 && !cur.chars().next().unwrap().is_ascii_digit() {
                            words.insert(std::mem::take(&mut cur));
                        }
                        cur.clear();
                    }
                }
            }
        }
        words.remove(&prefix);
        if words.is_empty() {
            self.popup = None;
            return;
        }
        let items = words
            .into_iter()
            .map(|w| CItem {
                filter: w.to_lowercase(),
                sort: w.clone(),
                text: w.clone(),
                label: w,
                kind: 1,
                detail: String::new(),
                snippet: false,
                start: None,
                end: None,
                extra: Vec::new(),
                raw: Value::Null,
            })
            .collect();
        let y = self.buf().cy;
        self.popup = Some(Popup {
            items,
            shown: Vec::new(),
            sel: 0,
            scroll: 0,
            y,
            start,
            lsp: false,
            incomplete: false,
        });
        self.refilter();
    }

    fn on_completion(&mut self, lang: &'static str, r: Value) {
        if self.mode != Mode::Insert || self.focus != Focus::Editor {
            return;
        }
        let enc = self.lsp.enc(lang);
        let (list, incomplete) = if r.is_array() {
            (r.as_array().cloned().unwrap_or_default(), false)
        } else {
            (
                r["items"].as_array().cloned().unwrap_or_default(),
                r["isIncomplete"].as_bool().unwrap_or(false),
            )
        };
        let b = self.buf();
        let y = b.cy;
        let line = &b.lines[y];
        let items: Vec<CItem> = list
            .iter()
            .map(|it| {
                let label = it["label"].as_str().unwrap_or("").to_string();
                let te = &it["textEdit"];
                let range = if te["range"].is_object() {
                    &te["range"]
                } else {
                    &te["replace"]
                };
                let (text, start, end) = match te["newText"].as_str() {
                    Some(t) if range["start"]["line"].as_u64() == Some(y as u64) => {
                        let s = from_lsp_col(line, range["start"]["character"].as_u64().unwrap_or(0) as usize, enc);
                        let e = from_lsp_col(line, range["end"]["character"].as_u64().unwrap_or(0) as usize, enc);
                        (t.to_string(), Some(s), Some(e))
                    }
                    Some(t) => (t.to_string(), None, None),
                    None => (it["insertText"].as_str().unwrap_or(&label).to_string(), None, None),
                };
                let mut detail = it["detail"]
                    .as_str()
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if detail.is_empty() {
                    if let Some(d) = it["labelDetails"]["description"].as_str() {
                        detail = d.to_string();
                    }
                }
                CItem {
                    filter: it["filterText"].as_str().unwrap_or(&label).to_lowercase(),
                    sort: it["sortText"].as_str().unwrap_or(&label).to_string(),
                    kind: it["kind"].as_u64().unwrap_or(0),
                    detail,
                    text,
                    snippet: it["insertTextFormat"].as_u64() == Some(2),
                    start,
                    end,
                    extra: it["additionalTextEdits"].as_array().cloned().unwrap_or_default(),
                    raw: it.clone(),
                    label,
                }
            })
            .collect();
        if items.is_empty() {
            self.popup = None;
            return;
        }
        let start = self.word_start();
        self.popup = Some(Popup {
            items,
            shown: Vec::new(),
            sel: 0,
            scroll: 0,
            y,
            start,
            lsp: true,
            incomplete,
        });
        self.refilter();
    }

    pub(crate) fn refilter(&mut self) {
        let b = &self.bufs[self.cur];
        let (cx, cy) = (b.cx.min(b.line_len(b.cy)), b.cy);
        let line = &b.lines[cy];
        let Some(p) = self.popup.as_mut() else { return };
        if cy != p.y || cx < p.start {
            self.popup = None;
            return;
        }
        let prefix: String = line[p.start..cx].iter().collect::<String>().to_lowercase();
        let mut scored: Vec<(u8, usize)> = p
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| fuzzy(&prefix, &it.filter).map(|s| (s, i)))
            .collect();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| p.items[a.1].sort.cmp(&p.items[b.1].sort)));
        p.shown = scored.into_iter().map(|x| x.1).collect();
        p.sel = 0;
        p.scroll = 0;
        let only_exact = p.shown.len() == 1 && p.items[p.shown[0]].filter == prefix;
        if p.shown.is_empty() || (!p.lsp && only_exact) {
            self.popup = None;
        }
    }

    pub(crate) fn popup_move(&mut self, d: isize) {
        let Some(p) = self.popup.as_mut() else { return };
        let n = p.shown.len() as isize;
        if n == 0 {
            return;
        }
        p.sel = (p.sel as isize + d).rem_euclid(n) as usize;
        if p.sel < p.scroll {
            p.scroll = p.sel;
        } else if p.sel >= p.scroll + POPUP_ROWS {
            p.scroll = p.sel + 1 - POPUP_ROWS;
        }
    }

    pub(crate) fn popup_fully_typed(&self) -> bool {
        let Some(p) = &self.popup else { return false };
        let Some(&i) = p.shown.get(p.sel) else { return false };
        let b = self.buf();
        let typed: String = b.lines[b.cy][p.start..b.cx.min(b.line_len(b.cy))].iter().collect();
        p.items[i].text == typed && p.items[i].extra.is_empty()
    }

    pub(crate) fn accept_completion(&mut self) {
        let Some(p) = self.popup.take() else { return };
        let Some(&idx) = p.shown.get(p.sel) else { return };
        let it = &p.items[idx];
        let enc = self.lsp_lang(self.cur).map(|l| self.lsp.enc(l)).unwrap_or(Enc::Utf16);
        let unit = self.indent_unit();
        let b = &mut self.bufs[self.cur];
        let y = b.cy;
        let len = b.line_len(y);
        let cx = b.cx.min(len);
        let start = it.start.filter(|s| *s <= cx).unwrap_or(p.start).min(cx);
        let end = it.end.filter(|e| *e > cx && *e <= len).unwrap_or(cx);
        let (raw, off) = if it.snippet {
            expand_snippet(&it.text)
        } else {
            (it.text.clone(), None)
        };
        let base = leading_ws(&b.lines[y]);
        let mut chars: Vec<char> = Vec::new();
        let mut new_off = None;
        let mut line_start = false;
        for (k, c) in raw.chars().enumerate() {
            if Some(k) == off {
                new_off = Some(chars.len());
            }
            if c == '\n' {
                chars.push('\n');
                chars.extend(&base);
                line_start = true;
                continue;
            }
            if line_start && c == '\t' {
                chars.extend(&unit);
                continue;
            }
            line_start = false;
            chars.push(c);
        }
        if off.is_some_and(|o| o >= raw.chars().count()) {
            new_off = Some(chars.len());
        }
        b.delete_text((start, y), (end, y));
        let endp = b.insert_text((start, y), &chars);
        let (mut nx, mut ny) = match new_off {
            Some(o) => {
                let before = &chars[..o];
                match before.iter().rposition(|c| *c == '\n') {
                    Some(nl) => (o - nl - 1, y + before.iter().filter(|c| **c == '\n').count()),
                    None => (start + o, y),
                }
            }
            None => endp,
        };
        let resolve = it.extra.is_empty() && !it.raw.is_null();
        let raw = it.raw.clone();
        if !it.extra.is_empty() {
            let extra = it.extra.clone();
            let d = self.apply_text_edits(self.cur, &extra, enc, Some(y));
            ny = (ny as isize + d).max(0) as usize;
            if d != 0 {
                nx = nx.min(self.bufs[self.cur].line_len(ny.min(self.bufs[self.cur].last())));
            }
        }
        let b = self.buf_mut();
        b.cy = ny.min(b.last());
        b.cx = nx;
        b.sync_want();
        if resolve {
            if let Some(lang) = self.cur_ready() {
                let can = self.lsp.servers.get(lang).is_some_and(|s| {
                    s.caps["completionProvider"]["resolveProvider"]
                        .as_bool()
                        .unwrap_or(false)
                });
                if can {
                    let cur = self.cur;
                    self.lsp
                        .request(lang, "completionItem/resolve", raw, Pending::CompResolve(cur));
                }
            }
        }
        if chars.last() == Some(&')') && new_off.is_some() {
            self.trigger_signature();
        }
    }

    fn on_comp_resolve(&mut self, lang: &'static str, buf: usize, r: Value) {
        let Some(edits) = r["additionalTextEdits"].as_array().filter(|e| !e.is_empty()).cloned() else {
            return;
        };
        if buf >= self.bufs.len() {
            return;
        }
        let enc = self.lsp.enc(lang);
        let y = self.bufs[buf].cy;
        let d = self.apply_text_edits(buf, &edits, enc, Some(y));
        let b = &mut self.bufs[buf];
        b.cy = ((b.cy as isize + d).max(0) as usize).min(b.last());
        b.clamp(self.mode == Mode::Insert);
    }

    pub(crate) fn after_insert_char(&mut self, c: char) {
        if self.burst {
            self.popup = None;
            return;
        }
        let lang = self.cur_ready();
        let comp_trig = lang
            .map(|l| self.lsp.trigger_chars(l, "completionProvider"))
            .unwrap_or_default();
        if is_ident(c) {
            match &self.popup {
                Some(p) if !p.incomplete => self.refilter(),
                Some(_) => {
                    self.refilter();
                    self.trigger_completion(false, None);
                }
                None => self.trigger_completion(false, None),
            }
        } else if comp_trig.contains(&c) {
            self.popup = None;
            self.trigger_completion(false, Some(c));
        } else {
            self.popup = None;
        }
        let mut sig_trig = lang
            .map(|l| self.lsp.trigger_chars(l, "signatureHelpProvider"))
            .unwrap_or_default();
        if sig_trig.is_empty() {
            sig_trig = vec!['(', ','];
        }
        if sig_trig.contains(&c) {
            self.trigger_signature();
        } else if c == ')' {
            self.sig = None;
        }
    }

    pub(crate) fn trigger_signature(&mut self) {
        let Some(lang) = self.cur_ready().filter(|l| self.lsp.cap(l, "signatureHelpProvider")) else {
            return;
        };
        self.lsp_sync(self.cur);
        let p = self.tdpp(lang);
        self.lsp
            .request(lang, "textDocument/signatureHelp", p, Pending::Signature);
    }

    pub(crate) fn menu_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let Some(m) = self.menu.as_mut() else { return };
        let n = m.items.len();
        if m.filter.is_some() {
            match k.code {
                KeyCode::Char(c) if !ctrl => {
                    m.filter.as_mut().unwrap().push(c);
                    m.apply_filter();
                    if m.ws_symbols {
                        let q = m.filter.clone().unwrap_or_default();
                        self.ws_symbols_query(&q);
                    }
                    return;
                }
                KeyCode::Backspace => {
                    m.filter.as_mut().unwrap().pop();
                    m.apply_filter();
                    if m.ws_symbols {
                        let q = m.filter.clone().unwrap_or_default();
                        self.ws_symbols_query(&q);
                    }
                    return;
                }
                KeyCode::Enter if n == 0 && m.custom_model => {
                    let f = m.filter.clone().unwrap_or_default();
                    self.menu = None;
                    if !f.trim().is_empty() {
                        self.ai.set_model(f.trim());
                        self.info(tf!("Model: {}", "Model: {}", f.trim()));
                    }
                    return;
                }
                KeyCode::Up
                | KeyCode::Down
                | KeyCode::Enter
                | KeyCode::Esc
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Tab
                | KeyCode::BackTab => {}
                _ => return,
            }
        }
        if let Some(d) = m.dir.clone() {
            match k.code {
                KeyCode::Backspace | KeyCode::Left => {
                    if let Some(p) = d.parent() {
                        self.folder_picker(p.to_path_buf(), Some(d));
                    }
                    return;
                }
                KeyCode::Right => {
                    let s = m.sel;
                    return self.menu_choose(s);
                }
                KeyCode::Char(c) if c.is_alphabetic() && !ctrl => {
                    let c = c.to_lowercase().next().unwrap_or(c);
                    let name = |t: &str| t.trim_start_matches(|ch: char| !ch.is_alphanumeric()).to_lowercase();
                    let start = m.sel + 1;
                    if let Some(i) = (0..n)
                        .map(|k| (start + k) % n)
                        .find(|&i| name(&m.items[i].0).starts_with(c))
                    {
                        m.sel = i;
                    }
                    return self.menu_follow();
                }
                _ => {}
            }
        }
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => self.menu = None,
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => m.sel = (m.sel + 1) % n.max(1),
            KeyCode::Char('n') if ctrl => m.sel = (m.sel + 1) % n.max(1),
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => m.sel = (m.sel + n.max(1) - 1) % n.max(1),
            KeyCode::Char('p') if ctrl => m.sel = (m.sel + n.max(1) - 1) % n.max(1),
            KeyCode::PageDown => m.sel = (m.sel + MENU_ROWS).min(n.saturating_sub(1)),
            KeyCode::PageUp => m.sel = m.sel.saturating_sub(MENU_ROWS),
            KeyCode::Home | KeyCode::Char('g') => m.sel = 0,
            KeyCode::End | KeyCode::Char('G') => m.sel = n.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char('l') => {
                let s = m.sel;
                return self.menu_choose(s);
            }
            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                let i = c.to_digit(10).unwrap() as usize - 1;
                if i < n {
                    return self.menu_choose(i);
                }
            }
            _ => {}
        }
        self.menu_follow();
    }

    pub(crate) fn menu_follow(&mut self) {
        if let Some(m) = self.menu.as_mut() {
            if m.sel < m.scroll {
                m.scroll = m.sel;
            } else if m.sel >= m.scroll + MENU_ROWS {
                m.scroll = m.sel + 1 - MENU_ROWS;
            }
        }
    }

    pub(crate) fn menu_choose(&mut self, idx: usize) {
        let Some(m) = self.menu.take() else { return };
        let Some((_, act)) = m.items.into_iter().nth(idx) else {
            return;
        };
        match act {
            MenuAction::Code(lang, v) => self.run_action(lang, v, true),
            MenuAction::Jump(p, l, c, e) => self.jump_to(&p, l, c, e),
            MenuAction::Cmd(c) => self.context_cmd(c),
            MenuAction::Dir(p) => self.folder_picker(p, None),
            MenuAction::Model(name) => {
                self.ai.set_model(&name);
                self.info(tf!("Model: {}", "Model: {}", name));
            }
            MenuAction::Chat(id) => {
                if self.ai.load_chat(&id) {
                    self.ai.visible = true;
                    self.focus = Focus::Ai;
                }
            }
            MenuAction::AiProvider(p) => {
                self.ai.set_provider(p);
                self.info(tf!("Sağlayıcı: {}", "Provider: {}", p.id()));
            }
            MenuAction::OpenDir(p) => {
                self.open_path(&p);
                let name = self.tree.root_name();
                self.info(tf!("Klasör açıldı: {}", "Opened folder: {}", name));
            }
        }
    }

    pub(crate) fn folder_picker(&mut self, dir: PathBuf, from: Option<PathBuf>) {
        self.back_to_normal();
        let mut items: Vec<(String, MenuAction)> = vec![(
            t!("✓  Bu klasörü aç", "✓  Open this folder").into(),
            MenuAction::OpenDir(dir.clone()),
        )];
        match dir.parent() {
            Some(p) => items.push(("↑  ..".into(), MenuAction::Dir(p.to_path_buf()))),
            None if cfg!(windows) => {
                for l in 'C'..='Z' {
                    let d = PathBuf::from(format!("{}:\\", l));
                    if d != dir && d.exists() {
                        items.push((format!("▸  {}:\\", l), MenuAction::Dir(d)));
                    }
                }
            }
            None => {}
        }
        let mut subs: Vec<(String, PathBuf)> = fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                    .filter(|(n, _)| !n.starts_with('.') && !n.starts_with('$'))
                    .collect()
            })
            .unwrap_or_default();
        subs.sort_by_key(|(n, _)| n.to_lowercase());
        let mut sel = 0;
        for (n, p) in subs {
            if from.as_ref() == Some(&p) {
                sel = items.len();
            }
            items.push((format!("▸  {}", n), MenuAction::Dir(p)));
        }
        let full = dir.display().to_string();
        let chars: Vec<char> = full.chars().collect();
        let title = if chars.len() > 50 {
            format!("…{}", chars[chars.len() - 49..].iter().collect::<String>())
        } else {
            full
        };
        self.menu = Some(Menu {
            title,
            items,
            sel,
            scroll: 0,
            anchor: None,
            dir: Some(dir),
            filter: None,
            base: Vec::new(),
            custom_model: false,
            ws_symbols: false,
        });
        self.menu_follow();
    }

    pub(crate) fn open_context_menu(&mut self, x: u16, y: u16) {
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        let mut items: Vec<(String, MenuAction)> = Vec::new();
        if self.cur_ready().is_some() {
            items.push((
                t!("Tanıma git          gd", "Go to definition    gd").into(),
                MenuAction::Cmd("def"),
            ));
            items.push((
                t!("Referansları bul    gr", "Find references     gr").into(),
                MenuAction::Cmd("refs"),
            ));
            items.push((
                t!("Kod eylemleri       ga", "Code actions        ga").into(),
                MenuAction::Cmd("actions"),
            ));
            items.push((
                t!("Bilgi göster        K", "Show info           K").into(),
                MenuAction::Cmd("hover"),
            ));
            items.push((
                t!("Yeniden adlandır    F2", "Rename              F2").into(),
                MenuAction::Cmd("rename"),
            ));
            items.push((t!("Biçimlendir", "Format document").into(), MenuAction::Cmd("format")));
            items.push((
                t!("Semboller           gs", "Symbols             gs").into(),
                MenuAction::Cmd("symbols"),
            ));
        }
        if visual {
            items.push((
                t!("Kopyala             Ctrl+c", "Copy                Ctrl+c").into(),
                MenuAction::Cmd("copy"),
            ));
            items.push((
                t!("Kes                 Ctrl+x", "Cut                 Ctrl+x").into(),
                MenuAction::Cmd("cut"),
            ));
        } else {
            items.push((
                t!("Satırı kopyala      yy", "Copy line           yy").into(),
                MenuAction::Cmd("copy"),
            ));
        }
        items.push((
            t!("Yapıştır            p", "Paste               p").into(),
            MenuAction::Cmd("paste"),
        ));
        items.push((t!("Tümünü seç", "Select all").into(), MenuAction::Cmd("selectall")));
        items.push((
            t!("Bul ve değiştir     Ctrl+H", "Find and replace    Ctrl+H").into(),
            MenuAction::Cmd("find"),
        ));
        self.ctx_pos = Some((x, y));
        self.menu = Some(Menu {
            title: String::new(),
            items,
            sel: 0,
            scroll: 0,
            anchor: Some((x, y)),
            dir: None,
            filter: None,
            base: Vec::new(),
            custom_model: false,
            ws_symbols: false,
        });
    }

    fn context_cmd(&mut self, c: &str) {
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        match c {
            "def" => self.lsp_definition(),
            "refs" => self.lsp_references(),
            "actions" => {
                self.menu_anchor = self.ctx_pos;
                self.lsp_code_actions();
            }
            "hover" => self.lsp_hover(),
            "rename" => self.start_rename(),
            "format" => self.lsp_format(),
            "symbols" => self.lsp_document_symbols(),
            "find" => self.open_find(true),
            "copy" => {
                if visual {
                    self.visual_op('y');
                } else {
                    self.op_line_n('y');
                }
                self.info(t!("Panoya kopyalandı", "Copied to clipboard"));
            }
            "cut" => self.visual_op('d'),
            "paste" => {
                if visual {
                    self.visual_paste();
                } else {
                    self.paste(true);
                }
            }
            "ai_provider" => self.ai_provider_menu(),
            "ai_new" => {
                self.ai.new_chat();
                self.ai.visible = true;
                self.focus = Focus::Ai;
            }
            "selectall" => {
                self.anchor = (0, 0);
                let b = self.buf_mut();
                b.cy = b.last();
                b.cx = b.line_len(b.cy);
                self.mode = Mode::VisualLine;
            }
            _ => {}
        }
    }

    pub(crate) fn push_jump(&mut self) {
        let b = self.buf();
        if let Some(p) = &b.path {
            self.jumps.push((p.clone(), (b.cx, b.cy)));
            if self.jumps.len() > 100 {
                self.jumps.remove(0);
            }
        }
    }

    pub(crate) fn jump_to(&mut self, path: &Path, line: usize, col: usize, enc: Enc) {
        self.push_jump();
        self.open_path(path);
        self.focus = Focus::Editor;
        let b = self.buf_mut();
        b.cy = line.min(b.last());
        b.cx = from_lsp_col(&b.lines[b.cy], col, enc);
        b.sync_want();
    }

    pub(crate) fn jump_back(&mut self) {
        let Some((p, (x, y))) = self.jumps.pop() else {
            return self.info(t!("Geri dönülecek konum yok", "Jump list is empty"));
        };
        self.open_path(&p);
        let b = self.buf_mut();
        b.cy = y.min(b.last());
        b.cx = x;
        b.clamp(false);
        b.sync_want();
    }

    pub(crate) fn lsp_command(&mut self, arg: &str) {
        let lang = self.buf().lang.name;
        match arg {
            "restart" | "start" => {
                self.lsp.stop(lang);
                self.lsp.failed.remove(lang);
                for i in 0..self.bufs.len() {
                    if self.bufs[i].lang.name == lang {
                        self.bufs[i].lsp_open = false;
                    }
                }
                self.info(t!("Dil sunucusu yeniden başlatılıyor…", "Restarting language server…"));
            }
            "stop" => {
                self.lsp.stop(lang);
                self.lsp.failed.insert(lang);
                for b in self.bufs.iter_mut() {
                    if b.lang.name == lang {
                        b.lsp_open = false;
                        if let Some(p) = &b.path {
                            self.lsp.diags.remove(&path_key(p));
                        }
                    }
                }
                self.info(t!("Dil sunucusu durduruldu", "Language server stopped"));
            }
            _ => {
                if self.lsp.servers.is_empty() {
                    let c: Vec<&str> = candidates(lang).iter().map(|c| c[0]).collect();
                    if c.is_empty() {
                        self.info(tf!(
                            "{} için tanımlı dil sunucusu yok",
                            "No language server configured for {}",
                            lang
                        ));
                    } else {
                        self.info(tf!(
                            "Çalışan sunucu yok. {} için aranan: {}",
                            "No server running. Looking for ({}): {}",
                            lang,
                            c.join(", ")
                        ));
                    }
                    return;
                }
                let s = self
                    .lsp
                    .servers
                    .iter()
                    .map(|(l, s)| {
                        format!(
                            "{}: {} [{}] {}",
                            l,
                            s.name,
                            if s.ready {
                                t!("hazır", "ready")
                            } else {
                                t!("başlıyor", "starting")
                            },
                            s.root.display()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("  |  ");
                self.info(s);
            }
        }
    }

    pub(crate) fn lsp_document_symbols(&mut self) {
        self.back_to_normal();
        let Some(lang) = self.ready_lang("documentSymbolProvider") else {
            return;
        };
        let uri = path_to_uri(self.buf().path.as_ref().unwrap());
        self.lsp.request(
            lang,
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
            Pending::DocSymbols,
        );
    }

    pub(crate) fn workspace_symbols(&mut self) {
        self.back_to_normal();
        let Some(_) = self.ready_lang("workspaceSymbolProvider") else {
            return;
        };
        self.menu = Some(Menu {
            title: t!("Proje sembolleri", "Workspace symbols").to_string(),
            items: Vec::new(),
            sel: 0,
            scroll: 0,
            anchor: None,
            dir: None,
            filter: Some(String::new()),
            base: Vec::new(),
            custom_model: false,
            ws_symbols: true,
        });
    }

    fn ws_symbols_query(&mut self, q: &str) {
        let Some(lang) = self.cur_ready() else { return };
        if let Some(old) = self.ws_sym_id.take() {
            self.lsp.cancel(lang, old);
        }
        if q.is_empty() {
            return;
        }
        self.ws_sym_id = self
            .lsp
            .request(lang, "workspace/symbol", json!({ "query": q }), Pending::WsSymbols);
    }

    fn symbol_label(kind: u64) -> &'static str {
        match kind {
            1 => "file",
            2 => "mod",
            3 => "ns",
            4 => "pkg",
            5 => "class",
            6 => "meth",
            7 => "prop",
            8 => "field",
            9 => "new",
            10 => "enum",
            11 => "trait",
            12 => "fn",
            13 => "var",
            14 => "const",
            15 => "str",
            16 => "num",
            17 => "bool",
            18 => "arr",
            19 => "obj",
            20 => "key",
            21 => "null",
            22 => "member",
            23 => "struct",
            24 => "event",
            25 => "op",
            26 => "type",
            _ => "",
        }
    }

    fn on_doc_symbols(&mut self, lang: &'static str, r: Value) {
        let enc = self.lsp.enc(lang);
        let Some(path) = self.buf().path.clone() else { return };
        let mut flat: Vec<(usize, u64, String, String, usize, usize)> = Vec::new();
        fn walk(v: &Value, depth: usize, out: &mut Vec<(usize, u64, String, String, usize, usize)>) {
            for s in v.as_array().into_iter().flatten() {
                let range = if s["selectionRange"].is_object() {
                    &s["selectionRange"]
                } else {
                    &s["location"]["range"]
                };
                out.push((
                    depth,
                    s["kind"].as_u64().unwrap_or(0),
                    s["name"].as_str().unwrap_or("").to_string(),
                    s["detail"]
                        .as_str()
                        .unwrap_or("")
                        .lines()
                        .next()
                        .unwrap_or("")
                        .to_string(),
                    range["start"]["line"].as_u64().unwrap_or(0) as usize,
                    range["start"]["character"].as_u64().unwrap_or(0) as usize,
                ));
                walk(&s["children"], depth + 1, out);
            }
        }
        walk(&r, 0, &mut flat);
        if flat.is_empty() {
            return self.info(t!("Sembol bulunamadı", "No symbols found"));
        }
        if flat.iter().all(|f| f.0 == 0) {
            flat.sort_by_key(|f| (f.4, f.5));
        }
        let cy = self.buf().cy;
        let sel = flat.iter().rposition(|f| f.4 <= cy).unwrap_or(0);
        let items: Vec<(String, MenuAction)> = flat
            .into_iter()
            .map(|(depth, kind, name, detail, line, col)| {
                let mut label = format!("{}{:<7}{}", "  ".repeat(depth), Self::symbol_label(kind), name);
                if !detail.is_empty() {
                    label = format!("{}  {}", label, detail);
                }
                (label, MenuAction::Jump(path.clone(), line, col, enc))
            })
            .collect();
        let n = items.len();
        let mut m = Menu::filterable(tf!("Semboller ({})", "Symbols ({})", n), items, sel, false);
        m.sel = sel;
        self.menu = Some(m);
        self.menu_follow();
    }

    fn on_ws_symbols(&mut self, lang: &'static str, id: i64, r: Value) {
        if self.ws_sym_id != Some(id) {
            return;
        }
        self.ws_sym_id = None;
        let enc = self.lsp.enc(lang);
        let items: Vec<(String, MenuAction)> = r
            .as_array()
            .into_iter()
            .flatten()
            .take(300)
            .filter_map(|s| {
                let loc = &s["location"];
                let path = loc["uri"].as_str().and_then(uri_to_path)?;
                let line = loc["range"]["start"]["line"].as_u64().unwrap_or(0) as usize;
                let col = loc["range"]["start"]["character"].as_u64().unwrap_or(0) as usize;
                let kind = Self::symbol_label(s["kind"].as_u64().unwrap_or(0));
                let name = s["name"].as_str().unwrap_or("");
                let container = s["containerName"].as_str().filter(|c| !c.is_empty());
                let mut label = format!("{:<7}{}", kind, name);
                if let Some(c) = container {
                    label = format!("{}  ({})", label, c);
                }
                label = format!("{}  {}:{}", label, self.rel(&path), line + 1);
                Some((label, MenuAction::Jump(path, line, col, enc)))
            })
            .collect();
        if let Some(m) = self.menu.as_mut().filter(|m| m.ws_symbols) {
            m.base = items.clone();
            m.items = items;
            m.sel = 0;
            m.scroll = 0;
        }
    }
}
