use crate::buffer::{char_width, disp_col};
use crate::editor::{find_all, Editor, Focus, Layout, Mode, MENU_ROWS, POPUP_ROWS};
use crate::lsp::from_lsp_col;
use crate::highlight::{self, Tok};
use crate::screen::{Screen, Style};
use crossterm::style::Color;
use unicode_width::UnicodeWidthStr;

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb { r, g, b }
}

pub(crate) const DIM: Color = rgb(0x5c, 0x63, 0x70);
pub(crate) const BRIGHT: Color = rgb(0xe6, 0xe6, 0xe6);
const KW: Color = rgb(0xc6, 0x78, 0xdd);
pub(crate) const STR: Color = rgb(0x98, 0xc3, 0x79);
const NUM: Color = rgb(0xd1, 0x9a, 0x66);
const FUNC: Color = rgb(0x61, 0xaf, 0xef);
pub(crate) const TYPE: Color = rgb(0xe5, 0xc0, 0x7b);
pub(crate) const MACRO: Color = rgb(0x56, 0xb6, 0xc2);
pub(crate) const ERR: Color = rgb(0xe0, 0x6c, 0x75);
pub(crate) const BAR: Color = rgb(0x21, 0x25, 0x2b);
pub(crate) const SEL: Color = rgb(0x3e, 0x44, 0x51);
const FIND: Color = rgb(0x5a, 0x4a, 0x1e);
const INK: Color = rgb(0x1b, 0x1e, 0x23);
pub(crate) const LOGO_GREEN: Color = rgb(0x3d, 0xd6, 0x8c);
pub(crate) const POP: Color = rgb(0x28, 0x2c, 0x34);
const BORDER: Color = rgb(0x4b, 0x52, 0x63);

fn sev_color(sev: u8) -> Color {
    match sev {
        1 => ERR,
        2 => TYPE,
        3 => FUNC,
        _ => DIM,
    }
}

fn kind_info(k: u64) -> (&'static str, Color) {
    match k {
        1 => ("txt", DIM),
        2 | 3 => ("fn", FUNC),
        4 => ("new", FUNC),
        5 => ("fld", MACRO),
        6 => ("var", BRIGHT),
        7 => ("cls", TYPE),
        8 => ("ifc", TYPE),
        9 => ("mod", KW),
        10 => ("prp", MACRO),
        11 => ("unit", NUM),
        12 => ("val", NUM),
        13 => ("enum", TYPE),
        14 => ("kw", KW),
        15 => ("snip", STR),
        16 => ("clr", NUM),
        17 => ("file", DIM),
        18 => ("ref", DIM),
        19 => ("dir", DIM),
        20 => ("mem", TYPE),
        21 => ("cst", NUM),
        22 => ("st", TYPE),
        23 => ("evt", KW),
        24 => ("op", KW),
        25 => ("T", TYPE),
        _ => ("", DIM),
    }
}

pub(crate) fn text_w(t: &str) -> usize {
    t.chars().map(|c| char_width(c, 0)).sum()
}

pub(crate) fn put_clip(s: &mut Screen, x: u16, y: u16, t: &str, st: Style, max: usize) -> u16 {
    let mut cx = x;
    let end = x as usize + max;
    for c in t.chars() {
        let c = if c == '\t' { ' ' } else { c };
        let w = char_width(c, 0);
        if cx as usize + w > end {
            break;
        }
        cx += s.put_char(cx, y, c, st);
    }
    cx
}

fn draw_box(s: &mut Screen, x: u16, y: u16, w: u16, h: u16, title: &str) {
    let b = Style::fg(BORDER).on(POP);
    for r in 0..h {
        s.fill(x, y + r, w, Style::new().on(POP));
    }
    s.put_char(x, y, '╭', b);
    s.put_char(x + w - 1, y, '╮', b);
    s.put_char(x, y + h - 1, '╰', b);
    s.put_char(x + w - 1, y + h - 1, '╯', b);
    for i in 1..w - 1 {
        s.put_char(x + i, y, '─', b);
        s.put_char(x + i, y + h - 1, '─', b);
    }
    for r in 1..h - 1 {
        s.put_char(x, y + r, '│', b);
        s.put_char(x + w - 1, y + r, '│', b);
    }
    if !title.is_empty() && w > 6 {
        put_clip(s, x + 2, y, &format!(" {} ", title), Style::fg(BRIGHT).on(POP).bold(), w as usize - 4);
    }
}

fn tok_style(t: Tok) -> Style {
    match t {
        Tok::Normal => Style::new(),
        Tok::Keyword => Style::fg(KW),
        Tok::Type => Style::fg(TYPE),
        Tok::Str => Style::fg(STR),
        Tok::Comment => Style::fg(DIM).italic(),
        Tok::Number => Style::fg(NUM),
        Tok::Func => Style::fg(FUNC),
        Tok::Macro => Style::fg(MACRO),
    }
}

impl Editor {
    pub fn draw(&mut self, s: &mut Screen) {
        s.clear();
        let l = self.layout();
        self.draw_tabs(s);
        if self.tree.visible {
            self.draw_tree(s, &l);
        }
        if self.is_splash() {
            self.draw_splash(s, &l);
        } else {
            self.draw_text(s, &l);
        }
        self.draw_ai(s, &l);
        self.draw_status(s);
        self.draw_cmdline(s);
        self.draw_sig(s, &l);
        self.draw_popup(s);
        self.draw_hover(s, &l);
        self.draw_menu(s, &l);
    }

    pub fn cursor_pos(&self) -> Option<(u16, u16)> {
        match self.mode {
            Mode::Command | Mode::Search => {
                let w: usize = self.cmd[..self.cmd_x].iter().map(|c| char_width(*c, 0)).sum();
                Some(((1 + w).min(self.w as usize - 1) as u16, self.h.saturating_sub(1)))
            }
            _ if self.menu.is_some() => None,
            _ if self.focus == Focus::Ai => self.ai_cursor.get(),
            _ if self.focus == Focus::Tree => None,
            _ if self.is_splash() => None,
            _ => {
                let l = self.layout();
                let b = self.buf();
                let dc = disp_col(&b.lines[b.cy], b.cx);
                if b.cy < b.scroll_y || dc < b.scroll_x {
                    return None;
                }
                let row = b.cy - b.scroll_y;
                let col = dc - b.scroll_x;
                if row >= l.text_h as usize || col >= l.text_w as usize {
                    return None;
                }
                Some((l.text_x + col as u16, l.top + row as u16))
            }
        }
    }

    pub fn bar_cursor(&self) -> bool {
        matches!(self.mode, Mode::Insert | Mode::Command | Mode::Search)
    }

    fn draw_tabs(&mut self, s: &mut Screen) {
        s.fill_row(0, Style::new().on(BAR));
        self.tab_spans.clear();
        let mut x = 0u16;
        for (i, b) in self.bufs.iter().enumerate() {
            let label = format!(" {}{} ", Editor::display_name(b), if b.dirty { " ●" } else { "" });
            let st = if i == self.cur { Style::fg(BRIGHT).bold() } else { Style::fg(DIM).on(BAR) };
            let nx = s.put(x, 0, &label, st);
            self.tab_spans.push((x, nx, i));
            x = nx;
            if x >= s.w {
                break;
            }
        }
        let brand = " zetsu ";
        let bw = brand.len() as u16;
        if x + bw + 2 <= s.w {
            s.put(s.w - bw, 0, brand, Style::fg(DIM).on(BAR));
        }
    }

    fn draw_tree(&self, s: &mut Screen, l: &Layout) {
        let t = &self.tree;
        let focused = self.focus == Focus::Tree;
        let w = l.tree_w;
        let header = format!(" {}", t.root_name().to_uppercase());
        s.put(0, l.top, &header, Style::fg(if focused { BRIGHT } else { DIM }).bold());
        let open_path = self.buf().path.as_deref();
        let rows = l.text_h.saturating_sub(1) as usize;
        for r in 0..rows {
            let i = t.scroll + r;
            let y = l.top + 1 + r as u16;
            let Some(n) = t.nodes.get(i) else { break };
            let selected = i == t.sel;
            let mut st = if n.is_dir {
                Style::fg(BRIGHT)
            } else if Some(n.path.as_path()) == open_path {
                Style::fg(FUNC)
            } else {
                Style::new()
            };
            if selected && focused {
                st = st.on(SEL);
                s.fill(0, y, w, Style::new().on(SEL));
            }
            let icon = if n.is_dir {
                if n.expanded {
                    "▾ "
                } else {
                    "▸ "
                }
            } else {
                "  "
            };
            let text = format!(" {}{}{}", "  ".repeat(n.depth), icon, n.name);
            let mut x = 0u16;
            for c in text.chars() {
                let cw = char_width(c, 0) as u16;
                if x + cw > w {
                    break;
                }
                x += s.put_char(x, y, c, st);
            }
        }
        let sep = Style::fg(if focused { FUNC } else { BAR });
        for r in 0..l.text_h {
            s.put_char(l.tree_w, l.top + r, '│', sep);
        }
    }

    fn draw_splash(&self, s: &mut Screen, l: &Layout) {
        const LOGO: [&str; 3] = ["▀▀█ █▀▀ ▀█▀ █▀▀ █ █  ", "▄▀  █▀▀  █  ▀▀█ █ █  ", "▀▀▀ ▀▀▀  ▀  ▀▀▀ ▀▀▀ ▄"];
        let keys: [(&str, &str); 6] = [
            ("Ctrl+k", t!("klasör aç", "open folder")),
            ("Ctrl+b", t!("dosya ağacını göster / gizle", "toggle file tree")),
            (t!(":e ad", ":e name"), t!("dosya aç ya da oluştur", "open or create a file")),
            ("i", t!("yazmaya başla  (Esc ile çık)", "start typing  (Esc to stop)")),
            (":help", t!("tüm kısayollar", "all shortcuts")),
            (":q", t!("çık", "quit")),
        ];
        let mut hits = self.splash_hits.borrow_mut();
        hits.clear();
        let area_x = l.ed_x;
        let area_w = l.ed_w;
        let recent_n = self.recent.len().min(5);
        let mut need = 3 + 2 + 2 + keys.len();
        let show_recent = recent_n > 0 && (l.text_h as usize) >= need + 3 + recent_n;
        if show_recent {
            need += 2 + recent_n;
        }
        let show_keys = (l.text_h as usize) >= 7 + 3;
        if !show_keys {
            need = 5;
        }
        let mut y = l.top + l.text_h.saturating_sub(need as u16) / 2;
        let center = |w: usize| area_x + area_w.saturating_sub(w as u16) / 2;

        let logo_w = text_w(LOGO[0]);
        let lx = center(logo_w);
        for row in LOGO {
            let mut x = lx;
            for c in row.chars() {
                let st = if c == '▄' { Style::fg(BRIGHT) } else { Style::fg(LOGO_GREEN) };
                x += s.put_char(x, y, c, st);
            }
            y += 1;
        }
        y += 1;
        let tag = tf!("performance-first terminal IDE  ·  v{}", "performance-first terminal IDE  ·  v{}", env!("CARGO_PKG_VERSION"));
        s.put(center(text_w(&tag)), y, &tag, Style::fg(DIM));
        y += 2;
        if !show_keys {
            return;
        }
        let block_w = 8 + keys.iter().map(|k| text_w(k.1)).max().unwrap_or(0);
        let kx = center(block_w.max(36));
        for (k, d) in keys {
            s.put(kx, y, k, Style::fg(LOGO_GREEN));
            s.put(kx + 8, y, d, Style::fg(DIM));
            if k == "Ctrl+k" {
                hits.push((y, crate::editor::SPLASH_OPEN));
            } else if k == ":help" {
                hits.push((y, crate::editor::SPLASH_HELP));
            }
            y += 1;
        }
        if show_recent {
            y += 1;
            s.put(kx, y, t!("Son klasörler", "Recent folders"), Style::fg(BRIGHT).bold());
            y += 1;
            let room = (area_w as usize).saturating_sub((kx - area_x) as usize + 2);
            for (i, p) in self.recent.iter().take(recent_n).enumerate() {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string());
                s.put(kx, y, &(i + 1).to_string(), Style::fg(LOGO_GREEN));
                let nx = put_clip(s, kx + 3, y, &name, Style::fg(BRIGHT), 22.min(room));
                let px = (kx + 3 + 22).max(nx + 2);
                let full = p.display().to_string();
                let left = room.saturating_sub((px - kx) as usize);
                let chars: Vec<char> = full.chars().collect();
                let shown = if chars.len() > left && left > 1 {
                    format!("…{}", chars[chars.len() - (left - 1)..].iter().collect::<String>())
                } else {
                    full
                };
                put_clip(s, px, y, &shown, Style::fg(DIM), left);
                hits.push((y, i));
                y += 1;
            }
        }
    }

    fn draw_text(&self, s: &mut Screen, l: &Layout) {
        let b = self.buf();
        let visual = matches!(self.mode, Mode::Visual | Mode::VisualLine);
        let line_mode = self.mode == Mode::VisualLine;
        let (sa, se) = self.selection();
        let in_sel = |x: usize, y: usize| -> bool {
            if !visual {
                return false;
            }
            if line_mode {
                return y >= sa.1 && y <= se.1;
            }
            (y, x) >= (sa.1, sa.0) && (y, x) <= (se.1, se.0)
        };
        let pat = if self.hl_search { self.search.as_ref() } else { None };

        let mut in_block = false;
        if b.lang.block.is_some() {
            for y in 0..b.scroll_y.min(b.lines.len()) {
                highlight::highlight_line(b.lang, &b.lines[y], &mut in_block);
            }
        }
        let right = b.scroll_x + l.text_w as usize;
        let diags = self.buf_diags(self.cur);
        for row in 0..l.text_h {
            let sy = l.top + row;
            let ly = b.scroll_y + row as usize;
            if ly >= b.lines.len() {
                s.put(l.ed_x + l.gutter.saturating_sub(2), sy, "~", Style::fg(BAR));
                continue;
            }
            let line = &b.lines[ly];
            let mut ulm = vec![0u8; line.len() + 1];
            let mut sign = 0u8;
            if let Some(ds) = diags {
                for d in ds.iter().filter(|d| d.l0 <= ly && ly <= d.l1) {
                    let x0 = if d.l0 == ly { from_lsp_col(line, d.c0, d.enc) } else { 0 };
                    let mut x1 = if d.l1 == ly { from_lsp_col(line, d.c1, d.enc) } else { line.len() };
                    if x1 <= x0 {
                        x1 = x0 + 1;
                    }
                    for k in x0..x1.min(line.len() + 1) {
                        if ulm[k] == 0 || d.sev < ulm[k] {
                            ulm[k] = d.sev;
                        }
                    }
                    if sign == 0 || d.sev < sign {
                        sign = d.sev;
                    }
                }
            }
            if sign != 0 && sign < 4 {
                s.put_char(l.ed_x, sy, '●', Style::fg(sev_color(sign)));
            }
            if self.numbers {
                let num = if self.relative && ly != b.cy { ly.abs_diff(b.cy) } else { ly + 1 };
                let txt = format!("{:>w$} ", num, w = (l.gutter - 3) as usize);
                let st = if ly == b.cy { Style::fg(BRIGHT) } else { Style::fg(DIM) };
                s.put(l.ed_x + 1, sy, &txt, st);
            }
            let toks = highlight::highlight_line(b.lang, line, &mut in_block);
            let mut mask = vec![false; line.len()];
            if let Some(p) = pat {
                for m in find_all(line, p) {
                    for k in m..m + p.len() {
                        mask[k] = true;
                    }
                }
            }
            let mut col = 0usize;
            for (i, &c) in line.iter().enumerate() {
                let w = char_width(c, col);
                let start = col;
                col += w;
                if col <= b.scroll_x {
                    continue;
                }
                if start >= right {
                    break;
                }
                let mut st = tok_style(toks[i]);
                if in_sel(i, ly) {
                    st = st.on(SEL);
                } else if mask[i] {
                    st = st.on(FIND);
                }
                if ulm[i] != 0 {
                    st = st.underline(sev_color(ulm[i]));
                }
                let vis_start = start.max(b.scroll_x);
                let vis_end = col.min(right);
                let sx = l.text_x + (vis_start - b.scroll_x) as u16;
                if c == '\t' || start < b.scroll_x || col > right {
                    s.fill(sx, sy, (vis_end - vis_start) as u16, st);
                } else {
                    s.put_char(sx, sy, c, st);
                }
            }
            let nl_selected = visual && (line_mode || (ly >= sa.1 && ly < se.1) || (ly == se.1 && se.0 >= line.len()));
            if col >= b.scroll_x && col < right {
                let ex = l.text_x + (col - b.scroll_x) as u16;
                if nl_selected {
                    s.put_char(ex, sy, ' ', Style::new().on(SEL));
                } else if ulm[line.len()] != 0 {
                    s.put_char(ex, sy, ' ', Style::new().underline(sev_color(ulm[line.len()])));
                }
            }
        }
    }

    fn draw_status(&self, s: &mut Screen) {
        let y = s.h.saturating_sub(2);
        let bar = Style::fg(DIM).on(BAR);
        s.fill_row(y, bar);
        let tree_mode = self.focus == Focus::Tree && !matches!(self.mode, Mode::Command | Mode::Search);
        let ai_mode = self.focus == Focus::Ai && !matches!(self.mode, Mode::Command | Mode::Search);
        let (label, color) = if ai_mode {
            ("AI", LOGO_GREEN)
        } else if tree_mode {
            (t!("AĞAÇ", "TREE"), MACRO)
        } else {
            match self.mode {
                Mode::Normal => ("NORMAL", FUNC),
                Mode::Insert => ("INSERT", STR),
                Mode::Visual => ("VISUAL", KW),
                Mode::VisualLine => ("V-LINE", KW),
                Mode::Command | Mode::Search => (t!("KOMUT", "COMMAND"), TYPE),
            }
        };
        let x = s.put(0, y, &format!(" {} ", label), Style::fg(INK).on(color).bold());
        let b = self.buf();
        let x = s.put(x, y, &format!(" {}", self.rel_path(b)), Style::fg(BRIGHT).on(BAR));
        if b.dirty {
            s.put(x, y, " [+]", Style::fg(TYPE).on(BAR));
        }
        let pct = if b.lines.len() <= 1 { 100 } else { b.cy * 100 / (b.lines.len() - 1) };
        let col = disp_col(&b.lines[b.cy], b.cx) + 1;
        let right = format!(
            "{}  {}{}  {}  {}:{}  {:>3}% ",
            self.pending_str(),
            self.lsp_status().map(|s| s + "  ").unwrap_or_default(),
            if b.lang.name == "Metin" { t!("Metin", "Text") } else { b.lang.name },
            if b.crlf { "CRLF" } else { "LF" },
            b.cy + 1,
            col,
            pct
        );
        let rw = right.width() as u16;
        if rw < s.w {
            s.put(s.w - rw, y, &right, bar);
            let (e, w) = self.diag_counts();
            let mut parts: Vec<(String, Color)> = Vec::new();
            if e > 0 {
                parts.push((format!("✖ {} ", e), ERR));
            }
            if w > 0 {
                parts.push((format!("▲ {} ", w), TYPE));
            }
            let total: u16 = parts.iter().map(|p| p.0.width() as u16).sum();
            if total + rw + 30 < s.w {
                let mut x = s.w - rw - total;
                for (t, c) in parts {
                    x = s.put(x, y, &t, Style::fg(c).on(BAR));
                }
            }
        }
    }

    fn draw_cmdline(&self, s: &mut Screen) {
        let y = s.h.saturating_sub(1);
        match self.mode {
            Mode::Command | Mode::Search => {
                let p = if self.mode == Mode::Command { ':' } else { '/' };
                let text: String = std::iter::once(p).chain(self.cmd.iter().copied()).collect();
                s.put(0, y, &text, Style::new());
            }
            _ => {
                if let Some((m, err)) = &self.msg {
                    let st = if *err { Style::fg(ERR) } else { Style::fg(DIM) };
                    s.put(0, y, m, st);
                } else if self.focus == Focus::Editor && self.mode != Mode::Insert {
                    if let Some((sev, m)) = self.diag_at_cursor() {
                        s.put(0, y, &format!("● {}", m), Style::fg(sev_color(sev)));
                    }
                }
            }
        }
    }
}

impl Editor {
    fn draw_popup(&self, s: &mut Screen) {
        self.popup_rect.set(None);
        let Some(p) = &self.popup else { return };
        let Some((cxs, cys)) = self.cursor_pos() else { return };
        let b = self.buf();
        let line = &b.lines[b.cy];
        let typed = disp_col(line, b.cx).saturating_sub(disp_col(line, p.start)) as u16;
        let rows = p.shown.len().saturating_sub(p.scroll).min(POPUP_ROWS);
        if rows == 0 {
            return;
        }
        let vis = &p.shown[p.scroll..p.scroll + rows];
        let labw = vis.iter().map(|&i| text_w(&p.items[i].label)).max().unwrap_or(0).min(42);
        let detw = vis.iter().map(|&i| text_w(&p.items[i].detail)).max().unwrap_or(0).min(32);
        let mut w = (6 + labw + if detw > 0 { 2 + detw } else { 0 } + 1) as u16;
        w = w.min(s.w);
        let mut x0 = cxs.saturating_sub(typed).saturating_sub(6);
        x0 = x0.min(s.w.saturating_sub(w));
        let below = s.h.saturating_sub(2).saturating_sub(cys + 1);
        let y0 = if rows as u16 <= below { cys + 1 } else { cys.saturating_sub(rows as u16) };
        for (r, &i) in vis.iter().enumerate() {
            let it = &p.items[i];
            let y = y0 + r as u16;
            let bg = if p.scroll + r == p.sel { SEL } else { POP };
            s.fill(x0, y, w, Style::new().on(bg));
            let (k, kc) = kind_info(it.kind);
            put_clip(s, x0 + 1, y, k, Style::fg(kc).on(bg), 4);
            let lx = put_clip(s, x0 + 6, y, &it.label, Style::fg(BRIGHT).on(bg), labw);
            if detw > 0 {
                let dx = (x0 + 6 + labw as u16 + 2).max(lx + 1);
                let room = (x0 + w).saturating_sub(dx + 1) as usize;
                put_clip(s, dx, y, &it.detail, Style::fg(DIM).on(bg), room);
            }
        }
        if p.shown.len() > rows {
            let n = p.shown.len();
            let thumb = (p.scroll * rows / n).min(rows - 1);
            s.put_char(x0 + w - 1, y0 + thumb as u16, '▐', Style::fg(BORDER).on(POP));
        }
        self.popup_rect.set(Some((x0, y0, w, rows as u16)));
    }

    fn draw_sig(&self, s: &mut Screen, l: &Layout) {
        let Some((label, hl)) = &self.sig else { return };
        let Some((cxs, cys)) = self.cursor_pos() else { return };
        let w = (text_w(label) + 2).min(s.w as usize) as u16;
        let y = if cys > l.top { cys - 1 } else { cys + 1 };
        let x0 = cxs.saturating_sub(4).min(s.w.saturating_sub(w));
        s.fill(x0, y, w, Style::new().on(POP));
        let mut x = x0 + 1;
        for (i, c) in label.chars().enumerate() {
            let active = hl.map_or(false, |(a, b)| i >= a && i < b);
            let st = if active { Style::fg(TYPE).on(POP).bold() } else { Style::fg(DIM).on(POP) };
            if x + 1 >= x0 + w {
                break;
            }
            x += s.put_char(x, y, c, st);
        }
    }

    fn draw_hover(&self, s: &mut Screen, l: &Layout) {
        let Some(lines) = &self.hover else { return };
        let (cxs, cys) = self.cursor_pos().unwrap_or((l.text_x, l.top));
        let maxw = (s.w as usize).saturating_sub(4).min(90).max(10);
        let inner = maxw - 4;
        let mut wrapped: Vec<String> = Vec::new();
        for ln in lines {
            let mut cur = String::new();
            let mut cw = 0;
            for c in ln.chars() {
                let w = char_width(c, 0);
                if cw + w > inner {
                    wrapped.push(std::mem::take(&mut cur));
                    cw = 0;
                }
                cur.push(c);
                cw += w;
            }
            wrapped.push(cur);
        }
        let rows = wrapped.len().min((s.h as usize).saturating_sub(6)).max(1);
        let w = (wrapped.iter().map(|t| text_w(t)).max().unwrap_or(0) + 4).max(12).min(maxw) as u16;
        let h = rows as u16 + 2;
        let y0 = if cys >= l.top + h { cys - h } else { (cys + 1).min(s.h.saturating_sub(2 + h)) };
        let x0 = cxs.min(s.w.saturating_sub(w));
        draw_box(s, x0, y0, w, h, "");
        for (r, t) in wrapped.iter().take(rows).enumerate() {
            let st = if r == 0 { Style::fg(FUNC).on(POP) } else { Style::fg(BRIGHT).on(POP) };
            put_clip(s, x0 + 2, y0 + 1 + r as u16, t, st, w as usize - 4);
        }
    }

    fn draw_menu(&self, s: &mut Screen, l: &Layout) {
        self.menu_rect.set(None);
        let Some(m) = &self.menu else { return };
        let rows = m.items.len().min(MENU_ROWS).max(1);
        let itemw = m.items.iter().map(|i| text_w(&i.0)).max().unwrap_or(0);
        let min_w = if m.filter.is_some() { 44 } else { 20 };
        let w = (itemw + 4).max(text_w(&m.title) + 8).max(min_w).min(s.w as usize - 2) as u16;
        let h = rows as u16 + 2;
        let (x0, y0) = match m.anchor {
            Some((ax, ay)) => {
                let y = if ay + h < s.h.saturating_sub(1) { ay } else { ay.saturating_sub(h + 1) };
                (ax.min(s.w.saturating_sub(w)), y)
            }
            None => {
                let area = l.ed_w;
                (l.ed_x + area.saturating_sub(w) / 2, l.top + 1)
            }
        };
        let title = match &m.filter {
            Some(f) => format!("{}: {}▏", m.title, f),
            None => m.title.clone(),
        };
        draw_box(s, x0, y0, w, h, &title);
        for r in 0..rows {
            let i = m.scroll + r;
            let Some((t, _)) = m.items.get(i) else { break };
            let y = y0 + 1 + r as u16;
            let sel = i == m.sel;
            let st = if sel { Style::fg(BRIGHT).on(SEL) } else { Style::new().on(POP) };
            s.fill(x0 + 1, y, w - 2, st);
            put_clip(s, x0 + 2, y, t, st, w as usize - 4);
        }
        if m.items.len() > rows {
            let c = format!(" {}/{} ", m.sel + 1, m.items.len());
            put_clip(s, x0 + w - 2 - text_w(&c) as u16, y0 + h - 1, &c, Style::fg(DIM).on(POP), 12);
        }
        self.menu_rect.set(Some((x0, y0, w, h - 1)));
    }
}
