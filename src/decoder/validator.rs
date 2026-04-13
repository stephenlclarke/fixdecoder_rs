// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2025 Steve Clarke <stephenlclarke@mac.com> - https://xyzzy.tools

use crate::decoder::fixparser::{FieldValue, parse_fix};
use crate::decoder::tag_lookup::{FixTagLookup, GroupSpec as MessageDefGroupSpec, MessageDef};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default)]
pub struct ValidationReport {
    pub errors: Vec<String>,
    pub tag_errors: HashMap<u32, Vec<String>>,
}

impl ValidationReport {
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Validate a single FIX message string against the provided dictionary,
/// returning a list of human-readable errors (or empty when valid).
pub fn validate_fix_message(msg: &str, dict: &FixTagLookup) -> ValidationReport {
    let fields = parse_fix(msg);
    let (field_map, seen_tags) = build_field_map(&fields);
    let mut errors = Vec::new();
    let mut tag_errors: HashMap<u32, Vec<String>> = HashMap::new();

    let (msg_type_errs, msg_def_opt) = validate_msg_type(&field_map, dict, &mut tag_errors);
    errors.extend(msg_type_errs);
    if let Some(msg_def) = msg_def_opt {
        errors.extend(validate_duplicate_fields(&fields, msg_def, &mut tag_errors));
    } else {
        errors.extend(validate_duplicate_fields_without_message(
            &fields,
            dict,
            &mut tag_errors,
        ));
    }
    errors.extend(validate_body_length(msg, &field_map, &mut tag_errors));
    errors.extend(validate_field_enums_and_types(
        &fields,
        dict,
        &mut tag_errors,
    ));

    if let Some(msg_def) = msg_def_opt {
        errors.extend(validate_required_fields(
            &msg_def.required,
            &seen_tags,
            dict,
            &mut tag_errors,
        ));
        errors.extend(validate_field_ordering(
            &fields,
            &msg_def.field_order,
            &mut tag_errors,
        ));
        errors.extend(validate_repeating_groups(
            &fields,
            msg_def,
            dict,
            &mut tag_errors,
        ));
    }
    errors.extend(validate_checksum_field(msg, &field_map, &mut tag_errors));

    ValidationReport { errors, tag_errors }
}

fn build_field_map(fields: &[FieldValue]) -> (HashMap<u32, String>, HashSet<u32>) {
    let mut field_map = HashMap::new();
    let mut seen = HashSet::new();
    for field in fields {
        seen.insert(field.tag);
        field_map.insert(field.tag, field.value.clone());
    }
    (field_map, seen)
}

fn validate_duplicate_fields_without_message(
    fields: &[FieldValue],
    dict: &FixTagLookup,
    tag_errors: &mut HashMap<u32, Vec<String>>,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut errors = Vec::new();

    for field in fields {
        if !seen.insert(field.tag) && !dict.is_repeatable(field.tag) {
            push_duplicate_error(field.tag, &mut errors, tag_errors);
        }
    }

    errors
}

fn validate_duplicate_fields(
    fields: &[FieldValue],
    msg_def: &MessageDef,
    tag_errors: &mut HashMap<u32, Vec<String>>,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut errors = Vec::new();
    let mut idx = 0;

    while idx < fields.len() {
        let tag = fields[idx].tag;
        if !seen.insert(tag) {
            push_duplicate_error(tag, &mut errors, tag_errors);
        }

        if let Some(spec) = msg_def.groups.get(&tag) {
            let (consumed, mut group_errors) =
                collect_group_duplicate_errors(fields, idx, spec, tag_errors);
            errors.append(&mut group_errors);
            idx += consumed.max(1);
        } else {
            idx += 1;
        }
    }

    errors
}

fn collect_group_duplicate_errors(
    fields: &[FieldValue],
    start_idx: usize,
    spec: &MessageDefGroupSpec,
    tag_errors: &mut HashMap<u32, Vec<String>>,
) -> (usize, Vec<String>) {
    let count = fields[start_idx].value.parse::<usize>().unwrap_or_default();
    let mut errors = Vec::new();
    let mut entries = 0usize;
    let mut idx = start_idx + 1;

    while idx < fields.len() && entries < count {
        let tag = fields[idx].tag;
        let belongs_to_entry = tag == spec.delim
            || spec.entry_tag_set.contains(&tag)
            || spec.nested.contains_key(&tag);
        if !belongs_to_entry {
            break;
        }

        let (consumed, mut entry_errors) =
            collect_group_entry_duplicate_errors(fields, idx, spec, tag_errors);
        errors.append(&mut entry_errors);
        idx += consumed.max(1);
        entries += 1;
    }

    (idx - start_idx, errors)
}

fn collect_group_entry_duplicate_errors(
    fields: &[FieldValue],
    start_idx: usize,
    spec: &MessageDefGroupSpec,
    tag_errors: &mut HashMap<u32, Vec<String>>,
) -> (usize, Vec<String>) {
    let mut seen = HashSet::new();
    let mut errors = Vec::new();
    let mut idx = start_idx;

    while idx < fields.len() {
        let tag = fields[idx].tag;
        if tag == spec.delim && idx != start_idx {
            break;
        }

        if let Some(nested) = spec.nested.get(&tag) {
            if !seen.insert(tag) {
                push_duplicate_error(tag, &mut errors, tag_errors);
            }
            let (consumed, mut nested_errors) =
                collect_group_duplicate_errors(fields, idx, nested, tag_errors);
            errors.append(&mut nested_errors);
            idx += consumed.max(1);
            continue;
        }

        if !spec.entry_tag_set.contains(&tag) {
            break;
        }

        if !seen.insert(tag) {
            push_duplicate_error(tag, &mut errors, tag_errors);
        }
        idx += 1;
    }

    (idx - start_idx, errors)
}

fn push_duplicate_error(
    tag: u32,
    errors: &mut Vec<String>,
    tag_errors: &mut HashMap<u32, Vec<String>>,
) {
    let err = format!("Duplicate tag {} encountered", tag);
    errors.push(err.clone());
    tag_errors.entry(tag).or_default().push(err);
}

fn validate_msg_type<'a>(
    field_map: &HashMap<u32, String>,
    dict: &'a FixTagLookup,
    tag_errors: &mut HashMap<u32, Vec<String>>,
) -> (Vec<String>, Option<&'a MessageDef>) {
    match field_map.get(&35) {
        None => {
            let err = "Missing required tag 35 (MsgType)".to_string();
            tag_errors.entry(35).or_default().push(err.clone());
            (vec![err], None)
        }
        Some(msg_type) => match dict.message_def(msg_type) {
            Some(def) => (Vec::new(), Some(def)),
            None => {
                let err = format!("Unknown MsgType: {}", msg_type);
                tag_errors.entry(35).or_default().push(err.clone());
                (vec![err], None)
            }
        },
    }
}

fn validate_required_fields(
    required: &[u32],
    seen_tags: &HashSet<u32>,
    dict: &FixTagLookup,
    tag_errors: &mut HashMap<u32, Vec<String>>,
) -> Vec<String> {
    let mut errors = Vec::new();
    for tag in required {
        if !seen_tags.contains(tag) {
            let err = format!("Missing required tag {} ({})", tag, dict.field_name(*tag));
            errors.push(err.clone());
            tag_errors.entry(*tag).or_default().push(err);
        }
    }
    errors
}

fn validate_field_enums_and_types(
    fields: &[FieldValue],
    dict: &FixTagLookup,
    tag_errors: &mut HashMap<u32, Vec<String>>,
) -> Vec<String> {
    let mut errors = Vec::new();
    for field in fields {
        let presence = dict.tag_presence(field.tag);
        if !presence.in_primary && !presence.in_fallback {
            let err = if let Some(fbk) = presence.fallback_key {
                format!(
                    "Unknown tag {} in FIX {} and FIX {}",
                    field.tag, presence.primary_key, fbk
                )
            } else {
                format!("Unknown tag {} in FIX {}", field.tag, presence.primary_key)
            };
            errors.push(err.clone());
            tag_errors.entry(field.tag).or_default().push(err);
            continue;
        }

        if presence.in_primary
            && !presence.in_fallback
            && matches!(
                presence.fallback_role,
                Some(crate::decoder::tag_lookup::FallbackKind::DetectedOverride)
            )
            && let Some(fbk) = presence.fallback_key
        {
            let err = format!(
                "Tag {} is defined in override FIX {} but unknown in detected FIX {}",
                field.tag, presence.primary_key, fbk
            );
            errors.push(err.clone());
            tag_errors.entry(field.tag).or_default().push(err);
        }

        if let Some(enums) = dict.enums_for(field.tag)
            && !enums.contains_key(&field.value)
        {
            let err = format!("Invalid enum value '{}'", field.value);
            errors.push(err.clone());
            tag_errors.entry(field.tag).or_default().push(err);
        }

        if let Some(field_type) = dict.field_type(field.tag)
            && !is_valid_type(&field.value, field_type)
        {
            let err = format!(
                "Invalid type: expected {}, got '{}'",
                field_type, field.value
            );
            errors.push(err.clone());
            tag_errors.entry(field.tag).or_default().push(err);
        }
    }
    errors
}

fn validate_field_ordering(
    fields: &[FieldValue],
    expected_order: &[u32],
    tag_errors: &mut HashMap<u32, Vec<String>>,
) -> Vec<String> {
    let mut order_index = HashMap::new();
    for (idx, tag) in expected_order.iter().enumerate() {
        order_index.insert(*tag, idx);
    }

    let mut errors = Vec::new();
    let mut last_index = -1isize;
    for field in fields {
        if let Some(&idx) = order_index.get(&field.tag) {
            let idx = idx as isize;
            if idx < last_index {
                let err = format!("Tag {} out of order", field.tag);
                errors.push(err.clone());
                tag_errors.entry(field.tag).or_default().push(err);
            }
            last_index = idx;
        }
    }
    errors
}

fn validate_repeating_groups(
    fields: &[FieldValue],
    msg_def: &MessageDef,
    dict: &FixTagLookup,
    tag_errors: &mut HashMap<u32, Vec<String>>,
) -> Vec<String> {
    let mut errors = Vec::new();
    let mut idx = 0;
    while idx < fields.len() {
        let tag = fields[idx].tag;
        if let Some(spec) = msg_def.groups.get(&tag) {
            let (consumed, mut errs) =
                validate_group_instance(fields, idx, spec, msg_def, dict, tag_errors);
            errors.append(&mut errs);
            idx += consumed;
        } else {
            if let Some(owner) = msg_def.group_membership.get(&tag) {
                let err = format!(
                    "Tag {} ({}) appears outside of repeating group {}",
                    tag,
                    dict.field_name(tag),
                    owner
                );
                errors.push(err.clone());
                tag_errors.entry(tag).or_default().push(err);
            }
            idx += 1;
        }
    }
    errors
}

fn validate_group_instance(
    fields: &[FieldValue],
    start_idx: usize,
    spec: &MessageDefGroupSpec,
    msg_def: &MessageDef,
    dict: &FixTagLookup,
    tag_errors: &mut HashMap<u32, Vec<String>>,
) -> (usize, Vec<String>) {
    let mut errors = Vec::new();
    let count = fields[start_idx]
        .value
        .parse::<usize>()
        .unwrap_or_else(|_| {
            let err = format!(
                "Invalid NumInGroup value '{}' for tag {}",
                fields[start_idx].value, spec.count_tag
            );
            errors.push(err.clone());
            tag_errors
                .entry(spec.count_tag)
                .or_default()
                .push(err.clone());
            0
        });
    let mut entries = 0usize;
    let mut idx = start_idx + 1;

    while idx < fields.len() && entries < count {
        if fields[idx].tag != spec.delim {
            if msg_def.group_membership.get(&fields[idx].tag) == Some(&spec.count_tag) {
                let err = format!(
                    "Expected group delimiter tag {} before tag {}",
                    spec.delim, fields[idx].tag
                );
                errors.push(err.clone());
                tag_errors.entry(fields[idx].tag).or_default().push(err);
                idx += 1;
                continue;
            } else {
                break;
            }
        }
        let (consumed, mut errs) =
            validate_group_entry(fields, idx, spec, msg_def, dict, tag_errors);
        errors.append(&mut errs);
        idx += consumed;
        entries += 1;
    }

    if entries != count {
        let err = format!(
            "NumInGroup {} declared {}, but {} instance(s) found",
            spec.count_tag, count, entries
        );
        errors.push(err.clone());
        tag_errors.entry(spec.count_tag).or_default().push(err);
    }
    (idx - start_idx, errors)
}

fn validate_group_entry(
    fields: &[FieldValue],
    start_idx: usize,
    spec: &MessageDefGroupSpec,
    msg_def: &MessageDef,
    dict: &FixTagLookup,
    tag_errors: &mut HashMap<u32, Vec<String>>,
) -> (usize, Vec<String>) {
    let mut errors = Vec::new();
    let mut idx = start_idx;
    let mut last_pos = -1isize;
    while idx < fields.len() {
        let tag = fields[idx].tag;
        if tag == spec.delim && idx != start_idx {
            break;
        }
        if let Some(nested) = spec.nested.get(&tag) {
            let (consumed, mut errs) =
                validate_group_instance(fields, idx, nested, msg_def, dict, tag_errors);
            errors.append(&mut errs);
            idx += consumed;
            continue;
        }
        if let Some(pos) = spec.entry_order.iter().position(|t| *t == tag) {
            if (pos as isize) < last_pos {
                let err = format!(
                    "Tag {} ({}) out of order within repeating group {}",
                    tag,
                    dict.field_name(tag),
                    spec.count_tag
                );
                errors.push(err.clone());
                tag_errors.entry(tag).or_default().push(err);
            }
            last_pos = pos as isize;
            idx += 1;
        } else {
            // Tag does not belong to this group; stop so parent can handle it.
            break;
        }
    }
    (idx - start_idx, errors)
}

fn validate_checksum_field(
    msg: &str,
    field_map: &HashMap<u32, String>,
    tag_errors: &mut HashMap<u32, Vec<String>>,
) -> Vec<String> {
    let mut errors = Vec::new();
    match field_map.get(&10) {
        None => errors.push("Missing required checksum tag 10".to_string()),
        Some(value) => {
            let expected = format!("{:03}", calculate_checksum(msg));
            if &expected != value {
                let err = format!("Checksum mismatch: got {}, expected {}", value, expected);
                errors.push(err.clone());
                tag_errors.entry(10).or_default().push(err);
            }
        }
    }
    errors
}

fn validate_body_length(
    msg: &str,
    field_map: &HashMap<u32, String>,
    tag_errors: &mut HashMap<u32, Vec<String>>,
) -> Vec<String> {
    let mut errors = Vec::new();
    match field_map.get(&9) {
        None => errors.push("Missing required BodyLength tag 9".to_string()),
        Some(value) => match value.parse::<usize>() {
            Err(_) => errors.push(format!("Invalid BodyLength value '{}'", value)),
            Ok(declared) => match compute_actual_body_length(msg) {
                None => errors.push("Unable to compute BodyLength from message".to_string()),
                Some(actual) if declared != actual => {
                    let err = format!("BodyLength mismatch: got {}, expected {}", declared, actual);
                    tag_errors.entry(9).or_default().push(err.clone());
                    errors.push(err);
                }
                _ => {}
            },
        },
    }
    errors
}

pub fn calculate_checksum(msg: &str) -> i32 {
    const SOH: &str = "\u{0001}";
    if let Some(idx) = msg.rfind(&(SOH.to_string() + "10=")) {
        let fragment = &msg[..idx + 1];
        let sum: i32 = fragment.bytes().map(|b| b as i32).sum();
        sum % 256
    } else {
        -1
    }
}

fn is_valid_type(value: &str, field_type: &str) -> bool {
    match field_type.to_ascii_uppercase().as_str() {
        "INT" | "LENGTH" | "NUMINGROUP" | "SEQNUM" | "DAYOFMONTH" => value.parse::<i64>().is_ok(),
        "FLOAT" | "QTY" | "PRICE" | "PRICEOFFSET" | "AMT" | "PERCENTAGE" => {
            value.parse::<f64>().is_ok()
        }
        "BOOLEAN" => value == "Y" || value == "N",
        "CHAR" => value.chars().count() == 1,
        "STRING"
        | "DATA"
        | "CURRENCY"
        | "EXCHANGE"
        | "COUNTRY"
        | "MULTIPLEVALUESTRING"
        | "MULTIPLESTRINGVALUE" => true,
        "UTCTIMESTAMP" => is_valid_timestamp(value),
        "UTCDATEONLY" => NaiveDate::parse_from_str(value, "%Y%m%d").is_ok(),
        "UTCTIMEONLY" => ["%H:%M", "%H:%M:%S", "%H:%M:%S%.3f"]
            .iter()
            .any(|fmt| NaiveTime::parse_from_str(value, fmt).is_ok()),
        "MONTHYEAR" => MONTH_YEAR_REGEX.is_match(value),
        _ => true,
    }
}

fn is_valid_timestamp(value: &str) -> bool {
    ["%Y%m%d-%H:%M:%S", "%Y%m%d-%H:%M:%S%.3f"]
        .iter()
        .any(|fmt| NaiveDateTime::parse_from_str(value, fmt).is_ok())
}

static MONTH_YEAR_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\d{6}(\d{2}|(-\d{1,2})|(-?w[1-5]))?$").expect("valid regex"));

fn compute_actual_body_length(msg: &str) -> Option<usize> {
    const SOH: u8 = 0x01;
    let bytes = msg.as_bytes();

    // find start of 9= field
    let mut len_pos = None;
    for i in 0..bytes.len().saturating_sub(1) {
        if bytes[i] == b'9' && bytes[i + 1] == b'=' {
            len_pos = Some(i);
            break;
        }
    }
    let len_pos = len_pos?;

    // find delimiter after 9= value
    let mut body_start = None;
    for (i, byte) in bytes.iter().enumerate().skip(len_pos) {
        if *byte == SOH {
            body_start = Some(i + 1);
            break;
        }
    }
    let body_start = body_start?;

    // find last occurrence of SOH10=
    let mut checksum_start = None;
    for i in (0..bytes.len().saturating_sub(3)).rev() {
        if bytes[i] == SOH && bytes[i + 1] == b'1' && bytes[i + 2] == b'0' && bytes[i + 3] == b'=' {
            checksum_start = Some(i);
            break;
        }
    }
    let checksum_start = checksum_start?;

    if checksum_start >= body_start {
        Some(checksum_start - body_start + 1)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::schema::{
        ComponentContainer, ComponentDef, Field, FieldContainer, FieldRef, FixDictionary, GroupDef,
        Message, MessageContainer, Value, ValuesWrapper,
    };

    const SOH: &str = "\u{0001}";

    fn field(name: &str, number: u32, field_type: &str) -> Field {
        Field {
            name: name.to_string(),
            number,
            field_type: field_type.to_string(),
            values: Vec::new(),
            values_wrapper: ValuesWrapper::default(),
        }
    }

    fn enum_field(name: &str, number: u32, field_type: &str, values: &[(&str, &str)]) -> Field {
        Field {
            name: name.to_string(),
            number,
            field_type: field_type.to_string(),
            values: values
                .iter()
                .map(|(enumeration, description)| Value {
                    enumeration: (*enumeration).to_string(),
                    description: (*description).to_string(),
                })
                .collect(),
            values_wrapper: ValuesWrapper::default(),
        }
    }

    fn test_lookup() -> FixTagLookup {
        let dict = FixDictionary {
            typ: "FIX".to_string(),
            major: "4".to_string(),
            minor: "4".to_string(),
            service_pack: None,
            fields: FieldContainer {
                items: vec![
                    field("BeginString", 8, "STRING"),
                    field("BodyLength", 9, "LENGTH"),
                    field("MsgType", 35, "STRING"),
                    field("CheckSum", 10, "STRING"),
                    enum_field("Side", 54, "CHAR", &[("1", "Buy"), ("2", "Sell")]),
                    field("TransactTime", 60, "UTCTIMESTAMP"),
                    field("NoItems", 100, "NUMINGROUP"),
                    field("ItemValue", 101, "STRING"),
                    field("ItemCode", 102, "STRING"),
                    field("ItemExtra", 103, "STRING"),
                ],
            },
            messages: MessageContainer {
                items: vec![Message {
                    name: "Test".to_string(),
                    msg_type: "Z".to_string(),
                    msg_cat: "app".to_string(),
                    fields: vec![FieldRef {
                        name: "NoItems".to_string(),
                        required: Some("Y".to_string()),
                    }],
                    groups: vec![GroupDef {
                        name: "NoItems".to_string(),
                        required: Some("Y".to_string()),
                        fields: vec![
                            FieldRef {
                                name: "ItemValue".to_string(),
                                required: Some("N".to_string()),
                            },
                            FieldRef {
                                name: "ItemCode".to_string(),
                                required: Some("N".to_string()),
                            },
                            FieldRef {
                                name: "ItemExtra".to_string(),
                                required: Some("N".to_string()),
                            },
                        ],
                        groups: Vec::new(),
                        components: Vec::new(),
                        entries: vec![
                            crate::decoder::schema::ContainerEntry::Field(FieldRef {
                                name: "ItemValue".to_string(),
                                required: Some("N".to_string()),
                            }),
                            crate::decoder::schema::ContainerEntry::Field(FieldRef {
                                name: "ItemCode".to_string(),
                                required: Some("N".to_string()),
                            }),
                            crate::decoder::schema::ContainerEntry::Field(FieldRef {
                                name: "ItemExtra".to_string(),
                                required: Some("N".to_string()),
                            }),
                        ],
                    }],
                    components: Vec::new(),
                    entries: vec![crate::decoder::schema::ContainerEntry::Group(GroupDef {
                        name: "NoItems".to_string(),
                        required: Some("Y".to_string()),
                        fields: vec![
                            FieldRef {
                                name: "ItemValue".to_string(),
                                required: Some("N".to_string()),
                            },
                            FieldRef {
                                name: "ItemCode".to_string(),
                                required: Some("N".to_string()),
                            },
                            FieldRef {
                                name: "ItemExtra".to_string(),
                                required: Some("N".to_string()),
                            },
                        ],
                        groups: Vec::new(),
                        components: Vec::new(),
                        entries: vec![
                            crate::decoder::schema::ContainerEntry::Field(FieldRef {
                                name: "ItemValue".to_string(),
                                required: Some("N".to_string()),
                            }),
                            crate::decoder::schema::ContainerEntry::Field(FieldRef {
                                name: "ItemCode".to_string(),
                                required: Some("N".to_string()),
                            }),
                            crate::decoder::schema::ContainerEntry::Field(FieldRef {
                                name: "ItemExtra".to_string(),
                                required: Some("N".to_string()),
                            }),
                        ],
                    })],
                }],
            },
            components: ComponentContainer { items: Vec::new() },
            header: ComponentDef {
                name: String::new(),
                fields: vec![
                    FieldRef {
                        name: "BeginString".to_string(),
                        required: Some("Y".to_string()),
                    },
                    FieldRef {
                        name: "BodyLength".to_string(),
                        required: Some("Y".to_string()),
                    },
                    FieldRef {
                        name: "MsgType".to_string(),
                        required: Some("Y".to_string()),
                    },
                ],
                groups: Vec::new(),
                components: Vec::new(),
                entries: vec![
                    crate::decoder::schema::ContainerEntry::Field(FieldRef {
                        name: "BeginString".to_string(),
                        required: Some("Y".to_string()),
                    }),
                    crate::decoder::schema::ContainerEntry::Field(FieldRef {
                        name: "BodyLength".to_string(),
                        required: Some("Y".to_string()),
                    }),
                    crate::decoder::schema::ContainerEntry::Field(FieldRef {
                        name: "MsgType".to_string(),
                        required: Some("Y".to_string()),
                    }),
                ],
            },
            trailer: ComponentDef {
                name: String::new(),
                fields: vec![FieldRef {
                    name: "CheckSum".to_string(),
                    required: Some("Y".to_string()),
                }],
                groups: Vec::new(),
                components: Vec::new(),
                entries: vec![crate::decoder::schema::ContainerEntry::Field(FieldRef {
                    name: "CheckSum".to_string(),
                    required: Some("Y".to_string()),
                })],
            },
        };

        FixTagLookup::from_dictionary(&dict, "TEST")
    }

    fn build_message(fields: &[(u32, &str)], declared_body_len: Option<usize>) -> String {
        let mut body = String::new();
        for (tag, value) in fields {
            body.push_str(&format!("{tag}={value}{SOH}"));
        }
        let body_len = declared_body_len.unwrap_or(body.len());
        let mut msg = format!("8=FIX.4.4{SOH}9={:03}{SOH}{}", body_len, body);
        let checksum = calculate_checksum(&format!("{msg}10=000{SOH}"));
        msg.push_str(&format!("10={:03}{SOH}", checksum));
        msg
    }

    fn lookup_from_xml(xml: &str) -> FixTagLookup {
        let dict = FixDictionary::from_xml(xml).expect("test dictionary parses");
        FixTagLookup::from_dictionary(&dict, "TEST")
    }

    #[test]
    fn allows_repeating_group_tags() {
        let dict = test_lookup();
        let msg = build_message(
            &[(35, "Z"), (100, "2"), (101, "ALPHA"), (101, "BETA")],
            None,
        );
        let errors = validate_fix_message(&msg, &dict);
        assert!(
            errors.is_clean(),
            "expected no errors for valid repeating group message: {:?}",
            errors.errors
        );
    }

    #[test]
    fn detects_duplicate_top_level_tag_even_if_repeatable_elsewhere() {
        let dict = lookup_from_xml(
            r#"
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
    <message name='Plain' msgtype='P' msgcat='app'>
      <field name='SharedField' required='N'/>
    </message>
    <message name='Grouped' msgtype='G' msgcat='app'>
      <group name='NoShared'>
        <field name='SharedField' required='N'/>
      </group>
    </message>
  </messages>
  <components/>
  <fields>
    <field number='8' name='BeginString' type='STRING'/>
    <field number='9' name='BodyLength' type='LENGTH'/>
    <field number='10' name='CheckSum' type='STRING'/>
    <field number='35' name='MsgType' type='STRING'>
      <value enum='P' description='Plain'/>
      <value enum='G' description='Grouped'/>
    </field>
    <field number='1000' name='NoShared' type='NUMINGROUP'/>
    <field number='1001' name='SharedField' type='STRING'/>
  </fields>
</fix>
"#,
        );
        let msg = build_message(&[(35, "P"), (1001, "ALPHA"), (1001, "BETA")], None);
        let errors = validate_fix_message(&msg, &dict);

        assert!(
            errors
                .errors
                .iter()
                .any(|err| err.contains("Duplicate tag 1001 encountered")),
            "expected duplicate top-level tag to be rejected: {:?}",
            errors.errors
        );
    }

    #[test]
    fn allows_component_first_group_entries() {
        let dict = lookup_from_xml(
            r#"
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
    <message name='LeggedOrder' msgtype='L' msgcat='app'>
      <group name='NoLegs'>
        <component name='InstrumentLeg'/>
      </group>
    </message>
  </messages>
  <components>
    <component name='InstrumentLeg'>
      <field name='LegSymbol' required='Y'/>
    </component>
  </components>
  <fields>
    <field number='8' name='BeginString' type='STRING'/>
    <field number='9' name='BodyLength' type='LENGTH'/>
    <field number='10' name='CheckSum' type='STRING'/>
    <field number='35' name='MsgType' type='STRING'>
      <value enum='L' description='LeggedOrder'/>
    </field>
    <field number='555' name='NoLegs' type='NUMINGROUP'/>
    <field number='600' name='LegSymbol' type='STRING'/>
  </fields>
</fix>
"#,
        );
        let msg = build_message(&[(35, "L"), (555, "1"), (600, "IBM")], None);
        let errors = validate_fix_message(&msg, &dict);

        assert!(
            errors.is_clean(),
            "component-first group entries should validate cleanly: {:?}",
            errors.errors
        );
    }

    #[test]
    fn detects_body_length_mismatch() {
        let dict = test_lookup();
        let msg = build_message(&[(35, "Z"), (100, "1"), (101, "ONLY")], Some(999));
        let errors = validate_fix_message(&msg, &dict);
        assert!(
            errors
                .errors
                .iter()
                .any(|e| e.contains("BodyLength mismatch")),
            "expected body length error, got {:?}",
            errors.errors
        );
    }

    #[test]
    fn detects_checksum_mismatch() {
        let dict = test_lookup();
        let mut msg = build_message(&[(35, "Z"), (100, "1"), (101, "ONLY")], None);
        // Replace checksum with an incorrect value while keeping length intact.
        if let Some(pos) = msg.rfind("10=") {
            msg.truncate(pos + 3);
            msg.push_str("999\u{0001}");
        }
        let errors = validate_fix_message(&msg, &dict);
        assert!(
            errors
                .errors
                .iter()
                .any(|e| e.contains("Checksum mismatch")),
            "expected checksum mismatch, got {:?}",
            errors.errors
        );
    }

    #[test]
    fn missing_msg_type_still_reports_length_and_tag() {
        let dict = test_lookup();
        let msg = format!("8=FIX.4.4{SOH}9=005{SOH}10=999{SOH}");
        let report = validate_fix_message(&msg, &dict);
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("Missing required tag 35")),
            "expected missing MsgType error"
        );
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("BodyLength mismatch") || e.contains("Checksum mismatch")),
            "expected invariant checks to run even without MsgType"
        );
        assert!(
            report.tag_errors.contains_key(&35),
            "tag error map should include tag 35 when missing"
        );
    }

    #[test]
    fn detects_unknown_msg_type() {
        let dict = test_lookup();
        let msg = build_message(&[(35, "Q"), (100, "0")], None);
        let report = validate_fix_message(&msg, &dict);
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("Unknown MsgType: Q")),
            "expected unknown MsgType error, got {:?}",
            report.errors
        );
        assert!(
            report
                .tag_errors
                .get(&35)
                .is_some_and(|errs| errs.iter().any(|e| e.contains("Unknown MsgType: Q")))
        );
    }

    #[test]
    fn detects_invalid_enum_and_type() {
        let dict = test_lookup();
        let msg = build_message(&[(35, "Z"), (54, "LONG"), (100, "0")], None);
        let report = validate_fix_message(&msg, &dict);
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("Invalid enum value 'LONG'")),
            "expected enum validation error, got {:?}",
            report.errors
        );
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("Invalid type: expected CHAR")),
            "expected type validation error, got {:?}",
            report.errors
        );
    }

    #[test]
    fn detects_invalid_numingroup_and_tag_outside_group() {
        let dict = test_lookup();
        let msg = build_message(&[(35, "Z"), (100, "oops"), (101, "ALPHA")], None);
        let report = validate_fix_message(&msg, &dict);
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("Invalid NumInGroup value 'oops'")),
            "expected invalid NumInGroup error, got {:?}",
            report.errors
        );
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("appears outside of repeating group 100")),
            "expected out-of-group error, got {:?}",
            report.errors
        );
    }

    #[test]
    fn detects_out_of_order_tags_within_group() {
        let dict = test_lookup();
        let msg = build_message(
            &[
                (35, "Z"),
                (100, "1"),
                (101, "ALPHA"),
                (103, "EXTRA"),
                (102, "CODE"),
            ],
            None,
        );
        let report = validate_fix_message(&msg, &dict);
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("Tag 102 (ItemCode) out of order within repeating group 100")),
            "expected group ordering error, got {:?}",
            report.errors
        );
    }

    #[test]
    fn missing_checksum_is_reported() {
        let dict = test_lookup();
        let msg = format!("8=FIX.4.4{SOH}9=010{SOH}35=Z{SOH}100=0{SOH}");
        let report = validate_fix_message(&msg, &dict);
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("Missing required checksum tag 10")),
            "expected missing checksum error, got {:?}",
            report.errors
        );
    }

    #[test]
    fn helper_type_validators_cover_temporal_formats() {
        assert!(is_valid_type("20240101-00:00:00", "UTCTIMESTAMP"));
        assert!(!is_valid_type("2024-01-01 00:00:00", "UTCTIMESTAMP"));
        assert!(is_valid_type("20240101", "UTCDATEONLY"));
        assert!(is_valid_type("12:34:56.789", "UTCTIMEONLY"));
        assert!(!is_valid_type("25:00:00", "UTCTIMEONLY"));
        assert!(is_valid_type("202401", "MONTHYEAR"));
        assert!(is_valid_type("202401w3", "MONTHYEAR"));
        assert!(!is_valid_type("2024-13", "MONTHYEAR"));
    }

    #[test]
    fn helper_checksum_and_body_length_fall_back_when_fields_are_missing() {
        let msg = format!("8=FIX.4.4{SOH}9=005{SOH}35=Z{SOH}");
        assert_eq!(calculate_checksum(&msg), -1);
        assert_eq!(compute_actual_body_length(&msg), None);
    }
}
