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
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::Console::{
    GetConsoleScreenBufferInfo, GetStdHandle, CONSOLE_SCREEN_BUFFER_INFO, STD_OUTPUT_HANDLE,
};
use windows::Win32::System::Threading::TerminateProcess;

/// Runtime state for the child terminals (tabs).
struct App {
    tabs: Vec<TabPty>,
    active: usize,
}

/// Helper: access active tab.
impl App {
    fn active_tab(&self) -> &TabPty {
        &self.tabs[self.active]
    }
}

/// Minimal state the status bar needs, shared between threads.
#[derive(Clone, Copy)]
struct StatusBarState {
    cols: u16,
    rows: u16,
    active: usize,
    tab_count: usize,
}

type StatusHandle = Arc<Mutex<StatusBarState>>;

fn console_size() -> (u16, u16) {
    // Prefer crossterm, fall back to Win32 if needed.
    terminal::size().unwrap_or_else(|_| unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE).unwrap();
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        let _ = GetConsoleScreenBufferInfo(h, &mut info);
        let cols =
            (info.srWindow.Right - info.srWindow.Left + 1).max(1) as u16;
        let rows =
            (info.srWindow.Bottom - info.srWindow.Top + 1).max(1) as u16;
        (cols, rows)
    })
}

/// Write bytes to a Win32 HANDLE (ConPTY input).
fn write_all(handle: HANDLE, bytes: &[u8]) {
    unsafe {
        let mut written = 0u32;
        let _ = WriteFile(handle, Some(bytes), Some(&mut written), None);
    }
}

/// Draw the status bar on the bottom line, using shared state.
fn draw_status_bar(status: &StatusHandle) {
    let snap = {
        // snapshot under the mutex, then drop the lock
        let s = status.lock().unwrap();
        *s
    };

    let mut stdout = io::stdout();

    // Live size if possible, snapshot as fallback.
    let (cols_u16, rows_u16) =
        terminal::size().unwrap_or((snap.cols, snap.rows));
    let last_row = rows_u16.saturating_sub(1);

    let text = format!(
        "[myux] tab {}/{} | F10: quit",
        snap.active + 1,
        snap.tab_count.max(1),
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
fn clear_status_bar(status: &StatusHandle) {
    let snap = {
        let s = status.lock().unwrap();
        *s
    };

    let mut stdout = io::stdout();
    let (cols_u16, rows_u16) =
        terminal::size().unwrap_or((snap.cols, snap.rows));
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
    // 1) Visible console size, reserve last row for status bar.
    let (cols, rows) = console_size();
    let conpty_rows: i16 = (rows as i16 - 1).max(1); // one less than host window

    println!(
        "Spawning ConPTY {}x{} (ConPTY rows {})...",
        cols, rows, conpty_rows
    );
    let first_tab = spawn_conpty("cmd.exe", cols as i16, conpty_rows)?; // swap to "pwsh.exe" later

    let out_raw: isize = first_tab.pty_out_read.0 as isize;

    let mut app = App {
        tabs: vec![first_tab],
        active: 0,
    };

    // Shared status bar state (used by both threads)
    let status: StatusHandle = Arc::new(Mutex::new(StatusBarState {
        cols,
        rows,
        active: 0,
        tab_count: 1,
    }));

    // 2) Enable raw mode and draw initial status bar
    terminal::enable_raw_mode().unwrap();
    draw_status_bar(&status);

    // 3) Reader thread: ConPTY output → stdout, then redraw bar
    let status_for_reader = Arc::clone(&status);
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

            // Keep bar pinned at bottom after each burst of output
            draw_status_bar(&status_for_reader);
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

                    // Brutal quit on F10
                    if code == KeyCode::F(10) {
                        unsafe {
                            let _ =
                                TerminateProcess(app.active_tab().child_process, 0);
                        }
                        clear_status_bar(&status);
                        let _ = terminal::disable_raw_mode();
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

                    // Main thread redraw too (after a keypress)
                    draw_status_bar(&status);
                }
                Event::Resize(new_cols, new_rows) => {
                    // Update status bar state
                    {
                        let mut s = status.lock().unwrap();
                        s.cols = new_cols;
                        s.rows = new_rows;
                        s.active = app.active;
                        s.tab_count = app.tabs.len();
                    }

                    // Resize the pseudo console (still reserving bottom row)
                    let conpty_rows = (new_rows as i16 - 1).max(1);
                    let _ = app
                        .active_tab()
                        .resize(new_cols as i16, conpty_rows);

                    draw_status_bar(&status);
                }
                _ => {}
            }
        }
    }
}
