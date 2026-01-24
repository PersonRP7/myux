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

fn write_all(handle: HANDLE, bytes: &[u8]) {
    unsafe {
        let mut written = 0u32;
        let _ = WriteFile(handle, Some(bytes), Some(&mut written), None);
    }
}

fn draw_status_bar(app: &App) {
    let mut stdout = io::stdout();

    let (cols_u16, rows_u16) =
        terminal::size().unwrap_or((app.cols as u16, app.rows as u16));
    let last_row = rows_u16.saturating_sub(1);

    let text = format!(
        "[myux] tab {}/{} | Esc: quit",
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

fn main() -> windows::core::Result<()> {
    let (cols, rows) = console_size();
    println!("Spawning ConPTY {cols}x{rows}...");
    let first_tab = spawn_conpty("cmd.exe", cols, rows)?; // cmd.exe for clarity

    let out_raw: isize = first_tab.pty_out_read.0 as isize;

    let mut app = App {
        tabs: vec![first_tab],
        active: 0,
        cols,
        rows,
    };

    terminal::enable_raw_mode().unwrap();

    // Initial bar (will get redrawn once the shell prints something)
    draw_status_bar(&app);

    // Reader thread: ConPTY output → stdout + redraw bar
    // NOTE: we clone just the scalar fields needed for drawing.
    let reader_cols = app.cols;
    let reader_rows = app.rows;
    let reader = thread::spawn(move || {
        let out_handle = HANDLE(out_raw as *mut c_void);
        let mut buf = [0u8; 8192];

        // tiny "app view" inside the thread, enough to draw the bar
        let thread_app = App {
            tabs: Vec::new(),        // not used here
            active: 0,               // not used
            cols: reader_cols,
            rows: reader_rows,
        };

        loop {
            unsafe {
                let mut read = 0u32;

                let res = ReadFile(out_handle, Some(&mut buf), Some(&mut read), None);

                if let Err(_) = res {
                    break;
                }

                if read == 0 {
                    break;
                }

                let _ = io::stdout().write_all(&buf[..read as usize]);
                let _ = io::stdout().flush();
            }

            // redraw the bar after each chunk of output so it stays at the bottom
            draw_status_bar(&thread_app);
        }
    });

    // Main input loop: Esc quits, everything else goes to cmd.exe
    loop {
        if event::poll(Duration::from_millis(16)).unwrap() {
            match event::read().unwrap() {
                Event::Key(KeyEvent { code, kind, .. }) => {
                    if kind != KeyEventKind::Press {
                        continue;
                    }

                    if code == KeyCode::Esc {
                        unsafe {
                            let _ = TerminateProcess(app.active_tab().child_process, 0);
                        }
                        break;
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

                    // and redraw from main side too
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

    terminal::disable_raw_mode().unwrap();
    let _ = reader.join();

    Ok(())
}