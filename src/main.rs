#[macro_use]
mod i18n;
mod ai;
mod buffer;
mod config;
mod discord;
mod editor;
mod highlight;
mod lsp;
mod render;
mod screen;
mod tree;

use crossterm::cursor::{SetCursorStyle, Show};
use crossterm::event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    self, DisableLineWrap, EnableLineWrap, EnterAlternateScreen, LeaveAlternateScreen,
};
use editor::Editor;
use screen::Screen;
use std::io::{self, BufWriter, Write};
use std::time::Duration;

fn setup() -> io::Result<()> {
    terminal::enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture, DisableLineWrap)?;
    let _ = execute!(out, EnableBracketedPaste);
    Ok(())
}

fn restore() {
    let mut out = io::stdout();
    let _ = execute!(out, DisableBracketedPaste);
    let _ = execute!(out, DisableMouseCapture, EnableLineWrap, SetCursorStyle::DefaultUserShape, Show, LeaveAlternateScreen);
    let _ = terminal::disable_raw_mode();
    let _ = out.flush();
}

fn run(ed: &mut Editor) -> io::Result<()> {
    let mut out = BufWriter::with_capacity(1 << 16, io::stdout());
    let mut screen = Screen::new(ed.w, ed.h);
    let mut redraw = true;
    loop {
        if redraw {
            screen.resize(ed.w, ed.h);
            ed.draw(&mut screen);
            screen.flush(&mut out, ed.cursor_pos(), ed.bar_cursor())?;
            redraw = false;
        }
        if ed.quit {
            return Ok(());
        }
        if event::poll(Duration::from_millis(20))? {
            let ev = event::read()?;
            ed.burst = event::poll(Duration::ZERO)?;
            ed.handle(ev);
            while !ed.quit && event::poll(Duration::ZERO)? {
                let ev = event::read()?;
                ed.burst = true;
                ed.handle(ev);
            }
            ed.burst = false;
            redraw = true;
        }
        if ed.lsp_tick() {
            redraw = true;
        }
        if ed.ai_tick() {
            redraw = true;
        }
        ed.presence_tick();
    }
}

fn main() -> io::Result<()> {
    i18n::init();
    screen::detect_colors();
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("-h") | Some("--help") => {
            println!("zetsu {}", env!("CARGO_PKG_VERSION"));
            println!("{}", t!("kullanım: zetsu [dosya | klasör]", "usage: zetsu [file | folder]"));
            println!("{}", t!("Editörde :help tüm kısayolları gösterir.", "Type :help inside the editor for all shortcuts."));
            return Ok(());
        }
        Some("-V") | Some("--version") => {
            println!("zetsu {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        _ => {}
    }
    let (w, h) = terminal::size()?;
    let mut ed = Editor::new(arg, w, h);

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        default_hook(info);
    }));

    setup()?;
    let res = run(&mut ed);
    ed.lsp.stop_all();
    restore();
    res
}
