// src/main.rs
mod conpty;
mod terminal;
mod renderer;

use conpty::{spawn_conpty, TabPty};
use renderer::Renderer;
use terminal::VirtualTerminal;

use core::ffi::c_void;
use crossterm::{
    cursor,
    event::{
        self,
        EnableMouseCapture,
        DisableMouseCapture,
        Event,
        KeyCode,
        KeyEvent,
        KeyEventKind,
        MouseEvent,
        MouseEventKind,
    },
    terminal::{disable_raw_mode, enable_raw_mode},
};
use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::Console::{
    GetConsoleMode, GetConsoleScreenBufferInfo, GetStdHandle, SetConsoleScreenBufferSize,
    SetConsoleMode, CONSOLE_SCREEN_BUFFER_INFO, CONSOLE_MODE,
    ENABLE_PROCESSED_OUTPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, STD_OUTPUT_HANDLE,
};
use windows::Win32::System::Console::COORD;
use windows::Win32::System::Threading::TerminateProcess;

struct Tab {
    pty: TabPty,
    term: VirtualTerminal,
}

const SCROLL_STEP: u16 = 5;

enum Mode {
    Normal,
    Scrollback,
}

struct App {
    tabs: Vec<Tab>,
    active: usize,
    mode: Mode,
}

impl App {
    fn active_tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    fn active_tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }
}

/// Enable VT sequences on host console.
fn enable_vt_mode() {
    unsafe {
        if let Ok(h) = GetStdHandle(STD_OUTPUT_HANDLE) {
            let mut mode = CONSOLE_MODE(0);
            if GetConsoleMode(h, &mut mode).is_ok() {
                let new_mode =
                    mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING | ENABLE_PROCESSED_OUTPUT;
                let _ = SetConsoleMode(h, new_mode);
            }
        }
    }
}

/// Make the console buffer height match the window height
/// so there is no native scrollback/scrollbar fighting us.
fn clamp_console_buffer_to_window() {
    unsafe {
        if let Ok(h) = GetStdHandle(STD_OUTPUT_HANDLE) {
            let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
            if GetConsoleScreenBufferInfo(h, &mut info).is_ok() {
                let width = info.srWindow.Right - info.srWindow.Left + 1;
                let height = info.srWindow.Bottom - info.srWindow.Top + 1;
                let size = COORD {
                    X: width as i16,
                    Y: height as i16,
                };
                let _ = SetConsoleScreenBufferSize(h, size);
            }
        }
    }
}

fn console_size() -> (u16, u16) {
    crossterm::terminal::size().unwrap_or_else(|_| unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE).unwrap();
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        let _ = GetConsoleScreenBufferInfo(h, &mut info);
        let cols = (info.srWindow.Right - info.srWindow.Left + 1).max(1) as u16;
        let rows = (info.srWindow.Bottom - info.srWindow.Top + 1).max(1) as u16;
        (cols, rows)
    })
}

/// Write bytes to ConPTY input.
fn write_all(handle: HANDLE, bytes: &[u8]) {
    unsafe {
        let mut written = 0u32;
        let _ = WriteFile(handle, Some(bytes), Some(&mut written), None);
    }
}

fn main() -> windows::core::Result<()> {
    // 1) Enable VT on host console and clamp buffer to window.
    enable_vt_mode();
    clamp_console_buffer_to_window();
    let (cols, rows) = console_size();

    // 2) Spawn a single ConPTY-backed cmd.exe.
    println!("Spawning ConPTY {}x{}...", cols, rows);
    let pty = spawn_conpty("cmd.exe", cols as i16, rows as i16)?;

    // We capture the raw value of the output handle for the reader thread.
    let out_raw: isize = pty.pty_out_read.0 as isize;

    let term = VirtualTerminal::new(cols, rows);
    let app = App {
        tabs: vec![Tab { pty, term }],
        active: 0,
        mode: Mode::Normal,
    };

    // 3) Channel: reader thread → main thread.
    let (tx, rx) = mpsc::channel::<Vec<u8>>();

    // Reader thread: ReadFile from ConPTY → send Vec<u8> via channel.
    let _reader = thread::spawn(move || {
        let out_handle = HANDLE(out_raw as *mut c_void);
        let mut buf = [0u8; 8192];

        loop {
            let mut read = 0u32;
            let res = unsafe { ReadFile(out_handle, Some(&mut buf), Some(&mut read), None) };

            if let Err(err) = res {
                eprintln!("[reader] ReadFile error: {err:?}");
                break;
            }
            if read == 0 {
                break;
            }

            let chunk = buf[..read as usize].to_vec();
            if tx.send(chunk).is_err() {
                break;
            }
        }
    });

    // 4) Terminal setup in main thread.
    enable_raw_mode().unwrap();
    // Clear once & enable mouse; Renderer will take over.
    crossterm::execute!(
        io::stdout(),
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
        EnableMouseCapture,
    )
    .ok();

    let mut app = app;
    let mut renderer = Renderer::new();

    // Hide cursor once; renderer no longer hides it every frame.
    crossterm::execute!(io::stdout(), cursor::Hide).ok();

    // Track whether we need to redraw.
    let mut dirty = true;

    // 5) Main loop: drain output, handle input, redraw.
    loop {
        // Drain ConPTY output into the virtual terminal.
        while let Ok(bytes) = rx.try_recv() {
            app.active_tab_mut().term.feed_bytes(&bytes);
            dirty = true;
        }

        // Build status line (include mode).
        let mode_str = match app.mode {
            Mode::Normal => "normal",
            Mode::Scrollback => "scroll",
        };

        let status_line = format!(
            "[myux] tab {}/{} | mode: {} | F10: quit",
            app.active + 1,
            app.tabs.len(),
            mode_str,
        );

        // Handle input if any.
        if event::poll(Duration::from_millis(50)).unwrap_or(false) {
            match event::read().unwrap() {
                Event::Key(KeyEvent { code, kind, .. }) => {
                    if kind != KeyEventKind::Press {
                        // ignore repeats / releases
                        continue;
                    }

                    // Global: F10 quits.
                    if code == KeyCode::F(10) {
                        unsafe {
                            let child = app.active_tab().pty.child_process;
                            let _ = TerminateProcess(child, 0);
                        }
                        disable_raw_mode().ok();
                        crossterm::execute!(
                            io::stdout(),
                            DisableMouseCapture,
                            cursor::Show,
                            crossterm::terminal::Clear(
                                crossterm::terminal::ClearType::All
                            ),
                            crossterm::cursor::MoveTo(0, 0),
                        )
                        .ok();
                        return Ok(());
                    }

                    // -------- Scrollback mode handling --------
                    match app.mode {
                        Mode::Normal => {
                            match code {
                                // Enter scrollback mode on PageUp
                                KeyCode::PageUp => {
                                    app.mode = Mode::Scrollback;
                                    app.active_tab_mut().term.scroll_up(5);
                                    dirty = true;
                                    continue; // don't send PageUp to the child
                                }
                                _ => { /* fall through to normal key handling */ }
                            }
                        }
                        Mode::Scrollback => {
                            match code {
                                KeyCode::PageUp => {
                                    app.active_tab_mut().term.scroll_up(5);
                                    dirty = true;
                                    continue;
                                }
                                KeyCode::PageDown => {
                                    app.active_tab_mut().term.scroll_down(5);
                                    if app.active_tab().term.is_at_bottom() {
                                        app.mode = Mode::Normal;
                                    }
                                    dirty = true;
                                    continue;
                                }
                                KeyCode::Esc => {
                                    app.active_tab_mut().term.reset_scrollback();
                                    app.mode = Mode::Normal;
                                    dirty = true;
                                    continue;
                                }
                                _ => {
                                    // while in scrollback, ignore all other keys
                                    continue;
                                }
                            }
                        }
                    }

                    // -------- Normal key → ConPTY --------
                    let pty_in = app.active_tab().pty.pty_in_write;
                    match code {
                        KeyCode::Enter => write_all(pty_in, b"\r"),
                        KeyCode::Backspace => write_all(pty_in, &[0x08]),
                        KeyCode::Tab => write_all(pty_in, b"\t"),
                        KeyCode::Char(c) => {
                            let mut s = [0u8; 4];
                            let n = c.encode_utf8(&mut s).len();
                            write_all(pty_in, &s[..n]);
                        }
                        KeyCode::Left => write_all(pty_in, b"\x1b[D"),
                        KeyCode::Right => write_all(pty_in, b"\x1b[C"),
                        KeyCode::Up => write_all(pty_in, b"\x1b[A"),
                        KeyCode::Down => write_all(pty_in, b"\x1b[B"),
                        KeyCode::Esc => write_all(pty_in, b"\x1b"),
                        _ => {}
                    }

                    dirty = true;
                }

                Event::Mouse(mouse) => {
                        use MouseEventKind::*;

                        match mouse.kind {
                            MouseEventKind::ScrollUp => {
                                match app.mode {
                                    Mode::Normal => {
                                        // Same as first PageUp: enter scrollback mode.
                                        app.mode = Mode::Scrollback;
                                        app.active_tab_mut().term.scroll_up(SCROLL_STEP);
                                    }
                                    Mode::Scrollback => {
                                        app.active_tab_mut().term.scroll_up(SCROLL_STEP);
                                    }
                                }
                                dirty = true;
                            }
                            MouseEventKind::ScrollDown => {
                                match app.mode {
                                    Mode::Normal => {
                                        // In normal mode at bottom: you could choose to ignore,
                                        // or later, pass wheel to child. For now: ignore.
                                    }
                                    Mode::Scrollback => {
                                        app.active_tab_mut().term.scroll_down(SCROLL_STEP);
                                        if app.active_tab().term.is_at_bottom() {
                                            app.mode = Mode::Normal;
                                        }
                                        dirty = true;
                                    }
                                }
                            }
                            _ => {
                                // Ignore other mouse events for now (clicks, moves).
                            }
                        }
                    }

                Event::Resize(new_cols, new_rows) => {
                    // Resize VT
                    app.active_tab_mut().term.resize(new_cols, new_rows);
                    // Resize ConPTY
                    let _ = app
                        .active_tab()
                        .pty
                        .resize(new_cols as i16, new_rows as i16);
                    dirty = true;
                }

                _ => {}
            }
        }

        // Redraw only when something changed.
        if dirty {
            renderer.draw(&app.active_tab().term, &status_line).ok();
            dirty = false;
        }
    }
}
