// src/renderer.rs

use crate::terminal::VirtualTerminal;
use crossterm::{
    cursor,
    queue,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{Clear, ClearType},
};
use std::io::{self, Write};
use vt100::Screen;

pub struct Renderer {
    last_screen: Option<Screen>,
}

impl Renderer {
    pub fn new() -> Self {
        Self { last_screen: None }
    }

    pub fn draw(&mut self, term: &VirtualTerminal, status_line: &str) -> io::Result<()> {
        let (cols, rows) = term.size();
        let cols = cols as usize;
        let status_row = rows.saturating_sub(1);

        let mut stdout = io::stdout();

//         let vt_bytes = match &self.last_screen {
//             Some(prev) => term.diff_render_bytes(prev),
//             None => term.full_render_bytes(),
//         };
        // Let vt100 produce the terminal redraw bytes.
        // repaints the whole visible terminal state again on
        // every dirty frame, seems to solve the bug when
        // pressing backspace on autocompleted text behaves
        // so that one backspace key press doesn't correlate
        // directly to one deleted character but instead deletes
        // the whole line when it reaches some arbitrary limit,
        // however causes visible churn (jitters).
        let vt_bytes = term.full_render_bytes();

        stdout.write_all(&vt_bytes)?;

        // Draw status bar without disturbing child cursor position.
        let mut status = status_line.to_string();
        if status.len() < cols {
            status.push_str(&" ".repeat(cols - status.len()));
        } else {
            status.truncate(cols);
        }

        queue!(
            stdout,
            cursor::SavePosition,
            cursor::MoveTo(0, status_row),
            SetBackgroundColor(Color::DarkGrey),
            SetForegroundColor(Color::White),
            Clear(ClearType::CurrentLine),
            Print(status),
            ResetColor,
            cursor::RestorePosition,
        )?;

        stdout.flush()?;

        self.last_screen = Some(term.snapshot());
        Ok(())
    }

    pub fn invalidate(&mut self) {
        self.last_screen = None;
    }
}