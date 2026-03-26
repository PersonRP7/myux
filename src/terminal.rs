// src/terminal.rs

use vt100::{Parser, Screen};

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

const SCROLLBACK_LEN: usize = 2000;

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

pub struct VirtualTerminal {
    parser: Parser,
    cols: u16,
    rows: u16,      // physical rows incl. status bar
    term_rows: u16, // child terminal rows only
}

impl VirtualTerminal {
    pub fn new(cols: u16, rows: u16) -> Self {
        let term_rows = rows.saturating_sub(1).max(1);
        let parser = Parser::new(term_rows, cols, SCROLLBACK_LEN);

        Self {
            parser,
            cols,
            rows,
            term_rows,
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;

        let term_rows = rows.saturating_sub(1).max(1);
        self.term_rows = term_rows;

        self.parser.screen_mut().set_size(term_rows, cols);
    }

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

    fn current_scrollback(&self) -> usize {
        self.parser.screen().scrollback()
    }

    pub fn scroll_up(&mut self, lines: u16) {
        let cur = self.current_scrollback();
        let new = cur.saturating_add(lines as usize);
        self.parser.screen_mut().set_scrollback(new);
    }

    pub fn scroll_down(&mut self, lines: u16) {
        let cur = self.current_scrollback();
        let new = cur.saturating_sub(lines as usize);
        self.parser.screen_mut().set_scrollback(new);
    }

    pub fn reset_scrollback(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }

    pub fn is_at_bottom(&self) -> bool {
        self.current_scrollback() == 0
    }

    // ---- new rendering API ----

    pub fn snapshot(&self) -> Screen {
        self.parser.screen().clone()
    }

    pub fn full_render_bytes(&self) -> Vec<u8> {
        self.parser.screen().contents_formatted().to_vec()
    }

    pub fn diff_render_bytes(&self, previous: &Screen) -> Vec<u8> {
        self.parser.screen().contents_diff(previous).to_vec()
    }
}