// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2025 Steve Clarke <stephenlclarke@mac.com> - https://xyzzy.tools

use crate::decoder::colours::palette;
use crate::decoder::display::{pad_ansi, visible_width};
use crate::decoder::fixparser::parse_fix;
use crate::decoder::tag_lookup::{
    FixTagLookup, clear_override_cache_for, load_dictionary_with_override,
};
use chrono::{Datelike, Duration, NaiveDate};
use std::collections::{HashMap, hash_map::Entry};
use std::io::Write;

/// Captures FIX order lifecycles while streaming messages so a concise summary
/// can be rendered after processing input.
#[derive(Default)]
pub struct OrderSummary {
    orders: HashMap<String, OrderRecord>,
    aliases: HashMap<String, String>,
    unknown_counter: usize,
    completed: Vec<OrderRecord>,
    total_orders: usize,
    terminal_orders: usize,
    footer_width: usize,
    fix_override_key: Option<String>,
    display_delimiter: char,
}

#[derive(Debug, Clone)]
struct OrderRecord {
    key: String,
    order_id: Option<String>,
    cl_ord_id: Option<String>,
    orig_cl_ord_id: Option<String>,
    symbol: Option<String>,
    side: Option<String>,
    qty: Option<String>,
    cum_qty: Option<String>,
    leaves_qty: Option<String>,
    avg_px: Option<String>,
    ord_type: Option<String>,
    time_in_force: Option<String>,
    trade_date: Option<String>,
    settl_date: Option<String>,
    settl_date2: Option<String>,
    currency: Option<String>,
    ord_type_desc: Option<String>,
    tif_desc: Option<String>,
    order_qty_name: Option<String>,
    cum_qty_name: Option<String>,
    leaves_qty_name: Option<String>,
    avg_px_name: Option<String>,
    ord_type_name: Option<String>,
    tif_name: Option<String>,
    trade_date_name: Option<String>,
    settl_date_name: Option<String>,
    settl_date2_name: Option<String>,
    ord_type_code: Option<String>,
    tif_code: Option<String>,
    price: Option<String>,
    spot_rate: Option<String>,
    spot_rate_name: Option<String>,
    last_qty: Option<String>,
    bn_seen: bool,
    bn_exec_amt: Option<String>,
    events: Vec<OrderEvent>,
    messages: Vec<String>,
}

#[derive(Debug, Clone)]
struct OrderEvent {
    time: Option<String>,
    msg_type: Option<String>,
    msg_type_desc: Option<String>,
    exec_type: Option<String>,
    ord_status: Option<String>,
    exec_ack_status: Option<String>,
    state: String,
    cum_qty: Option<String>,
    leaves_qty: Option<String>,
    last_qty: Option<String>,
    last_px: Option<String>,
    avg_px: Option<String>,
    text: Option<String>,
    cl_ord_id: Option<String>,
    orig_cl_ord_id: Option<String>,
}

impl OrderSummary {
    pub fn new(display_delimiter: char) -> Self {
        Self {
            display_delimiter,
            ..Self::default()
        }
    }

    pub fn record_message(&mut self, msg: &str, fix_override: Option<&str>) {
        let fields = parse_fix(msg);
        if fields.is_empty() {
            return;
        }
        if let Some(key) = fix_override {
            self.fix_override_key.get_or_insert_with(|| key.to_string());
        }

        let mut map = HashMap::new();
        for field in &fields {
            map.insert(field.tag, field.value.clone());
        }

        let order_id = map.get(&37).cloned();
        let cl_ord_id = map.get(&11).cloned();
        let orig_cl_ord_id = map.get(&41).cloned();

        let key = self.resolve_key(
            order_id.as_deref(),
            cl_ord_id.as_deref(),
            orig_cl_ord_id.as_deref(),
        );
        let dict = load_dictionary_with_override(msg, fix_override);
        self.note_aliases(&key, order_id, cl_ord_id, orig_cl_ord_id);
        let record = match self.orders.entry(key.clone()) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => {
                if let Some(pos) = self.completed.iter().position(|r| r.key == key) {
                    let rec = self.completed.remove(pos);
                    if rec.is_terminal() && self.terminal_orders > 0 {
                        self.terminal_orders -= 1;
                    }
                    v.insert(rec)
                } else {
                    self.total_orders += 1;
                    v.insert(OrderRecord::new(key.clone()))
                }
            }
        };

        record.merge_ids(
            map.get(&37).cloned(),
            map.get(&11).cloned(),
            map.get(&41).cloned(),
        );
        record.absorb_fields(&map, &dict, map.get(&35).map(|s| s.as_str()));

        let event = OrderEvent::from_fields(&map, &dict);
        record.events.push(event);
        record
            .messages
            .push(display_with_delimiter(msg, self.display_delimiter));

        if record.is_terminal() {
            self.completed.push(record.clone());
            self.orders.remove(&key);
            self.terminal_orders += 1;
        }
    }

    /// Render and clear any completed orders to allow streaming output in summary-only mode.
    pub fn render(&self, out: &mut dyn Write) -> std::io::Result<()> {
        let colours = palette();
        let mut keys: Vec<&String> = self.orders.keys().collect();
        keys.sort();
        let open = self.orders.len();
        let total = self.total_orders;

        if self.footer_width > 0 {
            writeln!(out, "\r{}", " ".repeat(self.footer_width))?;
        }

        for record in &self.completed {
            self.render_record(out, record)?;
            self.render_messages(out, record)?;
        }

        for key in keys {
            let record = &self.orders[key];
            self.render_record(out, record)?;
        }

        let res = writeln!(
            out,
            "{}Order Summary{} ({} open, {} total, to fill: {}/{})\n",
            colours.title, colours.reset, open, total, open, total
        );
        if !self.completed.is_empty() {
            self.clear_override_cache();
        }
        res
    }

    /// Render only newly completed orders and clear them. Returns true if anything was printed.
    pub fn render_completed(&mut self, out: &mut dyn Write) -> std::io::Result<bool> {
        if self.completed.is_empty() {
            return Ok(false);
        }
        if self.footer_width > 0 {
            write!(out, "\r{}\r", " ".repeat(self.footer_width))?;
        }
        for record in &self.completed {
            self.render_record(out, record)?;
            self.render_messages(out, record)?;
        }
        self.clear_override_cache();
        self.completed.clear();
        out.flush()?;
        Ok(true)
    }

    pub fn render_footer(&mut self, out: &mut dyn Write) -> std::io::Result<()> {
        let line = format!(
            "Status: open={} filled={} total={}",
            self.orders.len(),
            self.terminal_orders,
            self.total_orders
        );
        let width = visible_width(&line).max(self.footer_width);
        let pad = " ".repeat(width.saturating_sub(visible_width(&line)));
        write!(out, "\r{}{pad}", line)?;
        out.flush()?;
        self.footer_width = width;
        Ok(())
    }

    fn render_messages(&self, out: &mut dyn Write, record: &OrderRecord) -> std::io::Result<()> {
        if record.messages.is_empty() || !record.is_terminal() {
            return Ok(());
        }
        let colours = palette();
        writeln!(out, "    {}Raw FIX messages:{}", colours.tag, colours.reset)?;
        for msg in &record.messages {
            writeln!(out, "      {}{}{}", colours.line, msg, colours.reset)?;
        }
        writeln!(out)?;
        Ok(())
    }

    fn clear_override_cache(&self) {
        if let Some(key) = &self.fix_override_key {
            clear_override_cache_for(key);
        }
    }

    fn render_record(&self, out: &mut dyn Write, record: &OrderRecord) -> std::io::Result<()> {
        let colours = palette();
        render_record_header(out, record, colours)?;
        let (headers, values) = build_summary_row(record, colours);
        render_table_row(out, &headers, &values)?;

        writeln!(out)?;
        render_timeline(out, record, colours)?;
        writeln!(out)?;

        Ok(())
    }

    fn resolve_key(
        &mut self,
        order_id: Option<&str>,
        cl_ord_id: Option<&str>,
        orig: Option<&str>,
    ) -> String {
        for candidate in [order_id, cl_ord_id, orig].into_iter().flatten() {
            if let Some(key) = self.aliases.get(candidate) {
                return key.clone();
            }
        }

        if let Some(id) = order_id.or(cl_ord_id) {
            return id.to_string();
        }

        self.unknown_counter += 1;
        format!("UNKNOWN-{}", self.unknown_counter)
    }

    fn note_aliases(
        &mut self,
        key: &str,
        order_id: Option<String>,
        cl_ord_id: Option<String>,
        orig: Option<String>,
    ) {
        for id in [order_id, cl_ord_id, orig].into_iter().flatten() {
            self.aliases.entry(id).or_insert_with(|| key.to_string());
        }
    }
}

fn render_record_header(
    out: &mut dyn Write,
    record: &OrderRecord,
    colours: crate::decoder::colours::ColourPalette,
) -> std::io::Result<()> {
    writeln!(
        out,
        "  {}{}{} [{}{}{}] {}",
        colours.file,
        record.display_id(),
        colours.reset,
        colours.name,
        flow_label(&record.state_path()),
        colours.reset,
        colour_instrument(record.display_instrument()),
    )
}

fn build_summary_row(
    record: &OrderRecord,
    colours: crate::decoder::colours::ColourPalette,
) -> (Vec<&str>, Vec<String>) {
    let qty_label = record.order_qty_name.as_deref().unwrap_or("qty");
    let value_date =
        preferred_settl_date(record.settl_date.as_deref(), record.settl_date2.as_deref());
    let date_diff = date_diff_days(record.trade_date.as_deref(), value_date);

    let mut headers = vec![
        "Side",
        "Symbol",
        qty_label,
        "Price",
        record.trade_date_name.as_deref().unwrap_or("TradeDate"),
        "Tenor",
        record.tif_name.as_deref().unwrap_or("TimeInForce"),
        record.ord_type_name.as_deref().unwrap_or("OrdType"),
    ];
    let mut values = vec![
        colour_enum_text(
            colours,
            record
                .side
                .as_deref()
                .map(side_label)
                .map(|s| s.to_ascii_uppercase()),
        ),
        colour_value(colours, record.symbol.as_deref().unwrap_or("-")),
        colour_value(colours, record.qty.as_deref().unwrap_or("-")),
        format_price(colours, record.price.as_deref(), record.currency.as_deref()),
        colour_value(colours, record.trade_date.as_deref().unwrap_or("-")),
        format_tenor(colours, date_diff),
        colour_enum_text(colours, record.tif_desc.as_deref().map(|s| s.to_string())),
        colour_enum_text(
            colours,
            record.ord_type_desc.as_deref().map(|s| s.to_string()),
        ),
    ];

    if record.bn_seen {
        headers.push(record.spot_rate_name.as_deref().unwrap_or("SpotPrice"));
        headers.push("ExecAmt");
        values.push(colour_value(
            colours,
            record.spot_rate.as_deref().unwrap_or("-"),
        ));
        let exec_amt = record.bn_exec_amt.as_deref();
        values.push(colour_value(colours, exec_amt.unwrap_or("-")));
    }

    headers.push(settlement_header(record));
    values.push(colour_value(colours, value_date.unwrap_or("-")));

    (headers, values)
}

fn settlement_header(record: &OrderRecord) -> &str {
    if record.settl_date2.is_some() {
        record.settl_date2_name.as_deref().unwrap_or("SettlDate2")
    } else if record.settl_date.is_some() {
        record.settl_date_name.as_deref().unwrap_or("SettlDate")
    } else {
        record
            .settl_date2_name
            .as_deref()
            .or(record.settl_date_name.as_deref())
            .unwrap_or("ValueDate")
    }
}

fn render_timeline(
    out: &mut dyn Write,
    record: &OrderRecord,
    colours: crate::decoder::colours::ColourPalette,
) -> std::io::Result<()> {
    writeln!(out, "    {}Timeline:{}", colours.tag, colours.reset)?;
    let rendered_msgs: Vec<String> = record
        .events
        .iter()
        .map(|ev| format_msg_cell(colours, ev))
        .collect();
    let msg_width = rendered_msgs
        .iter()
        .map(|s| visible_width(s))
        .max()
        .unwrap_or(0)
        .max(42usize);

    let headers = build_timeline_headers(record, msg_width);
    render_timeline_headers(out, &headers, colours)?;

    for (ev, msg_cell) in record.events.iter().zip(rendered_msgs.iter()) {
        let cells = build_timeline_cells(record, ev, msg_cell, msg_width, colours);
        writeln!(out, "      {}{}", colours.line, cells.join(" "))?;
    }

    Ok(())
}

fn build_timeline_headers(record: &OrderRecord, msg_width: usize) -> Vec<(&'static str, usize)> {
    let mut timeline_headers = vec![
        ("time", 22usize),
        ("msg", msg_width),
        ("ExecType", 18),
        ("OrdStatus", 18),
        ("cum/leaves", 18),
        ("last@price", 18),
        ("avgPx", 10),
        ("text", 0),
    ];
    if record.bn_seen {
        timeline_headers.insert(2, ("ExecAckStatus", 18));
    }
    timeline_headers
}

fn render_timeline_headers(
    out: &mut dyn Write,
    headers: &[(&str, usize)],
    colours: crate::decoder::colours::ColourPalette,
) -> std::io::Result<()> {
    write!(out, "      ")?;
    for (label, width) in headers {
        let w = if *width == 0 { label.len() + 2 } else { *width };
        let coloured = format!("{}{}{}", colours.name, label, colours.reset);
        write!(out, "{} ", pad_ansi(&coloured, w))?;
    }
    writeln!(out)
}

fn build_timeline_cells(
    record: &OrderRecord,
    event: &OrderEvent,
    msg_cell: &str,
    msg_width: usize,
    colours: crate::decoder::colours::ColourPalette,
) -> Vec<String> {
    let time = event.time.as_deref().unwrap_or("-");
    let exec = colour_label_code(colours, event.exec_label(), event.exec_type.as_deref());
    let ord = colour_label_code(colours, event.ord_label(), event.ord_status.as_deref());
    let exec_ack = event
        .exec_ack_status
        .as_deref()
        .map(|code| colour_label_code(colours, label_exec_ack_status(Some(code)), Some(code)))
        .unwrap_or_else(|| colour_label_code(colours, "Unknown".to_string(), None));
    let last = format!(
        "{}{}@{}{}",
        colours.value,
        event.last_qty.as_deref().unwrap_or("-"),
        event.last_px.as_deref().unwrap_or("-"),
        colours.reset
    );
    let cum_leaves = format!(
        "{}{}/{}{}",
        colours.value,
        event.cum_qty.as_deref().unwrap_or("-"),
        event.leaves_qty.as_deref().unwrap_or("-"),
        colours.reset
    );

    let mut cells = Vec::new();
    cells.push(pad_ansi(
        &format!("{}{}{}", colours.value, time, colours.reset),
        22,
    ));
    cells.push(pad_ansi(msg_cell, msg_width));
    if record.bn_seen {
        cells.push(pad_ansi(&exec_ack, 18));
    }
    cells.push(pad_ansi(&exec, 18));
    cells.push(pad_ansi(&ord, 18));
    cells.push(pad_ansi(&cum_leaves, 18));
    cells.push(pad_ansi(&last, 18));
    cells.push(pad_ansi(
        &colour_value(colours, event.avg_px.as_deref().unwrap_or("-")),
        10,
    ));
    cells.push(pad_ansi(
        &colour_text(colours, event.text.as_deref().unwrap_or("")),
        0,
    ));

    cells
}

fn flow_label(states: &[String]) -> String {
    if states.is_empty() {
        return "Unknown".to_string();
    }
    let trimmed = if states.len() > 1 && states.first().map(|s| s.as_str()) == Some("Unknown") {
        states.iter().skip(1).cloned().collect::<Vec<_>>()
    } else {
        states.to_vec()
    };
    if trimmed.is_empty() {
        "Unknown".to_string()
    } else {
        trimmed.join(" -> ")
    }
}

impl OrderRecord {
    fn new(key: String) -> Self {
        Self {
            key,
            order_id: None,
            cl_ord_id: None,
            orig_cl_ord_id: None,
            symbol: None,
            side: None,
            qty: None,
            cum_qty: None,
            leaves_qty: None,
            avg_px: None,
            ord_type: None,
            time_in_force: None,
            trade_date: None,
            settl_date: None,
            settl_date2: None,
            currency: None,
            ord_type_desc: None,
            tif_desc: None,
            price: None,
            spot_rate: None,
            spot_rate_name: None,
            last_qty: None,
            bn_seen: false,
            bn_exec_amt: None,
            order_qty_name: None,
            cum_qty_name: None,
            leaves_qty_name: None,
            avg_px_name: None,
            ord_type_name: None,
            tif_name: None,
            trade_date_name: None,
            settl_date_name: None,
            settl_date2_name: None,
            ord_type_code: None,
            tif_code: None,
            events: Vec::new(),
            messages: Vec::new(),
        }
    }

    fn is_terminal(&self) -> bool {
        if let Some(state) = self.state_path().last()
            && matches!(
                state.as_str(),
                "Filled"
                    | "Canceled"
                    | "Rejected"
                    | "Done for Day"
                    | "Expired"
                    | "Stopped"
                    | "Suspended"
                    | "Calculated"
            )
        {
            return true;
        }

        if let Some(exec_ack) = self
            .events
            .iter()
            .rev()
            .find_map(|e| e.exec_ack_status.as_deref())
            && matches!(exec_ack, "1" | "3" | "4")
        {
            return true;
        }

        false
    }

    fn merge_ids(
        &mut self,
        order_id: Option<String>,
        cl_ord_id: Option<String>,
        orig: Option<String>,
    ) {
        if self.order_id.is_none() {
            self.order_id = order_id;
        }
        if self.cl_ord_id.is_none() {
            self.cl_ord_id = cl_ord_id;
        }
        if self.orig_cl_ord_id.is_none() {
            self.orig_cl_ord_id = orig;
        }
    }

    fn absorb_fields(
        &mut self,
        fields: &HashMap<u32, String>,
        dict: &FixTagLookup,
        msg_type: Option<&str>,
    ) {
        self.copy_core_fields(fields, dict);
        self.copy_enum_fields(fields, dict);
        self.copy_trade_and_settlement(fields, dict);
        if msg_type == Some("BN") {
            self.absorb_block_notice(fields, dict);
        }
    }

    fn copy_core_fields(&mut self, fields: &HashMap<u32, String>, dict: &FixTagLookup) {
        Self::set_value(&mut self.symbol, fields.get(&55));
        Self::set_value(&mut self.side, fields.get(&54));
        Self::set_named_field(&mut self.qty, &mut self.order_qty_name, fields, dict, 38);
        Self::set_value(&mut self.currency, fields.get(&15));
        Self::set_value(&mut self.last_qty, fields.get(&32));
        Self::set_named_field(&mut self.cum_qty, &mut self.cum_qty_name, fields, dict, 14);
        Self::set_named_field(
            &mut self.leaves_qty,
            &mut self.leaves_qty_name,
            fields,
            dict,
            151,
        );
        Self::set_named_field(&mut self.avg_px, &mut self.avg_px_name, fields, dict, 6);
        Self::set_value(&mut self.price, fields.get(&44));
        if let Some(spot) = fields.get(&190) {
            self.spot_rate = Some(spot.clone());
            self.spot_rate_name
                .get_or_insert_with(|| dict.field_name(190));
        }
    }

    fn copy_enum_fields(&mut self, fields: &HashMap<u32, String>, dict: &FixTagLookup) {
        Self::set_enum_field(
            &mut self.ord_type,
            &mut self.ord_type_code,
            &mut self.ord_type_desc,
            &mut self.ord_type_name,
            fields,
            dict,
            40,
        );
        Self::set_enum_field(
            &mut self.time_in_force,
            &mut self.tif_code,
            &mut self.tif_desc,
            &mut self.tif_name,
            fields,
            dict,
            59,
        );
    }

    fn copy_trade_and_settlement(&mut self, fields: &HashMap<u32, String>, dict: &FixTagLookup) {
        if let Some(trd60) = fields.get(&60) {
            // When TradeDate (75) is absent, derive a business-date value from
            // TransactTime (60) so tenor calculations still work.
            let date = extract_date_part(trd60).unwrap_or_else(|| trd60.clone());
            Self::set_value(&mut self.trade_date, Some(&date));
            self.trade_date_name
                .get_or_insert_with(|| dict.field_name(75));
        }
        if let Some(trd75) = fields.get(&75) {
            self.trade_date = Some(trd75.clone());
            self.trade_date_name = Some(dict.field_name(75));
        }
        if let Some(s64) = fields.get(&64) {
            Self::set_value(&mut self.settl_date, Some(s64));
            self.settl_date_name
                .get_or_insert_with(|| dict.field_name(64));
        }
        if let Some(s193) = fields.get(&193) {
            Self::set_value(&mut self.settl_date2, Some(s193));
            self.settl_date2_name
                .get_or_insert_with(|| dict.field_name(193));
        }
    }

    fn absorb_block_notice(&mut self, fields: &HashMap<u32, String>, dict: &FixTagLookup) {
        self.bn_seen = true;
        if let Some(last_px) = fields.get(&31) {
            self.spot_rate = Some(last_px.clone());
            self.spot_rate_name
                .get_or_insert_with(|| dict.field_name(31));
        }
        if let Some(exec_amt) = fields.get(&38) {
            self.bn_exec_amt = Some(exec_amt.clone());
        }
    }

    fn set_value(target: &mut Option<String>, value: Option<&String>) {
        if let Some(val) = value {
            *target = Some(val.clone());
        }
    }

    fn set_named_field(
        target: &mut Option<String>,
        name_slot: &mut Option<String>,
        fields: &HashMap<u32, String>,
        dict: &FixTagLookup,
        tag: u32,
    ) {
        if let Some(val) = fields.get(&tag) {
            *target = Some(val.clone());
            name_slot.get_or_insert_with(|| dict.field_name(tag));
        }
    }

    fn set_enum_field(
        target: &mut Option<String>,
        code_slot: &mut Option<String>,
        desc_slot: &mut Option<String>,
        name_slot: &mut Option<String>,
        fields: &HashMap<u32, String>,
        dict: &FixTagLookup,
        tag: u32,
    ) {
        if let Some(val) = fields.get(&tag) {
            *target = Some(enum_label(dict, tag, val));
            *code_slot = Some(val.clone());
            name_slot.get_or_insert_with(|| dict.field_name(tag));
            if let Some(desc) = dict.enum_description(tag, val) {
                *desc_slot = Some(desc.to_ascii_uppercase());
            }
        }
    }

    fn state_path(&self) -> Vec<String> {
        let mut states = Vec::new();
        for ev in &self.events {
            if let Some(last) = states.last()
                && last == &ev.state
            {
                continue;
            }
            states.push(ev.state.clone());
        }
        states
    }

    fn display_id(&self) -> String {
        if let Some(order_id) = &self.order_id {
            return order_id.clone();
        }
        if let Some(cl) = &self.cl_ord_id {
            return cl.clone();
        }
        self.key.clone()
    }

    fn display_instrument(&self) -> String {
        let side = self.side.as_deref().map(side_label).unwrap_or("-");
        let symbol = self.symbol.as_deref().unwrap_or("-");
        format!("{side} {symbol}")
    }
}

impl OrderEvent {
    fn from_fields(fields: &HashMap<u32, String>, dict: &FixTagLookup) -> Self {
        let exec_type = fields.get(&150).cloned();
        let ord_status = fields.get(&39).cloned();
        let exec_ack_status = fields.get(&1036).cloned();
        let leaves_qty = fields.get(&151).cloned();
        let state = derive_state(
            exec_type.as_deref(),
            ord_status.as_deref(),
            leaves_qty.as_deref(),
            exec_ack_status.as_deref(),
        );

        Self {
            time: fields
                .get(&60)
                .cloned()
                .or_else(|| fields.get(&52).cloned()),
            msg_type: fields.get(&35).cloned(),
            msg_type_desc: fields
                .get(&35)
                .and_then(|mt| dict.enum_description(35, mt).map(|d| d.to_string())),
            exec_type,
            ord_status,
            exec_ack_status,
            state,
            cum_qty: fields.get(&14).cloned(),
            leaves_qty,
            last_qty: fields.get(&32).cloned(),
            last_px: fields.get(&31).cloned(),
            avg_px: fields.get(&6).cloned(),
            text: fields.get(&58).cloned(),
            cl_ord_id: fields.get(&11).cloned(),
            orig_cl_ord_id: fields.get(&41).cloned(),
        }
    }

    fn exec_label(&self) -> String {
        label_exec_type(self.exec_type.as_deref())
    }

    fn ord_label(&self) -> String {
        label_ord_status(self.ord_status.as_deref())
    }
}

fn derive_state(
    exec_type: Option<&str>,
    ord_status: Option<&str>,
    leaves_qty: Option<&str>,
    exec_ack_status: Option<&str>,
) -> String {
    if let Some(label) = label_ord_status_raw(ord_status) {
        return label.to_string();
    }
    if let Some(label) = label_exec_type_raw(exec_type) {
        return label.to_string();
    }
    if let Some(label) = label_exec_ack_status_raw(exec_ack_status) {
        return label.to_string();
    }

    if let Some(leaves) = leaves_qty
        && leaves == "0"
    {
        return "Filled".to_string();
    }

    "Unknown".to_string()
}

fn label_ord_status_raw(value: Option<&str>) -> Option<&'static str> {
    match value.unwrap_or("") {
        "A" => Some("Pending New"),
        "0" => Some("New"),
        "1" => Some("Partially Filled"),
        "2" => Some("Filled"),
        "3" => Some("Done for Day"),
        "4" => Some("Canceled"),
        "5" => Some("Replaced"),
        "6" => Some("Pending Cancel"),
        "7" => Some("Stopped"),
        "8" => Some("Rejected"),
        "9" => Some("Suspended"),
        "B" => Some("Calculated"),
        "C" => Some("Expired"),
        "D" => Some("Accepted for Bidding"),
        "E" => Some("Pending Replace"),
        _ => None,
    }
}

fn label_exec_type_raw(value: Option<&str>) -> Option<&'static str> {
    match value.unwrap_or("") {
        "A" => Some("Pending New"),
        "0" => Some("New"),
        "1" => Some("Partially Filled"),
        "2" => Some("Filled"),
        "3" => Some("Done for Day"),
        "4" => Some("Canceled"),
        "5" => Some("Replaced"),
        "6" => Some("Pending Cancel"),
        "7" => Some("Stopped"),
        "8" => Some("Rejected"),
        "9" => Some("Suspended"),
        "C" => Some("Expired"),
        "E" => Some("Pending Replace"),
        "F" => Some("Trade"),
        "G" => Some("Trade Correct"),
        "H" => Some("Trade Cancel"),
        "I" => Some("Order Status"),
        _ => None,
    }
}

fn label_exec_ack_status_raw(value: Option<&str>) -> Option<&'static str> {
    match value.unwrap_or("") {
        "0" => Some("Received"),
        "1" => Some("Accepted"),
        "2" => Some("Dont Know"),
        "3" => Some("Rejected"),
        "4" => Some("Accepted With Errors"),
        _ => None,
    }
}

fn label_exec_type(value: Option<&str>) -> String {
    label_exec_type_raw(value).unwrap_or("Unknown").to_string()
}

fn label_ord_status(value: Option<&str>) -> String {
    label_ord_status_raw(value).unwrap_or("Unknown").to_string()
}

fn label_exec_ack_status(value: Option<&str>) -> String {
    label_exec_ack_status_raw(value)
        .unwrap_or("Unknown")
        .to_string()
}

fn side_label(value: &str) -> &'static str {
    match value {
        "1" => "Buy",
        "2" => "Sell",
        "5" => "SellShort",
        "6" => "SellShortExempt",
        "8" => "Cross",
        _ => "Side?",
    }
}

fn enum_label(dict: &FixTagLookup, tag: u32, value: &str) -> String {
    if let Some(desc) = dict.enum_description(tag, value) {
        let label = normalise_enum_desc(desc);
        return format!("{label} ({value})");
    }
    value.to_string()
}

fn normalise_enum_desc(desc: &str) -> String {
    let mut chars = desc.chars();
    if let Some(first) = chars.next() {
        let mut out = String::new();
        out.push(first.to_ascii_uppercase());
        out.extend(chars.map(|c| c.to_ascii_lowercase()));
        out
    } else {
        String::new()
    }
}

fn colour_value(colours: crate::decoder::colours::ColourPalette, value: &str) -> String {
    format!("{}{}{}", colours.value, value, colours.reset)
}

fn colour_text(colours: crate::decoder::colours::ColourPalette, value: &str) -> String {
    if value.is_empty() {
        return format!("{}-{}", colours.name, colours.reset);
    }
    format!("{}{}{}", colours.name, value, colours.reset)
}

fn colour_label_code(
    colours: crate::decoder::colours::ColourPalette,
    label: String,
    code: Option<&str>,
) -> String {
    if label != "Unknown" {
        return format!("{}{}{}", colours.enumeration, label, colours.reset);
    }
    let code = code.unwrap_or("-");
    format!("{}{}{}", colours.error, code, colours.reset)
}

fn format_price(
    colours: crate::decoder::colours::ColourPalette,
    price: Option<&str>,
    currency: Option<&str>,
) -> String {
    let Some(price) = price else {
        return colour_value(colours, "-");
    };
    if let Some(curr) = currency {
        return format!(
            "{}{}{} ({}{}{})",
            colours.value, price, colours.reset, colours.enumeration, curr, colours.reset
        );
    }
    colour_value(colours, price)
}

fn colour_enum_text(
    colours: crate::decoder::colours::ColourPalette,
    text: Option<String>,
) -> String {
    let val = text.unwrap_or_else(|| "-".to_string());
    format!("{}{}{}", colours.enumeration, val, colours.reset)
}

fn format_msg_cell(colours: crate::decoder::colours::ColourPalette, ev: &OrderEvent) -> String {
    let base = if let Some(desc) = ev.msg_type_desc.as_deref() {
        format!("{}{}{}", colours.enumeration, desc, colours.reset)
    } else if let Some(code) = ev.msg_type.as_deref() {
        format!("{}{}{}", colours.error, code, colours.reset)
    } else {
        format!("{}-{}", colours.error, colours.reset)
    };

    let mut ids = Vec::new();
    if let Some(cl) = ev.cl_ord_id.as_deref() {
        ids.push(format!("{}{}{}", colours.value, cl, colours.reset));
    }
    if let Some(orig) = ev.orig_cl_ord_id.as_deref() {
        ids.push(format!("{}{}{}", colours.value, orig, colours.reset));
    }
    if ids.is_empty() {
        return base;
    }
    let sep = format!("{},{}", colours.reset, colours.reset);
    let joined = ids.join(&sep);
    format!("{base} [{}{}{}]", colours.reset, joined, colours.reset)
}

fn format_tenor(colours: crate::decoder::colours::ColourPalette, diff: Option<i64>) -> String {
    let Some(days) = diff else {
        return colour_value(colours, "-");
    };
    let tenor = match days {
        0 => "TOD",
        1 => "TOM",
        2 => "SPOT",
        _ => "FWD",
    };
    format!(
        "{}T+{}{} ({}{}{})",
        colours.value, days, colours.reset, colours.enumeration, tenor, colours.reset
    )
}

fn display_with_delimiter(msg: &str, delimiter: char) -> String {
    const SOH: char = '\u{0001}';
    if delimiter == SOH {
        return msg.to_string();
    }
    msg.chars()
        .map(|c| if c == SOH { delimiter } else { c })
        .collect()
}

/// Compute business-day diff skipping only weekends (no holiday calendar).
fn date_diff_days(trade: Option<&str>, settl: Option<&str>) -> Option<i64> {
    let trade = NaiveDate::parse_from_str(trade?, "%Y%m%d").ok()?;
    let settl = NaiveDate::parse_from_str(settl?, "%Y%m%d").ok()?;
    if settl < trade {
        return None;
    }
    let mut days = 0i64;
    let mut cursor = trade;
    while cursor < settl {
        cursor = cursor.checked_add_signed(Duration::days(1))?;
        if is_business_day(cursor) {
            days += 1;
        }
    }
    Some(days)
}

fn preferred_settl_date<'a>(s64: Option<&'a str>, s193: Option<&'a str>) -> Option<&'a str> {
    s193.or(s64)
}

fn is_business_day(date: NaiveDate) -> bool {
    !matches!(date.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun)
}

fn extract_date_part(ts: &str) -> Option<String> {
    if ts.len() >= 8 && ts.chars().take(8).all(|c| c.is_ascii_digit()) {
        return Some(ts.chars().take(8).collect());
    }
    if let Some((prefix, _)) = ts.split_once('-')
        && prefix.len() == 8
        && prefix.chars().all(|c| c.is_ascii_digit())
    {
        return Some(prefix.to_string());
    }
    None
}

fn render_table_row(
    out: &mut dyn Write,
    headers: &[&str],
    values: &[String],
) -> std::io::Result<()> {
    let colours = palette();
    let mut widths = [8usize, 16, 14, 14, 12, 10, 18, 16, 12, 12, 12, 10];
    for (i, val) in values.iter().enumerate() {
        let w = visible_width(val);
        if let Some(slot) = widths.get_mut(i) {
            *slot = (*slot).max(w + 2);
        }
        if let Some(h) = headers.get(i) {
            let hw = visible_width(h);
            if let Some(slot) = widths.get_mut(i) {
                *slot = (*slot).max(hw + 2);
            }
        }
    }

    write!(out, "    ")?;
    for (i, head) in headers.iter().enumerate() {
        let w = widths.get(i).copied().unwrap_or(10);
        let coloured = format!("{}{}{}", colours.name, head, colours.reset);
        write!(out, "{} ", pad_ansi(&coloured, w))?;
    }
    writeln!(out)?;

    write!(out, "    ")?;
    for (i, val) in values.iter().enumerate() {
        let w = widths.get(i).copied().unwrap_or(10);
        write!(out, "{} ", pad_ansi(val, w))?;
    }
    writeln!(out)
}

fn colour_instrument(text: String) -> String {
    let colours = palette();
    // Apply value/yellow tone to side+symbol for parity with decoded FIX fields.
    format!("{}{}{}", colours.value, text, colours.reset)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOH: &str = "\u{0001}";

    fn msg(fields: &[(&str, &str)]) -> String {
        let mut out = String::new();
        for (tag, val) in fields {
            out.push_str(tag);
            out.push('=');
            out.push_str(val);
            out.push_str(SOH);
        }
        out
    }

    #[test]
    fn collects_states_for_single_order() {
        let mut summary = OrderSummary::new('\u{0001}');
        summary.record_message(
            &msg(&[
                ("35", "D"),
                ("11", "ABC"),
                ("55", "AAPL"),
                ("54", "1"),
                ("38", "100"),
                ("40", "2"),
                ("59", "0"),
                ("75", "20250101"),
                ("64", "20250103"),
                ("193", "20250104"),
            ]),
            None,
        );
        summary.record_message(
            &msg(&[
                ("35", "8"),
                ("11", "ABC"),
                ("150", "0"),
                ("39", "0"),
                ("55", "AAPL"),
                ("54", "1"),
                ("38", "100"),
                ("14", "0"),
                ("151", "100"),
            ]),
            None,
        );
        summary.record_message(
            &msg(&[
                ("35", "8"),
                ("11", "ABC"),
                ("150", "1"),
                ("39", "1"),
                ("55", "AAPL"),
                ("54", "1"),
                ("32", "40"),
                ("31", "10.00"),
                ("14", "40"),
                ("151", "60"),
            ]),
            None,
        );
        summary.record_message(
            &msg(&[
                ("35", "8"),
                ("11", "ABC"),
                ("150", "2"),
                ("39", "2"),
                ("55", "AAPL"),
                ("54", "1"),
                ("32", "60"),
                ("31", "10.10"),
                ("14", "100"),
                ("151", "0"),
                ("6", "10.06"),
            ]),
            None,
        );

        let record = summary
            .orders
            .get("ABC")
            .or_else(|| summary.completed.iter().find(|r| r.key == "ABC"))
            .expect("order captured");
        assert_eq!(
            record.state_path(),
            vec!["Unknown", "New", "Partially Filled", "Filled"]
        );
        assert_eq!(record.cum_qty.as_deref(), Some("100"));
        assert_eq!(record.leaves_qty.as_deref(), Some("0"));
        assert_eq!(record.ord_type.as_deref(), Some("Limit (2)"));
        assert_eq!(record.time_in_force.as_deref(), Some("Day (0)"));
        assert_eq!(record.trade_date.as_deref(), Some("20250101"));
        assert_eq!(record.settl_date.as_deref(), Some("20250103"));
        assert_eq!(record.settl_date2.as_deref(), Some("20250104"));
    }

    #[test]
    fn links_orders_using_order_id_and_cl_ord_id() {
        let mut summary = OrderSummary::new('\u{0001}');
        summary.record_message(
            &msg(&[
                ("35", "D"),
                ("11", "ABC"),
                ("55", "MSFT"),
                ("54", "2"),
                ("38", "50"),
                ("193", "20250106"),
            ]),
            None,
        );
        summary.record_message(
            &msg(&[
                ("35", "8"),
                ("37", "OID1"),
                ("11", "ABC"),
                ("150", "0"),
                ("39", "0"),
                ("38", "50"),
                ("151", "50"),
                ("75", "20250102"),
                ("193", "20250106"),
            ]),
            None,
        );
        summary.record_message(
            &msg(&[
                ("35", "8"),
                ("37", "OID1"),
                ("11", "DEF"),
                ("41", "ABC"),
                ("150", "5"),
                ("39", "5"),
                ("38", "75"),
                ("151", "75"),
            ]),
            None,
        );

        assert_eq!(summary.orders.len(), 1, "replacements should merge");
        let record = summary.orders.values().next().unwrap();
        assert_eq!(record.display_id(), "OID1");
        assert_eq!(record.qty.as_deref(), Some("75"));
        assert_eq!(
            date_diff_days(
                record.trade_date.as_deref(),
                preferred_settl_date(record.settl_date.as_deref(), record.settl_date2.as_deref())
            ),
            Some(2)
        );
    }

    #[test]
    fn transact_time_falls_back_to_trade_date_for_tenor_math() {
        let mut summary = OrderSummary::new('\u{0001}');
        summary.record_message(
            &msg(&[
                ("35", "D"),
                ("11", "ABC"),
                ("55", "EUR/USD"),
                ("54", "1"),
                ("60", "20250102-12:34:56.000"),
                ("193", "20250106"),
            ]),
            None,
        );

        let record = summary.orders.get("ABC").expect("order captured");
        assert_eq!(record.trade_date.as_deref(), Some("20250102"));
        assert_eq!(
            date_diff_days(
                record.trade_date.as_deref(),
                preferred_settl_date(record.settl_date.as_deref(), record.settl_date2.as_deref())
            ),
            Some(2)
        );
    }

    #[test]
    fn render_outputs_state_headline() {
        let mut summary = OrderSummary::new('\u{0001}');
        summary.record_message(
            &msg(&[
                ("35", "D"),
                ("11", "XYZ"),
                ("55", "GBP/USD"),
                ("54", "1"),
                ("38", "10"),
            ]),
            None,
        );
        summary.record_message(
            &msg(&[("35", "8"), ("11", "XYZ"), ("150", "4"), ("39", "4")]),
            None,
        );

        let mut buf = Vec::new();
        summary.render(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(
            text.contains("Canceled"),
            "state headline should include final status: {text}"
        );
        assert!(text.contains("XYZ"), "order id should be present: {text}");
    }

    #[test]
    fn bn_message_sets_state_and_spot_price() {
        let mut summary = OrderSummary::new('\u{0001}');
        summary.record_message(
            &msg(&[
                ("35", "BN"),
                ("11", "OID1"),
                ("55", "EUR/USD"),
                ("54", "1"),
                ("38", "1000000"),
                ("31", "1.2345"),
                ("1036", "1"),
            ]),
            None,
        );

        let record = summary
            .orders
            .get("OID1")
            .or_else(|| summary.completed.iter().find(|r| r.key == "OID1"))
            .expect("bn order captured");
        assert_eq!(record.state_path(), vec!["Accepted"]);
        assert_eq!(record.spot_rate.as_deref(), Some("1.2345"));
        assert!(record.bn_seen, "bn flag should be set");
        assert_eq!(record.bn_exec_amt.as_deref(), Some("1000000"));
    }

    #[test]
    fn terminal_status_from_non_exec_report_updates_header() {
        let mut summary = OrderSummary::new('\u{0001}');
        summary.record_message(
            &msg(&[
                ("35", "D"),
                ("11", "OID1"),
                ("55", "IBM"),
                ("54", "1"),
                ("38", "200"),
            ]),
            None,
        );
        summary.record_message(
            &msg(&[
                ("35", "9"), // Order Cancel Reject, treated as terminal via OrdStatus
                ("11", "OID1"),
                ("39", "4"),  // Canceled
                ("14", "50"), // CumQty
                ("151", "0"), // LeavesQty
                ("32", "50"),
                ("31", "10.00"),
            ]),
            None,
        );

        let record = summary
            .orders
            .get("OID1")
            .or_else(|| summary.completed.iter().find(|r| r.key == "OID1"))
            .expect("order captured");
        assert_eq!(
            record.leaves_qty.as_deref(),
            Some("0"),
            "terminal non-8 message should overwrite leaves"
        );
        assert_eq!(record.cum_qty.as_deref(), Some("50"));
        assert_eq!(
            record.state_path().last().cloned().unwrap_or_default(),
            "Canceled"
        );
    }

    #[test]
    fn absorb_fields_sets_core_values() {
        let dict = crate::decoder::tag_lookup::load_dictionary(
            "8=FIX.4.4\u{0001}35=D\u{0001}10=000\u{0001}",
        );
        let mut record = OrderRecord::new("KEY".into());
        let mut fields = HashMap::new();
        fields.insert(55u32, "AAPL".to_string());
        fields.insert(54u32, "1".to_string());
        fields.insert(38u32, "100".to_string());
        fields.insert(14u32, "10".to_string());
        fields.insert(151u32, "90".to_string());
        fields.insert(6u32, "12.3".to_string());
        fields.insert(44u32, "15.0".to_string());
        record.absorb_fields(&fields, &dict, Some("D"));
        assert_eq!(record.symbol.as_deref(), Some("AAPL"));
        assert_eq!(record.qty.as_deref(), Some("100"));
        assert_eq!(record.cum_qty_name.as_deref(), Some("CumQty"));
        assert_eq!(record.leaves_qty.as_deref(), Some("90"));
        assert_eq!(record.price.as_deref(), Some("15.0"));
    }

    #[test]
    fn absorb_fields_sets_block_notice_specifics() {
        let dict = crate::decoder::tag_lookup::load_dictionary(
            "8=FIX.4.4\u{0001}35=BN\u{0001}10=000\u{0001}",
        );
        let mut record = OrderRecord::new("KEY".into());
        let mut fields = HashMap::new();
        fields.insert(31u32, "1.2345".to_string());
        fields.insert(38u32, "500".to_string());
        record.absorb_fields(&fields, &dict, Some("BN"));
        assert!(record.bn_seen);
        assert_eq!(record.spot_rate.as_deref(), Some("1.2345"));
        assert_eq!(record.bn_exec_amt.as_deref(), Some("500"));
    }

    #[test]
    fn flow_label_skips_leading_unknown() {
        let states = [
            "Unknown".to_string(),
            "New".to_string(),
            "Filled".to_string(),
        ];
        let flow = flow_label(&states);
        assert_eq!(flow, "New -> Filled");
    }

    #[test]
    fn build_summary_row_includes_bn_headers() {
        let colours = palette();
        let mut record = OrderRecord::new("KEY".into());
        record.bn_seen = true;
        record.spot_rate = Some("1.25".into());
        record.bn_exec_amt = Some("1000".into());
        let (headers, values) = build_summary_row(&record, colours);
        assert!(headers.contains(&"ExecAmt"));
        assert!(values.iter().any(|v| v.contains("1.25")));
    }

    #[test]
    fn render_record_header_includes_id_and_instrument() {
        let colours = palette();
        let mut record = OrderRecord::new("ORD123".into());
        record.symbol = Some("AAPL".into());
        record.side = Some("1".into());
        let mut out = Vec::new();
        render_record_header(&mut out, &record, colours).unwrap();
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("ORD123"));
        assert!(output.contains("AAPL"));
    }

    #[test]
    fn resolve_key_prefers_alias_then_ids() {
        let mut summary = OrderSummary::new('|');
        summary.aliases.insert("ALIAS".into(), "RESOLVED".into());
        // alias hit
        assert_eq!(
            summary.resolve_key(Some("ALIAS"), Some("OTHER"), None),
            "RESOLVED"
        );
        // order_id fallback
        assert_eq!(
            summary.resolve_key(Some("OID"), Some("CLID"), None),
            "OID".to_string()
        );
        // unknown increments counter
        let unk = summary.resolve_key(None, None, None);
        assert!(unk.starts_with("UNKNOWN-"));
    }

    #[test]
    fn display_instrument_formats_side_and_symbol() {
        let mut record = OrderRecord::new("KEY".into());
        record.side = Some("2".into());
        record.symbol = Some("MSFT".into());
        assert_eq!(record.display_instrument(), "Sell MSFT");
    }

    #[test]
    fn preferred_settlement_date_prefers_primary_then_secondary() {
        assert_eq!(
            preferred_settl_date(Some("20250101"), Some("20250102")),
            Some("20250102")
        );
        assert_eq!(
            preferred_settl_date(None, Some("20250102")),
            Some("20250102")
        );
        assert_eq!(preferred_settl_date(None, None), None);
    }

    #[test]
    fn extract_date_part_handles_timestamp() {
        assert_eq!(
            extract_date_part("20250101-12:00:01.000"),
            Some("20250101".into())
        );
        assert_eq!(extract_date_part(""), None);
    }

    #[test]
    fn date_diff_days_returns_none_when_incomplete() {
        assert_eq!(date_diff_days(None, Some("20250101")), None);
        assert_eq!(date_diff_days(Some("20250101"), None), None);
    }

    #[test]
    fn state_path_deduplicates_consecutive_states() {
        let mut record = OrderRecord::new("KEY".into());
        record.events.push(OrderEvent {
            time: None,
            msg_type: None,
            msg_type_desc: None,
            exec_type: Some("0".into()),
            ord_status: None,
            exec_ack_status: None,
            state: "New".into(),
            cum_qty: None,
            leaves_qty: None,
            last_qty: None,
            last_px: None,
            avg_px: None,
            text: None,
            cl_ord_id: None,
            orig_cl_ord_id: None,
        });
        record.events.push(OrderEvent {
            state: "New".into(),
            ..record.events[0].clone()
        });
        record.events.push(OrderEvent {
            state: "Filled".into(),
            ..record.events[0].clone()
        });
        assert_eq!(record.state_path(), vec!["New", "Filled"]);
    }
}
