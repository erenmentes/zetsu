use super::*;
use crate::ai::{code_blocks, AiEvent, Ctx, Msg, Provider, Role};
use crate::buffer::char_width;
use crate::render::{put_clip, text_w, BAR, BRIGHT, DIM, ERR, LOGO_GREEN, MACRO, POP, SEL, STR, TYPE};
use crate::screen::{Screen, Style};
use ide::{Menu, MenuAction};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AiHit {
    Close,
    Model,
    New,
    History,
    Context,
    Apply(usize, usize),
    Copy(usize, usize),
    Input,
}

type Cells = Vec<(char, Style)>;

struct PLine {
    cells: Cells,
    bg: Option<crossterm::style::Color>,
    hits: Vec<(u16, u16, AiHit)>,
}

impl PLine {
    fn new(cells: Cells) -> Self {
        PLine {
            cells,
            bg: None,
            hits: Vec::new(),
        }
    }
}

fn cells(s: &str, st: Style) -> Cells {
    s.chars().map(|c| (if c == '\t' { ' ' } else { c }, st)).collect()
}

fn cells_w(c: &[(char, Style)]) -> usize {
    c.iter().map(|x| char_width(x.0, 0)).sum()
}

fn wrap(src: &[(char, Style)], width: usize, hard: bool) -> Vec<Cells> {
    let width = width.max(4);
    let mut lines = Vec::new();
    let mut cur: Cells = Vec::new();
    let mut w = 0;
    for &(c, st) in src {
        let cw = char_width(c, 0);
        if w + cw > width && !cur.is_empty() {
            let sp = if hard {
                None
            } else {
                cur.iter().rposition(|x| x.0 == ' ')
            };
            match sp {
                Some(i) if i > 0 => {
                    let rest = cur.split_off(i + 1);
                    lines.push(std::mem::replace(&mut cur, rest));
                }
                _ => lines.push(std::mem::take(&mut cur)),
            }
            w = cells_w(&cur);
        }
        cur.push((c, st));
        w += cw;
    }
    lines.push(cur);
    lines
}

fn inline_md(line: &str, base: Style) -> Cells {
    let mut out = Vec::new();
    let mut code = false;
    let mut bold = false;
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            code = !code;
            i += 1;
            continue;
        }
        if !code && c == '*' && chars.get(i + 1) == Some(&'*') {
            bold = !bold;
            i += 2;
            continue;
        }
        let mut st = if code { Style::fg(MACRO) } else { base };
        if bold {
            st = st.bold();
        }
        out.push((if c == '\t' { ' ' } else { c }, st));
        i += 1;
    }
    out
}

impl Editor {
    pub(crate) fn ai_toggle(&mut self) {
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        if self.ai.visible && self.focus == Focus::Ai && !visual {
            self.ai.visible = false;
            self.focus = Focus::Editor;
            return;
        }
        if visual {
            self.ai.sel_ctx = self.selection_ctx();
        }
        self.back_to_normal();
        self.ai.visible = true;
        self.focus = Focus::Ai;
        if self.ai.model.is_empty() && (!self.ai.needs_key() || !self.ai.key().is_empty()) {
            self.ai_pick_pending = true;
            self.ai.fetch_models();
        }
    }

    pub(crate) fn ai_close(&mut self) {
        self.ai.visible = false;
        if self.focus == Focus::Ai {
            self.focus = Focus::Editor;
        }
    }

    fn lang_tag(&self, i: usize) -> String {
        let b = &self.bufs[i];
        match &b.path {
            Some(p) => crate::lsp::language_id(b.lang.name, p).to_string(),
            None => String::new(),
        }
    }

    fn selection_ctx(&self) -> Option<Ctx> {
        let (a, e) = self.selection();
        let b = self.buf();
        let (y0, y1) = (a.1.min(b.last()), e.1.min(b.last()));
        let content: String = b.lines[y0..=y1]
            .iter()
            .map(|l| l.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        Some(Ctx {
            path: b.path.clone(),
            label: format!("{}:{}-{}", self.rel_path(b), y0 + 1, y1 + 1),
            lang: self.lang_tag(self.cur),
            content,
            sel: Some((y0, y1)),
        })
    }

    fn file_ctx(&self) -> Option<Ctx> {
        let b = self.buf();
        if b.path.is_none() && b.lines.len() == 1 && b.lines[0].is_empty() {
            return None;
        }
        if b.path.is_none() && b.name != NONAME {
            return None;
        }
        let mut content = String::new();
        let mut truncated = false;
        for (i, l) in b.lines.iter().enumerate() {
            if i >= 3000 || content.len() > 120_000 {
                truncated = true;
                break;
            }
            if i > 0 {
                content.push('\n');
            }
            content.extend(l.iter());
        }
        if truncated {
            content.push_str("\n... (truncated)");
        }
        Some(Ctx {
            path: b.path.clone(),
            label: self.rel_path(b),
            lang: self.lang_tag(self.cur),
            content,
            sel: None,
        })
    }

    pub(crate) fn ai_send(&mut self) {
        if self.ai.streaming.is_some() {
            return;
        }
        let text: String = self.ai.input.iter().collect::<String>().trim().to_string();
        if text.is_empty() {
            return;
        }
        if self.ai.needs_key() && self.ai.key().is_empty() {
            return self.error(t!("API anahtarı yok — :ai key <anahtar>", "No API key — :ai key <key>"));
        }
        if self.ai.model.is_empty() {
            self.ai_pick_pending = true;
            self.ai.fetch_models();
            return self.info(t!("Önce bir model seçin", "Pick a model first"));
        }
        let mut ctx = self.ai.sel_ctx.take();
        if ctx.is_none() && self.ai.attach {
            ctx = self.file_ctx().filter(|c| {
                !self
                    .ai
                    .msgs
                    .iter()
                    .rev()
                    .filter_map(|m| m.ctx.as_ref())
                    .any(|p| p.path == c.path && p.content == c.content)
            });
        }
        self.ai.msgs.push(Msg {
            role: Role::User,
            text,
            ctx,
            error: false,
        });
        let history: Vec<Msg> = self.ai.msgs.iter().filter(|m| !m.error).cloned().collect();
        self.ai.msgs.push(Msg {
            role: Role::Assistant,
            text: String::new(),
            ctx: None,
            error: false,
        });
        self.ai.input.clear();
        self.ai.input_x = 0;
        self.ai.scroll = 0;
        self.ai.send(history);
    }

    pub fn ai_tick(&mut self) -> bool {
        let events = self.ai.poll();
        if events.is_empty() {
            return false;
        }
        for ev in events {
            match ev {
                AiEvent::Delta(id, s) => {
                    if self.ai.streaming.as_ref().map(|x| x.0) == Some(id) {
                        if let Some(m) = self.ai.msgs.last_mut() {
                            m.text.push_str(&s);
                        }
                    }
                }
                AiEvent::Done(id) => {
                    if self.ai.streaming.as_ref().map(|x| x.0) == Some(id) {
                        self.ai.streaming = None;
                    }
                    if self
                        .ai
                        .msgs
                        .last()
                        .is_some_and(|m| m.role == Role::Assistant && m.text.is_empty())
                    {
                        self.ai.msgs.pop();
                    }
                    self.ai.save_chat();
                }
                AiEvent::Error(id, e) => {
                    if self.ai.streaming.as_ref().map(|x| x.0) == Some(id) {
                        self.ai.streaming = None;
                    }
                    if self
                        .ai
                        .msgs
                        .last()
                        .is_some_and(|m| m.role == Role::Assistant && m.text.is_empty())
                    {
                        self.ai.msgs.pop();
                    }
                    self.ai.msgs.push(Msg {
                        role: Role::Assistant,
                        text: e,
                        ctx: None,
                        error: true,
                    });
                    self.ai.save_chat();
                }
                AiEvent::Models(list) => {
                    self.ai.loading_models = false;
                    self.ai.models = list;
                    if self.ai_pick_pending {
                        self.ai_pick_pending = false;
                        self.ai_model_menu();
                    }
                }
                AiEvent::ModelsError(e) => {
                    self.ai.loading_models = false;
                    if self.ai_pick_pending {
                        self.ai_pick_pending = false;
                        self.ai_model_menu();
                    }
                    self.error(tf!("Model listesi alınamadı: {}", "Could not load models: {}", e));
                }
            }
        }
        true
    }

    pub(crate) fn ai_model_menu(&mut self) {
        if self.ai.models.is_empty() && !self.ai.loading_models && (!self.ai.needs_key() || !self.ai.key().is_empty()) {
            self.ai_pick_pending = true;
            self.ai.fetch_models();
            return self.info(t!("Modeller yükleniyor…", "Loading models…"));
        }
        let mut items: Vec<(String, MenuAction)> = vec![(
            tf!("⚙  Sağlayıcı: {}", "⚙  Provider: {}", self.ai.provider.id()),
            MenuAction::Cmd("ai_provider"),
        )];
        let mut sel = if self.ai.models.is_empty() { 0 } else { 1 };
        for m in &self.ai.models {
            if *m == self.ai.model {
                sel = items.len();
            }
            let mark = if *m == self.ai.model { "● " } else { "  " };
            items.push((format!("{}{}", mark, m), MenuAction::Model(m.clone())));
        }
        self.menu = Some(Menu::filterable(t!("Model", "Model").to_string(), items, sel, true));
        self.menu_follow();
    }

    pub(crate) fn ai_provider_menu(&mut self) {
        let items = [Provider::OpenAI, Provider::Anthropic, Provider::Gemini]
            .iter()
            .map(|p| {
                let label = match p {
                    Provider::OpenAI => t!(
                        "OpenAI uyumlu (OpenAI, OpenRouter, Groq, DeepSeek, Ollama…)",
                        "OpenAI-compatible (OpenAI, OpenRouter, Groq, DeepSeek, Ollama…)"
                    ),
                    Provider::Anthropic => "Anthropic",
                    Provider::Gemini => "Google Gemini",
                };
                let mark = if *p == self.ai.provider { "● " } else { "  " };
                (format!("{}{}", mark, label), MenuAction::AiProvider(*p))
            })
            .collect();
        self.menu = Some(Menu {
            title: t!("Sağlayıcı", "Provider").to_string(),
            items,
            sel: 0,
            scroll: 0,
            anchor: None,
            dir: None,
            filter: None,
            base: Vec::new(),
            custom_model: false,
        });
    }

    pub(crate) fn ai_history_menu(&mut self) {
        let mut items: Vec<(String, MenuAction)> = vec![(
            t!("+  Yeni sohbet", "+  New chat").to_string(),
            MenuAction::Cmd("ai_new"),
        )];
        for c in self.ai.list_chats().into_iter().take(200) {
            let age = crate::ai::now().saturating_sub(c.updated);
            let when = if age < 3600 {
                tf!("{} dk", "{}m", age / 60)
            } else if age < 86400 {
                tf!("{} sa", "{}h", age / 3600)
            } else {
                tf!("{} gün", "{}d", age / 86400)
            };
            let title = if c.title.is_empty() { "…".to_string() } else { c.title };
            items.push((format!("{:>6}  {}", when, title), MenuAction::Chat(c.id)));
        }
        self.menu = Some(Menu::filterable(
            t!("Sohbet geçmişi", "Chat history").to_string(),
            items,
            0,
            false,
        ));
    }

    pub(crate) fn ai_command(&mut self, arg: &str) {
        let (sub, val) = match arg.split_once(char::is_whitespace) {
            Some((a, b)) => (a, b.trim()),
            None => (arg, ""),
        };
        match sub {
            "" | "toggle" => self.ai_toggle(),
            "close" => self.ai_close(),
            "open" => {
                if !self.ai.visible || self.focus != Focus::Ai {
                    self.ai_toggle();
                }
            }
            "key" => {
                if val.is_empty() {
                    let st = if self.ai.key().is_empty() {
                        t!("yok", "not set")
                    } else {
                        t!("ayarlı", "set")
                    };
                    return self.info(tf!("{} anahtarı: {}", "{} key: {}", self.ai.provider.id(), st));
                }
                match self.ai.set_key(val) {
                    Ok(()) => {
                        self.info(tf!("{} anahtarı kaydedildi", "{} key saved", self.ai.provider.id()));
                        if self.ai.model.is_empty() {
                            self.ai_pick_pending = true;
                            self.ai.fetch_models();
                        }
                    }
                    Err(e) => self.error(tf!("Ayar yazılamadı: {}", "Could not write config: {}", e)),
                }
            }
            "provider" => match Provider::parse(val) {
                Some(p) => {
                    self.ai.set_provider(p);
                    self.info(tf!("Sağlayıcı: {}", "Provider: {}", p.id()));
                }
                None if val.is_empty() => self.ai_provider_menu(),
                None => self.error(t!(
                    "Kullanım: :ai provider openai|anthropic|gemini",
                    "Usage: :ai provider openai|anthropic|gemini"
                )),
            },
            "model" => {
                if val.is_empty() {
                    self.ai_model_menu();
                } else {
                    self.ai.set_model(val);
                    self.info(tf!("Model: {}", "Model: {}", val));
                }
            }
            "url" => {
                if val.is_empty() {
                    return self.info(self.ai.base_url.clone());
                }
                let u = if val == "reset" {
                    self.ai.provider.default_url().to_string()
                } else {
                    val.to_string()
                };
                self.ai.set_url(&u);
                self.info(tf!("Adres: {}", "Endpoint: {}", u));
            }
            "new" => {
                self.ai.new_chat();
                self.info(t!("Yeni sohbet", "New chat"));
            }
            "history" => self.ai_history_menu(),
            "delete" => {
                if !self.ai.chat_id.is_empty() {
                    let id = self.ai.chat_id.clone();
                    self.ai.delete_chat(&id);
                }
                self.ai.new_chat();
                self.info(t!("Sohbet silindi", "Chat deleted"));
            }
            "apply" | "copy" => {
                let n: usize = val.parse().unwrap_or(1).max(1);
                let Some(mi) = self.ai.msgs.iter().rposition(|m| m.role == Role::Assistant && !m.error) else {
                    return self.error(t!("Uygulanacak cevap yok", "No reply to use"));
                };
                if sub == "apply" {
                    self.ai_apply(mi, n - 1);
                } else {
                    self.ai_copy(mi, n - 1);
                }
            }
            _ => self.error(t!(
                "Kullanım: :ai [key|provider|model|url|new|history|delete|apply N|copy N|close]",
                "Usage: :ai [key|provider|model|url|new|history|delete|apply N|copy N|close]"
            )),
        }
    }

    pub(crate) fn ai_apply(&mut self, mi: usize, bi: usize) {
        let Some(msg) = self.ai.msgs.get(mi) else { return };
        let Some((_, code)) = code_blocks(&msg.text).into_iter().nth(bi) else {
            return self.error(t!("Kod bloğu bulunamadı", "Code block not found"));
        };
        let lines: Vec<Vec<char>> = code.split('\n').map(|l| l.chars().collect()).collect();
        let target = self.ai.msgs[..mi]
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .and_then(|m| m.ctx.clone())
            .filter(|c| c.sel.is_some() && c.path.is_some());
        if let Some(c) = target {
            let key = path_key(c.path.as_ref().unwrap());
            if let Some(i) = self
                .bufs
                .iter()
                .position(|b| b.path.as_ref().map(|p| path_key(p)) == Some(key.clone()))
            {
                let (y0, y1) = c.sel.unwrap();
                let b = &mut self.bufs[i];
                let y0 = y0.min(b.last());
                let y1 = y1.min(b.last()).max(y0);
                b.checkpoint();
                b.lines.splice(y0..=y1, lines);
                b.touch();
                b.cy = y0;
                b.cx = 0;
                self.cur = i;
                self.focus = Focus::Editor;
                return self.info(t!(
                    "Seçimin yerine uygulandı (geri almak için u)",
                    "Applied over the selection (u to undo)"
                ));
            }
        }
        let b = self.buf_mut();
        b.checkpoint();
        let y = b.cy;
        if b.lines[y].iter().all(|c| c.is_whitespace()) {
            b.lines.splice(y..=y, lines);
        } else {
            b.lines.splice(y + 1..y + 1, lines);
            b.cy = y + 1;
        }
        b.cx = 0;
        b.touch();
        self.focus = Focus::Editor;
        self.info(t!(
            "İmleç konumuna eklendi (geri almak için u)",
            "Inserted at the cursor (u to undo)"
        ));
    }

    pub(crate) fn ai_copy(&mut self, mi: usize, bi: usize) {
        let Some(msg) = self.ai.msgs.get(mi) else { return };
        let Some((_, code)) = code_blocks(&msg.text).into_iter().nth(bi) else {
            return;
        };
        let lines: Vec<Vec<char>> = code.split('\n').map(|l| l.chars().collect()).collect();
        self.set_reg(Reg::Lines(lines));
        self.info(t!("Kod panoya kopyalandı", "Code copied to clipboard"));
    }

    pub(crate) fn ai_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let ai = &mut self.ai;
        match k.code {
            KeyCode::Esc => self.focus = Focus::Editor,
            KeyCode::Enter if alt || k.modifiers.contains(KeyModifiers::SHIFT) => {
                ai.input.insert(ai.input_x, '\n');
                ai.input_x += 1;
            }
            KeyCode::Enter => self.ai_send(),
            KeyCode::Char('j') if ctrl => {
                ai.input.insert(ai.input_x, '\n');
                ai.input_x += 1;
            }
            KeyCode::Char('c') if ctrl => {
                if ai.streaming.is_some() {
                    ai.cancel();
                    if let Some(m) = ai.msgs.last_mut() {
                        if m.text.is_empty() {
                            m.text = t!("(durduruldu)", "(stopped)").to_string();
                        }
                    }
                    ai.save_chat();
                } else {
                    self.focus = Focus::Editor;
                }
            }
            KeyCode::Char('t') if ctrl => {
                if ai.sel_ctx.is_some() {
                    ai.sel_ctx = None;
                } else {
                    ai.attach = !ai.attach;
                }
            }
            KeyCode::Char('n') if ctrl => ai.new_chat(),
            KeyCode::Char('h') if ctrl => self.ai_history_menu(),
            KeyCode::Char('u') if ctrl => {
                ai.input.drain(..ai.input_x);
                ai.input_x = 0;
            }
            KeyCode::Char(':') if !ctrl && ai.input.is_empty() => self.start_cmd(Mode::Command),
            KeyCode::Char(c) if !ctrl => {
                ai.input.insert(ai.input_x, c);
                ai.input_x += 1;
            }
            KeyCode::Backspace => {
                if ai.input_x > 0 {
                    ai.input_x -= 1;
                    ai.input.remove(ai.input_x);
                }
            }
            KeyCode::Delete => {
                if ai.input_x < ai.input.len() {
                    ai.input.remove(ai.input_x);
                }
            }
            KeyCode::Left => ai.input_x = ai.input_x.saturating_sub(1),
            KeyCode::Right => ai.input_x = (ai.input_x + 1).min(ai.input.len()),
            KeyCode::Home => ai.input_x = 0,
            KeyCode::End => ai.input_x = ai.input.len(),
            KeyCode::Up => ai.scroll += 1,
            KeyCode::Down => ai.scroll = ai.scroll.saturating_sub(1),
            KeyCode::PageUp => ai.scroll += 10,
            KeyCode::PageDown => ai.scroll = ai.scroll.saturating_sub(10),
            _ => {}
        }
    }

    pub(crate) fn ai_paste(&mut self, text: &str) {
        for c in text.replace("\r\n", "\n").chars() {
            self.ai.input.insert(self.ai.input_x, c);
            self.ai.input_x += 1;
        }
    }

    pub(crate) fn ai_click(&mut self, x: u16, y: u16) {
        self.focus = Focus::Ai;
        let hit = self
            .ai_hits
            .borrow()
            .iter()
            .find(|h| h.0 == y && x >= h.1 && x < h.2)
            .map(|h| h.3);
        match hit {
            Some(AiHit::Close) => self.ai_close(),
            Some(AiHit::Model) => self.ai_model_menu(),
            Some(AiHit::New) => self.ai.new_chat(),
            Some(AiHit::History) => self.ai_history_menu(),
            Some(AiHit::Context) => {
                if self.ai.sel_ctx.is_some() {
                    self.ai.sel_ctx = None;
                } else {
                    self.ai.attach = !self.ai.attach;
                }
            }
            Some(AiHit::Apply(m, b)) => self.ai_apply(m, b),
            Some(AiHit::Copy(m, b)) => self.ai_copy(m, b),
            Some(AiHit::Input) | None => {}
        }
    }

    fn ai_lines(&self, width: usize) -> Vec<PLine> {
        let mut out: Vec<PLine> = Vec::new();
        let ai = &self.ai;
        if ai.msgs.is_empty() {
            let key_set = !ai.key().is_empty();
            let rows: Vec<(String, Style)> = vec![
                (t!("AI asistanı", "AI assistant").to_string(), Style::fg(BRIGHT).bold()),
                (String::new(), Style::new()),
                (
                    tf!("Sağlayıcı  {}", "Provider   {}", ai.provider.id()),
                    Style::fg(BRIGHT),
                ),
                (tf!("Adres      {}", "Endpoint   {}", ai.base_url), Style::fg(DIM)),
                (
                    tf!(
                        "Anahtar    {}",
                        "API key    {}",
                        if key_set {
                            t!("ayarlı ✓", "set ✓")
                        } else if !ai.needs_key() {
                            t!("gerekmez", "not required")
                        } else {
                            t!("yok", "not set")
                        }
                    ),
                    Style::fg(if key_set || !ai.needs_key() { STR } else { TYPE }),
                ),
                (
                    tf!(
                        "Model      {}",
                        "Model      {}",
                        if ai.model.is_empty() {
                            t!("seçilmedi", "not selected")
                        } else {
                            ai.model.as_str()
                        }
                    ),
                    Style::fg(if ai.model.is_empty() { TYPE } else { BRIGHT }),
                ),
                (String::new(), Style::new()),
                (":ai key <API_KEY>".into(), Style::fg(LOGO_GREEN)),
                (":ai provider openai|anthropic|gemini".into(), Style::fg(LOGO_GREEN)),
                (":ai model".into(), Style::fg(LOGO_GREEN)),
                (":ai url https://openrouter.ai/api/v1".into(), Style::fg(LOGO_GREEN)),
                (String::new(), Style::new()),
                (
                    t!("Enter gönder · Ctrl+J yeni satır", "Enter send · Ctrl+J newline").to_string(),
                    Style::fg(DIM),
                ),
                (
                    t!("Ctrl+T dosya bağlamını aç/kapat", "Ctrl+T toggle file context").to_string(),
                    Style::fg(DIM),
                ),
                (
                    t!("Ctrl+N yeni sohbet · Ctrl+H geçmiş", "Ctrl+N new chat · Ctrl+H history").to_string(),
                    Style::fg(DIM),
                ),
                (
                    t!(
                        "Ctrl+L paneli aç/kapat · Esc editöre dön",
                        "Ctrl+L toggle panel · Esc back to editor"
                    )
                    .to_string(),
                    Style::fg(DIM),
                ),
            ];
            for (t, st) in rows {
                for l in wrap(&cells(&t, st), width, false) {
                    out.push(PLine::new(l));
                }
            }
            return out;
        }
        let streaming_last = ai.streaming.is_some();
        let n = ai.msgs.len();
        for (mi, m) in ai.msgs.iter().enumerate() {
            if !out.is_empty() {
                out.push(PLine::new(Vec::new()));
            }
            match m.role {
                Role::User => {
                    let mut first = true;
                    for line in m.text.lines() {
                        let mut c = cells(if first { "› " } else { "  " }, Style::fg(LOGO_GREEN).bold());
                        c.extend(cells(line, Style::fg(BRIGHT)));
                        for (k, l) in wrap(&c, width, false).into_iter().enumerate() {
                            let mut l = l;
                            if k > 0 {
                                let mut p = cells("  ", Style::new());
                                p.extend(l);
                                l = p;
                            }
                            out.push(PLine::new(l));
                        }
                        first = false;
                    }
                    if let Some(c) = &m.ctx {
                        out.push(PLine::new(cells(&format!("  @ {}", c.label), Style::fg(DIM))));
                    }
                }
                Role::Assistant if m.error => {
                    for line in m.text.lines() {
                        for l in wrap(&cells(line, Style::fg(ERR)), width, false) {
                            out.push(PLine::new(l));
                        }
                    }
                }
                Role::Assistant => {
                    let mut in_code = false;
                    let mut block = 0usize;
                    let apply = t!("⏎ uygula", "⏎ apply");
                    let copy = t!("⧉ kopyala", "⧉ copy");
                    for line in m.text.lines() {
                        let tl = line.trim_start();
                        if let Some(lang) = tl.strip_prefix("```") {
                            if in_code {
                                in_code = false;
                                block += 1;
                                let mut pl = PLine::new(Vec::new());
                                pl.bg = Some(POP);
                                out.push(pl);
                                continue;
                            }
                            in_code = true;
                            let left = format!(" {}", if lang.trim().is_empty() { "code" } else { lang.trim() });
                            let right_w = text_w(apply) + 2 + text_w(copy) + 1;
                            let mut c = cells(&left, Style::fg(DIM).on(POP));
                            let pad = width.saturating_sub(text_w(&left) + right_w);
                            c.extend(cells(&" ".repeat(pad), Style::new().on(POP)));
                            let ax = cells_w(&c) as u16;
                            c.extend(cells(apply, Style::fg(STR).on(POP)));
                            c.extend(cells("  ", Style::new().on(POP)));
                            let cx = cells_w(&c) as u16;
                            c.extend(cells(copy, Style::fg(MACRO).on(POP)));
                            let mut pl = PLine::new(c);
                            pl.bg = Some(POP);
                            pl.hits.push((ax, ax + text_w(apply) as u16, AiHit::Apply(mi, block)));
                            pl.hits.push((cx, cx + text_w(copy) as u16, AiHit::Copy(mi, block)));
                            out.push(pl);
                            continue;
                        }
                        if in_code {
                            let mut c = cells(" ", Style::new().on(POP));
                            c.extend(cells(line, Style::fg(BRIGHT).on(POP)));
                            for l in wrap(&c, width, true) {
                                let mut pl = PLine::new(l);
                                pl.bg = Some(POP);
                                out.push(pl);
                            }
                            continue;
                        }
                        let (text, base) = if let Some(h) = tl.strip_prefix('#') {
                            (h.trim_start_matches('#').trim().to_string(), Style::fg(BRIGHT).bold())
                        } else if let Some(r) = tl.strip_prefix("- ").or_else(|| tl.strip_prefix("* ")) {
                            let ind = line.len() - tl.len();
                            (format!("{}• {}", " ".repeat(ind), r), Style::new())
                        } else {
                            (line.to_string(), Style::new())
                        };
                        for l in wrap(&inline_md(&text, base), width, false) {
                            out.push(PLine::new(l));
                        }
                    }
                    if mi == n - 1 && streaming_last {
                        match out.last_mut() {
                            Some(l) if !m.text.is_empty() && !m.text.ends_with('\n') => {
                                l.cells.push(('▍', Style::fg(DIM)))
                            }
                            _ => out.push(PLine::new(cells("▍", Style::fg(DIM)))),
                        }
                    }
                }
            }
        }
        out
    }

    pub(crate) fn draw_ai(&self, s: &mut Screen, l: &Layout) {
        let mut hits = self.ai_hits.borrow_mut();
        hits.clear();
        self.ai_cursor.set(None);
        if !self.ai.visible || l.ai_w == 0 {
            return;
        }
        let focused = self.focus == Focus::Ai;
        let sep = Style::fg(if focused { LOGO_GREEN } else { BAR });
        for r in 0..l.text_h {
            s.put_char(l.ai_x, l.top + r, '│', sep);
        }
        let x0 = l.ai_x + 1;
        let pw = l.ai_w;
        let inner = pw.saturating_sub(2) as usize;
        let top = l.top;
        let bottom = l.top + l.text_h;

        s.fill(x0, top, pw, Style::new().on(BAR));
        s.put(x0 + 1, top, "✕", Style::fg(ERR).on(BAR).bold());
        hits.push((top, x0, x0 + 3, AiHit::Close));
        let x = s.put(x0 + 3, top, " AI ", Style::fg(BRIGHT).on(BAR).bold());
        let model = if self.ai.model.is_empty() {
            t!("model seç", "pick model").to_string()
        } else {
            self.ai.model.clone()
        };
        let label = format!(" {} · {} ▾", self.ai.provider.id(), model);
        let right = " + ≡ ";
        let room = (x0 + pw).saturating_sub(x + text_w(right) as u16 + 1) as usize;
        let mx = put_clip(s, x, top, &label, Style::fg(DIM).on(BAR), room);
        hits.push((top, x, mx, AiHit::Model));
        let rx = x0 + pw - text_w(right) as u16;
        s.put(rx, top, right, Style::fg(BRIGHT).on(BAR));
        hits.push((top, rx, rx + 2, AiHit::New));
        hits.push((top, rx + 2, rx + 4, AiHit::History));

        let input_w = inner.saturating_sub(2).max(4);
        let mut rows: Vec<Vec<char>> = vec![Vec::new()];
        let mut cur_pos = (0usize, 0usize);
        let mut col = 0usize;
        for (i, &c) in self.ai.input.iter().enumerate() {
            if i == self.ai.input_x {
                cur_pos = (rows.len() - 1, col);
            }
            if c == '\n' {
                rows.push(Vec::new());
                col = 0;
                continue;
            }
            let w = char_width(c, 0);
            if col + w > input_w {
                rows.push(Vec::new());
                col = 0;
            }
            rows.last_mut().unwrap().push(c);
            col += w;
        }
        if self.ai.input_x >= self.ai.input.len() {
            if col >= input_w {
                rows.push(Vec::new());
                col = 0;
            }
            cur_pos = (rows.len() - 1, col);
        }
        let max_rows = 6.min((l.text_h as usize).saturating_sub(6)).max(1);
        let in_rows = rows.len().min(max_rows);
        let first_row = if cur_pos.0 >= in_rows {
            cur_pos.0 + 1 - in_rows
        } else {
            0
        };

        let hint_y = bottom - 1;
        let input_y = hint_y - in_rows as u16;
        let sep_y = input_y - 1;

        let ctx_label = if let Some(c) = &self.ai.sel_ctx {
            Some(format!(" @ {} ", c.label))
        } else if self.ai.attach {
            let b = self.buf();
            if b.path.is_some() || b.name == NONAME {
                Some(format!(" @ {} ", self.rel_path(b)))
            } else {
                None
            }
        } else {
            None
        };
        for i in 0..pw {
            s.put_char(x0 + i, sep_y, '─', Style::fg(BAR));
        }
        let chip = ctx_label.unwrap_or_else(|| t!(" bağlam yok ", " no context ").to_string());
        let cx = x0 + 1;
        let ce = put_clip(
            s,
            cx,
            sep_y,
            &chip,
            Style::fg(if self.ai.attach || self.ai.sel_ctx.is_some() {
                MACRO
            } else {
                DIM
            }),
            inner,
        );
        hits.push((sep_y, cx, ce, AiHit::Context));

        for r in 0..in_rows {
            let y = input_y + r as u16;
            let idx = first_row + r;
            let prefix = if idx == 0 { "› " } else { "  " };
            s.put(
                x0 + 1,
                y,
                prefix,
                Style::fg(if focused { LOGO_GREEN } else { DIM }).bold(),
            );
            if let Some(row) = rows.get(idx) {
                let t: String = row.iter().collect();
                put_clip(s, x0 + 3, y, &t, Style::fg(BRIGHT), input_w);
            }
            hits.push((y, x0, x0 + pw, AiHit::Input));
        }
        if self.ai.input.is_empty() && !focused {
            s.put(x0 + 3, input_y, t!("Bir şey sor…", "Ask something…"), Style::fg(DIM));
        }
        if focused {
            let (cr, cc) = cur_pos;
            if cr >= first_row && cr < first_row + in_rows {
                self.ai_cursor
                    .set(Some((x0 + 3 + cc as u16, input_y + (cr - first_row) as u16)));
            }
        }
        let hint = if self.ai.streaming.is_some() {
            t!("yanıt geliyor… Ctrl+C durdur", "streaming… Ctrl+C to stop")
        } else if focused {
            t!(
                "Enter gönder · Ctrl+J satır · Esc editör",
                "Enter send · Ctrl+J newline · Esc editor"
            )
        } else {
            t!("Ctrl+L ile odaklan", "Ctrl+L to focus")
        };
        put_clip(s, x0 + 1, hint_y, hint, Style::fg(DIM), inner);

        let area_top = top + 1;
        let area_h = sep_y.saturating_sub(area_top) as usize;
        if area_h == 0 {
            return;
        }
        let lines = self.ai_lines(inner);
        let total = lines.len();
        let max_scroll = total.saturating_sub(area_h);
        let scroll = self.ai.scroll.min(max_scroll);
        let start = total.saturating_sub(area_h + scroll);
        for (r, pl) in lines.iter().skip(start).take(area_h).enumerate() {
            let y = area_top + r as u16;
            if let Some(bg) = pl.bg {
                s.fill(x0 + 1, y, inner as u16, Style::new().on(bg));
            }
            let mut x = x0 + 1;
            for &(c, st) in &pl.cells {
                if x >= x0 + 1 + inner as u16 {
                    break;
                }
                x += s.put_char(x, y, c, st);
            }
            for &(a, b, h) in &pl.hits {
                hits.push((y, x0 + 1 + a, x0 + 1 + b, h));
            }
        }
        if scroll > 0 {
            let t = tf!(" ↓ {} satır ", " ↓ {} lines ", scroll);
            let w = text_w(&t) as u16;
            s.put(x0 + pw - w - 1, sep_y.saturating_sub(1), &t, Style::fg(BRIGHT).on(SEL));
        }
    }
}
