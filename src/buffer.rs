use crate::highlight::{self, LangDef};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthChar;

pub const TAB_W: usize = 4;

pub type Pos = (usize, usize);

pub fn char_width(c: char, col: usize) -> usize {
    if c == '\t' {
        TAB_W - col % TAB_W
    } else {
        c.width().unwrap_or(1)
    }
}

pub fn disp_col(line: &[char], x: usize) -> usize {
    let mut col = 0;
    for &c in &line[..x.min(line.len())] {
        col += char_width(c, col);
    }
    col
}

pub fn idx_at_col(line: &[char], target: usize) -> usize {
    let mut col = 0;
    for (i, &c) in line.iter().enumerate() {
        let w = char_width(c, col);
        if col + w > target {
            return i;
        }
        col += w;
    }
    line.len()
}

pub fn cls(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

pub fn leading_ws(line: &[char]) -> Vec<char> {
    line.iter().take_while(|c| **c == ' ' || **c == '\t').copied().collect()
}

struct Snapshot {
    lines: Vec<Vec<char>>,
    cx: usize,
    cy: usize,
}

pub struct Buffer {
    pub lines: Vec<Vec<char>>,
    pub path: Option<PathBuf>,
    pub name: String,
    pub dirty: bool,
    pub crlf: bool,
    pub bom: bool,
    pub use_tabs: bool,
    pub cx: usize,
    pub cy: usize,
    pub want_col: usize,
    pub scroll_y: usize,
    pub scroll_x: usize,
    pub lang: &'static LangDef,
    pub rev: u64,
    pub lsp_rev: u64,
    pub lsp_version: i32,
    pub lsp_open: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

impl Buffer {
    pub fn scratch(name: &str) -> Self {
        Buffer {
            lines: vec![Vec::new()],
            path: None,
            name: name.to_string(),
            dirty: false,
            crlf: false,
            bom: false,
            use_tabs: false,
            cx: 0,
            cy: 0,
            want_col: 0,
            scroll_y: 0,
            scroll_x: 0,
            lang: highlight::plain(),
            rev: 0,
            lsp_rev: 0,
            lsp_version: 0,
            lsp_open: false,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn from_text(name: &str, text: &str) -> Self {
        let mut b = Self::scratch(name);
        b.set_text(text);
        b
    }

    pub fn open(path: &Path) -> io::Result<Self> {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let mut b = Self::scratch(&name);
        b.lang = highlight::detect(path);
        let mut tabs = 0;
        let mut spaces = 0;
        if path.exists() {
            let bytes = fs::read(path)?;
            let mut text = String::from_utf8_lossy(&bytes).into_owned();
            if let Some(t) = text.strip_prefix('\u{feff}') {
                text = t.to_string();
                b.bom = true;
            }
            b.crlf = text.contains("\r\n");
            b.set_text(&text);
            tabs = b.lines.iter().filter(|l| l.first() == Some(&'\t')).count();
            spaces = b.lines.iter().filter(|l| l.first() == Some(&' ')).count();
        }
        b.use_tabs = if tabs + spaces == 0 {
            b.lang.name == "Go" || name == "Makefile" || name == "makefile"
        } else {
            tabs > spaces
        };
        b.path = Some(path.to_path_buf());
        Ok(b)
    }

    pub fn set_text(&mut self, text: &str) {
        let mut lines: Vec<Vec<char>> = text
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l).chars().collect())
            .collect();
        if lines.len() > 1 && text.ends_with('\n') {
            lines.pop();
        }
        if lines.is_empty() {
            lines.push(Vec::new());
        }
        self.lines = lines;
    }

    pub fn save(&mut self) -> io::Result<usize> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, t!("dosya adı yok", "no file name")))?;
        let nl = if self.crlf { "\r\n" } else { "\n" };
        let mut out = String::new();
        if self.bom {
            out.push('\u{feff}');
        }
        if !(self.lines.len() == 1 && self.lines[0].is_empty()) {
            for l in &self.lines {
                out.extend(l.iter());
                out.push_str(nl);
            }
        }
        fs::write(&path, out.as_bytes())?;
        self.dirty = false;
        Ok(out.len())
    }

    pub fn touch(&mut self) {
        self.dirty = true;
        self.rev += 1;
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        for (i, l) in self.lines.iter().enumerate() {
            if i > 0 {
                s.push('\n');
            }
            s.extend(l.iter());
        }
        s.push('\n');
        s
    }

    pub fn checkpoint(&mut self) {
        self.undo.push(Snapshot { lines: self.lines.clone(), cx: self.cx, cy: self.cy });
        if self.undo.len() > 1000 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn drop_noop_checkpoint(&mut self) {
        if let Some(s) = self.undo.last() {
            if s.lines == self.lines {
                self.undo.pop();
            }
        }
    }

    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            Some(s) => {
                let cur = Snapshot { lines: std::mem::replace(&mut self.lines, s.lines), cx: self.cx, cy: self.cy };
                self.redo.push(cur);
                self.cx = s.cx;
                self.cy = s.cy;
                self.touch();
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.redo.pop() {
            Some(s) => {
                let cur = Snapshot { lines: std::mem::replace(&mut self.lines, s.lines), cx: self.cx, cy: self.cy };
                self.undo.push(cur);
                self.cx = s.cx;
                self.cy = s.cy;
                self.touch();
                true
            }
            None => false,
        }
    }

    pub fn line_len(&self, y: usize) -> usize {
        self.lines.get(y).map_or(0, |l| l.len())
    }

    pub fn last(&self) -> usize {
        self.lines.len() - 1
    }

    pub fn first_nb(&self, y: usize) -> usize {
        self.lines[y].iter().position(|c| !c.is_whitespace()).unwrap_or(0)
    }

    pub fn clamp(&mut self, allow_end: bool) {
        if self.lines.is_empty() {
            self.lines.push(Vec::new());
        }
        self.cy = self.cy.min(self.lines.len() - 1);
        let len = self.line_len(self.cy);
        let max = if allow_end || len == 0 { len } else { len - 1 };
        self.cx = self.cx.min(max);
    }

    pub fn sync_want(&mut self) {
        self.want_col = disp_col(&self.lines[self.cy], self.cx);
    }

    pub fn insert_text(&mut self, (x, y): Pos, text: &[char]) -> Pos {
        let x = x.min(self.line_len(y));
        let tail: Vec<char> = self.lines[y].split_off(x);
        let (mut cx, mut cy) = (x, y);
        for &c in text {
            if c == '\n' {
                cy += 1;
                self.lines.insert(cy, Vec::new());
                cx = 0;
            } else {
                self.lines[cy].push(c);
                cx += 1;
            }
        }
        self.lines[cy].extend(tail);
        self.touch();
        (cx, cy)
    }

    pub fn get_text(&self, a: Pos, b: Pos) -> Vec<char> {
        let mut out = Vec::new();
        let bx = b.0.min(self.line_len(b.1));
        if a.1 == b.1 {
            if a.0 < bx {
                out.extend(&self.lines[a.1][a.0..bx]);
            }
            return out;
        }
        out.extend(&self.lines[a.1][a.0.min(self.line_len(a.1))..]);
        out.push('\n');
        for y in a.1 + 1..b.1 {
            out.extend(&self.lines[y]);
            out.push('\n');
        }
        out.extend(&self.lines[b.1][..bx]);
        out
    }

    pub fn delete_text(&mut self, a: Pos, b: Pos) -> Vec<char> {
        let t = self.get_text(a, b);
        let ax = a.0.min(self.line_len(a.1));
        let bx = b.0.min(self.line_len(b.1));
        if a.1 == b.1 {
            if ax < bx {
                self.lines[a.1].drain(ax..bx);
            }
        } else {
            let tail: Vec<char> = self.lines[b.1][bx..].to_vec();
            self.lines[a.1].truncate(ax);
            self.lines[a.1].extend(tail);
            self.lines.drain(a.1 + 1..=b.1);
        }
        self.touch();
        t
    }

    pub fn delete_lines(&mut self, y0: usize, y1: usize) -> Vec<Vec<char>> {
        let removed: Vec<_> = self.lines.drain(y0..=y1).collect();
        if self.lines.is_empty() {
            self.lines.push(Vec::new());
        }
        self.touch();
        removed
    }

    pub fn insert_lines(&mut self, at: usize, ls: &[Vec<char>]) {
        let at = at.min(self.lines.len());
        self.lines.splice(at..at, ls.iter().cloned());
        self.touch();
    }

    pub fn next_word(&self, (x, y): Pos) -> Pos {
        let (mut x, mut y) = (x, y);
        let line = &self.lines[y];
        if x < line.len() {
            let c0 = cls(line[x]);
            if c0 != 0 {
                while x < line.len() && cls(line[x]) == c0 {
                    x += 1;
                }
            }
        }
        loop {
            let line = &self.lines[y];
            while x < line.len() && cls(line[x]) == 0 {
                x += 1;
            }
            if x < line.len() {
                return (x, y);
            }
            if y + 1 >= self.lines.len() {
                return (line.len(), y);
            }
            y += 1;
            x = 0;
            if self.lines[y].is_empty() {
                return (0, y);
            }
        }
    }

    pub fn prev_word(&self, (x, y): Pos) -> Pos {
        let (mut x, mut y) = (x.min(self.line_len(y)), y);
        loop {
            if x == 0 {
                if y == 0 {
                    return (0, 0);
                }
                y -= 1;
                x = self.lines[y].len();
                if x == 0 {
                    return (0, y);
                }
                continue;
            }
            x -= 1;
            if cls(self.lines[y][x]) != 0 {
                break;
            }
        }
        let c0 = cls(self.lines[y][x]);
        while x > 0 && cls(self.lines[y][x - 1]) == c0 {
            x -= 1;
        }
        (x, y)
    }

    pub fn end_word(&self, (x, y): Pos) -> Pos {
        let (mut x, mut y) = (x + 1, y);
        loop {
            let len = self.lines[y].len();
            while x < len && cls(self.lines[y][x]) == 0 {
                x += 1;
            }
            if x < len {
                break;
            }
            if y + 1 >= self.lines.len() {
                return (len.saturating_sub(1), y);
            }
            y += 1;
            x = 0;
        }
        let c0 = cls(self.lines[y][x]);
        while x + 1 < self.lines[y].len() && cls(self.lines[y][x + 1]) == c0 {
            x += 1;
        }
        (x, y)
    }

    pub fn match_bracket(&self, (x, y): Pos) -> Option<Pos> {
        let line = &self.lines[y];
        let sx = (x..line.len()).find(|&i| "()[]{}".contains(line[i]))?;
        let c = line[sx];
        let (open, close, fwd) = match c {
            '(' => ('(', ')', true),
            '[' => ('[', ']', true),
            '{' => ('{', '}', true),
            ')' => ('(', ')', false),
            ']' => ('[', ']', false),
            _ => ('{', '}', false),
        };
        let mut depth = 0i32;
        let (mut cx, mut cy) = (sx as isize, y);
        loop {
            let l = &self.lines[cy];
            if cx >= 0 && (cx as usize) < l.len() {
                let ch = l[cx as usize];
                if ch == open {
                    depth += if fwd { 1 } else { -1 };
                } else if ch == close {
                    depth += if fwd { -1 } else { 1 };
                }
                if depth == 0 {
                    return Some((cx as usize, cy));
                }
            }
            if fwd {
                cx += 1;
                while cx as usize >= self.lines[cy].len() {
                    cy += 1;
                    if cy >= self.lines.len() {
                        return None;
                    }
                    cx = 0;
                }
            } else {
                cx -= 1;
                while cx < 0 {
                    if cy == 0 {
                        return None;
                    }
                    cy -= 1;
                    cx = self.lines[cy].len() as isize - 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(s: &str) -> Buffer {
        Buffer::from_text("t", s)
    }

    #[test]
    fn insert_delete() {
        let mut b = buf("hello\nworld");
        let end = b.insert_text((5, 0), &"!\nnew".chars().collect::<Vec<_>>());
        assert_eq!(end, (3, 1));
        assert_eq!(b.lines.len(), 3);
        let t = b.delete_text((5, 0), (3, 1));
        assert_eq!(t.iter().collect::<String>(), "!\nnew");
        assert_eq!(b.lines[0].iter().collect::<String>(), "hello");
    }

    #[test]
    fn words() {
        let b = buf("foo bar.baz\n\nqux");
        assert_eq!(b.next_word((0, 0)), (4, 0));
        assert_eq!(b.next_word((4, 0)), (7, 0));
        assert_eq!(b.next_word((8, 0)), (0, 1));
        assert_eq!(b.prev_word((0, 2)), (0, 1));
        assert_eq!(b.end_word((0, 0)), (2, 0));
    }

    #[test]
    fn brackets() {
        let b = buf("fn a() {\n  x\n}");
        assert_eq!(b.match_bracket((0, 0)), Some((5, 0)));
        assert_eq!(b.match_bracket((7, 0)), Some((0, 2)));
        assert_eq!(b.match_bracket((0, 2)), Some((7, 0)));
    }

    #[test]
    fn cols() {
        let l: Vec<char> = "\tab".chars().collect();
        assert_eq!(disp_col(&l, 1), 4);
        assert_eq!(idx_at_col(&l, 2), 0);
        assert_eq!(idx_at_col(&l, 5), 2);
    }
}
