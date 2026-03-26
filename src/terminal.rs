// src/terminal.rs

use vt100::Parser;

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

static VT_LOG_FILE: OnceLock<Mutex<std::fs::File>> = OnceLock::new();

fn get_vt_log_file() -> &'static Mutex<std::fs::File> {
    VT_LOG_FILE.get_or_init(|| {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open("vt_debug.log")
            .expect("failed to open vt_debug.log");
        Mutex::new(file)
    })
}

fn log_vt_debug(text: &str) {
    if let Ok(mut file) = get_vt_log_file().lock() {
        let _ = file.write_all(text.as_bytes());
    }
}

const SCROLLBACK_LEN: usize = 2000; // number of lines of history

/// A virtual terminal backed by vt100.
/// - `rows` / `cols` are the *physical* console size.
/// - We reserve the last physical row for the status bar.
/// - The vt100 screen height is therefore `rows - 1`.
pub struct VirtualTerminal {
    parser: Parser,
    cols: u16,
    rows: u16,      // physical rows (incl. status bar)
    term_rows: u16, // rows dedicated to the child terminal (rows - 1)
}

//logger
fn contains_csi_ech(bytes: &[u8]) -> bool {
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == 0x1B && bytes[i + 1] == b'[' {
            let mut j = i + 2;

            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }

            if j > i + 2 && j < bytes.len() && bytes[j] == b'X' {
                return true;
            }
        }
        i += 1;
    }
    false
}

impl VirtualTerminal {
    pub fn new(cols: u16, rows: u16) -> Self {
        // At least 1 row for the child.
        let term_rows = rows.saturating_sub(1).max(1);

        // vt100 takes: height, width, scrollback_len.
        let parser = Parser::new(term_rows as u16, cols as u16, SCROLLBACK_LEN);

        Self {
            parser,
            cols,
            rows,
            term_rows,
        }
    }

    pub fn cursor_pos(&self) -> (u16, u16) {
        // vt100 uses (row, col)
        self.parser.screen().cursor_position()
    }

    /// Physical console size (what the renderer cares about).
    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Called when the host console is resized.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;

        let term_rows = rows.saturating_sub(1).max(1);
        self.term_rows = term_rows;

        // Resize the vt100 screen.
        self.parser
            .screen_mut()
            .set_size(term_rows as u16, cols as u16);
    }

    /// Feed raw bytes from ConPTY into the VT parser.
    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        if self.is_at_bottom() {
            self.reset_scrollback();
        }

        let watch = contains_csi_ech(bytes);

        let before = if watch {
            Some(self.parser.screen().contents().to_string())
        } else {
            None
        };

        self.parser.process(bytes);

        if watch {
            let after = self.parser.screen().contents().to_string();
            if let Some(before) = before {
                let (row, col) = self.parser.screen().cursor_position();

                let msg = format!(
                    "=== ECH CHUNK ===\nBYTES: {:?}\nBEFORE:\n{:?}\nAFTER:\n{:?}\nCURSOR: row={} col={}\n\n",
                    bytes, before, after, row, col
                );

                log_vt_debug(&msg);
            }
        }
    }

    // ---------- Scrollback control ----------

    /// Current scrollback offset (0 = bottom/live).
    fn current_scrollback(&self) -> usize {
        self.parser.screen().scrollback()
    }

    /// Scroll "up" into history by the given number of rows
    /// (toward older content).
    pub fn scroll_up(&mut self, lines: u16) {
        let cur = self.current_scrollback();
        let new = cur.saturating_add(lines as usize);
        self.parser.screen_mut().set_scrollback(new);
    }

    /// Scroll "down" toward the live view.
    pub fn scroll_down(&mut self, lines: u16) {
        let cur = self.current_scrollback();
        let new = cur.saturating_sub(lines as usize);
        self.parser.screen_mut().set_scrollback(new);
    }

    /// Jump back to the live view (bottom).
    pub fn reset_scrollback(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    /// Are we currently looking at the live view?
    pub fn is_at_bottom(&self) -> bool {
        self.current_scrollback() == 0
    }

    // ---------- Rendering ----------
    pub fn render_lines(&self) -> Vec<String> {
        self.parser.screen().contents()
            .split('\n')
            .map(|s| s.to_string())
            .collect()
    }
}
