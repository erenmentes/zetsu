use crossterm::cursor::{self, SetCursorStyle};
use crossterm::queue;
use crossterm::style::{
    Attribute, Color, Print, SetAttribute, SetBackgroundColor, SetForegroundColor, SetUnderlineColor,
};
use crossterm::terminal::{self, ClearType};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use unicode_width::UnicodeWidthChar;

static TRUECOLOR: AtomicBool = AtomicBool::new(true);

pub fn detect_colors() {
    let ct = std::env::var("COLORTERM").unwrap_or_default().to_lowercase();
    let prog = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let full = ct == "truecolor" || ct == "24bit" || cfg!(windows) || prog != "Apple_Terminal";
    TRUECOLOR.store(full, Ordering::Relaxed);
}

fn ansi256(r: u8, g: u8, b: u8) -> u8 {
    if r == g && g == b {
        if r < 8 {
            return 16;
        }
        if r > 248 {
            return 231;
        }
        return 232 + ((r as u16 - 8) * 24 / 247) as u8;
    }
    let q = |v: u8| ((v as u16 * 5 + 127) / 255) as u8;
    16 + 36 * q(r) + 6 * q(g) + q(b)
}

fn fix(c: Color) -> Color {
    match c {
        Color::Rgb { r, g, b } if !TRUECOLOR.load(Ordering::Relaxed) => Color::AnsiValue(ansi256(r, g, b)),
        c => c,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub ul: Option<Color>,
}

impl Style {
    pub const fn new() -> Self {
        Style { fg: Color::Reset, bg: Color::Reset, bold: false, italic: false, ul: None }
    }
    pub const fn fg(c: Color) -> Self {
        Style { fg: c, bg: Color::Reset, bold: false, italic: false, ul: None }
    }
    pub const fn on(mut self, c: Color) -> Self {
        self.bg = c;
        self
    }
    pub const fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
    pub const fn italic(mut self) -> Self {
        self.italic = true;
        self
    }
    pub const fn underline(mut self, c: Color) -> Self {
        self.ul = Some(c);
        self
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Cell {
    ch: char,
    st: Style,
}

const BLANK: Cell = Cell { ch: ' ', st: Style::new() };
const WIDE_TAIL: char = '\0';

pub struct Screen {
    pub w: u16,
    pub h: u16,
    cells: Vec<Cell>,
    prev: Vec<Cell>,
}

impl Screen {
    pub fn new(w: u16, h: u16) -> Self {
        Screen { w, h, cells: vec![BLANK; w as usize * h as usize], prev: Vec::new() }
    }

    pub fn resize(&mut self, w: u16, h: u16) {
        if w != self.w || h != self.h {
            *self = Screen::new(w, h);
        }
    }

    pub fn clear(&mut self) {
        self.cells.fill(BLANK);
    }

    fn set(&mut self, x: u16, y: u16, ch: char, st: Style) {
        if x < self.w && y < self.h {
            self.cells[y as usize * self.w as usize + x as usize] = Cell { ch, st };
        }
    }

    pub fn fill(&mut self, x: u16, y: u16, n: u16, st: Style) {
        for i in 0..n {
            self.set(x.saturating_add(i), y, ' ', st);
        }
    }

    pub fn fill_row(&mut self, y: u16, st: Style) {
        self.fill(0, y, self.w, st);
    }

    pub fn put_char(&mut self, x: u16, y: u16, c: char, st: Style) -> u16 {
        let c = if c.is_control() { '·' } else { c };
        let w = c.width().unwrap_or(0) as u16;
        if w == 0 {
            return 0;
        }
        if x + w > self.w {
            if x < self.w {
                self.set(x, y, ' ', st);
            }
            return w;
        }
        self.set(x, y, c, st);
        if w == 2 {
            self.set(x + 1, y, WIDE_TAIL, st);
        }
        w
    }

    pub fn put(&mut self, x: u16, y: u16, s: &str, st: Style) -> u16 {
        let mut x = x;
        for c in s.chars() {
            if x >= self.w {
                break;
            }
            x += self.put_char(x, y, c, st);
        }
        x.min(self.w)
    }

    pub fn flush(&mut self, out: &mut impl Write, cursor: Option<(u16, u16)>, bar: bool) -> io::Result<()> {
        queue!(out, terminal::BeginSynchronizedUpdate, cursor::Hide)?;
        let full = self.prev.len() != self.cells.len();
        if full {
            queue!(out, SetAttribute(Attribute::Reset), terminal::Clear(ClearType::All))?;
        }
        let w = self.w as usize;
        let mut cur: Option<Style> = None;
        for y in 0..self.h as usize {
            let a = y * w;
            let row = &self.cells[a..a + w];
            if !full && &self.prev[a..a + w] == row {
                continue;
            }
            queue!(out, cursor::MoveTo(0, y as u16))?;
            for cell in row {
                if cell.ch == WIDE_TAIL {
                    continue;
                }
                if cur != Some(cell.st) {
                    let st = cell.st;
                    queue!(out, SetAttribute(Attribute::Reset), SetForegroundColor(fix(st.fg)), SetBackgroundColor(fix(st.bg)))?;
                    if st.bold {
                        queue!(out, SetAttribute(Attribute::Bold))?;
                    }
                    if st.italic {
                        queue!(out, SetAttribute(Attribute::Italic))?;
                    }
                    if let Some(c) = st.ul {
                        queue!(out, SetAttribute(Attribute::Underlined), SetUnderlineColor(fix(c)))?;
                    }
                    cur = Some(st);
                }
                queue!(out, Print(cell.ch))?;
            }
        }
        queue!(out, SetAttribute(Attribute::Reset))?;
        self.prev.clone_from(&self.cells);
        if let Some((x, y)) = cursor {
            let shape = if bar { SetCursorStyle::SteadyBar } else { SetCursorStyle::SteadyBlock };
            queue!(out, shape, cursor::MoveTo(x, y), cursor::Show)?;
        }
        queue!(out, terminal::EndSynchronizedUpdate)?;
        out.flush()
    }
}
