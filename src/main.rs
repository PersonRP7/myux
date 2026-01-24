mod conpty;

use conpty::{spawn_conpty, TabPty};

use core::ffi::c_void;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind},
    queue,
    style::{Color, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};
use std::io::{self, Write};
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::Console::{
    GetConsoleScreenBufferInfo, GetStdHandle, CONSOLE_SCREEN_BUFFER_INFO, STD_OUTPUT_HANDLE,
};
use windows::Win32::System::Threading::TerminateProcess;

/// Simple application state (one tab for now).
struct App {
    tabs: Vec<TabPty>,
    active: usize,
    cols: i16,
    rows: i16,
}

impl App {
    fn active_tab(&self) -> &TabPty {
        &self.tabs[self.active]
    }
}

/// Get the current console size (columns, rows).
fn console_size() -> (i16, i16) {
    unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE).unwrap();
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        let _ = GetConsoleScreenBufferInfo(h, &mut info);
        let cols = info.srWindow.Right - info.srWindow.Left + 1;
        let rows = info.srWindow.Bottom - info.srWindow.Top + 1;
        (cols, rows)
    }
}

/// Write bytes to a Win32 HANDLE (ConPTY input).
fn write_all(handle: HANDLE, bytes: &[u8]) {
    unsafe {
        let mut written = 0u32;
        let _ = WriteFile(handle, Some(bytes), Some(&mut written), None);
    }
}

/// Draw a simple status bar on the bottom line.
fn draw_status_bar(app: &App) {
    let mut stdout = io::stdout();

    let (cols_u16, rows_u16) =
        terminal::size().unwrap_or((app.cols as u16, app.rows as u16));
    let last_row = rows_u16.saturating_sub(1);

    let text = format!(
        "[myux] tab {}/{} | F10: quit",
        app.active + 1,
        app.tabs.len()
    );

    let mut line = text;
    let cols = cols_u16 as usize;
    if line.len() < cols {
        line.push_str(&" ".repeat(cols - line.len()));
    } else {
        line.truncate(cols);
    }

    let _ = queue!(
        stdout,
        cursor::SavePosition,
        cursor::MoveTo(0, last_row),
        Clear(ClearType::CurrentLine),
        SetBackgroundColor(Color::DarkGrey),
        SetForegroundColor(Color::White),
    );
    let _ = write!(stdout, "{}", line);
    let _ = queue!(stdout, ResetColor, cursor::RestorePosition);
    let _ = stdout.flush();
}

/// Clear the status bar line (used when exiting).
fn clear_status_bar(app: &App) {
    let mut stdout = io::stdout();
    let (cols_u16, rows_u16) =
        terminal::size().unwrap_or((app.cols as u16, app.rows as u16));
    let last_row = rows_u16.saturating_sub(1);

    let _ = queue!(
        stdout,
        cursor::SavePosition,
        cursor::MoveTo(0, last_row),
        Clear(ClearType::CurrentLine),
        cursor::RestorePosition,
    );
    let _ = stdout.flush();
}

fn main() -> windows::core::Result<()> {
    // 1) Spawn ConPTY with a cmd.exe child
    let (cols, rows) = console_size();
    println!("Spawning ConPTY {cols}x{rows}...");
    let first_tab = spawn_conpty("cmd.exe", cols, rows)?; // swap to "pwsh.exe" later if you like

    let out_raw: isize = first_tab.pty_out_read.0 as isize;

    let mut app = App {
        tabs: vec![first_tab],
        active: 0,
        cols,
        rows,
    };

    // 2) Enable raw mode and draw initial status bar
    terminal::enable_raw_mode().unwrap();
    draw_status_bar(&app);

    // 3) Reader thread: pump ConPTY output to stdout.
    //    We don't bother trying to distinguish "normal" vs "error" shutdown here;
    //    the whole process will exit on F10 anyway.
    let _reader = thread::spawn(move || {
        let out_handle = HANDLE(out_raw as *mut c_void);
        let mut buf = [0u8; 8192];

        loop {
            unsafe {
                let mut read = 0u32;

                let res = ReadFile(out_handle, Some(&mut buf), Some(&mut read), None);

                if let Err(err) = res {
                    eprintln!("[reader] ReadFile error: {err:?}");
                    break;
                }

                if read == 0 {
                    break;
                }

                let _ = io::stdout().write_all(&buf[..read as usize]);
                let _ = io::stdout().flush();
            }
        }
    });

    // 4) Main input loop: F10 quits, other keys go into the child
    loop {
        if event::poll(Duration::from_millis(16)).unwrap() {
            match event::read().unwrap() {
                Event::Key(KeyEvent { code, kind, .. }) => {
                    if kind != KeyEventKind::Press {
                        continue;
                    }

                    // Brutal quit on F10:
                    if code == KeyCode::F(10) {
                        unsafe {
                            let _ =
                                TerminateProcess(app.active_tab().child_process, 0);
                        }
                        clear_status_bar(&app);
                        // Turn off raw mode so host console behaves normally again
                        let _ = terminal::disable_raw_mode();
                        // Hard-exit the whole process; OS will tear down the reader thread.
                        std::process::exit(0);
                    }

                    let pty_in = app.active_tab().pty_in_write;

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

                    draw_status_bar(&app);
                }
                Event::Resize(new_cols, new_rows) => {
                    app.cols = new_cols as i16;
                    app.rows = new_rows as i16;
                    let _ = app.active_tab().resize(app.cols, app.rows);
                    draw_status_bar(&app);
                }
                _ => {}
            }
        }
    }

    // (Unreachable because of std::process::exit, but kept for completeness)
    // clear_status_bar(&app);
    // let _ = terminal::disable_raw_mode();
    // Ok(())
}