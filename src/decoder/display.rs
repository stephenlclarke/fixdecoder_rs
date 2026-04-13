// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2025 Steve Clarke <stephenlclarke@mac.com> - https://xyzzy.tools

//! Presentation helpers for dictionary browsing and FIX message output.
//! Rendering is intentionally opinionated: we optimise for legibility in
//! consoles, reusing colour palettes and column layouts wherever possible.
//! The module-level comment keeps the tone informal yet informative.

use crate::decoder::colours::{ColourPalette, palette};
use crate::decoder::layout::{NEST_INDENT, TAG_WIDTH};
use crate::decoder::schema::{
    ComponentNode, ContainerNode, ContainerNodeVisitor, Field, FieldNode, GroupNode, MessageNode,
    SchemaTree, Value, walk_container_nodes,
};
use std::cmp;
use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt;
use std::io::{self, Write};
use terminal_size::{Width, terminal_size};

/// Captures how many columns we can render enums in and how wide each column
/// needs to be for tidy terminal output.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct ColumnLayout {
    column_width: usize,
    columns: usize,
    max_indent: usize,
}

/// Colour + layout preferences passed around the render stack.  Allows the
/// caller to toggle column mode once and reuse the result everywhere.
#[derive(Clone, Copy)]
pub struct DisplayStyle {
    colours: ColourPalette,
    columns: bool,
    layout: Option<ColumnLayout>,
}

impl DisplayStyle {
    pub fn new(colours: ColourPalette, columns: bool) -> Self {
        Self {
            colours,
            columns,
            layout: None,
        }
    }

    fn colours(self) -> ColourPalette {
        self.colours
    }

    fn columns_enabled(self) -> bool {
        self.columns
    }

    fn layout(self) -> Option<ColumnLayout> {
        self.layout
    }

    fn with_layout(self, layout: Option<ColumnLayout>) -> Self {
        Self { layout, ..self }
    }

    fn ensure_layout<F>(self, compute: F) -> Self
    where
        F: FnOnce() -> Option<ColumnLayout>,
    {
        if !self.columns || self.layout.is_some() {
            self
        } else {
            self.with_layout(compute())
        }
    }
}

/// Running stats used to find the optimal column width given all fields
/// in a message/component/group.
#[derive(Default)]
struct LayoutStats {
    max_entry_len: usize,
    max_indent: usize,
}

impl LayoutStats {
    fn record(&mut self, entry_len: usize, indent: usize) {
        if entry_len == 0 {
            return;
        }
        self.max_entry_len = self.max_entry_len.max(entry_len);
        self.max_indent = self.max_indent.max(indent);
    }

    fn finalize(self) -> Option<ColumnLayout> {
        if self.max_entry_len == 0 {
            return None;
        }
        let column_width = self.max_entry_len + 2;
        let usable_width = terminal_width().saturating_sub(self.max_indent);
        let columns = cmp::max(1, usable_width / column_width);
        Some(ColumnLayout {
            column_width,
            columns: columns.max(1),
            max_indent: self.max_indent,
        })
    }
}

pub(crate) fn terminal_width() -> usize {
    if let Some((Width(w), _)) = terminal_size() {
        w as usize
    } else {
        80
    }
}

pub(crate) fn visible_width(text: &str) -> usize {
    let mut width = 0;
    let mut in_esc = false;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if in_esc {
            if b == b'm' {
                in_esc = false;
            }
            i += 1;
            continue;
        }
        if b == 0x1b {
            in_esc = true;
            i += 1;
            continue;
        }
        width += 1;
        i += 1;
    }
    width
}

pub(crate) fn pad_ansi(text: &str, width: usize) -> String {
    let visible = visible_width(text);
    if visible >= width {
        return text.to_string();
    }
    let pad = width - visible;
    format!("{text}{}", " ".repeat(pad))
}

/// Tiny helper that implements `Display` for indentation without building
/// temporary `String`s.
#[derive(Clone, Copy)]
pub(crate) struct Indent(usize);

impl fmt::Display for Indent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:width$}", "", width = self.0)
    }
}

pub(crate) fn indent(level: usize) -> Indent {
    Indent(level)
}

/// Buffer reused when we collate enum/value pairs so we avoid allocating a
/// new vector for every field.
type EnumBuffer<'a> = Vec<&'a Value>;

/// Collect values into the shared buffer and sort them for deterministic
/// column output.  Saves the caller from having to clone per field.
fn collect_sorted_values<'a, 'b, I>(buf: &'b mut EnumBuffer<'a>, iter: I) -> &'b [&'a Value]
where
    I: IntoIterator<Item = &'a Value>,
{
    buf.clear();
    buf.extend(iter);
    buf.sort_by(|a, b| a.enumeration.cmp(&b.enumeration));
    buf
}

fn format_required(required: bool, colours: ColourPalette) -> String {
    if required {
        format!(" - ({}Y{})", colours.title, colours.reset)
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::schema::ValuesWrapper;

    #[test]
    fn visible_width_ignores_ansi_sequences() {
        let coloured = "\u{1b}[31mred\u{1b}[0m";
        assert_eq!(visible_width(coloured), 3);
    }

    #[test]
    fn pad_ansi_extends_to_requested_width() {
        let coloured = "\u{1b}[32mok\u{1b}[0m";
        let padded = pad_ansi(coloured, 5);
        assert_eq!(visible_width(&padded), 5);
        assert!(padded.ends_with("   "));
    }

    #[test]
    fn collect_sorted_values_orders_by_enum() {
        let mut buf = Vec::new();
        let values = [
            Value {
                enumeration: "B".into(),
                description: "Second".into(),
            },
            Value {
                enumeration: "A".into(),
                description: "First".into(),
            },
        ];
        let sorted = collect_sorted_values(&mut buf, values.iter());
        let enums: Vec<&str> = sorted.iter().map(|v| v.enumeration.as_str()).collect();
        assert_eq!(enums, vec!["A", "B"]);
    }

    #[test]
    fn layout_stats_produces_layout() {
        let mut stats = LayoutStats::default();
        stats.record(5, 2);
        stats.record(10, 4);
        let layout = stats.finalize().expect("layout expected");
        assert!(layout.column_width >= 12);
        assert!(layout.columns >= 1);
    }

    #[test]
    fn terminal_width_is_positive() {
        assert!(terminal_width() > 0);
    }

    fn sample_value(enum_code: &str, desc: &str) -> Value {
        Value {
            enumeration: enum_code.to_string(),
            description: desc.to_string(),
        }
    }

    fn sample_field_node(required: bool) -> FieldNode {
        use std::sync::Arc;
        let field = Field {
            name: "TestField".into(),
            number: 999,
            field_type: "STRING".into(),
            values: vec![sample_value("A", "Alpha")],
            values_wrapper: ValuesWrapper::default(),
        };
        FieldNode {
            required,
            field: Arc::new(field),
        }
    }

    fn schema_with_structures() -> SchemaTree {
        use std::collections::BTreeMap;
        use std::sync::Arc;

        let msg_type_field = Arc::new(Field {
            name: "MsgType".into(),
            number: 35,
            field_type: "STRING".into(),
            values: vec![
                sample_value("D", "NewOrderSingle"),
                sample_value("8", "ExecutionReport"),
            ],
            values_wrapper: ValuesWrapper::default(),
        });

        let aux_field = sample_field_node(false);
        let group_count_field = Arc::new(Field {
            name: "Nested".into(),
            number: 200,
            field_type: "NUMINGROUP".into(),
            values: Vec::new(),
            values_wrapper: ValuesWrapper::default(),
        });
        let allocs_count_field = Arc::new(Field {
            name: "Allocs".into(),
            number: 201,
            field_type: "NUMINGROUP".into(),
            values: Vec::new(),
            values_wrapper: ValuesWrapper::default(),
        });
        let group_field = sample_field_node(true);
        let nested_group = GroupNode {
            name: "Nested".into(),
            required: false,
            fields: vec![group_field.clone()],
            components: Vec::new(),
            groups: Vec::new(),
            entries: vec![ContainerNode::Field(group_field.clone())],
        };

        let component = ComponentNode {
            name: "Block".into(),
            fields: vec![aux_field.clone()],
            groups: vec![nested_group.clone()],
            components: Vec::new(),
            entries: vec![
                ContainerNode::Field(aux_field.clone()),
                ContainerNode::Group(nested_group.clone()),
            ],
        };

        let header = ComponentNode {
            name: "Header".into(),
            fields: vec![FieldNode {
                required: true,
                field: msg_type_field.clone(),
            }],
            groups: Vec::new(),
            components: Vec::new(),
            entries: vec![ContainerNode::Field(FieldNode {
                required: true,
                field: msg_type_field.clone(),
            })],
        };

        let trailer = ComponentNode {
            name: "Trailer".into(),
            fields: vec![aux_field.clone()],
            groups: Vec::new(),
            components: Vec::new(),
            entries: vec![ContainerNode::Field(aux_field.clone())],
        };

        let allocs_group = GroupNode {
            name: "Allocs".into(),
            required: true,
            fields: vec![group_field.clone()],
            components: vec![component.clone()],
            groups: Vec::new(),
            entries: vec![
                ContainerNode::Field(group_field.clone()),
                ContainerNode::Component(component.clone()),
            ],
        };

        let message = MessageNode {
            name: "NewOrder".into(),
            msg_type: "D".into(),
            msg_cat: "app".into(),
            fields: vec![
                FieldNode {
                    required: true,
                    field: msg_type_field.clone(),
                },
                aux_field.clone(),
            ],
            components: vec![component.clone()],
            groups: vec![allocs_group.clone()],
            entries: vec![
                ContainerNode::Field(FieldNode {
                    required: true,
                    field: msg_type_field.clone(),
                }),
                ContainerNode::Field(aux_field.clone()),
                ContainerNode::Component(component.clone()),
                ContainerNode::Group(allocs_group),
            ],
        };

        let mut fields = BTreeMap::new();
        fields.insert(msg_type_field.name.clone(), msg_type_field.clone());
        fields.insert(aux_field.field.name.clone(), aux_field.field.clone());
        fields.insert(group_count_field.name.clone(), group_count_field.clone());
        fields.insert(allocs_count_field.name.clone(), allocs_count_field.clone());

        let mut components = BTreeMap::new();
        components.insert(header.name.clone(), header);
        components.insert(trailer.name.clone(), trailer);
        components.insert(component.name.clone(), component);

        let mut messages = BTreeMap::new();
        messages.insert(message.name.clone(), message);

        SchemaTree {
            fields,
            components,
            messages,
            version: "FIX 4.4".into(),
            service_pack: "-".into(),
        }
    }

    #[test]
    fn print_field_renders_required_indicator() {
        let node = sample_field_node(true);
        let mut out = Vec::new();
        print_field(&mut out, &node, 2, palette()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("999"));
        assert!(s.contains("TestField"));
        assert!(s.contains("STRING"));
        assert!(s.contains('Y'), "required marker should be present: {s}");
    }

    #[test]
    fn print_enum_outputs_coloured_enum() {
        let value = sample_value("B", "Beta");
        let mut out = Vec::new();
        print_enum(&mut out, &value, 0, palette()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("B"));
        assert!(s.contains("Beta"));
    }

    #[test]
    fn print_enum_columns_respects_layout_columns() {
        let values = [sample_value("C", "Gamma"), sample_value("A", "Alpha")];
        let refs: Vec<&Value> = values.iter().collect();
        let mut out = Vec::new();
        let layout = ColumnLayout {
            column_width: 12,
            columns: 2,
            max_indent: 0,
        };
        print_enum_columns(&mut out, &refs, 0, palette(), Some(layout)).unwrap();
        let s = String::from_utf8(out).unwrap();
        // Two entries sorted and rendered in at most two lines.
        assert!(s.contains("A"));
        assert!(s.contains("C"));
        assert!(s.lines().count() <= 2);
    }

    #[test]
    fn compute_values_layout_uses_max_entry() {
        let values = [sample_value("LONG", "desc"), sample_value("S", "short")];
        let refs: Vec<&Value> = values.iter().collect();
        let layout = compute_values_layout(&refs, 4).expect("layout expected");
        assert!(layout.column_width >= "LONG: desc".len());
        assert!(layout.columns >= 1);
    }

    #[test]
    fn render_message_includes_header_and_trailer() {
        let schema = schema_with_structures();
        let msg = schema.messages.get("NewOrder").unwrap();
        let mut out = Vec::new();
        let style = DisplayStyle::new(palette(), true);
        let mut ctx = RenderContext::new(&mut out, &schema, style, true);
        ctx.render_message(msg, true, true, 0).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("Component:"));
        assert!(s.contains("Header"));
        assert!(s.contains("Message: "));
        assert!(s.contains("Body"));
        assert!(s.contains("Allocs"));
        assert!(s.contains("201")); // group count tag number
        assert!(s.contains("Trailer"));
    }

    #[test]
    fn render_component_prints_matching_msg_type_enum_only() {
        let schema = schema_with_structures();
        let msg = schema.messages.get("NewOrder").unwrap();
        let header = schema.components.get("Header").unwrap();
        let mut out = Vec::new();
        let mut ctx =
            RenderContext::new(&mut out, &schema, DisplayStyle::new(palette(), false), true);
        ctx.render_component_with_style(Some(msg), header, 0, DisplayStyle::new(palette(), false))
            .unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("NewOrderSingle"));
        assert!(!s.contains("ExecutionReport"));
    }

    #[test]
    fn render_message_preserves_interleaved_container_order() {
        use std::collections::BTreeMap;
        use std::sync::Arc;

        crate::decoder::colours::disable_colours();

        let before = Arc::new(Field {
            name: "BeforeField".into(),
            number: 11,
            field_type: "STRING".into(),
            values: Vec::new(),
            values_wrapper: ValuesWrapper::default(),
        });
        let after = Arc::new(Field {
            name: "AfterField".into(),
            number: 22,
            field_type: "STRING".into(),
            values: Vec::new(),
            values_wrapper: ValuesWrapper::default(),
        });
        let component = ComponentNode {
            name: "Block".into(),
            fields: vec![FieldNode {
                required: false,
                field: after.clone(),
            }],
            groups: Vec::new(),
            components: Vec::new(),
            entries: vec![ContainerNode::Field(FieldNode {
                required: false,
                field: after.clone(),
            })],
        };
        let message = MessageNode {
            name: "Ordered".into(),
            msg_type: "Z".into(),
            msg_cat: "app".into(),
            fields: vec![
                FieldNode {
                    required: true,
                    field: before.clone(),
                },
                FieldNode {
                    required: false,
                    field: after.clone(),
                },
            ],
            components: vec![component.clone()],
            groups: Vec::new(),
            entries: vec![
                ContainerNode::Field(FieldNode {
                    required: true,
                    field: before.clone(),
                }),
                ContainerNode::Component(component),
                ContainerNode::Field(FieldNode {
                    required: false,
                    field: after.clone(),
                }),
            ],
        };
        let mut fields = BTreeMap::new();
        fields.insert(before.name.clone(), before);
        fields.insert(after.name.clone(), after);
        let mut components = BTreeMap::new();
        components.insert("Block".into(), message.components[0].clone());
        let schema = SchemaTree {
            fields,
            components,
            messages: BTreeMap::from([(message.name.clone(), message.clone())]),
            version: "FIX 4.4".into(),
            service_pack: "-".into(),
        };

        let mut out = Vec::new();
        let mut ctx = RenderContext::new(
            &mut out,
            &schema,
            DisplayStyle::new(palette(), false),
            false,
        );
        ctx.render_message(&message, false, false, 0).unwrap();

        let rendered = String::from_utf8(out).unwrap();
        let before_pos = rendered.find("BeforeField").expect("before field present");
        let component_pos = rendered
            .find("Component: ")
            .expect("component label present");
        let after_pos = rendered.find("AfterField").expect("after field present");

        assert!(
            before_pos < component_pos && component_pos < after_pos,
            "rendered order should follow message entries: {rendered}"
        );
    }

    #[test]
    fn cached_layout_is_reused_for_component() {
        let schema = schema_with_structures();
        let component = schema.components.get("Block").unwrap();
        let mut out = Vec::new();
        let mut ctx =
            RenderContext::new(&mut out, &schema, DisplayStyle::new(palette(), true), true);
        let first = ctx.cached_component_layout(component, 2);
        let second = ctx.cached_component_layout(component, 2);
        assert_eq!(first, second);
        assert_eq!(ctx.layout_cache.len(), 1);
    }

    #[test]
    fn compute_message_layout_counts_header_and_trailer() {
        let schema = schema_with_structures();
        let msg = schema.messages.get("NewOrder").unwrap();
        let layout =
            compute_message_layout(&schema, msg, true, true, 0).expect("layout should be computed");
        assert!(layout.column_width > 0);
        assert!(layout.columns >= 1);
    }

    #[test]
    fn collect_group_layout_counts_nested_components() {
        let field = sample_field_node(false);
        let component = ComponentNode {
            name: "Comp".into(),
            fields: vec![field.clone()],
            groups: Vec::new(),
            components: Vec::new(),
            entries: vec![ContainerNode::Field(field.clone())],
        };
        let group = GroupNode {
            name: "Group".into(),
            required: false,
            fields: vec![field.clone()],
            components: vec![component.clone()],
            groups: Vec::new(),
            entries: vec![
                ContainerNode::Field(field.clone()),
                ContainerNode::Component(component),
            ],
        };
        let mut stats = LayoutStats::default();
        collect_group_layout(&group, 0, &mut stats);
        assert!(stats.max_entry_len > 0);
    }

    #[test]
    fn tag_and_message_cells_include_expected_text() {
        let colours = palette();
        let tag = tag_cell(1, "Test", "INT", true, colours);
        assert!(tag.text.contains("Test"));
        let msg = MessageNode {
            name: "Heartbeat".into(),
            msg_type: "0".into(),
            msg_cat: "app".into(),
            fields: Vec::new(),
            components: Vec::new(),
            groups: Vec::new(),
            entries: Vec::new(),
        };
        let cell = message_cell(&msg, colours);
        assert!(cell.text.contains("Heartbeat"));
    }

    #[test]
    fn visible_len_ignores_escape_sequences() {
        let text = "\u{1b}[33mhello\u{1b}[0m";
        assert_eq!(visible_len(text), 5);
    }

    #[test]
    fn write_with_padding_adds_spaces() {
        let mut out = Vec::new();
        write_with_padding(&mut out, 3, 5, |w| write!(w, "hey")).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.ends_with("  "));
    }
}

fn print_field(
    out: &mut dyn Write,
    field: &FieldNode,
    indent_level: usize,
    colours: ColourPalette,
) -> io::Result<()> {
    writeln!(
        out,
        "{}{}{:width$}{}: {}{}{} ({}{}{}){}",
        indent(indent_level),
        colours.tag,
        field.field.number,
        colours.reset,
        colours.name,
        field.field.name,
        colours.reset,
        colours.value,
        field.field.field_type,
        colours.reset,
        format_required(field.required, colours),
        width = TAG_WIDTH
    )
}

fn print_enum(
    out: &mut dyn Write,
    value: &Value,
    indent_level: usize,
    colours: ColourPalette,
) -> io::Result<()> {
    writeln!(
        out,
        "{}{}{}{} : {}{}{}",
        indent(indent_level + 4),
        colours.value,
        value.enumeration,
        colours.reset,
        colours.enumeration,
        value.description,
        colours.reset
    )
}

fn print_enum_columns(
    out: &mut dyn Write,
    values: &[&Value],
    indent_level: usize,
    colours: ColourPalette,
    layout: Option<ColumnLayout>,
) -> io::Result<()> {
    if values.is_empty() {
        return Ok(());
    }

    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.enumeration.cmp(&b.enumeration));

    let layout_params = determine_enum_layout(indent_level, layout, &sorted);
    let rows = sorted.len().div_ceil(layout_params.cols);

    for row in 0..rows {
        for col in 0..layout_params.cols {
            let idx = col * rows + row;
            if idx >= sorted.len() {
                continue;
            }
            write_enum_cell(
                out,
                colours,
                indent_level,
                sorted[idx],
                col,
                layout_params.col_width,
                layout_params.extra_pad,
            )?;
        }
        writeln!(out)?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct EnumLayout {
    cols: usize,
    col_width: usize,
    extra_pad: usize,
}

fn determine_enum_layout(
    indent_level: usize,
    layout: Option<ColumnLayout>,
    values: &[&Value],
) -> EnumLayout {
    if let Some(layout) = layout {
        return EnumLayout {
            cols: layout.columns.max(1),
            col_width: layout.column_width.max(1),
            extra_pad: layout.max_indent.saturating_sub(indent_level),
        };
    }

    let max_len = values
        .iter()
        .map(|v| v.enumeration.len() + 2 + v.description.len())
        .max()
        .unwrap_or(0);
    let usable_width = terminal_width().saturating_sub(indent_level);
    let cols = cmp::max(1, usable_width / (max_len + 2));

    EnumLayout {
        cols,
        col_width: max_len + 2,
        extra_pad: 0,
    }
}

fn write_enum_cell(
    out: &mut dyn Write,
    colours: ColourPalette,
    indent_level: usize,
    value: &Value,
    col: usize,
    col_width: usize,
    extra_pad: usize,
) -> io::Result<()> {
    let plain_width = value.enumeration.len() + 2 + value.description.len();
    let (visible, width) = if col == 0 {
        let indent_adjust = indent_level + extra_pad;
        (plain_width + indent_adjust, col_width + indent_adjust)
    } else {
        (plain_width, col_width)
    };

    write_with_padding(out, visible, width, |inner| {
        if col == 0 {
            write!(inner, "{}", indent(indent_level))?;
            if extra_pad > 0 {
                write!(inner, "{:pad$}", "", pad = extra_pad)?;
            }
        }
        write!(
            inner,
            "{}{}{}: {}{}{}",
            colours.value,
            value.enumeration,
            colours.reset,
            colours.enumeration,
            value.description,
            colours.reset
        )
    })
}

#[derive(Clone)]
struct DisplayCell {
    text: String,
    width: usize,
}

impl DisplayCell {
    /// Build a cell with precalculated visible width for column rendering.
    fn new(text: String) -> Self {
        let width = visible_len(&text);
        Self { text, width }
    }
}

fn message_cell(msg: &MessageNode, colours: ColourPalette) -> DisplayCell {
    DisplayCell::new(format!(
        "{}{:>2}{}: {}{}{} ({})",
        colours.tag,
        msg.msg_type,
        colours.reset,
        colours.name,
        msg.name,
        colours.reset,
        msg.msg_cat
    ))
}

fn component_cell(name: &str, colours: ColourPalette) -> DisplayCell {
    DisplayCell::new(format!("{}{}{}", colours.name, name, colours.reset))
}

fn tag_cell(
    number: u32,
    name: &str,
    ty: &str,
    required: bool,
    colours: ColourPalette,
) -> DisplayCell {
    DisplayCell::new(format!(
        "{}{:4}{}: {}{}{} ({}{}{}){}",
        colours.tag,
        number,
        colours.reset,
        colours.name,
        name,
        colours.reset,
        colours.value,
        ty,
        colours.reset,
        format_required(required, colours)
    ))
}

fn print_string_columns(items: &[DisplayCell]) -> io::Result<()> {
    if items.is_empty() {
        return Ok(());
    }

    let width = terminal_width();
    let max_len = items.iter().map(|s| s.width).max().unwrap_or(0);
    let cols = cmp::max(1, width / (max_len + 2));
    let rows = items.len().div_ceil(cols);

    let mut stdout = io::stdout().lock();
    for row in 0..rows {
        for col in 0..cols {
            let idx = col * rows + row;
            if idx < items.len() {
                write_with_padding(&mut stdout, items[idx].width, max_len + 2, |out| {
                    write!(out, "{}", items[idx].text)
                })?;
            }
        }
        writeln!(stdout)?;
    }
    Ok(())
}

#[derive(Hash, Eq, PartialEq)]
enum LayoutKind {
    Component,
    Group,
}

#[derive(Hash, Eq, PartialEq)]
struct LayoutCacheKey {
    indent: usize,
    kind: LayoutKind,
    name: String,
}

impl LayoutCacheKey {
    /// Build a cache key for a component layout at a given indentation level.
    fn component(name: &str, indent: usize) -> Self {
        Self {
            indent,
            kind: LayoutKind::Component,
            name: name.to_string(),
        }
    }

    /// Build a cache key for a group layout at a given indentation level.
    fn group(name: &str, indent: usize) -> Self {
        Self {
            indent,
            kind: LayoutKind::Group,
            name: name.to_string(),
        }
    }
}

/// Coordinates rendering to a `Write` sink whilst caching layouts and
/// enum buffers.  Designed to be short-lived per CLI action.
struct RenderContext<'a, 'b, W: Write> {
    out: &'a mut W,
    schema: &'b SchemaTree,
    verbose: bool,
    style: DisplayStyle,
    enum_buf: EnumBuffer<'b>,
    layout_cache: HashMap<LayoutCacheKey, ColumnLayout>,
}

impl<'a, 'b, W: Write> RenderContext<'a, 'b, W> {
    /// Create a new rendering context bound to an output sink and schema.
    fn new(out: &'a mut W, schema: &'b SchemaTree, style: DisplayStyle, verbose: bool) -> Self {
        Self {
            out,
            schema,
            verbose,
            style,
            enum_buf: Vec::new(),
            layout_cache: HashMap::new(),
        }
    }

    /// Render a single message definition, optionally including header and
    /// trailer blocks.  Responsible for kicking off component/group rendering.
    fn render_message(
        &mut self,
        msg: &'b MessageNode,
        include_header: bool,
        include_trailer: bool,
        indent_level: usize,
    ) -> io::Result<()> {
        let colours = self.style.colours();
        writeln!(
            self.out,
            "Message: {}{}{} ({}{}{})",
            colours.name, msg.name, colours.reset, colours.tag, msg.msg_type, colours.reset
        )?;

        let shared_style = if self.verbose && self.style.columns_enabled() {
            let schema = self.schema;
            self.style.ensure_layout(|| {
                compute_message_layout(schema, msg, include_header, include_trailer, indent_level)
            })
        } else {
            self.style
        };

        if include_header && let Some(header) = self.schema.components.get("Header") {
            self.render_component_with_style(Some(msg), header, indent_level, shared_style)?;
        }

        writeln!(
            self.out,
            "{}Message: {}Body{}",
            indent(indent_level),
            colours.name,
            colours.reset
        )?;

        self.render_entries(
            Some(msg),
            &msg.entries,
            indent_level + NEST_INDENT,
            shared_style,
        )?;

        if include_trailer && let Some(trailer) = self.schema.components.get("Trailer") {
            self.render_component_with_style(Some(msg), trailer, indent_level, shared_style)?;
        }
        Ok(())
    }

    /// Render a component, respecting shared layout state when verbose column
    /// mode is enabled.
    fn render_component(
        &mut self,
        msg: Option<&'b MessageNode>,
        component: &'b ComponentNode,
        indent_level: usize,
    ) -> io::Result<()> {
        self.render_component_with_style(msg, component, indent_level, self.style)
    }

    fn render_component_with_style(
        &mut self,
        msg: Option<&'b MessageNode>,
        component: &'b ComponentNode,
        indent_level: usize,
        style: DisplayStyle,
    ) -> io::Result<()> {
        let style = self.resolve_component_style(component, indent_level, style);
        self.render_component_header(component, indent_level, style)?;
        self.render_entries(msg, &component.entries, indent_level + NEST_INDENT, style)?;
        Ok(())
    }

    /// Render a repeating group (and any nested structures) with shared
    /// column layouts to keep verbose output tidy.
    fn render_group(&mut self, group: &'b GroupNode, indent_level: usize) -> io::Result<()> {
        self.render_group_with_style(group, indent_level, self.style)
    }

    fn render_group_with_style(
        &mut self,
        group: &'b GroupNode,
        indent_level: usize,
        style: DisplayStyle,
    ) -> io::Result<()> {
        let style = self.resolve_group_style(group, indent_level, style);
        self.render_group_header(group, indent_level, style)?;
        self.render_entries(None, &group.entries, indent_level + NEST_INDENT, style)?;
        Ok(())
    }

    fn render_entries(
        &mut self,
        msg: Option<&'b MessageNode>,
        entries: &'b [ContainerNode],
        indent_level: usize,
        style: DisplayStyle,
    ) -> io::Result<()> {
        let mut visitor = RenderVisitor::new(self, msg, style);
        walk_container_nodes(entries, indent_level, NEST_INDENT, &mut visitor)
    }

    fn resolve_component_style(
        &mut self,
        component: &'b ComponentNode,
        indent_level: usize,
        style: DisplayStyle,
    ) -> DisplayStyle {
        if self.verbose && style.columns_enabled() && style.layout().is_none() {
            style.with_layout(self.cached_component_layout(component, indent_level))
        } else {
            style
        }
    }

    fn resolve_group_style(
        &mut self,
        group: &'b GroupNode,
        indent_level: usize,
        style: DisplayStyle,
    ) -> DisplayStyle {
        if self.verbose && style.columns_enabled() && style.layout().is_none() {
            style.with_layout(self.cached_group_layout(group, indent_level))
        } else {
            style
        }
    }

    fn render_component_header(
        &mut self,
        component: &'b ComponentNode,
        indent_level: usize,
        style: DisplayStyle,
    ) -> io::Result<()> {
        let colours = style.colours();
        writeln!(
            self.out,
            "{}Component: {}{}{}",
            indent(indent_level),
            colours.name,
            component.name,
            colours.reset
        )
    }

    fn render_group_header(
        &mut self,
        group: &'b GroupNode,
        indent_level: usize,
        style: DisplayStyle,
    ) -> io::Result<()> {
        let colours = style.colours();
        if let Some(count_field) = self.schema.fields.get(&group.name) {
            let count_node = FieldNode {
                required: group.required,
                field: count_field.clone(),
            };
            print_field(self.out, &count_node, indent_level, colours)
        } else {
            writeln!(
                self.out,
                "{}Group: {}{}{}{}",
                indent(indent_level),
                colours.name,
                group.name,
                colours.reset,
                format_required(group.required, colours)
            )
        }
    }

    fn print_enums_for_field(
        &mut self,
        field: &'b FieldNode,
        msg: Option<&'b MessageNode>,
        indent_level: usize,
        style: DisplayStyle,
    ) -> io::Result<()> {
        let colours = style.colours();
        if field.field.number == 35
            && let Some(message) = msg
        {
            for value in field.field.values_iter() {
                if value.enumeration == message.msg_type {
                    writeln!(
                        self.out,
                        "{}{}{}{} : {}{}{}",
                        indent(indent_level + 4),
                        colours.value,
                        value.enumeration,
                        colours.reset,
                        colours.enumeration,
                        value.description,
                        colours.reset
                    )?;
                    return Ok(());
                }
            }
        }

        if style.columns_enabled() {
            let values = collect_sorted_values(&mut self.enum_buf, field.field.values_iter());
            print_enum_columns(self.out, values, indent_level, colours, style.layout())?;
        } else {
            for value in field.field.values_iter() {
                print_enum(self.out, value, indent_level, colours)?;
            }
        }
        Ok(())
    }

    fn cached_component_layout(
        &mut self,
        component: &'b ComponentNode,
        indent: usize,
    ) -> Option<ColumnLayout> {
        let key = LayoutCacheKey::component(&component.name, indent);
        if let Some(layout) = self.layout_cache.get(&key) {
            return Some(*layout);
        }
        let layout = compute_component_layout(component, indent);
        if let Some(value) = layout {
            self.layout_cache.insert(key, value);
        }
        layout
    }

    fn cached_group_layout(&mut self, group: &'b GroupNode, indent: usize) -> Option<ColumnLayout> {
        let key = LayoutCacheKey::group(&group.name, indent);
        if let Some(layout) = self.layout_cache.get(&key) {
            return Some(*layout);
        }
        let layout = compute_group_layout(group, indent);
        if let Some(value) = layout {
            self.layout_cache.insert(key, value);
        }
        layout
    }
}

struct RenderVisitor<'ctx, 'out, 'schema, W: Write> {
    ctx: &'ctx mut RenderContext<'out, 'schema, W>,
    msg: Option<&'schema MessageNode>,
    style_stack: Vec<DisplayStyle>,
}

impl<'ctx, 'out, 'schema, W: Write> RenderVisitor<'ctx, 'out, 'schema, W> {
    fn new(
        ctx: &'ctx mut RenderContext<'out, 'schema, W>,
        msg: Option<&'schema MessageNode>,
        style: DisplayStyle,
    ) -> Self {
        Self {
            ctx,
            msg,
            style_stack: vec![style],
        }
    }

    fn current_style(&self) -> DisplayStyle {
        *self
            .style_stack
            .last()
            .expect("render visitor always has an active style")
    }
}

impl<'ctx, 'out, 'schema, W: Write> ContainerNodeVisitor<'schema>
    for RenderVisitor<'ctx, 'out, 'schema, W>
{
    type Error = io::Error;

    fn visit_field(
        &mut self,
        field: &'schema FieldNode,
        indent_level: usize,
    ) -> Result<(), Self::Error> {
        let style = self.current_style();
        let colours = style.colours();
        print_field(self.ctx.out, field, indent_level, colours)?;
        if self.ctx.verbose {
            self.ctx
                .print_enums_for_field(field, self.msg, indent_level + 2, style)?;
        }
        Ok(())
    }

    fn enter_component(
        &mut self,
        component: &'schema ComponentNode,
        indent_level: usize,
    ) -> Result<(), Self::Error> {
        let style = self
            .ctx
            .resolve_component_style(component, indent_level, self.current_style());
        self.ctx
            .render_component_header(component, indent_level, style)?;
        self.style_stack.push(style);
        Ok(())
    }

    fn leave_component(
        &mut self,
        _component: &'schema ComponentNode,
        _indent_level: usize,
    ) -> Result<(), Self::Error> {
        self.style_stack.pop();
        Ok(())
    }

    fn enter_group(
        &mut self,
        group: &'schema GroupNode,
        indent_level: usize,
    ) -> Result<(), Self::Error> {
        let style = self
            .ctx
            .resolve_group_style(group, indent_level, self.current_style());
        self.ctx.render_group_header(group, indent_level, style)?;
        self.style_stack.push(style);
        Ok(())
    }

    fn leave_group(
        &mut self,
        _group: &'schema GroupNode,
        _indent_level: usize,
    ) -> Result<(), Self::Error> {
        self.style_stack.pop();
        Ok(())
    }
}

pub fn print_message_columns(schema: &SchemaTree) -> io::Result<()> {
    let colours = palette();
    let mut entries: Vec<_> = schema.messages.values().collect();
    entries.sort_by(|a, b| a.msg_type.cmp(&b.msg_type));
    let cells: Vec<_> = entries
        .iter()
        .map(|msg| message_cell(msg, colours))
        .collect();
    print_string_columns(&cells)
}

/// Print component names in columns for quick scanning.
/// Print components in column form, primarily used by `--component` listings.
pub fn print_component_columns(schema: &SchemaTree) -> io::Result<()> {
    let colours = palette();
    let mut names: Vec<_> = schema.components.keys().cloned().collect();
    names.sort();
    let cells: Vec<_> = names
        .iter()
        .map(|name| component_cell(name, colours))
        .collect();
    print_string_columns(&cells)
}

/// List all messages with MsgType and name, one per line.
pub fn list_all_messages(schema: &SchemaTree) -> io::Result<()> {
    let colours = palette();
    let mut entries: Vec<_> = schema.messages.values().collect();
    entries.sort_by(|a, b| a.msg_type.cmp(&b.msg_type));

    let mut stdout = io::stdout().lock();
    for msg in entries {
        let cell = message_cell(msg, colours);
        writeln!(stdout, "{}", cell.text)?;
    }
    Ok(())
}

/// List all components by name, one per line.
pub fn list_all_components(schema: &SchemaTree) -> io::Result<()> {
    let colours = palette();
    let mut names: Vec<_> = schema.components.keys().cloned().collect();
    names.sort();

    let mut stdout = io::stdout().lock();
    for name in names {
        let cell = component_cell(&name, colours);
        writeln!(stdout, "{}", cell.text)?;
    }
    Ok(())
}

/// List all tags (fields) in numeric order, one per line.
pub fn list_all_tags(schema: &SchemaTree) -> io::Result<()> {
    let colours = palette();
    let mut fields: Vec<_> = schema.fields.values().collect();
    fields.sort_by_key(|f| f.number);

    let mut stdout = io::stdout().lock();
    for field in fields {
        let cell = tag_cell(field.number, &field.name, &field.field_type, false, colours);
        writeln!(stdout, "{}", cell.text)?;
    }
    Ok(())
}

/// Print all tags in column form for compact display.
pub fn print_tags_in_columns(schema: &SchemaTree) -> io::Result<()> {
    let colours = palette();
    let mut fields: Vec<_> = schema.fields.values().collect();
    fields.sort_by_key(|f| f.number);

    let cells: Vec<_> = fields
        .iter()
        .map(|field| tag_cell(field.number, &field.name, &field.field_type, false, colours))
        .collect();

    print_string_columns(&cells)
}

/// Print details for a single tag, optionally including its enum values.
pub fn print_tag_details(field: &Field, verbose: bool, columns: bool) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    print_tag_details_with_writer(&mut handle, field, verbose, columns)
}

fn print_tag_details_with_writer(
    out: &mut dyn Write,
    field: &Field,
    verbose: bool,
    columns: bool,
) -> io::Result<()> {
    let colours = palette();
    let cell = tag_cell(field.number, &field.name, &field.field_type, false, colours);
    writeln!(out, "{}", cell.text)?;

    if verbose {
        if columns {
            let mut buf = Vec::new();
            let values = collect_sorted_values(&mut buf, field.values_iter());
            let layout = compute_values_layout(values, 4);
            print_enum_columns(out, values, 4, colours, layout)?;
        } else {
            for value in field.values_iter() {
                print_enum(out, value, 4, colours)?;
            }
        }
    }
    Ok(())
}

/// Display a message definition with optional header/trailer and enum verbosity.
pub fn display_message(
    schema: &SchemaTree,
    msg: &MessageNode,
    verbose: bool,
    include_header: bool,
    include_trailer: bool,
    indent_level: usize,
    style: DisplayStyle,
) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    RenderContext::new(&mut handle, schema, style, verbose).render_message(
        msg,
        include_header,
        include_trailer,
        indent_level,
    )
}

pub fn display_component(
    schema: &SchemaTree,
    msg: Option<&MessageNode>,
    component: &ComponentNode,
    verbose: bool,
    indent_level: usize,
    style: DisplayStyle,
) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    RenderContext::new(&mut handle, schema, style, verbose).render_component(
        msg,
        component,
        indent_level,
    )
}

#[allow(dead_code)]
/// Display a group tree with the chosen style/verbosity.
pub fn display_group(
    schema: &SchemaTree,
    group: &GroupNode,
    verbose: bool,
    indent_level: usize,
    style: DisplayStyle,
) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    RenderContext::new(&mut handle, schema, style, verbose).render_group(group, indent_level)
}

fn compute_message_layout(
    schema: &SchemaTree,
    msg: &MessageNode,
    include_header: bool,
    include_trailer: bool,
    indent_level: usize,
) -> Option<ColumnLayout> {
    let mut stats = LayoutStats::default();
    collect_container_layout(&msg.entries, indent_level + NEST_INDENT, &mut stats);
    if include_header && let Some(header) = schema.components.get("Header") {
        collect_component_layout(header, indent_level, &mut stats);
    }
    if include_trailer && let Some(trailer) = schema.components.get("Trailer") {
        collect_component_layout(trailer, indent_level, &mut stats);
    }
    stats.finalize()
}

fn compute_component_layout(
    component: &ComponentNode,
    indent_level: usize,
) -> Option<ColumnLayout> {
    let mut stats = LayoutStats::default();
    collect_component_layout(component, indent_level, &mut stats);
    stats.finalize()
}

fn compute_group_layout(group: &GroupNode, indent_level: usize) -> Option<ColumnLayout> {
    let mut stats = LayoutStats::default();
    collect_group_layout(group, indent_level, &mut stats);
    stats.finalize()
}

fn compute_values_layout(values: &[&Value], indent_level: usize) -> Option<ColumnLayout> {
    if values.is_empty() {
        return None;
    }
    let mut stats = LayoutStats::default();
    let max_entry = values
        .iter()
        .map(|v| v.enumeration.len() + 2 + v.description.len())
        .max()
        .unwrap_or(0);
    stats.record(max_entry, indent_level);
    stats.finalize()
}

fn collect_component_layout(
    component: &ComponentNode,
    indent_level: usize,
    stats: &mut LayoutStats,
) {
    collect_container_layout(&component.entries, indent_level + NEST_INDENT, stats);
}

fn collect_group_layout(group: &GroupNode, indent_level: usize, stats: &mut LayoutStats) {
    collect_container_layout(&group.entries, indent_level + NEST_INDENT, stats);
}

fn collect_container_layout(
    entries: &[ContainerNode],
    indent_level: usize,
    stats: &mut LayoutStats,
) {
    let mut visitor = LayoutVisitor { stats };
    walk_container_nodes(entries, indent_level, NEST_INDENT, &mut visitor)
        .expect("layout collection cannot fail");
}

fn field_layout_width(field: &FieldNode) -> usize {
    field
        .field
        .values_iter()
        .map(|value| value.enumeration.len() + 2 + value.description.len())
        .max()
        .unwrap_or(0)
}

struct LayoutVisitor<'a> {
    stats: &'a mut LayoutStats,
}

impl<'a> ContainerNodeVisitor<'a> for LayoutVisitor<'_> {
    type Error = Infallible;

    fn visit_field(
        &mut self,
        field: &'a FieldNode,
        indent_level: usize,
    ) -> Result<(), Self::Error> {
        self.stats
            .record(field_layout_width(field), indent_level + 2);
        Ok(())
    }

    fn enter_component(
        &mut self,
        _component: &'a ComponentNode,
        _indent_level: usize,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn enter_group(
        &mut self,
        _group: &'a GroupNode,
        _indent_level: usize,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[allow(dead_code)]
/// Print a one-line schema summary (counts + version information) to stdout.
pub fn print_schema_summary(schema: &SchemaTree) {
    let mut stdout = io::stdout().lock();
    let _ = writeln!(
        stdout,
        "Fields: {}   Components: {}   Messages: {}   Version: {}  Service Pack: {}",
        schema.fields.len(),
        schema.components.len(),
        schema.messages.len(),
        schema.version,
        schema.service_pack
    );
}

fn visible_len(text: &str) -> usize {
    let mut len = 0;
    let mut in_escape = false;

    for ch in text.chars() {
        if in_escape {
            if ch == 'm' {
                in_escape = false;
            }
            continue;
        }

        if ch == '\x1b' {
            in_escape = true;
            continue;
        }

        len += 1;
    }

    len
}

fn write_with_padding<F>(
    out: &mut dyn Write,
    visible_len: usize,
    width: usize,
    render: F,
) -> io::Result<()>
where
    F: FnOnce(&mut dyn Write) -> io::Result<()>,
{
    render(out)?;
    if width > visible_len {
        let pad = width - visible_len;
        write!(out, "{:width$}", "", width = pad)?;
    }
    Ok(())
}
