// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2025 Steve Clarke <stephenlclarke@mac.com> - https://xyzzy.tools

use crate::decoder::colours::{disable_colours, palette};
use crate::decoder::display::{indent, pad_ansi, terminal_width, visible_width};
use crate::decoder::fixparser::{FieldValue, parse_fix};
use crate::decoder::layout::{BASE_INDENT, ENTRY_FIELD_INDENT, NAME_TEXT_OFFSET};
use crate::decoder::summary::OrderSummary;
#[cfg(test)]
use crate::decoder::tag_lookup::MessageDef;
use crate::decoder::tag_lookup::{
    FixTagLookup, GroupSpec as MessageDefGroupSpec, MessageDef as LookupMessageDef,
    default_appl_ver_key, load_dictionary_with_session_default,
};
use crate::decoder::validator;
use crate::fix;
use once_cell::sync::Lazy;
use regex::Regex;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Shared context for prettification to keep function signatures concise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputStyle {
    pub show_numbers: bool,
    pub show_header: bool,
    pub show_grid: bool,
}

impl OutputStyle {
    pub const fn plain() -> Self {
        Self {
            show_numbers: false,
            show_header: false,
            show_grid: false,
        }
    }

    pub const fn full() -> Self {
        Self {
            show_numbers: true,
            show_header: true,
            show_grid: true,
        }
    }

    pub fn default_for_terminal(is_terminal: bool) -> Self {
        if is_terminal {
            Self {
                show_numbers: false,
                show_header: true,
                show_grid: true,
            }
        } else {
            Self::plain()
        }
    }
}

pub struct PrettifyContext<'a> {
    pub out: &'a mut dyn Write,
    pub err_out: &'a mut dyn Write,
    pub obfuscator: &'a fix::Obfuscator,
    pub display_delimiter: char,
    pub style: OutputStyle,
    pub wide_grid: bool,
    pub summary: &'a mut Option<OrderSummary>,
    pub fix_override: Option<&'a str>,
    pub follow: bool,
    pub live_status_enabled: bool,
    pub validation_enabled: bool,
    pub message_counts: HashMap<String, MsgTypeCount>,
    pub fixt_session_defaults: HashMap<FixtSessionKey, String>,
    pub counts_dirty: bool,
    pub interrupted: &'static AtomicBool,
}

#[derive(Default, Clone)]
pub struct MsgTypeCount {
    pub count: usize,
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FixtSessionParty {
    comp_id: String,
    sub_id: Option<String>,
    location_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FixtSessionKey {
    first: FixtSessionParty,
    second: FixtSessionParty,
}

impl FixtSessionParty {
    fn from_message(msg: &str, comp_tag: &str, sub_tag: &str, location_tag: &str) -> Option<Self> {
        Some(Self {
            comp_id: extract_tag_value(msg, comp_tag)?.to_string(),
            sub_id: extract_tag_value(msg, sub_tag).map(str::to_string),
            location_id: extract_tag_value(msg, location_tag).map(str::to_string),
        })
    }
}

static FIX_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"8=FIX.*?10=\d{3}\u{0001}").expect("valid regex"));

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
const FOLLOW_SLEEP: Duration = Duration::from_millis(250);

/// Shared interruption flag set by the SIGINT handler to allow graceful shutdowns.
pub fn interrupt_flag() -> &'static AtomicBool {
    &INTERRUPTED
}

/// Render a single FIX message into a human-friendly string using the provided dictionary.
/// When a validation report is supplied, tag-level errors are annotated inline and missing
/// required fields are surfaced in the output.
pub fn prettify_with_report(
    msg: &str,
    dict: &FixTagLookup,
    report: Option<&validator::ValidationReport>,
) -> String {
    let colours = palette();
    let mut output = String::new();
    let fields = parse_fix(msg);
    let annotations = report.map(|r| &r.tag_errors);

    let mut seen_tags = HashSet::new();
    let msg_def = fields
        .iter()
        .find(|f| f.tag == 35)
        .and_then(|f| dict.message_def(&f.value));
    let renderer = msg_def.map(|def| GroupRenderer {
        dict,
        annotations,
        colours: &colours,
        msg_def: def,
        fields: &fields,
    });

    let mut idx = 0;
    while idx < fields.len() {
        let field = &fields[idx];
        seen_tags.insert(field.tag);
        if let Some(render) = renderer.as_ref()
            && let Some(spec) = render.msg_def.groups.get(&field.tag)
        {
            let consumed = render.render_group(&mut output, idx, spec, BASE_INDENT);
            idx += consumed.max(1);
        } else {
            write_field_line(&mut output, dict, field, annotations, &colours, BASE_INDENT);
            idx += 1;
        }
    }

    if let Some(ann) = annotations {
        for (tag, errs) in ann {
            if seen_tags.contains(tag) || errs.is_empty() {
                continue;
            }
            write_missing_line(&mut output, dict, *tag, errs, &colours);
        }
    }

    output
}

struct GroupRenderer<'a> {
    dict: &'a FixTagLookup,
    annotations: Option<&'a std::collections::HashMap<u32, Vec<String>>>,
    colours: &'a crate::decoder::colours::ColourPalette,
    msg_def: &'a LookupMessageDef,
    fields: &'a [FieldValue],
}

impl<'a> GroupRenderer<'a> {
    fn write_field(&self, output: &mut String, field: &FieldValue, indent_spaces: usize) {
        write_field_line(
            output,
            self.dict,
            field,
            self.annotations,
            self.colours,
            indent_spaces,
        );
    }

    fn render_group(
        &self,
        output: &mut String,
        start_idx: usize,
        spec: &MessageDefGroupSpec,
        indent_spaces: usize,
    ) -> usize {
        let mut consumed = 0usize;
        let mut entries = 0usize;
        let expected = self.fields[start_idx]
            .value
            .parse::<usize>()
            .unwrap_or_default();
        self.write_field(output, &self.fields[start_idx], indent_spaces);
        let mut idx = start_idx + 1;
        while idx < self.fields.len() && entries < expected {
            if self.fields[idx].tag != spec.delim {
                if self.msg_def.group_membership.get(&self.fields[idx].tag) == Some(&spec.count_tag)
                {
                    if entries == 0 {
                        let entry_consumed =
                            self.render_group_entry(output, idx, spec, indent_spaces, entries + 1);
                        idx += entry_consumed;
                        entries += 1;
                        consumed = idx - start_idx;
                        continue;
                    }
                    self.write_field(
                        output,
                        &self.fields[idx],
                        indent_spaces + ENTRY_FIELD_INDENT,
                    );
                    idx += 1;
                    consumed = idx - start_idx;
                    continue;
                }
                break;
            }
            let entry_consumed =
                self.render_group_entry(output, idx, spec, indent_spaces, entries + 1);
            idx += entry_consumed;
            entries += 1;
            consumed = idx - start_idx;
        }

        if entries != expected {
            if let Some(errs) = self
                .annotations
                .and_then(|ann| ann.get(&spec.count_tag))
                .filter(|errs| !errs.is_empty())
            {
                write_missing_line(output, self.dict, spec.count_tag, errs, self.colours);
            } else {
                output.push_str(&format!(
                    "{}{}Warning:{} NumInGroup {} ({}) declared {}, found {}\n",
                    indent(indent_spaces + 2),
                    self.colours.error,
                    self.colours.reset,
                    spec.count_tag,
                    spec.name,
                    expected,
                    entries
                ));
            }
        }
        consumed
    }

    fn render_group_entry(
        &self,
        output: &mut String,
        start_idx: usize,
        spec: &MessageDefGroupSpec,
        indent_spaces: usize,
        entry_idx: usize,
    ) -> usize {
        let entry_label = format!("Group {}", entry_idx);
        let dash_count = 60usize.saturating_sub(entry_label.len());
        let dashes = "-".repeat(dash_count);
        let dash_start_col = indent_spaces + NAME_TEXT_OFFSET;
        let label_indent = dash_start_col.saturating_sub(entry_label.len());
        output.push_str(&format!(
            "{}{} {}{}{}\n",
            indent(label_indent),
            entry_label,
            self.colours.error,
            dashes,
            self.colours.reset
        ));
        let mut idx = start_idx;
        let mut last_pos = -1isize;
        while idx < self.fields.len() {
            let tag = self.fields[idx].tag;
            if tag == spec.delim && idx != start_idx {
                break;
            }
            if let Some(nested) = spec.nested.get(&tag) {
                let nested_consumed =
                    self.render_group(output, idx, nested, indent_spaces + ENTRY_FIELD_INDENT);
                idx += nested_consumed.max(1);
                continue;
            }
            if let Some(pos) = spec.entry_pos.get(&tag).copied() {
                if (pos as isize) < last_pos
                    && let Some(errs) = self
                        .annotations
                        .and_then(|ann| ann.get(&tag))
                        .filter(|errs| !errs.is_empty())
                {
                    write_missing_line(output, self.dict, tag, errs, self.colours);
                }
                last_pos = pos as isize;
                self.write_field(
                    output,
                    &self.fields[idx],
                    indent_spaces + ENTRY_FIELD_INDENT,
                );
                idx += 1;
            } else {
                break;
            }
        }
        idx - start_idx
    }
}

/// Bucket each field by tag so repeat occurrences can be emitted in order.
#[allow(dead_code)]
fn bucket_fields(
    fields: &[FieldValue],
) -> std::collections::HashMap<u32, std::collections::VecDeque<&FieldValue>> {
    use std::collections::{HashMap, VecDeque};
    let mut buckets: HashMap<u32, VecDeque<&FieldValue>> = HashMap::new();
    for field in fields {
        buckets.entry(field.tag).or_default().push_back(field);
    }
    buckets
}

/// Build the emission order of tags using the message definition when known, falling back
/// to a header-first order when MsgType is absent, and appending tags referenced in
/// validation annotations.
#[allow(dead_code)]
fn build_tag_order(
    fields: &[FieldValue],
    dict: &FixTagLookup,
    annotations: Option<&std::collections::HashMap<u32, Vec<String>>>,
) -> Vec<u32> {
    let trailer_order = trailer_tags(dict);
    let trailer_set: HashSet<u32> = trailer_order.iter().copied().collect();
    let mut trailer_present = collect_trailer_tags(fields, &trailer_set);

    let canonical_header = canonical_header_tags();
    let mut final_order = Vec::new();
    final_order.extend_from_slice(canonical_header);

    let base_order = base_message_order(
        fields,
        dict,
        canonical_header,
        &trailer_set,
        &mut trailer_present,
    );
    final_order.extend(base_order);

    if let Some(ann) = annotations {
        append_annotation_tags(
            &mut final_order,
            ann,
            canonical_header,
            &trailer_set,
            &mut trailer_present,
        );
    }

    append_message_fields(fields, &mut final_order, &trailer_set, &mut trailer_present);
    append_trailer_tags(&mut final_order, &trailer_order, &trailer_present);

    final_order
}

#[allow(dead_code)]
fn canonical_header_tags() -> &'static [u32; 7] {
    &[8u32, 9, 35, 49, 56, 34, 52]
}

#[allow(dead_code)]
fn trailer_tags(dict: &FixTagLookup) -> Vec<u32> {
    let order = dict.trailer_tags();
    if order.is_empty() {
        vec![10u32]
    } else {
        order.to_vec()
    }
}

#[allow(dead_code)]
fn collect_trailer_tags(fields: &[FieldValue], trailer_set: &HashSet<u32>) -> HashSet<u32> {
    fields
        .iter()
        .filter(|f| trailer_set.contains(&f.tag))
        .map(|f| f.tag)
        .collect()
}

fn message_field_order(fields: &[FieldValue], dict: &FixTagLookup) -> Option<Vec<u32>> {
    let msg_type = fields.iter().find(|f| f.tag == 35).map(|f| f.value.clone());
    msg_type
        .as_deref()
        .and_then(|mt| dict.message_def(mt).cloned())
        .map(|def| def.field_order)
}

#[allow(dead_code)]
fn fallback_field_order(fields: &[FieldValue]) -> Vec<u32> {
    let mut base = vec![8, 9, 35];
    for f in fields {
        if !base.contains(&f.tag) {
            base.push(f.tag);
        }
    }
    base
}

#[allow(dead_code)]
fn dedup_order(order: Vec<u32>) -> Vec<u32> {
    let mut seen = HashSet::new();
    order.into_iter().filter(|tag| seen.insert(*tag)).collect()
}

#[allow(dead_code)]
fn base_message_order(
    fields: &[FieldValue],
    dict: &FixTagLookup,
    canonical_header: &[u32],
    trailer_set: &HashSet<u32>,
    trailer_present: &mut HashSet<u32>,
) -> Vec<u32> {
    let order = message_field_order(fields, dict).unwrap_or_else(|| fallback_field_order(fields));
    let mut deduped = dedup_order(order);
    deduped.retain(|tag| {
        if trailer_set.contains(tag) {
            trailer_present.insert(*tag);
            return false;
        }
        !canonical_header.contains(tag)
    });
    deduped
}

#[allow(dead_code)]
fn append_annotation_tags(
    final_order: &mut Vec<u32>,
    annotations: &std::collections::HashMap<u32, Vec<String>>,
    canonical_header: &[u32],
    trailer_set: &HashSet<u32>,
    trailer_present: &mut HashSet<u32>,
) {
    let mut missing: Vec<u32> = annotations.keys().copied().collect();
    missing.sort();
    for tag in missing {
        if trailer_set.contains(&tag) {
            trailer_present.insert(tag);
            continue;
        }
        if canonical_header.contains(&tag) || final_order.contains(&tag) {
            continue;
        }
        final_order.push(tag);
    }
}

#[allow(dead_code)]
fn append_message_fields(
    fields: &[FieldValue],
    final_order: &mut Vec<u32>,
    trailer_set: &HashSet<u32>,
    trailer_present: &mut HashSet<u32>,
) {
    for field in fields {
        let tag = field.tag;
        if trailer_set.contains(&tag) {
            trailer_present.insert(tag);
            continue;
        }
        if !final_order.contains(&tag) {
            final_order.push(tag);
        }
    }
}

#[allow(dead_code)]
fn append_trailer_tags(
    final_order: &mut Vec<u32>,
    trailer_order: &[u32],
    trailer_present: &HashSet<u32>,
) {
    for tag in trailer_order {
        if trailer_present.contains(tag) && !final_order.contains(tag) {
            final_order.push(*tag);
        }
    }
}

pub fn prettify_files(paths: &[String], ctx: &mut PrettifyContext) -> i32 {
    let mut had_error = false;
    let sources = if paths.is_empty() {
        vec!["-".to_string()]
    } else {
        paths.to_vec()
    };

    for path in sources {
        let res = if path == "-" {
            handle_stdin(ctx)
        } else {
            handle_file(&path, ctx).map(|_| 0).unwrap_or(1)
        };
        if res != 0 {
            had_error = true;
        }
    }

    if let Some(ref mut tracker) = ctx.summary.as_mut() {
        tracker.render(ctx.out).ok();
    }
    let _ = print_message_counts(ctx);

    if had_error { 1 } else { 0 }
}

pub fn print_message_counts(ctx: &mut PrettifyContext) -> io::Result<()> {
    if ctx.message_counts.is_empty() || !ctx.counts_dirty {
        return Ok(());
    }
    let mut entries: Vec<(&String, &MsgTypeCount)> = ctx.message_counts.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    let colours = palette();
    let mut prepared = Vec::new();
    let mut max_label_width = 0usize;
    for (mt, info) in &entries {
        let label_text = info.label.as_deref().unwrap_or("");
        let label_display = format!(
            "{}({}{}){}",
            colours.reset, colours.enumeration, label_text, colours.reset
        );
        let width = visible_width(&label_display);
        max_label_width = max_label_width.max(width);
        prepared.push((mt, info.count, label_display));
    }

    let count_col_start = 2 + 3 + 3 + max_label_width + 3;
    let header_pad = count_col_start.saturating_sub("Message Type".len());
    writeln!(ctx.out, "Message Type{:<pad$}Count:", "", pad = header_pad)?;

    for (mt, count, label_display) in prepared {
        let padded_label = pad_ansi(&label_display, max_label_width);
        writeln!(
            ctx.out,
            "  {}{:<3}{}   {}   {}{:>6}{}",
            colours.value, mt, colours.reset, padded_label, colours.value, count, colours.reset
        )?;
    }
    ctx.counts_dirty = false;
    Ok(())
}

/// Write a single field line, including optional enum descriptions and validation errors.
fn write_field_line(
    output: &mut String,
    dict: &FixTagLookup,
    field: &crate::decoder::fixparser::FieldValue,
    annotations: Option<&std::collections::HashMap<u32, Vec<String>>>,
    colours: &crate::decoder::colours::ColourPalette,
    indent_spaces: usize,
) {
    let tag_errors: Option<&Vec<String>> = annotations.and_then(|ann| ann.get(&field.tag));
    let tag_colour = if tag_errors.is_some() {
        colours.error
    } else {
        colours.tag
    };
    let name = dict.field_name(field.tag);
    let is_unknown = name.parse::<u32>().ok() == Some(field.tag);
    let name_coloured = if is_unknown {
        format!("{}{}{}", colours.error, name, colours.reset)
    } else {
        format!("{}{}{}", colours.name, name, colours.reset)
    };
    let name_section = format!("{}({}){}", colours.name, name_coloured, colours.reset);
    let desc = dict.enum_description(field.tag, &field.value);
    output.push_str(&format!(
        "{}{}{:4}{} {}: {}{}{}",
        indent(indent_spaces),
        tag_colour,
        field.tag,
        colours.reset,
        name_section,
        colours.value,
        field.value,
        colours.reset
    ));

    if let Some(description) = desc {
        output.push_str(&format!(
            " ({}{}{})",
            colours.enumeration, description, colours.reset
        ));
    }

    if let Some(errs) = tag_errors {
        let msg = errs.join(", ");
        output.push_str(&format!("  {}{}{}", colours.error, msg, colours.reset));
    }

    output.push('\n');
}

/// Write a placeholder line for a missing field, showing validation errors when present.
fn write_missing_line(
    output: &mut String,
    dict: &FixTagLookup,
    tag: u32,
    errors: &[String],
    colours: &crate::decoder::colours::ColourPalette,
) {
    let name = dict.field_name(tag);
    let err_text = if errors.is_empty() {
        "Missing".to_string()
    } else {
        errors.join(", ")
    };
    output.push_str(&format!(
        "{}{}{:4}{} ({}{}{}): {}{}{}\n",
        indent(BASE_INDENT),
        colours.error,
        tag,
        colours.reset,
        colours.name,
        name,
        colours.reset,
        colours.error,
        err_text,
        colours.reset
    ));
}

/// Handle decoding from stdin (used when no file paths are provided).
fn handle_stdin(ctx: &mut PrettifyContext) -> i32 {
    ctx.obfuscator.reset();
    announce_source("(stdin)", ctx);
    let mut reader = BufReader::new(io::stdin().lock());
    match stream_until_complete(&mut reader, ctx) {
        Ok(_) => 0,
        Err(_) => {
            let colours = palette();
            let _ = writeln!(
                ctx.err_out,
                "{}Error reading input{}",
                colours.error, colours.reset
            );
            1
        }
    }
}

/// Handle decoding from a single file path, printing progress when validation is disabled.
fn handle_file(path: &str, ctx: &mut PrettifyContext) -> io::Result<()> {
    ctx.obfuscator.reset();
    announce_source(path, ctx);

    let file = File::open(path).map_err(|err| {
        let colours = palette();
        let _ = writeln!(
            ctx.err_out,
            "{}Cannot open file: {}{}",
            colours.error, err, colours.reset
        );
        err
    })?;
    let mut reader = BufReader::new(file);
    stream_until_complete(&mut reader, ctx)
}

/// Stream lines from a reader, emitting formatted FIX messages (and optionally validation output).
fn stream_reader<R: BufRead>(reader: &mut R, ctx: &mut PrettifyContext) -> io::Result<bool> {
    let mut line = String::new();

    let mut line_number = 0usize;
    let mut read_any = false;
    while !ctx.interrupted.load(Ordering::Relaxed) {
        line.clear();
        let bytes = read_line_with_follow(reader, &mut line, ctx.follow, ctx.interrupted)?;
        if bytes == 0 {
            break;
        }
        read_any = true;
        line_number += 1;

        trim_line_endings(&mut line);

        let processed = ctx.obfuscator.enabled_line(&line);
        handle_log_line(&processed, line_number, ctx)?;
    }

    Ok(read_any)
}

fn stream_until_complete<R: BufRead>(reader: &mut R, ctx: &mut PrettifyContext) -> io::Result<()> {
    loop {
        let read_any = stream_reader(reader, ctx)?;
        if ctx.interrupted.load(Ordering::Relaxed) || !ctx.follow {
            return Ok(());
        }
        if !read_any {
            std::thread::sleep(FOLLOW_SLEEP);
        }
        if ctx.counts_dirty && ctx.live_status_enabled {
            let _ = print_message_counts(ctx);
        }
    }
}

fn announce_source(label: &str, ctx: &mut PrettifyContext) {
    if !ctx.style.show_header {
        return;
    }
    let colours = palette();
    let prefix = format!("-- {label} ");
    let fill = "-".repeat(terminal_width().saturating_sub(prefix.len()).max(4));
    let _ = writeln!(
        ctx.out,
        "{}{}{}{}\n",
        colours.file, prefix, fill, colours.reset
    );
}

fn extract_tag_value<'a>(msg: &'a str, tag: &str) -> Option<&'a str> {
    for field in msg.split('\u{0001}') {
        if let Some((lhs, rhs)) = field.split_once('=')
            && lhs == tag
        {
            return Some(rhs);
        }
    }
    None
}

fn fixt_session_key(msg: &str) -> Option<FixtSessionKey> {
    if extract_tag_value(msg, "8")? != "FIXT.1.1" {
        return None;
    }

    let sender = FixtSessionParty::from_message(msg, "49", "50", "142")?;
    let target = FixtSessionParty::from_message(msg, "56", "57", "143")?;
    let (first, second) = if sender <= target {
        (sender, target)
    } else {
        (target, sender)
    };
    Some(FixtSessionKey { first, second })
}

fn resolve_dictionary(msg: &str, ctx: &mut PrettifyContext) -> Arc<FixTagLookup> {
    let session_default =
        fixt_session_key(msg).and_then(|key| ctx.fixt_session_defaults.get(&key).cloned());
    let dict =
        load_dictionary_with_session_default(msg, ctx.fix_override, session_default.as_deref());

    if let Some(default_key) = default_appl_ver_key(msg)
        && let Some(session_key) = fixt_session_key(msg)
    {
        ctx.fixt_session_defaults.insert(session_key, default_key);
    }

    dict
}

fn trim_line_endings(line: &mut String) {
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
}

fn read_line_with_follow<R: BufRead>(
    reader: &mut R,
    buf: &mut String,
    follow: bool,
    interrupted: &AtomicBool,
) -> io::Result<usize> {
    loop {
        match reader.read_line(buf) {
            Ok(n) => return Ok(n),
            Err(e) if !follow => return Err(e),
            Err(_) => {
                if interrupted.load(Ordering::Relaxed) {
                    return Ok(0);
                }
                std::thread::sleep(FOLLOW_SLEEP);
            }
        }
    }
}

/// Process a single log line, extracting FIX messages and rendering prettified output.
fn handle_log_line(line: &str, line_number: usize, ctx: &mut PrettifyContext) -> io::Result<()> {
    if !ctx.validation_enabled {
        return process_without_validation(line, line_number, ctx);
    }

    process_with_validation(line, line_number, ctx)
}

fn process_without_validation(
    line: &str,
    line_number: usize,
    ctx: &mut PrettifyContext,
) -> io::Result<()> {
    let matches = find_fix_message_indices(line);
    let colours = palette();

    if matches.is_empty() {
        if ctx.summary.is_none() {
            let rendered = format!("{}{}{}", colours.line, line, colours.reset);
            write_source_line(ctx, line_number, &rendered)?;
        }
        return Ok(());
    }

    let (messages, coloured_line) =
        extract_messages_and_format(line, &matches, ctx.display_delimiter);
    let dictionaries: Vec<Arc<FixTagLookup>> = messages
        .iter()
        .map(|msg| resolve_dictionary(msg, ctx))
        .collect();

    if ctx.summary.is_none() {
        let separator_width = messages.iter().zip(dictionaries.iter()).fold(
            max_visible_line_width(&coloured_line),
            |acc, (msg, dict)| acc.max(separator_width_for_message(msg, dict.as_ref(), false)),
        );
        write_source_line(ctx, line_number, &coloured_line)?;
        write!(
            ctx.out,
            "{}",
            render_separator(ctx.style, separator_width, ctx.wide_grid)
        )?;
    }

    record_messages(&messages, &dictionaries, ctx);
    emit_messages(&messages, &dictionaries, ctx)?;

    render_summary_footer(ctx)
}

fn process_with_validation(
    line: &str,
    line_number: usize,
    ctx: &mut PrettifyContext,
) -> io::Result<()> {
    let matches = find_fix_message_indices(line);
    if matches.is_empty() {
        return Ok(());
    }

    let resolved: Vec<(&str, Arc<FixTagLookup>)> = matches
        .iter()
        .map(|(start, end)| {
            let msg = &line[*start..*end];
            let dict = resolve_dictionary(msg, ctx);
            (msg, dict)
        })
        .collect();
    for (msg, dict) in &resolved {
        track_message(msg, dict.as_ref(), ctx);
    }
    render_summary_footer(ctx)?;

    let mut header_emitted = false;
    let colours = palette();
    let display_line = apply_display_delimiter(line, ctx.display_delimiter);

    for (msg, dict) in resolved {
        let report = validator::validate_fix_message(msg, &dict);
        if report.is_clean() {
            continue;
        }
        if !header_emitted {
            if ctx.style.show_numbers {
                let rendered = format!("{}{}{}", colours.line, display_line, colours.reset);
                write_source_line(ctx, line_number, &rendered)?;
            } else {
                writeln!(
                    ctx.out,
                    "Line {}: {}{}{}",
                    line_number, colours.line, display_line, colours.reset
                )?;
            }
            header_emitted = true;
        }
        stream_invalid_message(ctx, msg, &dict, &report)?;
    }

    Ok(())
}

fn stream_invalid_message(
    ctx: &mut PrettifyContext,
    msg: &str,
    dict: &FixTagLookup,
    report: &validator::ValidationReport,
) -> io::Result<()> {
    let pretty = prettify_with_report(msg, dict, Some(report));
    write!(ctx.out, "{pretty}")?;
    writeln!(ctx.out)?;
    Ok(())
}

fn record_messages(
    messages: &[String],
    dictionaries: &[Arc<FixTagLookup>],
    ctx: &mut PrettifyContext,
) {
    for (msg, dict) in messages.iter().zip(dictionaries.iter()) {
        track_message(msg, dict.as_ref(), ctx);
    }
}

fn track_message(msg: &str, dict: &FixTagLookup, ctx: &mut PrettifyContext) {
    if !ctx.validation_enabled {
        record_msg_type(msg, dict, ctx);
    }
    if let Some(ref mut tracker) = ctx.summary.as_mut() {
        tracker.record_message_with_lookup(msg, dict, ctx.fix_override);
    }
}

fn record_msg_type(msg: &str, dict: &FixTagLookup, ctx: &mut PrettifyContext) {
    if let Some(mt) = extract_msg_type(msg) {
        let entry = ctx.message_counts.entry(mt.clone()).or_default();
        entry.count += 1;
        if entry.label.is_none() {
            entry.label = dict.enum_description(35, &mt).map(|s| s.to_string());
        }
        ctx.counts_dirty = true;
    }
}

fn extract_msg_type(msg: &str) -> Option<String> {
    const SOH: char = '\u{0001}';
    for field in msg.split(SOH) {
        if let Some((tag, val)) = field.split_once('=')
            && tag == "35"
        {
            return Some(val.to_string());
        }
    }
    None
}

fn emit_messages(
    messages: &[String],
    dictionaries: &[Arc<FixTagLookup>],
    ctx: &mut PrettifyContext,
) -> io::Result<()> {
    if ctx.summary.is_some() {
        return Ok(());
    }

    for (msg, dict) in messages.iter().zip(dictionaries.iter()) {
        process_fix_message(
            msg,
            dict.as_ref(),
            ctx.out,
            ctx.validation_enabled,
            ctx.style,
            ctx.wide_grid,
        )?;
    }
    Ok(())
}

fn render_summary_footer(ctx: &mut PrettifyContext) -> io::Result<()> {
    if !ctx.live_status_enabled {
        return Ok(());
    }
    if let Some(ref mut tracker) = ctx.summary.as_mut() {
        if ctx.follow {
            let _printed = tracker.render_completed(ctx.out)?;
            tracker.render_footer(ctx.out)?;
        } else {
            tracker.render_footer(ctx.out)?;
        }
    }
    Ok(())
}

/// Locate FIX message spans within a line using a permissive regex.
fn find_fix_message_indices(line: &str) -> Vec<(usize, usize)> {
    FIX_REGEX
        .find_iter(line)
        .map(|m| (m.start(), m.end()))
        .collect()
}

/// Extract FIX messages from a line while also returning a coloured representation.
fn extract_messages_and_format(
    line: &str,
    matches: &[(usize, usize)],
    display_delimiter: char,
) -> (Vec<String>, String) {
    let colours = palette();
    let mut output = String::new();
    let mut fix_messages = Vec::new();
    let mut last = 0;

    for (start, end) in matches {
        output.push_str(colours.line);
        let before = &line[last..*start];
        let before_display = apply_display_delimiter(before, display_delimiter);
        output.push_str(&before_display);

        output.push_str(colours.message);
        let fix_segment = &line[*start..*end];
        let fix_display = apply_display_delimiter(fix_segment, display_delimiter);
        output.push_str(&fix_display);
        fix_messages.push(line[*start..*end].to_string());
        last = *end;
    }

    if last < line.len() {
        output.push_str(colours.line);
        let tail_display = apply_display_delimiter(&line[last..], display_delimiter);
        output.push_str(&tail_display);
    } else {
        output.push_str(colours.line);
    }

    output.push_str(colours.reset);

    (fix_messages, output)
}

fn render_separator(style: OutputStyle, content_width: usize, wide_grid: bool) -> String {
    if !style.show_grid {
        return "\n".to_string();
    }
    let colours = palette();
    let width = if wide_grid {
        content_width.max(terminal_width()).max(1)
    } else {
        terminal_width()
    };
    format!("{}{}{}\n", colours.title, "=".repeat(width), colours.reset)
}

fn max_visible_line_width(text: &str) -> usize {
    text.lines().map(visible_width).max().unwrap_or_default()
}

fn write_source_line(
    ctx: &mut PrettifyContext,
    line_number: usize,
    content: &str,
) -> io::Result<()> {
    if ctx.style.show_numbers {
        let colours = palette();
        write!(
            ctx.out,
            "{}{:>6}{} | ",
            colours.file, line_number, colours.reset
        )?;
    }
    writeln!(ctx.out, "{content}")
}

/// Replace SOH display delimiters for human-readable rendering without mutating inputs.
fn apply_display_delimiter<'a>(text: &'a str, delimiter: char) -> Cow<'a, str> {
    const SOH: char = '\u{0001}';
    if delimiter == SOH || !text.contains(SOH) {
        Cow::Borrowed(text)
    } else {
        let mut output = String::with_capacity(text.len());
        for ch in text.chars() {
            if ch == SOH {
                output.push(delimiter);
            } else {
                output.push(ch);
            }
        }
        Cow::Owned(output)
    }
}

/// Render a single FIX message (and validation errors when enabled) to the output stream.
fn process_fix_message(
    msg: &str,
    dict: &FixTagLookup,
    out: &mut dyn Write,
    validation_enabled: bool,
    style: OutputStyle,
    wide_grid: bool,
) -> io::Result<()> {
    let pretty = prettify_with_report(msg, dict, None);
    let report = validation_enabled.then(|| validator::validate_fix_message(msg, dict));
    let separator_width = separator_width_for_pretty(&pretty, report.as_ref());
    let separator = render_separator(style, separator_width, wide_grid);
    write!(out, "{pretty}")?;

    if let Some(report) = report
        && !report.errors.is_empty()
    {
        let colours = palette();
        write!(out, "{separator}")?;
        for err in report.errors {
            writeln!(out, "{}== {}{}", colours.error, err, colours.reset)?;
        }
    }

    write!(out, "{separator}")?;
    Ok(())
}

fn separator_width_for_message(msg: &str, dict: &FixTagLookup, validation_enabled: bool) -> usize {
    let pretty = prettify_with_report(msg, dict, None);
    let report = validation_enabled.then(|| validator::validate_fix_message(msg, dict));
    separator_width_for_pretty(&pretty, report.as_ref())
}

fn separator_width_for_pretty(pretty: &str, report: Option<&validator::ValidationReport>) -> usize {
    let mut separator_width = max_visible_line_width(pretty);
    if let Some(report) = report {
        for err in &report.errors {
            separator_width = separator_width.max(err.len() + 3);
        }
    }
    separator_width
}

pub fn disable_output_colours() {
    disable_colours();
}

#[cfg(test)]
fn test_lookup_with_order(field_order: Vec<u32>) -> FixTagLookup {
    use std::collections::HashMap;

    let mut messages = HashMap::new();
    messages.insert(
        "X".to_string(),
        MessageDef {
            _name: "X".to_string(),
            _msg_type: "X".to_string(),
            field_order,
            required: Vec::new(),
            groups: HashMap::new(),
            group_membership: HashMap::new(),
        },
    );
    FixTagLookup::new_for_tests(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::schema::FixDictionary;
    use crate::decoder::tag_lookup::load_dictionary;
    use crate::decoder::validator;
    use crate::fix;
    use std::collections::HashMap;
    use std::io::Cursor;
    use std::sync::Mutex;
    use std::sync::atomic::Ordering;
    use tempfile::NamedTempFile;

    const SOH: char = '\u{0001}';
    static TEST_GUARD: once_cell::sync::Lazy<Mutex<()>> =
        once_cell::sync::Lazy::new(|| Mutex::new(()));

    fn small_group_lookup() -> FixTagLookup {
        let xml = r#"
<fix type='FIX' major='4' minor='4'>
  <header>
    <field name='BeginString' required='Y'/>
    <field name='BodyLength' required='Y'/>
    <field name='MsgType' required='Y'/>
  </header>
  <trailer>
    <field name='CheckSum' required='Y'/>
  </trailer>
  <messages>
    <message name='MDSnapshot' msgtype='W' msgcat='app'>
      <field name='MsgType' required='Y'/>
      <group name='NoMDEntries'>
        <field name='MDEntryType' required='Y'/>
        <field name='MDEntryPx'/>
      </group>
    </message>
  </messages>
  <components/>
  <fields>
    <field number='8' name='BeginString' type='STRING'/>
    <field number='9' name='BodyLength' type='LENGTH'/>
    <field number='10' name='CheckSum' type='STRING'/>
    <field number='35' name='MsgType' type='STRING'>
      <value enum='W' description='MDSnapshot'/>
    </field>
    <field number='268' name='NoMDEntries' type='NUMINGROUP'/>
    <field number='269' name='MDEntryType' type='CHAR'/>
    <field number='270' name='MDEntryPx' type='PRICE'/>
  </fields>
</fix>
"#;
        let dict = FixDictionary::from_xml(xml).expect("tiny dictionary parses");
        FixTagLookup::from_dictionary(&dict, "TEST")
    }

    #[test]
    fn prettify_aligns_group_entries_without_header() {
        let _lock = TEST_GUARD.lock().unwrap();
        disable_output_colours();
        let dict = small_group_lookup();
        let msg = format!(
            "8=FIX.4.4{SOH}35=W{SOH}268=2{SOH}269=0{SOH}270=12.34{SOH}269=1{SOH}270=56.78{SOH}10=000{SOH}"
        );
        let rendered = prettify_with_report(&msg, &dict, None);
        assert!(
            !rendered.contains("Group: NoMDEntries"),
            "group header line should be omitted: {rendered}"
        );
        let count_line = rendered
            .lines()
            .find(|l| l.contains("NoMDEntries"))
            .expect("count tag line present");
        let group_line = rendered
            .lines()
            .find(|l| l.contains("Group 1"))
            .expect("group entry label present");
        let paren_col = count_line.find('(').expect("open paren present");
        let dash_col = group_line.find('-').expect("dashes present");
        assert_eq!(
            dash_col,
            paren_col + 1,
            "group separator should start one space after '(' anchor"
        );
    }

    #[test]
    fn validation_only_outputs_invalid_messages() {
        let _lock = TEST_GUARD.lock().unwrap();
        let obfuscator = fix::create_obfuscator(false);
        let body = format!("35=0{SOH}34=1{SOH}49=AAA{SOH}52=20240101-00:00:00{SOH}56=BBB{SOH}");
        let declared_len = body.len() + 1; // intentionally wrong
        let msg_without_checksum = format!("8=FIX.4.4{SOH}9={:03}{SOH}{}", declared_len, body);
        let checksum = validator::calculate_checksum(&format!("{msg_without_checksum}10=000{SOH}"));
        let msg = format!("{msg_without_checksum}10={checksum:03}{SOH}");
        let line = format!("{msg}\n");
        let mut out = Vec::new();
        let mut err = io::sink();
        let mut summary = None;
        let mut ctx = PrettifyContext {
            out: &mut out,
            err_out: &mut err,
            obfuscator: &obfuscator,
            display_delimiter: '|',
            style: OutputStyle::plain(),
            wide_grid: false,
            summary: &mut summary,
            fix_override: None,
            follow: false,
            live_status_enabled: true,
            validation_enabled: true,
            message_counts: HashMap::new(),
            fixt_session_defaults: HashMap::new(),
            counts_dirty: false,
            interrupted: interrupt_flag(),
        };
        let mut reader = BufReader::new(Cursor::new(line));
        stream_reader(&mut reader, &mut ctx).unwrap();

        let output = String::from_utf8(out).unwrap();
        assert!(
            output.contains("Line 1:"),
            "line number should be printed for invalid message"
        );
        assert!(
            output.contains("BodyLength mismatch"),
            "error annotations should be rendered: {output}"
        );
        assert!(
            output.contains('|'),
            "default display delimiter replacement should appear"
        );
    }

    #[test]
    fn validation_skips_valid_messages() {
        let _lock = TEST_GUARD.lock().unwrap();
        interrupt_flag().store(false, Ordering::Relaxed);
        let obfuscator = fix::create_obfuscator(false);
        let lookup = load_dictionary(&format!("8=FIX.4.4{SOH}35=0{SOH}10=000{SOH}"));
        let order = lookup
            .message_def("0")
            .expect("heartbeat definition")
            .field_order
            .clone();
        let mut values = HashMap::new();
        values.insert(35u32, "0");
        values.insert(34u32, "1");
        values.insert(49u32, "AAA");
        values.insert(52u32, "20240101-00:00:00");
        values.insert(56u32, "BBB");

        let body = build_body_from_order(&order, &values);
        let msg_without_checksum = format!("8=FIX.4.4{SOH}9={:03}{SOH}{}", body.len(), body);
        let checksum = validator::calculate_checksum(&format!("{msg_without_checksum}10=000{SOH}"));
        let msg = format!("{msg_without_checksum}10={checksum:03}{SOH}");
        let dict = load_dictionary(&msg);
        let errs = validator::validate_fix_message(&msg, &dict);
        assert!(
            errs.is_clean(),
            "message used for validation bypass should be valid, got {:?}",
            errs.errors
        );
        let line = format!("{msg}\n");
        let mut out = Vec::new();
        let mut err = io::sink();
        let mut summary = None;
        let mut ctx = PrettifyContext {
            out: &mut out,
            err_out: &mut err,
            obfuscator: &obfuscator,
            display_delimiter: '|',
            style: OutputStyle::plain(),
            wide_grid: false,
            summary: &mut summary,
            fix_override: None,
            follow: false,
            live_status_enabled: true,
            validation_enabled: true,
            message_counts: HashMap::new(),
            fixt_session_defaults: HashMap::new(),
            counts_dirty: false,
            interrupted: interrupt_flag(),
        };
        let mut reader = BufReader::new(Cursor::new(line));
        stream_reader(&mut reader, &mut ctx).unwrap();

        let output = String::from_utf8(out).unwrap();
        assert!(
            output.trim().is_empty(),
            "valid messages should not produce output in validation mode"
        );
    }

    #[test]
    fn prettify_files_validation_skips_message_counts_for_clean_messages() {
        let _lock = TEST_GUARD.lock().unwrap();
        interrupt_flag().store(false, Ordering::Relaxed);
        let obfuscator = fix::create_obfuscator(false);
        let lookup = load_dictionary(&format!("8=FIX.4.4{SOH}35=0{SOH}10=000{SOH}"));
        let order = lookup
            .message_def("0")
            .expect("heartbeat definition")
            .field_order
            .clone();
        let mut values = HashMap::new();
        values.insert(35u32, "0");
        values.insert(34u32, "1");
        values.insert(49u32, "AAA");
        values.insert(52u32, "20240101-00:00:00");
        values.insert(56u32, "BBB");

        let body = build_body_from_order(&order, &values);
        let msg_without_checksum = format!("8=FIX.4.4{SOH}9={:03}{SOH}{}", body.len(), body);
        let checksum = validator::calculate_checksum(&format!("{msg_without_checksum}10=000{SOH}"));
        let msg = format!("{msg_without_checksum}10={checksum:03}{SOH}");

        let mut file = NamedTempFile::new().expect("temp file");
        std::io::Write::write_all(&mut file, format!("{msg}\n").as_bytes()).expect("write temp");

        let mut out = Vec::new();
        let mut err = io::sink();
        let mut summary = None;
        let mut ctx = PrettifyContext {
            out: &mut out,
            err_out: &mut err,
            obfuscator: &obfuscator,
            display_delimiter: '|',
            style: OutputStyle::plain(),
            wide_grid: false,
            summary: &mut summary,
            fix_override: None,
            follow: false,
            live_status_enabled: true,
            validation_enabled: true,
            message_counts: HashMap::new(),
            fixt_session_defaults: HashMap::new(),
            counts_dirty: false,
            interrupted: interrupt_flag(),
        };

        let status = prettify_files(&[file.path().display().to_string()], &mut ctx);
        let output = String::from_utf8(out).unwrap();
        assert_eq!(status, 0);
        assert!(
            output.trim().is_empty(),
            "clean validation runs should not emit decoded output or message counts"
        );
    }

    #[test]
    fn validation_inserts_missing_tags() {
        let _lock = TEST_GUARD.lock().unwrap();
        disable_output_colours();
        let obfuscator = fix::create_obfuscator(false);
        let msg = format!("8=FIX.4.4{SOH}9=005{SOH}10=999{SOH}");
        let line = format!("{msg}\n");
        let mut out = Vec::new();
        let mut err = io::sink();
        let mut summary = None;
        let mut ctx = PrettifyContext {
            out: &mut out,
            err_out: &mut err,
            obfuscator: &obfuscator,
            display_delimiter: '|',
            style: OutputStyle::plain(),
            wide_grid: false,
            summary: &mut summary,
            fix_override: None,
            follow: false,
            live_status_enabled: true,
            validation_enabled: true,
            message_counts: HashMap::new(),
            fixt_session_defaults: HashMap::new(),
            counts_dirty: false,
            interrupted: interrupt_flag(),
        };
        let mut reader = BufReader::new(Cursor::new(line));
        stream_reader(&mut reader, &mut ctx).unwrap();

        let output = String::from_utf8(out).unwrap();
        assert!(
            output.contains("35 (MsgType): Missing"),
            "missing tag should be shown in decoded output: {output}"
        );
    }

    #[test]
    fn prettify_includes_missing_tag_annotations_once() {
        let _lock = TEST_GUARD.lock().unwrap();
        disable_output_colours();
        let msg = format!("8=FIX.4.4{SOH}9=005{SOH}35=0{SOH}10=000{SOH}");
        let dict = load_dictionary(&msg);

        let mut report = validator::ValidationReport::default();
        report
            .tag_errors
            .insert(34, vec!["missing sequence".to_string()]);

        let pretty = prettify_with_report(&msg, &dict, Some(&report));
        let lines: Vec<&str> = pretty.lines().collect();
        let missing_lines: Vec<&str> = lines
            .iter()
            .copied()
            .filter(|l| l.contains("34") && l.contains("missing sequence"))
            .collect();

        assert_eq!(
            missing_lines.len(),
            1,
            "missing tag 34 should appear exactly once: {pretty}"
        );
    }

    #[test]
    fn build_tag_order_respects_annotations_and_trailer() {
        let _lock = TEST_GUARD.lock().unwrap();
        let mut messages = HashMap::new();
        messages.insert(
            "X".to_string(),
            MessageDef {
                _name: "X".to_string(),
                _msg_type: "X".to_string(),
                field_order: vec![8, 9, 35, 55],
                required: Vec::new(),
                groups: HashMap::new(),
                group_membership: HashMap::new(),
            },
        );
        let dict = FixTagLookup::new_for_tests(messages);
        let fields = vec![
            FieldValue {
                tag: 8,
                value: "FIX.4.4".into(),
            },
            FieldValue {
                tag: 9,
                value: "5".into(),
            },
            FieldValue {
                tag: 35,
                value: "X".into(),
            },
            FieldValue {
                tag: 55,
                value: "AAPL".into(),
            },
            FieldValue {
                tag: 99,
                value: "Z".into(),
            },
            FieldValue {
                tag: 10,
                value: "000".into(),
            },
        ];
        let mut annotations = std::collections::HashMap::new();
        annotations.insert(77u32, vec!["missing".into()]);

        let order = build_tag_order(&fields, &dict, Some(&annotations));
        assert!(order.starts_with(&[8, 9, 35, 49, 56, 34, 52]));
        assert!(order.contains(&55));
        assert!(order.contains(&99));
        assert!(order.contains(&77));
        assert_eq!(order.last(), Some(&10));
    }

    #[test]
    fn trim_line_endings_strips_crlf() {
        let mut line = "abc\r\n".to_string();
        trim_line_endings(&mut line);
        assert_eq!(line, "abc");
    }

    #[test]
    fn max_visible_line_width_tracks_longest_rendered_line() {
        let text = "short\n\u{1b}[31mthis is longer\u{1b}[0m\nmid";
        assert_eq!(max_visible_line_width(text), "this is longer".len());
    }

    #[test]
    fn render_separator_expands_when_wide_grid_is_enabled() {
        disable_output_colours();
        let wide = terminal_width() + 25;
        let rendered = render_separator(OutputStyle::full(), wide, true);
        let line = rendered.trim_end_matches('\n');
        assert_eq!(visible_width(line), wide);

        let regular = render_separator(OutputStyle::full(), wide, false);
        let regular_line = regular.trim_end_matches('\n');
        assert_eq!(visible_width(regular_line), terminal_width());
    }

    #[test]
    fn source_separator_matches_decoded_bottom_separator_in_wide_grid_mode() {
        let _lock = TEST_GUARD.lock().unwrap();
        disable_output_colours();
        let obfuscator = fix::create_obfuscator(false);
        let line = format!("8=FIX.4.4{SOH}9=005{SOH}35=0{SOH}10=000{SOH}\n");
        let mut out = Vec::new();
        let mut err = io::sink();
        let mut summary = None;
        let mut ctx = PrettifyContext {
            out: &mut out,
            err_out: &mut err,
            obfuscator: &obfuscator,
            display_delimiter: '|',
            style: OutputStyle {
                show_numbers: false,
                show_header: false,
                show_grid: true,
            },
            wide_grid: true,
            summary: &mut summary,
            fix_override: None,
            follow: false,
            live_status_enabled: true,
            validation_enabled: false,
            message_counts: HashMap::new(),
            fixt_session_defaults: HashMap::new(),
            counts_dirty: false,
            interrupted: interrupt_flag(),
        };
        let mut reader = BufReader::new(Cursor::new(line));
        stream_reader(&mut reader, &mut ctx).unwrap();

        let output = String::from_utf8(out).unwrap();
        let separators: Vec<&str> = output
            .lines()
            .filter(|line| line.starts_with('='))
            .collect();
        assert_eq!(
            separators.len(),
            2,
            "expected source-to-decoded separator and decoded bottom separator: {output}"
        );
        assert_eq!(
            visible_width(separators[0]),
            visible_width(separators[1]),
            "source separator should match decoded block width: {output}"
        );
    }

    #[test]
    fn read_line_with_follow_returns_zero_on_eof() {
        let mut reader = Cursor::new("");
        let mut buf = String::new();
        let n = read_line_with_follow(&mut reader, &mut buf, true, interrupt_flag()).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn prettify_orders_without_msg_type_header_first() {
        let _lock = TEST_GUARD.lock().unwrap();
        disable_output_colours();
        let msg = format!("8=FIX.4.4{SOH}9=005{SOH}55=IBM{SOH}10=999{SOH}");
        let dict = load_dictionary(&msg);

        let pretty = prettify_with_report(&msg, &dict, None);
        let tags: Vec<u32> = pretty
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter_map(|tag| tag.parse::<u32>().ok())
            .collect();

        assert!(
            tags.starts_with(&[8, 9]),
            "header tags should lead when MsgType is missing: {:?}",
            tags
        );
        let pos_55 = tags.iter().position(|t| *t == 55);
        let pos_10 = tags.iter().position(|t| *t == 10);
        assert!(
            pos_55 < pos_10,
            "body tag 55 should appear before checksum: {:?}",
            tags
        );
    }

    #[test]
    fn header_and_trailer_are_repositioned_when_out_of_place() {
        let _lock = TEST_GUARD.lock().unwrap();
        disable_output_colours();

        let dict = test_lookup_with_order(vec![37, 11, 150, 8, 9, 35, 10]);
        let fields = vec![
            FieldValue {
                tag: 8,
                value: "FIX.4.4".into(),
            },
            FieldValue {
                tag: 9,
                value: "100".into(),
            },
            FieldValue {
                tag: 35,
                value: "X".into(),
            },
            FieldValue {
                tag: 37,
                value: "ABC".into(),
            },
            FieldValue {
                tag: 150,
                value: "0".into(),
            },
            FieldValue {
                tag: 553,
                value: "user".into(),
            },
            FieldValue {
                tag: 10,
                value: "000".into(),
            },
        ];

        let order = build_tag_order(&fields, &dict, None);
        let header_prefix: Vec<u32> = order.iter().take(7).copied().collect();
        assert_eq!(
            header_prefix,
            vec![8, 9, 35, 49, 56, 34, 52],
            "canonical header should lead the order"
        );

        let pos_order_id = order
            .iter()
            .position(|t| *t == 37)
            .expect("body tag should be present");
        assert!(
            pos_order_id >= 7,
            "body tags should follow header: {:?}",
            order
        );
        assert_eq!(
            order.last(),
            Some(&10),
            "checksum must be forced to the end: {:?}",
            order
        );
        let pos_user = order.iter().position(|t| *t == 553).unwrap();
        let pos_checksum = order.iter().position(|t| *t == 10).unwrap();
        assert!(
            pos_user < pos_checksum,
            "unknown body tags should remain before trailer: {:?}",
            order
        );
    }

    fn build_body_from_order(order: &[u32], values: &HashMap<u32, &str>) -> String {
        let mut out = String::new();
        for tag in order {
            if *tag == 8 || *tag == 9 || *tag == 10 {
                continue;
            }
            if let Some(val) = values.get(tag) {
                out.push_str(&format!("{tag}={val}{SOH}"));
            }
        }
        out
    }
}
