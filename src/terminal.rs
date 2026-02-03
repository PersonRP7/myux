// src/terminal.rs

use vt100::Parser;

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

        // If we're at the live view, we keep following the bottom.
        if self.is_at_bottom() {
            self.reset_scrollback();
        }

        self.parser.process(bytes);
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

    /// Render the current screen contents (no status bar) as plain text lines.
    /// This already respects the current scrollback offset.
    pub fn render_lines(&self) -> Vec<String> {
        let screen = self.parser.screen();
        let rows = self.term_rows as u16;
        let cols = self.cols as u16;

        let mut out = Vec::with_capacity(self.term_rows as usize);

        for row in 0..rows {
            let mut line = String::new();

            for col in 0..cols {
                if let Some(cell) = screen.cell(row, col) {
                    let ch = cell.contents();
                    // vt100 uses "\0" for empty cells.
                    if ch != "\0" {
                        line.push_str(&ch);
                    } else {
                        line.push(' ');
                    }
                } else {
                    line.push(' ');
                }
            }

            // Trim trailing spaces for aesthetics.
            while line.ends_with(' ') {
                line.pop();
            }

            out.push(line);
        }

        out
    }
}
