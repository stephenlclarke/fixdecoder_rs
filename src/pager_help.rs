// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2025 Steve Clarke <stephenlclarke@mac.com> - https://xyzzy.tools

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::queue;
use crossterm::style::Print;
use crossterm::terminal::{
    self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use std::io::{self, IsTerminal, Write};

const HELP_LINES: &[&str] = &[
    "fixdecoder pager help",
    "",
    "Navigation",
    "  Up/Down or k/j       move one line",
    "  Page Up/b            move back one page",
    "  Page Down/Space      move forward one page",
    "  Home/g               go to the first line",
    "  End/G                go to the last line",
    "  Left/Right           scroll horizontally with --nowrap",
    "",
    "Search and commands",
    "  /text                 search forward",
    "  n/N                   next/previous search result",
    "  ?                     show this help",
    "  q                     quit the pager",
    "",
    "Press any key to return",
];

struct TerminalSession;

impl TerminalSession {
    fn enter(stdout: &mut io::Stdout) -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(err) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(err);
        }
        Ok(Self)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
    }
}

pub fn display() -> io::Result<()> {
    let mut stdout = io::stdout();
    if !io::stdin().is_terminal() || !stdout.is_terminal() {
        writeln!(stdout, "{}", HELP_LINES.join("\n"))?;
        return Ok(());
    }

    let _session = TerminalSession::enter(&mut stdout)?;
    display_overlay()
}

fn display_overlay() -> io::Result<()> {
    let mut stdout = io::stdout();
    render(&mut stdout)?;
    loop {
        match event::read()? {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                return Ok(());
            }
            Event::Resize(_, _) => render(&mut stdout)?,
            _ => {}
        }
    }
}

fn render(stdout: &mut io::Stdout) -> io::Result<()> {
    let (width, height) = terminal::size()?;
    let width = width as usize;
    let height = height as usize;

    queue!(stdout, MoveTo(0, 0), Clear(ClearType::All))?;
    if height == 0 {
        return stdout.flush();
    }

    let (prompt, body) = HELP_LINES
        .split_last()
        .expect("pager help must include a prompt");
    for (row, line) in body.iter().take(height.saturating_sub(1)).enumerate() {
        let clipped: String = line.chars().take(width).collect();
        queue!(stdout, MoveTo(0, row as u16), Print(clipped))?;
    }
    let clipped_prompt: String = prompt.chars().take(width).collect();
    queue!(
        stdout,
        MoveTo(0, height.saturating_sub(1) as u16),
        Print(clipped_prompt)
    )?;
    stdout.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_lists_trigger_and_return_behaviour() {
        assert!(HELP_LINES.iter().any(|line| line.contains('?')));
        assert!(
            HELP_LINES
                .iter()
                .any(|line| line.contains("Press any key to return"))
        );
    }
}
