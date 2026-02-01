// src/main.rs
mod conpty;
mod terminal;
mod renderer;

use conpty::{spawn_conpty, TabPty};
use terminal::VirtualTerminal;
use renderer::Renderer;

use core::ffi::c_void;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::Console::{
    GetConsoleMode, GetConsoleScreenBufferInfo, GetStdHandle, SetConsoleMode,
    CONSOLE_SCREEN_BUFFER_INFO, CONSOLE_MODE, ENABLE_PROCESSED_OUTPUT,
    ENABLE_VIRTUAL_TERMINAL_PROCESSING, STD_OUTPUT_HANDLE,
};
use windows::Win32::System::Threading::TerminateProcess;

struct Tab {
    pty: TabPty,
    term: VirtualTerminal,
}

struct App {
    tabs: Vec<Tab>,
    active: usize,
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
    // 1) Enable VT on host console and get size.
    enable_vt_mode();
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
    // Clear once; Renderer will take over.
    crossterm::execute!(
        io::stdout(),
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
    )
    .ok();

    let mut app = app;
    let mut renderer = Renderer::new();

    // 5) Main loop: drain output, handle input, redraw.
    loop {
        // Drain ConPTY output into the virtual terminal.
        while let Ok(bytes) = rx.try_recv() {
            app.active_tab_mut().term.feed_bytes(&bytes);
        }

        // Build status line.
        let status_line = format!(
            "[myux] tab {}/{} | F10: quit",
            app.active + 1,
            app.tabs.len()
        );

        // Handle input if any.
        if event::poll(Duration::from_millis(10)).unwrap_or(false) {
            match event::read().unwrap() {
                Event::Key(KeyEvent { code, kind, .. }) => {
                    if kind != KeyEventKind::Press {
                        // ignore repeats / releases
                        continue;
                    }

                    if code == KeyCode::F(10) {
                        // Brutal quit: kill child, restore console.
                        unsafe {
                            let child = app.active_tab().pty.child_process;
                            let _ = TerminateProcess(child, 0);
                        }
                        disable_raw_mode().ok();
                        // Optionally clear on exit:
                        crossterm::execute!(
                            io::stdout(),
                            crossterm::terminal::Clear(
                                crossterm::terminal::ClearType::All
                            ),
                            crossterm::cursor::MoveTo(0, 0),
                        )
                        .ok();
                        return Ok(());
                    }

                    // Forward basic keys to ConPTY.
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
                        _ => {}
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
                }

                _ => {}
            }
        }

        // Redraw from the VT model.
        renderer.draw(&app.active_tab().term, &status_line).ok();
    }
}
