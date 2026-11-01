//! nanote in the terminal: a nano-style editor (`nanote-tui [+LINE] [files...]`).
//!
//! Text encodings, language detection and the UI strings are shared with the GUI editor by
//! including its modules, so both open and save files the same way.

#[path = "../../../src/encoding.rs"]
#[allow(dead_code)]
mod encoding;
#[path = "../../../src/git.rs"]
#[allow(dead_code)]
mod git;
#[path = "../../../src/i18n.rs"]
#[allow(dead_code)]
mod i18n;

mod app;
mod buffer;
mod doc;
mod highlight;
mod menu;
mod ui;

use std::{
    io::{self, IsTerminal, Write},
    time::Duration,
};

use ratatui::{
    crossterm::{
        event::{
            self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste,
            EnableMouseCapture, Event, KeyEventKind,
        },
        execute,
    },
    layout::Size,
};

use app::App;
use highlight::Highlighter;

const USAGE: &str = "\
nanote-tui - nano-style terminal editor with syntax highlighting

Usage: nanote-tui [options] [+LINE] [files...]

  +LINE            put the cursor on a line of the first file
  --theme NAME     colour theme (default base16-ocean.dark; GitHub for light terminals)
  --themes         list the colour themes
  -V, --version    print the version
  -h, --help       print this help

Press F1 in the editor for the keys. As a Git editor: git config --global core.editor nanote-tui";

struct Args {
    files: Vec<String>,
    line: Option<usize>,
    theme: Option<String>,
}

fn parse_args() -> Result<Args, i32> {
    let mut args = Args {
        files: Vec::new(),
        line: None,
        theme: None,
    };
    let mut it = std::env::args().skip(1);
    let mut only_files = false;
    while let Some(a) = it.next() {
        if only_files {
            args.files.push(a);
            continue;
        }
        match a.as_str() {
            "--" => only_files = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Err(0);
            }
            "-V" | "--version" => {
                println!("nanote-tui {}", env!("CARGO_PKG_VERSION"));
                return Err(0);
            }
            "--themes" => {
                for name in Highlighter::theme_names() {
                    println!("{name}");
                }
                return Err(0);
            }
            "--theme" => match it.next() {
                Some(name) => args.theme = Some(name),
                None => {
                    eprintln!("--theme needs a name (see --themes)");
                    return Err(2);
                }
            },
            _ if a.starts_with('+') && a[1..].parse::<usize>().is_ok() => {
                args.line = a[1..].parse().ok();
            }
            _ if a.starts_with('-') && a.len() > 1 => {
                eprintln!("unknown option: {a}\n\n{USAGE}");
                return Err(2);
            }
            _ => args.files.push(a),
        }
    }
    Ok(args)
}

/// Dark by default; light when the terminal says its background is light (COLORFGBG).
fn default_theme() -> &'static str {
    let light = std::env::var("COLORFGBG")
        .ok()
        .and_then(|v| v.rsplit(';').next().and_then(|b| b.parse::<u8>().ok()))
        .is_some_and(|bg| bg == 7 || bg == 15);
    if light { "GitHub" } else { "base16-ocean.dark" }
}

fn osc52(text: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in text.as_bytes().chunks(3) {
        let n = chunk.len();
        let v = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(TABLE[(v >> 18) as usize & 63] as char);
        out.push(TABLE[(v >> 12) as usize & 63] as char);
        out.push(if n > 1 { TABLE[(v >> 6) as usize & 63] as char } else { '=' });
        out.push(if n > 2 { TABLE[v as usize & 63] as char } else { '=' });
    }
    format!("\x1b]52;c;{out}\x07")
}

fn run(args: Args) -> io::Result<()> {
    let theme = args
        .theme
        .or_else(|| std::env::var("NANOTE_TUI_THEME").ok())
        .unwrap_or_else(|| default_theme().to_owned());
    let highlighter = Highlighter::new(&theme);

    let mut terminal = ratatui::init();
    execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let size = terminal.size()?;
    let mut app = App::new(highlighter, size);
    for file in &args.files {
        app.open(file, None);
    }
    app.show_first();
    if let Some(line) = args.line {
        app.goto(line);
    }
    app.resize(size);

    let result = (|| -> io::Result<()> {
        loop {
            terminal.draw(|f| ui::draw(f, &mut app))?;
            if let Some(text) = app.osc52.take() {
                let mut out = io::stdout();
                out.write_all(osc52(&text).as_bytes())?;
                out.flush()?;
            }
            if app.quit {
                return Ok(());
            }
            if !event::poll(Duration::from_millis(500))? {
                continue;
            }
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => app.key(k),
                Event::Mouse(m) => app.mouse(m),
                Event::Paste(text) => app.paste(&text),
                Event::Resize(w, h) => app.resize(Size::new(w, h)),
                _ => {}
            }
        }
    })();

    let _ = execute!(io::stdout(), DisableBracketedPaste, DisableMouseCapture);
    ratatui::restore();
    result
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(code) => std::process::exit(code),
    };
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!("nanote-tui needs a terminal");
        std::process::exit(1);
    }
    if let Err(e) = run(args) {
        eprintln!("nanote-tui: {e}");
        std::process::exit(1);
    }
}
