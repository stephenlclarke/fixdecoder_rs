// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2025 Steve Clarke <stephenlclarke@mac.com> - https://xyzzy.tools

use crate::decoder::colours::palette;
use crate::decoder::display::{pad_ansi, rendered_char_width, visible_width};
use crate::decoder::prettifier::{MsgTypeCount, render_message_counts};
use crate::decoder::summary::{SummaryPagerMessageCount, SummaryPagerSection};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::queue;
use crossterm::style::Print;
use crossterm::terminal::{
    self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use std::collections::HashMap;
use std::io::{self, Write};

const MIN_LEFT_WIDTH: usize = 28;
const MIN_RIGHT_WIDTH: usize = 48;
const DIVIDER_WIDTH: usize = 3;

pub struct SummaryPagerContent {
    pub sections: Vec<SummaryPagerSection>,
    pub message_counts: Vec<SummaryPagerMessageCount>,
}

struct SectionView {
    summary_lines: Vec<String>,
    overview_lines: Vec<String>,
    message_count_lines: Vec<String>,
    detail_start: usize,
}

struct PagerModel {
    empty_overview_lines: Vec<String>,
    fallback_message_count_lines: Vec<String>,
    sections: Vec<SectionView>,
    right_lines: Vec<String>,
    left_width_hint: usize,
    max_right_width: usize,
}

struct PagerState {
    scroll_x: usize,
    scroll_y: usize,
}

struct TerminalSession;

impl TerminalSession {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        Ok(Self)
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
    }
}

pub fn run(content: SummaryPagerContent) -> io::Result<()> {
    let model = PagerModel::from_content(content)?;
    let mut state = PagerState {
        scroll_x: 0,
        scroll_y: 0,
    };
    let _session = TerminalSession::enter()?;
    let mut stdout = io::stdout();

    loop {
        render(&mut stdout, &model, &mut state)?;
        match event::read()? {
            Event::Key(key) if is_press(key) => {
                if handle_key(&model, &mut state, key)? {
                    break;
                }
            }
            Event::Resize(_, _) => continue,
            _ => {}
        }
    }

    Ok(())
}

fn is_press(key: KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

fn handle_key(model: &PagerModel, state: &mut PagerState, key: KeyEvent) -> io::Result<bool> {
    let (_, height) = terminal::size()?;
    let viewport_height = height as usize;
    let max_y = model.max_vertical_scroll(viewport_height);

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Up | KeyCode::Char('k') => {
            state.scroll_y = state.scroll_y.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.scroll_y = state.scroll_y.saturating_add(1).min(max_y);
        }
        KeyCode::Left | KeyCode::Char('h') => {
            state.scroll_x = state.scroll_x.saturating_sub(1);
        }
        KeyCode::Right | KeyCode::Char('l') => {
            state.scroll_x = state.scroll_x.saturating_add(1);
        }
        KeyCode::PageUp => {
            state.scroll_y = state
                .scroll_y
                .saturating_sub(viewport_height.saturating_sub(1));
        }
        KeyCode::PageDown | KeyCode::Char(' ') => {
            state.scroll_y = state
                .scroll_y
                .saturating_add(viewport_height.saturating_sub(1))
                .min(max_y);
        }
        KeyCode::Home => {
            state.scroll_x = 0;
            state.scroll_y = 0;
        }
        KeyCode::End => {
            state.scroll_y = max_y;
        }
        _ => {}
    }

    Ok(false)
}

fn render(stdout: &mut io::Stdout, model: &PagerModel, state: &mut PagerState) -> io::Result<()> {
    let (width, height) = terminal::size()?;
    let width = width as usize;
    let height = height as usize;
    let colours = palette();

    queue!(stdout, MoveTo(0, 0), Clear(ClearType::All))?;

    if width <= DIVIDER_WIDTH + 10 || height == 0 {
        queue!(
            stdout,
            MoveTo(0, 0),
            Print(format!(
                "{}Terminal too small for split summary pager. Resize or use --paging=never.{}",
                colours.error, colours.reset
            ))
        )?;
        stdout.flush()?;
        return Ok(());
    }

    let left_width = model.left_width(width);
    let right_width = width.saturating_sub(left_width + DIVIDER_WIDTH);
    if left_width < 8 || right_width < 16 {
        queue!(
            stdout,
            MoveTo(0, 0),
            Print(format!(
                "{}Terminal too small for split summary pager. Resize or use --paging=never.{}",
                colours.error, colours.reset
            ))
        )?;
        stdout.flush()?;
        return Ok(());
    }

    let max_x = model.max_horizontal_scroll(right_width);
    let max_y = model.max_vertical_scroll(height);
    state.scroll_x = state.scroll_x.min(max_x);
    state.scroll_y = state.scroll_y.min(max_y);

    let current_section = model.section_index_for_line(state.scroll_y);
    let visible_section = model.visible_section_index(state.scroll_y, height);
    let left_lines = model.left_lines(current_section, visible_section);
    let divider = format!("{} | {}", colours.line, colours.reset);

    for row in 0..height {
        let left = left_lines
            .get(row)
            .map(|line| pad_ansi(&slice_ansi(line, 0, left_width), left_width))
            .unwrap_or_else(|| " ".repeat(left_width));
        let right = model
            .right_lines
            .get(state.scroll_y + row)
            .map(|line| slice_ansi(line, state.scroll_x, right_width))
            .unwrap_or_default();
        queue!(
            stdout,
            MoveTo(0, row as u16),
            Clear(ClearType::CurrentLine),
            Print(left),
            Print(&divider),
            Print(right)
        )?;
    }

    stdout.flush()
}

impl PagerModel {
    fn from_content(content: SummaryPagerContent) -> io::Result<Self> {
        let mut right_lines = Vec::new();
        let mut sections = Vec::new();
        let mut left_width_hint = 0usize;
        let section_count = content.sections.len();
        let mut terminal_seen = 0usize;

        let empty_overview_lines = render_overview_lines(0, 0);
        update_left_width_hint(&mut left_width_hint, &empty_overview_lines);
        let fallback_message_count_lines = render_message_count_lines(&content.message_counts)?;
        update_left_width_hint(&mut left_width_hint, &fallback_message_count_lines);

        for (index, section) in content.sections.into_iter().enumerate() {
            let summary_lines = split_lines(&section.summary);
            update_left_width_hint(&mut left_width_hint, &summary_lines);

            let detail_start = right_lines.len();
            right_lines.extend(split_lines(&section.detail));
            if index + 1 < section_count {
                right_lines.push(String::new());
            }

            if section.terminal {
                terminal_seen += 1;
            }
            let total_seen = index + 1;
            let open_seen = total_seen.saturating_sub(terminal_seen);
            let overview_lines = render_overview_lines(total_seen, open_seen);
            update_left_width_hint(&mut left_width_hint, &overview_lines);
            let message_count_lines = render_message_count_lines(&section.message_counts)?;
            update_left_width_hint(&mut left_width_hint, &message_count_lines);

            sections.push(SectionView {
                summary_lines,
                overview_lines,
                message_count_lines,
                detail_start,
            });
        }

        if right_lines.is_empty() {
            right_lines.push(format!(
                "{}No order-flow messages captured.{}",
                palette().name,
                palette().reset
            ));
        }

        let max_right_width = right_lines
            .iter()
            .map(|line| visible_width(line))
            .max()
            .unwrap_or(0);

        Ok(Self {
            empty_overview_lines,
            fallback_message_count_lines,
            sections,
            right_lines,
            left_width_hint,
            max_right_width,
        })
    }

    fn left_width(&self, terminal_width: usize) -> usize {
        let available = terminal_width.saturating_sub(DIVIDER_WIDTH);
        let cap = (terminal_width / 3).max(MIN_LEFT_WIDTH);
        let mut width = self
            .left_width_hint
            .max(MIN_LEFT_WIDTH)
            .min(cap)
            .min(available);
        if available.saturating_sub(width) < MIN_RIGHT_WIDTH && available > MIN_RIGHT_WIDTH {
            width = available.saturating_sub(MIN_RIGHT_WIDTH);
        }
        width
            .max(
                available
                    .saturating_sub(self.max_right_width)
                    .min(available),
            )
            .max(1)
    }

    fn max_horizontal_scroll(&self, right_width: usize) -> usize {
        self.max_right_width.saturating_sub(right_width)
    }

    fn max_vertical_scroll(&self, viewport_height: usize) -> usize {
        self.right_lines.len().saturating_sub(viewport_height)
    }

    fn section_index_for_line(&self, line: usize) -> Option<usize> {
        if self.sections.is_empty() {
            return None;
        }
        let mut current = 0usize;
        for (index, section) in self.sections.iter().enumerate() {
            if section.detail_start <= line {
                current = index;
            } else {
                break;
            }
        }
        Some(current)
    }

    fn visible_section_index(&self, scroll_y: usize, viewport_height: usize) -> Option<usize> {
        if self.right_lines.is_empty() {
            return None;
        }
        let bottom = scroll_y
            .saturating_add(viewport_height.saturating_sub(1))
            .min(self.right_lines.len().saturating_sub(1));
        self.section_index_for_line(bottom)
    }

    fn left_lines(
        &self,
        current_section: Option<usize>,
        visible_section: Option<usize>,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(index) = current_section {
            lines.extend(self.sections[index].summary_lines.iter().cloned());
        }

        let overview_lines = visible_section
            .and_then(|index| self.sections.get(index))
            .map(|section| section.overview_lines.as_slice())
            .unwrap_or(self.empty_overview_lines.as_slice());
        if !overview_lines.is_empty() {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.extend(overview_lines.iter().cloned());
        }

        let message_count_lines = visible_section
            .and_then(|index| self.sections.get(index))
            .map(|section| section.message_count_lines.as_slice())
            .filter(|lines| !lines.is_empty())
            .unwrap_or(self.fallback_message_count_lines.as_slice());

        if !message_count_lines.is_empty() {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.extend(message_count_lines.iter().cloned());
        }

        lines
    }
}

fn update_left_width_hint(left_width_hint: &mut usize, lines: &[String]) {
    for line in lines {
        *left_width_hint = (*left_width_hint).max(visible_width(line));
    }
}

fn render_overview_lines(total_orders: usize, open_orders: usize) -> Vec<String> {
    let colours = palette();
    vec![format!(
        "{}Order Summary{} ({} open, {} total, to fill: {}/{})",
        colours.title, colours.reset, open_orders, total_orders, open_orders, total_orders
    )]
}

fn render_message_count_lines(
    message_counts: &[SummaryPagerMessageCount],
) -> io::Result<Vec<String>> {
    if message_counts.is_empty() {
        return Ok(Vec::new());
    }
    let mut grouped = HashMap::new();
    for count in message_counts {
        grouped.insert(
            count.msg_type.clone(),
            MsgTypeCount {
                count: count.count,
                label: count.label.clone(),
                bucket: count.bucket,
            },
        );
    }
    let mut rendered = Vec::new();
    render_message_counts(&mut rendered, &grouped)?;
    Ok(split_lines(
        &String::from_utf8(rendered).unwrap_or_default(),
    ))
}

fn split_lines(text: &str) -> Vec<String> {
    text.lines().map(|line| line.to_string()).collect()
}

fn slice_ansi(text: &str, offset: usize, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let mut output = String::new();
    let mut active_style = String::new();
    let mut saw_style = false;
    let mut visible = 0usize;
    let bytes = text.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index] == 0x1b {
            let start = index;
            index += 1;
            while index < bytes.len() && bytes[index] != b'm' {
                index += 1;
            }
            if index < bytes.len() {
                index += 1;
            }
            let sequence = &text[start..index];
            saw_style = true;
            if sequence == "\u{001b}[0m" {
                active_style.clear();
            } else {
                active_style = sequence.to_string();
            }
            if visible >= offset && visible < offset + width {
                output.push_str(sequence);
            }
            continue;
        }

        let ch = text[index..].chars().next().unwrap_or_default();
        if visible >= offset && visible < offset + width {
            if output.is_empty() && !active_style.is_empty() {
                output.push_str(&active_style);
            }
            output.push(ch);
        }
        visible += rendered_char_width(ch);
        index += ch.len_utf8();
        if visible >= offset + width {
            break;
        }
    }

    if !output.is_empty() && saw_style && !output.ends_with("\u{001b}[0m") {
        output.push_str("\u{001b}[0m");
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::message_groups::classify_message_bucket;

    fn section(
        summary: &str,
        detail: &str,
        terminal: bool,
        counts: &[(&str, &str, usize)],
    ) -> SummaryPagerSection {
        SummaryPagerSection {
            summary: summary.into(),
            detail: detail.into(),
            terminal,
            message_counts: counts
                .iter()
                .map(|(msg_type, label, count)| SummaryPagerMessageCount {
                    msg_type: (*msg_type).into(),
                    label: Some((*label).into()),
                    count: *count,
                    bucket: classify_message_bucket(msg_type, Some(label), None),
                })
                .collect(),
        }
    }

    fn count(msg_type: &str, label: &str, count: usize, msg_cat: &str) -> SummaryPagerMessageCount {
        SummaryPagerMessageCount {
            msg_type: msg_type.into(),
            label: Some(label.into()),
            count,
            bucket: classify_message_bucket(msg_type, Some(label), Some(msg_cat)),
        }
    }

    #[test]
    fn slice_ansi_respects_visible_offsets() {
        let text = "\u{001b}[31mAB\u{001b}[0mCD";
        let slice = slice_ansi(text, 1, 2);
        assert_eq!(visible_width(&slice), 2);
        assert!(slice.contains('B'));
        assert!(slice.contains('C'));
    }

    #[test]
    fn slice_ansi_offsets_ignore_control_characters() {
        let text = "8=FIX.4.4\u{0001}9=169\u{0001}35=D";
        let slice = slice_ansi(text, 9, 5);
        let visible: String = slice.chars().filter(|ch| !ch.is_control()).collect();
        assert_eq!(visible, "9=169");
        assert_eq!(visible_width(&slice), 5);
    }

    #[test]
    fn section_index_tracks_scroll_position() {
        let model = PagerModel::from_content(SummaryPagerContent {
            message_counts: vec![
                count("A", "LOGON", 1, "admin"),
                count("D", "New Order Single", 1, "app"),
                count("8", "Execution Report", 2, "app"),
            ],
            sections: vec![
                section(
                    "one",
                    "d1\nd2",
                    false,
                    &[("A", "LOGON", 1), ("D", "New Order Single", 1)],
                ),
                section(
                    "two",
                    "d3\nd4",
                    true,
                    &[
                        ("A", "LOGON", 1),
                        ("D", "New Order Single", 1),
                        ("8", "Execution Report", 2),
                    ],
                ),
            ],
        })
        .expect("build pager model");

        assert_eq!(model.section_index_for_line(0), Some(0));
        assert_eq!(model.section_index_for_line(2), Some(0));
        assert_eq!(model.section_index_for_line(3), Some(1));
    }

    #[test]
    fn cumulative_summary_tracks_bottom_of_visible_pane() {
        let model = PagerModel::from_content(SummaryPagerContent {
            message_counts: vec![
                count("A", "LOGON", 1, "admin"),
                count("D", "New Order Single", 1, "app"),
                count("8", "Execution Report", 2, "app"),
            ],
            sections: vec![
                section(
                    "first summary",
                    "first detail",
                    false,
                    &[("A", "LOGON", 1), ("D", "New Order Single", 1)],
                ),
                section(
                    "second summary",
                    "second detail",
                    true,
                    &[
                        ("A", "LOGON", 1),
                        ("D", "New Order Single", 1),
                        ("8", "Execution Report", 2),
                    ],
                ),
            ],
        })
        .expect("build pager model");

        let left = model.left_lines(Some(0), model.visible_section_index(0, 3));
        assert!(left.iter().any(|line| line.contains("first summary")));
        assert!(left.iter().any(|line| line.contains("1 open, 2 total")));
        assert!(left.iter().any(|line| line.contains("Session/Admin")));
        assert!(left.iter().any(|line| line.contains("LOGON")));
        assert!(left.iter().any(|line| line.contains("D")));
        assert!(left.iter().any(|line| line.contains("8")));
    }
}
