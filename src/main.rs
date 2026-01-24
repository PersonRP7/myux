mod conpty;

use conpty::spawn_conpty;
use core::ffi::c_void;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    terminal,
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

fn main() -> windows::core::Result<()> {
    // 1) Spawn the pseudo console with a shell
    let (cols, rows) = console_size();
    println!("Spawning ConPTY {cols}x{rows}...");
    let tab = spawn_conpty("cmd.exe", cols, rows)?; // or "pwsh.exe" if you prefer

    // 2) Enable raw mode so we get keypresses immediately
    terminal::enable_raw_mode().unwrap();
    println!("\r\n[myux] Interactive shell started. Press Ctrl+Q to exit.\r\n");

    // 3) Spawn a reader thread that pumps ConPTY output to stdout
    let out_raw: isize = tab.pty_out_read.0 as isize;

    let reader = thread::spawn(move || {
        let out_handle = HANDLE(out_raw as *mut c_void);
        let mut buf = [0u8; 8192];

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
        }
    });

    // 4) Main input loop: read keyboard events and forward them into ConPTY
    loop {
        if event::poll(Duration::from_millis(16)).unwrap() {
            match event::read().unwrap() {
                Event::Key(KeyEvent { code, modifiers, kind, .. }) => {
                    // Only act on actual key presses, ignore release/repeat
                    if kind != KeyEventKind::Press {
                        continue;
                    }

                    // Exit: Ctrl+Q
                    if code == KeyCode::Char('q') && modifiers.contains(KeyModifiers::CONTROL) {
                        unsafe {
                            let _ = TerminateProcess(tab.child_process, 0);
                        }
                        break;
                    }

                    match code {
                        KeyCode::Enter => {
                            write_all(tab.pty_in_write, b"\r");
                        }
                        KeyCode::Backspace => write_all(tab.pty_in_write, &[0x08]),
                        KeyCode::Tab => write_all(tab.pty_in_write, b"\t"),
                        KeyCode::Char(c) => {
                            let mut s = [0u8; 4];
                            let n = c.encode_utf8(&mut s).len();

                            // Ctrl+A..Ctrl+Z -> 0x01..0x1A
                            if modifiers.contains(KeyModifiers::CONTROL) && c.is_ascii_alphabetic()
                            {
                                let ctrl = (c.to_ascii_lowercase() as u8) - b'a' + 1;
                                write_all(tab.pty_in_write, &[ctrl]);
                            } else {
                                write_all(tab.pty_in_write, &s[..n]);
                            }
                        }
                        KeyCode::Left => write_all(tab.pty_in_write, b"\x1b[D"),
                        KeyCode::Right => write_all(tab.pty_in_write, b"\x1b[C"),
                        KeyCode::Up => write_all(tab.pty_in_write, b"\x1b[A"),
                        KeyCode::Down => write_all(tab.pty_in_write, b"\x1b[B"),
                        _ => {}
                    }
                }
                Event::Resize(new_cols, new_rows) => {
                    let _ = tab.resize(new_cols as i16, new_rows as i16);
                }
                _ => {}
            }
        }
    }

    // 5) Cleanup: disable raw mode and wait for reader to finish
    terminal::disable_raw_mode().unwrap();
    let _ = reader.join();

    Ok(())
}
