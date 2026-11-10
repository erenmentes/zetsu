use super::*;
use crate::buffer::char_width;
use crate::render::{put_clip, text_w, BAR, BRIGHT, DIM, ERR, LOGO_GREEN, POP, SEL};
use crate::screen::{Screen, Style};
use regex::{NoExpand, Regex, RegexBuilder};

pub(crate) type MatchCache = Option<(u64, usize, String, Vec<(usize, usize, usize)>)>;

#[derive(Clone)]
pub struct Matcher {
    pub pattern: String,
    pub case: bool,
    pub word: bool,
    pub regex: bool,
    re: Option<Regex>,
}

fn char_starts(s: &str) -> Vec<usize> {
    let mut v: Vec<usize> = s.char_indices().map(|(b, _)| b).collect();
    v.push(s.len());
    v
}

fn to_char(starts: &[usize], byte: usize) -> usize {
    starts.binary_search(&byte).unwrap_or_else(|i| i)
}

impl Matcher {
    pub fn new(pattern: &str, case: bool, word: bool, regex: bool) -> Result<Self, String> {
        let re = if pattern.is_empty() {
            None
        } else {
            let mut p = if regex {
                pattern.to_string()
            } else {
                regex::escape(pattern)
            };
            if word {
                p = format!(r"\b(?:{})\b", p);
            }
            let r = RegexBuilder::new(&p)
                .case_insensitive(!case)
                .size_limit(1 << 22)
                .build()
                .map_err(|e| e.to_string().lines().last().unwrap_or("").trim().to_string())?;
            Some(r)
        };
        Ok(Matcher {
            pattern: pattern.to_string(),
            case,
            word,
            regex,
            re,
        })
    }

    pub fn smart(pattern: &str, word: bool) -> Self {
        let case = pattern.chars().any(|c| c.is_uppercase());
        Matcher::new(pattern, case, word, false).unwrap_or(Matcher {
            pattern: pattern.to_string(),
            case,
            word,
            regex: false,
            re: None,
        })
    }

    pub fn find_line(&self, line: &[char]) -> Vec<(usize, usize)> {
        let Some(re) = &self.re else { return Vec::new() };
        let s: String = line.iter().collect();
        let starts = char_starts(&s);
        re.find_iter(&s)
            .filter(|m| m.start() < m.end())
            .map(|m| (to_char(&starts, m.start()), to_char(&starts, m.end())))
            .collect()
    }

    pub fn replace_at(&self, line: &[char], at: usize, repl: &str) -> Option<(Vec<char>, usize)> {
        let re = self.re.as_ref()?;
        let s: String = line.iter().collect();
        let starts = char_starts(&s);
        let byte = *starts.get(at)?;
        let caps = re.captures_at(&s, byte)?;
        let m = caps.get(0)?;
        if m.start() != byte || m.start() == m.end() {
            return None;
        }
        let mut out = String::new();
        if self.regex {
            caps.expand(repl, &mut out);
        } else {
            out.push_str(repl);
        }
        let new_len = out.chars().count();
        let mut res: Vec<char> = s[..m.start()].chars().collect();
        res.extend(out.chars());
        res.extend(s[m.end()..].chars());
        Some((res, at + new_len))
    }

    pub fn replace_all_line(&self, line: &[char], repl: &str) -> (Vec<char>, usize) {
        let Some(re) = &self.re else { return (line.to_vec(), 0) };
        let s: String = line.iter().collect();
        let n = re.find_iter(&s).filter(|m| m.start() < m.end()).count();
        if n == 0 {
            return (line.to_vec(), 0);
        }
        let out = if self.regex {
            re.replace_all(&s, repl)
        } else {
            re.replace_all(&s, NoExpand(repl))
        };
        (out.chars().collect(), n)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FindHit {
    Expand,
    Query,
    Replace,
    Case,
    Word,
    Regex,
    Prev,
    Next,
    Close,
    ReplaceOne,
    ReplaceAll,
}

pub struct FindBar {
    pub query: Vec<char>,
    pub qx: usize,
    pub repl: Vec<char>,
    pub rx: usize,
    pub show_replace: bool,
    pub field: u8,
    pub case: bool,
    pub word: bool,
    pub regex: bool,
    pub error: Option<String>,
    pub fresh: bool,
    origin: Pos,
}

impl Editor {
    pub(crate) fn open_find(&mut self, replace: bool) {
        let mut seed: Option<Vec<char>> = None;
        if matches!(self.mode, Mode::Visual) {
            let (a, e) = self.selection();
            if a.1 == e.1 {
                let line = &self.buf().lines[a.1];
                let end = (e.0 + 1).min(line.len());
                if a.0 < end {
                    seed = Some(line[a.0..end].to_vec());
                }
            }
        }
        if self.mode == Mode::Insert {
            self.leave_insert();
        }
        self.mode = Mode::Normal;
        self.reset_pending();
        let origin = (self.buf().cx, self.buf().cy);
        let fb = self.find.get_or_insert_with(|| FindBar {
            query: Vec::new(),
            qx: 0,
            repl: Vec::new(),
            rx: 0,
            show_replace: false,
            field: 0,
            case: false,
            word: false,
            regex: false,
            error: None,
            fresh: false,
            origin,
        });
        fb.origin = origin;
        if let Some(q) = seed {
            fb.query = q;
        }
        fb.qx = fb.query.len();
        fb.fresh = !fb.query.is_empty();
        if replace {
            fb.show_replace = true;
            fb.field = if fb.query.is_empty() { 0 } else { 1 };
        } else {
            fb.field = 0;
        }
        self.focus = Focus::Find;
        self.find_update(false);
    }

    pub(crate) fn close_find(&mut self) {
        self.find = None;
        self.hl_search = false;
        if self.focus == Focus::Find {
            self.focus = Focus::Editor;
        }
    }

    fn find_update(&mut self, jump: bool) {
        let Some(fb) = self.find.as_mut() else { return };
        let q: String = fb.query.iter().collect();
        match Matcher::new(&q, fb.case, fb.word, fb.regex) {
            Ok(m) => {
                fb.error = None;
                let empty = q.is_empty();
                let origin = fb.origin;
                self.search = if empty { None } else { Some(m) };
                self.hl_search = !empty;
                if jump && !empty {
                    let b = self.buf_mut();
                    b.cx = origin.0;
                    b.cy = origin.1;
                    self.goto_match(true, true);
                }
            }
            Err(e) => {
                fb.error = Some(e);
                self.search = None;
            }
        }
    }

    pub(crate) fn all_matches(&self) -> Vec<(usize, usize, usize)> {
        let Some(m) = &self.search else { return Vec::new() };
        let b = self.buf();
        let key = format!("{}|{}|{}|{}", m.pattern, m.case, m.word, m.regex);
        if let Some((rev, cur, k, v)) = self.find_cache.borrow().as_ref() {
            if *rev == b.rev && *cur == self.cur && *k == key {
                return v.clone();
            }
        }
        let mut out = Vec::new();
        for (y, l) in b.lines.iter().enumerate() {
            for (s, e) in m.find_line(l) {
                out.push((y, s, e));
                if out.len() >= 100_000 {
                    break;
                }
            }
        }
        *self.find_cache.borrow_mut() = Some((b.rev, self.cur, key, out.clone()));
        out
    }

    pub(crate) fn goto_match(&mut self, fwd: bool, inclusive: bool) -> bool {
        let all = self.all_matches();
        if all.is_empty() {
            return false;
        }
        let b = self.buf();
        let cur = (b.cy, b.cx);
        let pick = if fwd {
            all.iter()
                .find(|m| if inclusive { (m.0, m.1) >= cur } else { (m.0, m.1) > cur })
                .or(all.first())
        } else {
            all.iter().rev().find(|m| (m.0, m.1) < cur).or(all.last())
        };
        if let Some(&(y, x, _)) = pick {
            let b = self.buf_mut();
            b.cy = y;
            b.cx = x;
            b.sync_want();
            return true;
        }
        false
    }

    fn find_replace_one(&mut self) {
        let Some(m) = self.search.clone() else { return };
        let repl: String = self.find.as_ref().map(|f| f.repl.iter().collect()).unwrap_or_default();
        let b = self.buf();
        let (cx, cy) = (b.cx, b.cy);
        let on_match = m.find_line(&b.lines[cy]).iter().any(|r| r.0 == cx);
        if !on_match {
            self.goto_match(true, true);
            return;
        }
        if let Some((line, end)) = m.replace_at(&self.buf().lines[cy], cx, &repl) {
            let b = self.buf_mut();
            b.checkpoint();
            b.lines[cy] = line;
            b.cx = end;
            b.touch();
        }
        if !self.goto_match(true, true) {
            self.info(t!("Başka eşleşme yok", "No more matches"));
        }
    }

    fn find_replace_all(&mut self) {
        let Some(m) = self.search.clone() else { return };
        let repl: String = self.find.as_ref().map(|f| f.repl.iter().collect()).unwrap_or_default();
        let b = self.buf_mut();
        b.checkpoint();
        let mut total = 0;
        for y in 0..b.lines.len() {
            let (line, n) = m.replace_all_line(&b.lines[y], &repl);
            if n > 0 {
                b.lines[y] = line;
                total += n;
            }
        }
        if total == 0 {
            b.drop_noop_checkpoint();
            self.info(t!("Eşleşme yok", "No matches"));
        } else {
            b.touch();
            b.clamp(false);
            self.info(tf!(
                "{} eşleşme değiştirildi (geri almak için Ctrl+Z)",
                "Replaced {} matches (Ctrl+Z to undo)",
                total
            ));
        }
    }

    pub(crate) fn find_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT);
        let alt = k.modifiers.contains(KeyModifiers::ALT) && !k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let Some(fb) = self.find.as_mut() else {
            self.focus = Focus::Editor;
            return;
        };
        match k.code {
            KeyCode::Esc => return self.close_find(),
            KeyCode::Enter if fb.field == 1 && (ctrl || alt) => return self.find_replace_all(),
            KeyCode::Enter if fb.field == 1 => return self.find_replace_one(),
            KeyCode::Enter | KeyCode::F(3) | KeyCode::Down | KeyCode::Up => {
                let fwd = !(shift || k.code == KeyCode::Up);
                if !self.goto_match(fwd, false) {
                    self.info(t!("Eşleşme yok", "No matches"));
                }
                return;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                if fb.show_replace {
                    fb.field = 1 - fb.field;
                }
                return;
            }
            KeyCode::Char('f') if ctrl => {
                fb.field = 0;
                fb.qx = fb.query.len();
                fb.fresh = true;
                return;
            }
            KeyCode::Char('h') if ctrl => {
                fb.show_replace = !fb.show_replace;
                fb.field = if fb.show_replace { 1 } else { 0 };
                return;
            }
            KeyCode::Char('c') | KeyCode::Char('C') if alt => {
                fb.case = !fb.case;
            }
            KeyCode::Char('w') | KeyCode::Char('W') if alt => {
                fb.word = !fb.word;
            }
            KeyCode::Char('r') | KeyCode::Char('R') if alt => {
                fb.regex = !fb.regex;
            }
            _ => {
                let (buf, x) = if fb.field == 0 {
                    (&mut fb.query, &mut fb.qx)
                } else {
                    (&mut fb.repl, &mut fb.rx)
                };
                match k.code {
                    KeyCode::Char('u') if ctrl => {
                        buf.drain(..*x);
                        *x = 0;
                    }
                    KeyCode::Char(c) if !ctrl && !alt || (ctrl && alt) => {
                        if fb.field == 0 && fb.fresh {
                            buf.clear();
                            *x = 0;
                        }
                        buf.insert(*x, c);
                        *x += 1;
                    }
                    KeyCode::Backspace => {
                        if fb.field == 0 && fb.fresh {
                            buf.clear();
                            *x = 0;
                        } else if *x > 0 {
                            *x -= 1;
                            buf.remove(*x);
                        }
                    }
                    KeyCode::Delete => {
                        if *x < buf.len() {
                            buf.remove(*x);
                        }
                    }
                    KeyCode::Left => *x = x.saturating_sub(1),
                    KeyCode::Right => *x = (*x + 1).min(buf.len()),
                    KeyCode::Home => *x = 0,
                    KeyCode::End => *x = buf.len(),
                    _ => return,
                }
                if fb.field == 1 {
                    fb.fresh = false;
                    return;
                }
                fb.fresh = false;
            }
        }
        self.find_update(true);
    }

    pub(crate) fn find_paste(&mut self, text: &str) {
        let Some(fb) = self.find.as_mut() else { return };
        let t: Vec<char> = text.lines().next().unwrap_or("").chars().collect();
        if fb.field == 0 {
            if fb.fresh {
                fb.query.clear();
                fb.qx = 0;
                fb.fresh = false;
            }
            for c in t {
                fb.query.insert(fb.qx, c);
                fb.qx += 1;
            }
            self.find_update(true);
        } else {
            for c in t {
                fb.repl.insert(fb.rx, c);
                fb.rx += 1;
            }
        }
    }

    pub(crate) fn find_click(&mut self, x: u16, y: u16) -> bool {
        let hit = self
            .find_hits
            .borrow()
            .iter()
            .find(|h| h.0 == y && x >= h.1 && x < h.2)
            .map(|h| h.3);
        let Some(hit) = hit else { return false };
        self.focus = Focus::Find;
        let fb = self.find.as_mut().unwrap();
        match hit {
            FindHit::Expand => {
                fb.show_replace = !fb.show_replace;
                fb.field = 0;
            }
            FindHit::Query => {
                fb.field = 0;
                fb.fresh = false;
            }
            FindHit::Replace => fb.field = 1,
            FindHit::Case => {
                fb.case = !fb.case;
                self.find_update(true);
            }
            FindHit::Word => {
                fb.word = !fb.word;
                self.find_update(true);
            }
            FindHit::Regex => {
                fb.regex = !fb.regex;
                self.find_update(true);
            }
            FindHit::Prev => {
                self.goto_match(false, false);
            }
            FindHit::Next => {
                self.goto_match(true, false);
            }
            FindHit::Close => self.close_find(),
            FindHit::ReplaceOne => self.find_replace_one(),
            FindHit::ReplaceAll => self.find_replace_all(),
        }
        true
    }

    pub(crate) fn draw_find(&self, s: &mut Screen, l: &Layout) {
        let mut hits = self.find_hits.borrow_mut();
        hits.clear();
        self.find_cursor.set(None);
        let Some(fb) = &self.find else { return };
        let right = l.ed_r;
        let w = 66.min(l.ed_w.saturating_sub(2));
        if w < 30 {
            return;
        }
        let x0 = right - w - 1;
        let y0 = l.top;
        let rows = if fb.show_replace { 2 } else { 1 };
        let focused = self.focus == Focus::Find;
        let bar = Style::new().on(BAR);
        for r in 0..rows {
            s.fill(x0, y0 + r, w, bar);
        }

        let chev = if fb.show_replace { "▾" } else { "▸" };
        s.put(x0 + 1, y0, chev, Style::fg(BRIGHT).on(BAR));
        hits.push((y0, x0, x0 + 3, FindHit::Expand));

        let all = self.all_matches();
        let b = self.buf();
        let count = if fb.error.is_some() {
            t!("regex hatası", "bad regex").to_string()
        } else if fb.query.is_empty() {
            String::new()
        } else if all.is_empty() {
            t!("sonuç yok", "no results").to_string()
        } else {
            match all.iter().position(|m| m.0 == b.cy && m.1 == b.cx) {
                Some(i) => format!("{}/{}", i + 1, all.len()),
                None => format!("?/{}", all.len()),
            }
        };
        let buttons: [(&str, FindHit, bool); 6] = [
            ("Aa", FindHit::Case, fb.case),
            ("W", FindHit::Word, fb.word),
            (".*", FindHit::Regex, fb.regex),
            ("↑", FindHit::Prev, false),
            ("↓", FindHit::Next, false),
            ("✕", FindHit::Close, false),
        ];
        let btn_w: u16 = buttons.iter().map(|b| text_w(b.0) as u16 + 1).sum::<u16>() + 1;
        let count_w = 12u16;
        let fx = x0 + 3;
        let fw = w.saturating_sub(3 + count_w + btn_w + 1);

        let field = |s: &mut Screen, y: u16, text: &[char], cur: usize, active: bool, placeholder: &str, err: bool| {
            let st = Style::fg(if err { ERR } else { BRIGHT }).on(POP);
            s.fill(fx, y, fw, Style::new().on(POP));
            let inner = fw.saturating_sub(2) as usize;
            let mut start = 0;
            let mut wsum: usize = text[..cur.min(text.len())].iter().map(|c| char_width(*c, 0)).sum();
            while wsum >= inner && start < text.len() {
                wsum -= char_width(text[start], 0);
                start += 1;
            }
            if text.is_empty() && !active {
                put_clip(s, fx + 1, y, placeholder, Style::fg(DIM).on(POP), inner);
            } else {
                let t: String = text[start..].iter().collect();
                put_clip(s, fx + 1, y, &t, st, inner);
            }
            let cx = fx
                + 1
                + text[start..cur.min(text.len())]
                    .iter()
                    .map(|c| char_width(*c, 0) as u16)
                    .sum::<u16>();
            (cx, y)
        };

        let qc = field(
            s,
            y0,
            &fb.query,
            fb.qx,
            focused && fb.field == 0,
            t!("Bul", "Find"),
            fb.error.is_some(),
        );
        if fb.fresh && focused && fb.field == 0 && !fb.query.is_empty() {
            let t: String = fb.query.iter().collect();
            put_clip(
                s,
                fx + 1,
                y0,
                &t,
                Style::fg(BRIGHT).on(SEL),
                fw.saturating_sub(2) as usize,
            );
        }
        hits.push((y0, fx, fx + fw, FindHit::Query));
        let cx = fx + fw + 1;
        put_clip(
            s,
            cx,
            y0,
            &count,
            Style::fg(if all.is_empty() && !fb.query.is_empty() {
                ERR
            } else {
                DIM
            })
            .on(BAR),
            count_w as usize,
        );
        let mut bx = cx + count_w;
        for (label, hit, active) in buttons {
            let st = if active {
                Style::fg(LOGO_GREEN).on(SEL).bold()
            } else {
                Style::fg(BRIGHT).on(BAR)
            };
            let wlen = text_w(label) as u16;
            s.put(bx, y0, label, st);
            hits.push((y0, bx, bx + wlen, hit));
            bx += wlen + 1;
        }

        let mut cursor = if fb.field == 0 { Some(qc) } else { None };
        if fb.show_replace {
            let y1 = y0 + 1;
            let rc = field(
                s,
                y1,
                &fb.repl,
                fb.rx,
                focused && fb.field == 1,
                t!("Değiştir", "Replace"),
                false,
            );
            hits.push((y1, fx, fx + fw, FindHit::Replace));
            let one = t!("⇄ değiştir", "⇄ replace");
            let allb = t!("⇄ tümü", "⇄ all");
            let ox = fx + fw + 1;
            s.put(ox, y1, one, Style::fg(BRIGHT).on(BAR));
            hits.push((y1, ox, ox + text_w(one) as u16, FindHit::ReplaceOne));
            let ax = ox + text_w(one) as u16 + 2;
            s.put(ax, y1, allb, Style::fg(BRIGHT).on(BAR));
            hits.push((y1, ax, ax + text_w(allb) as u16, FindHit::ReplaceAll));
            if fb.field == 1 {
                cursor = Some(rc);
            }
        }
        if focused {
            self.find_cursor.set(cursor);
        }
        if let Some(e) = &fb.error {
            let y = y0 + rows;
            let msg = format!(" {} ", e);
            let mw = (text_w(&msg) as u16).min(w);
            s.fill(x0, y, mw, Style::new().on(BAR));
            put_clip(s, x0, y, &msg, Style::fg(ERR).on(BAR), mw as usize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn plain_and_case() {
        let m = Matcher::new("ab", false, false, false).unwrap();
        assert_eq!(m.find_line(&chars("Ab xab")), vec![(0, 2), (4, 6)]);
        let m = Matcher::new("ab", true, false, false).unwrap();
        assert_eq!(m.find_line(&chars("Ab xab")), vec![(4, 6)]);
    }

    #[test]
    fn word_and_unicode() {
        let m = Matcher::new("şey", false, true, false).unwrap();
        assert_eq!(m.find_line(&chars("şey bişey şey")), vec![(0, 3), (10, 13)]);
    }

    #[test]
    fn regex_replace() {
        let m = Matcher::new(r"(\w+)=(\d+)", true, false, true).unwrap();
        let (out, n) = m.replace_all_line(&chars("a=1 b=2"), "$2:$1");
        assert_eq!(out.iter().collect::<String>(), "1:a 2:b");
        assert_eq!(n, 2);
        let m = Matcher::new("a.b", true, false, false).unwrap();
        let (out, n) = m.replace_all_line(&chars("a.b axb"), "$1");
        assert_eq!(out.iter().collect::<String>(), "$1 axb");
        assert_eq!(n, 1);
    }

    #[test]
    fn replace_at_offset() {
        let m = Matcher::new("o", true, false, false).unwrap();
        let (line, end) = m.replace_at(&chars("foo"), 2, "00").unwrap();
        assert_eq!(line.iter().collect::<String>(), "fo00");
        assert_eq!(end, 4);
        assert!(m.replace_at(&chars("foo"), 0, "x").is_none());
    }

    #[test]
    fn bad_regex() {
        assert!(Matcher::new("(", true, false, true).is_err());
    }
}
