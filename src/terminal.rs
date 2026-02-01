// src/terminal.rs


pub struct VirtualTerminal {
    cols: u16,
    rows: u16,
    /// Very simple: each entry is a logical line of text.
    lines: Vec<String>,
}

impl VirtualTerminal {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            lines: Vec::new(),
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        // For now, just keep the existing lines, but trim to new visible height.
        let max_visible = rows.saturating_sub(1) as usize; // leave room for status bar in renderer
        if self.lines.len() > max_visible {
            let drop = self.lines.len() - max_visible;
            self.lines.drain(0..drop);
        }
    }

    /// Feed raw bytes from ConPTY into our model.
    /// For now:
    ///   - treat everything as UTF-8 text,
    ///   - split on '\n',
    ///   - keep only the most recent `rows - 1` lines.
    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        let s = String::from_utf8_lossy(bytes);
        for chunk in s.split('\n') {
            // Strip trailing '\r'
            let chunk = chunk.trim_end_matches('\r');

            if self.lines.is_empty() {
                self.lines.push(String::new());
            }

            // Append to the current last line.
            if let Some(last) = self.lines.last_mut() {
                last.push_str(chunk);
            }

            // Every '\n' starts a new line; split() discards it so we
            // simulate that by pushing a new line after each chunk.
            self.lines.push(String::new());
        }

        // Cap visible lines to `rows - 1`
        let max_visible = self.rows.saturating_sub(1) as usize;
        if self.lines.len() > max_visible && max_visible > 0 {
            let drop = self.lines.len() - max_visible;
            self.lines.drain(0..drop);
        }
    }

    /// Lines that should be displayed (top to bottom).
    pub fn visible_lines(&self) -> &[String] {
        &self.lines
    }
}
