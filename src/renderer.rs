// src/renderer.rs
use crate::terminal::VirtualTerminal;
use crossterm::{
    cursor,
    queue,
    style::{Color, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{Clear, ClearType},
};
use std::io::{self, Write};

pub struct Renderer;

impl Renderer {
    pub fn new() -> Self {
        Renderer
    }

    /// Redraw the entire screen from the VT model plus a status bar.
    pub fn draw(
        &mut self,
        term: &VirtualTerminal,
        status_line: &str,
    ) -> io::Result<()> {
        let (cols, rows) = term.size();
        let cols = cols as usize;
        let rows_u16 = rows;

        let mut stdout = io::stdout();

        // Simple full redraw for now.
        queue!(stdout, cursor::Hide, cursor::MoveTo(0, 0), Clear(ClearType::All))?;

        let visible_lines = term.visible_lines();

        let usable_height = rows_u16.saturating_sub(1); // last line reserved for status
        let usable_height_usize = usable_height as usize;

        // If we have fewer lines than height, show them at the bottom.
        let start = if visible_lines.len() > usable_height_usize {
            visible_lines.len() - usable_height_usize
        } else {
            0
        };

        // Pad top with empty lines if necessary.
        let top_padding = usable_height_usize.saturating_sub(visible_lines.len());
        for row in 0..usable_height_usize {
            queue!(stdout, cursor::MoveTo(0, row as u16))?;

            let text = if row < top_padding {
                ""
            } else {
                let idx = start + row - top_padding;
                if idx < visible_lines.len() {
                    &visible_lines[idx]
                } else {
                    ""
                }
            };

            let mut line = text.to_string();
            if line.len() < cols {
                line.push_str(&" ".repeat(cols - line.len()));
            } else {
                line.truncate(cols);
            }

            write!(stdout, "{}", line)?;
        }

        // Status bar on the last line.
        let last_row = rows_u16.saturating_sub(1);
        let mut status = status_line.to_string();
        if status.len() < cols {
            status.push_str(&" ".repeat(cols - status.len()));
        } else {
            status.truncate(cols);
        }

        queue!(
            stdout,
            cursor::MoveTo(0, last_row),
            SetBackgroundColor(Color::DarkGrey),
            SetForegroundColor(Color::White),
            Clear(ClearType::CurrentLine),
        )?;
        write!(stdout, "{}", status)?;
        queue!(stdout, ResetColor)?;

        stdout.flush()?;
        Ok(())
    }
}
