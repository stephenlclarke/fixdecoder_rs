// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2025 Steve Clarke <stephenlclarke@mac.com> - https://xyzzy.tools

use crate::decoder::schema::{ComponentDef, FixDictionary, GroupDef, Message, MessageContainer};
use crate::fix;
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug)]
pub struct MessageDef {
    pub _name: String,
    pub _msg_type: String,
    pub field_order: Vec<u32>,
    pub required: Vec<u32>,
    pub groups: HashMap<u32, GroupSpec>,
    pub group_membership: HashMap<u32, u32>,
}

#[derive(Debug, Clone)]
pub struct GroupSpec {
    pub name: String,
    pub count_tag: u32,
    pub delim: u32,
    pub entry_order: Vec<u32>,
    pub entry_pos: HashMap<u32, usize>,
    pub entry_tag_set: HashSet<u32>,
    pub nested: HashMap<u32, GroupSpec>,
}

#[derive(Debug, Default, Clone)]
pub struct FixTagLookup {
    schema_key: String,
    tag_to_name: Arc<HashMap<u32, String>>,
    enum_map: Arc<HashMap<u32, HashMap<String, String>>>,
    field_types: Arc<HashMap<u32, String>>,
    messages: Arc<HashMap<String, MessageDef>>,
    repeatable_tags: Arc<HashSet<u32>>,
    #[allow(dead_code)]
    trailer_order: Arc<Vec<u32>>,
    fallback: Option<Arc<FixTagLookup>>,
    fallback_role: Option<FallbackKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackKind {
    Session,
    DetectedOverride,
}

#[derive(Debug, Clone)]
pub struct TagPresence {
    pub in_primary: bool,
    pub in_fallback: bool,
    pub primary_key: String,
    pub fallback_key: Option<String>,
    pub fallback_role: Option<FallbackKind>,
}

impl FixTagLookup {
    pub fn from_dictionary(dict: &FixDictionary, key: &str) -> Self {
        let mut tag_to_name = HashMap::new();
        let mut enum_map = HashMap::new();
        let mut field_types = HashMap::new();
        let mut name_to_tag = HashMap::new();
        let mut component_map: HashMap<String, ComponentDef> = HashMap::new();

        for field in &dict.fields.items {
            tag_to_name.insert(field.number, field.name.clone());
            name_to_tag.insert(field.name.clone(), field.number);
            field_types.insert(field.number, field.field_type.clone());

            let mut enums = HashMap::new();
            for value in field.values_iter() {
                enums.insert(value.enumeration.clone(), value.description.clone());
            }
            if !enums.is_empty() {
                enum_map.insert(field.number, enums);
            }
        }

        for comp in dict.components.items.iter() {
            component_map.insert(comp.name.clone(), comp.clone());
        }
        let mut header = dict.header.clone();
        header.name = "Header".to_string();
        component_map.insert(header.name.clone(), header);
        let mut trailer = dict.trailer.clone();
        trailer.name = "Trailer".to_string();
        component_map.insert(trailer.name.clone(), trailer);

        let messages = build_message_defs(&dict.messages, &component_map, &name_to_tag);
        let repeatable_tags = collect_repeatable_from_specs(&messages);
        let mut trailer_order = Vec::new();
        let mut stack = Vec::new();
        append_component_fields(
            "Trailer",
            &component_map,
            &name_to_tag,
            &mut stack,
            &mut trailer_order,
            &mut Vec::new(),
        );
        dedupe(&mut trailer_order);

        FixTagLookup {
            schema_key: key.to_string(),
            tag_to_name: Arc::new(tag_to_name),
            enum_map: Arc::new(enum_map),
            field_types: Arc::new(field_types),
            messages: Arc::new(messages),
            repeatable_tags: Arc::new(repeatable_tags),
            trailer_order: Arc::new(trailer_order),
            fallback: None,
            fallback_role: None,
        }
    }

    pub fn field_name(&self, tag: u32) -> String {
        if let Some(name) = self.tag_to_name.get(&tag) {
            return name.clone();
        }
        if let Some(fallback) = &self.fallback {
            return fallback.field_name(tag);
        }
        tag.to_string()
    }

    pub fn enum_description(&self, tag: u32, value: &str) -> Option<&str> {
        if let Some(enums) = self.enum_map.get(&tag) {
            return enums.get(value).map(|s| s.as_str());
        }
        self.fallback
            .as_ref()
            .and_then(|fallback| fallback.enum_description(tag, value))
    }

    pub fn enums_for(&self, tag: u32) -> Option<&HashMap<String, String>> {
        self.enum_map
            .get(&tag)
            .or_else(|| self.fallback.as_ref().and_then(|f| f.enums_for(tag)))
    }

    pub fn field_type(&self, tag: u32) -> Option<&str> {
        self.field_types
            .get(&tag)
            .map(|s| s.as_str())
            .or_else(|| self.fallback.as_ref().and_then(|f| f.field_type(tag)))
    }

    pub fn message_def(&self, msg_type: &str) -> Option<&MessageDef> {
        self.messages
            .get(msg_type)
            .or_else(|| self.fallback.as_ref().and_then(|f| f.message_def(msg_type)))
    }

    pub fn is_repeatable(&self, tag: u32) -> bool {
        self.repeatable_tags.contains(&tag)
            || self
                .fallback
                .as_ref()
                .map(|f| f.is_repeatable(tag))
                .unwrap_or(false)
    }

    pub fn trailer_tags(&self) -> &[u32] {
        if !self.trailer_order.is_empty() {
            self.trailer_order.as_slice()
        } else if let Some(fallback) = &self.fallback {
            fallback.trailer_tags()
        } else {
            self.trailer_order.as_slice()
        }
    }

    pub fn tag_presence(&self, tag: u32) -> TagPresence {
        let in_primary = self.tag_to_name.contains_key(&tag);
        let fallback_key = self.fallback.as_ref().map(|f| f.schema_key.clone());
        let in_fallback = self
            .fallback
            .as_ref()
            .map(|f| f.has_tag(tag))
            .unwrap_or(false);
        TagPresence {
            in_primary,
            in_fallback,
            primary_key: self.schema_key.clone(),
            fallback_key,
            fallback_role: self.fallback_role,
        }
    }

    fn has_tag(&self, tag: u32) -> bool {
        self.tag_to_name.contains_key(&tag)
            || self
                .fallback
                .as_ref()
                .map(|f| f.has_tag(tag))
                .unwrap_or(false)
    }
}

#[cfg(test)]
impl FixTagLookup {
    pub fn new_for_tests(messages: HashMap<String, MessageDef>) -> Self {
        FixTagLookup {
            schema_key: "TEST".to_string(),
            tag_to_name: Arc::new(HashMap::new()),
            enum_map: Arc::new(HashMap::new()),
            field_types: Arc::new(HashMap::new()),
            messages: Arc::new(messages),
            repeatable_tags: Arc::new(HashSet::new()),
            trailer_order: Arc::new(vec![10]),
            fallback: None,
            fallback_role: None,
        }
    }
}

static LOOKUPS: Lazy<RwLock<HashMap<String, Arc<FixTagLookup>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

static OVERRIDE_MISS: AtomicBool = AtomicBool::new(false);

const SESSION_KEY: &str = "FIXT11";

/// Remove any cached override+detected combos that reference the given key.
pub fn clear_override_cache_for(key: &str) {
    if let Ok(mut guard) = LOOKUPS.write() {
        drop_combo_entries_for(key, &mut guard);
    }
}

fn schema_to_xml_id(key: &str) -> Option<&'static str> {
    match key {
        "FIX27" => Some("40"),
        "FIX30" => Some("40"),
        "FIX40" => Some("40"),
        "FIX41" => Some("41"),
        "FIX42" => Some("42"),
        "FIX43" => Some("43"),
        "FIX44" => Some("44"),
        "FIX50" => Some("50"),
        "FIX50SP1" => Some("50SP1"),
        "FIX50SP2" => Some("50SP2"),
        "FIXT11" => Some("T11"),
        _ => None,
    }
}

fn needs_session_merge(key: &str) -> bool {
    matches!(key, "FIX50" | "FIX50SP1" | "FIX50SP2")
}

fn get_dictionary(key: &str) -> Option<Arc<FixTagLookup>> {
    if let Some(existing) = LOOKUPS.read().ok()?.get(key).cloned() {
        return Some(existing);
    }

    let xml_id = schema_to_xml_id(key)?;
    let xml = fix::choose_embedded_xml(xml_id);
    let dict = match FixDictionary::from_xml(xml) {
        Ok(dict) => dict,
        Err(err) => {
            eprintln!("failed to parse embedded FIX XML for {key}: {err}");
            return None;
        }
    };
    let lookup = build_lookup_from_dict(key, &dict);

    let arc = Arc::new(lookup);
    let mut guard = LOOKUPS.write().ok()?;
    let entry = guard.entry(key.to_string()).or_insert_with(|| arc.clone());
    Some(entry.clone())
}

fn get_tag_value<'a>(msg: &'a str, tag: &str) -> Option<&'a str> {
    for field in msg.split('\u{0001}') {
        if let Some((lhs, rhs)) = field.split_once('=')
            && lhs == tag
        {
            return Some(rhs);
        }
    }
    None
}

#[cfg(test)]
fn detect_schema_key(msg: &str) -> String {
    detect_schema_key_with_default(msg, None)
}

fn detect_schema_key_with_default(msg: &str, session_default_key: Option<&str>) -> String {
    if let Some(begin) = get_tag_value(msg, "8") {
        if begin == "FIXT.1.1" {
            if let Some(appl_ver_id) =
                get_tag_value(msg, "1128").or_else(|| get_tag_value(msg, "1137"))
                && let Some(schema) = appl_ver_to_schema(appl_ver_id)
            {
                return schema.to_string();
            }
            if let Some(schema) = session_default_key {
                return schema.to_string();
            }
            return "FIX50".to_string();
        }
        return begin.replace('.', "");
    }
    "FIX44".to_string()
}

fn appl_ver_to_schema(value: &str) -> Option<&'static str> {
    match value {
        "0" => Some("FIX27"),
        "1" => Some("FIX30"),
        "2" => Some("FIX40"),
        "3" => Some("FIX41"),
        "4" => Some("FIX42"),
        "5" => Some("FIX43"),
        "6" => Some("FIX44"),
        "7" => Some("FIX50"),
        "8" => Some("FIX50SP1"),
        "9" => Some("FIX50SP2"),
        _ => None,
    }
}

#[cfg(test)]
pub fn load_dictionary(msg: &str) -> Arc<FixTagLookup> {
    let key = detect_schema_key_with_default(msg, None);
    get_dictionary(&key)
        .or_else(|| get_dictionary("FIX44"))
        .expect("FIX44 dictionary available")
}

pub fn default_appl_ver_key(msg: &str) -> Option<String> {
    get_tag_value(msg, "1137")
        .and_then(appl_ver_to_schema)
        .map(str::to_string)
}

pub fn load_dictionary_with_session_default(
    msg: &str,
    override_key: Option<&str>,
    session_default_key: Option<&str>,
) -> Arc<FixTagLookup> {
    let detected_key = detect_schema_key_with_default(msg, session_default_key);
    if let Some(key) = override_key {
        let combo_key = format!("{key}+{detected_key}");
        if let Some(existing) = LOOKUPS.read().ok().and_then(|l| l.get(&combo_key).cloned()) {
            return existing;
        }

        if let Some(dict) = get_dictionary(key) {
            let fallback = get_dictionary(&detected_key)
                .or_else(|| get_dictionary("FIX44"))
                .expect("FIX44 dictionary available");
            if Arc::ptr_eq(&dict, &fallback) {
                return dict;
            }
            let merged = merge_with_fallback(&dict, fallback, FallbackKind::DetectedOverride);
            if let Ok(mut guard) = LOOKUPS.write() {
                guard.insert(combo_key, merged.clone());
            }
            return merged;
        }
        eprintln!(
            "warning: FIX override '{}' not found; falling back to auto-detected dictionary",
            key
        );
        warn_override_miss();
    }

    get_dictionary(&detected_key)
        .or_else(|| get_dictionary("FIX44"))
        .expect("FIX44 dictionary available")
}

/// Load a dictionary, allowing an override schema key to force the selection used for decoding.
#[cfg(test)]
pub fn load_dictionary_with_override(msg: &str, override_key: Option<&str>) -> Arc<FixTagLookup> {
    load_dictionary_with_session_default(msg, override_key, None)
}

fn warn_override_miss() {
    OVERRIDE_MISS.store(true, Ordering::Relaxed);
}

fn merge_with_fallback(
    primary: &Arc<FixTagLookup>,
    fallback: Arc<FixTagLookup>,
    role: FallbackKind,
) -> Arc<FixTagLookup> {
    let mut merged: FixTagLookup = (**primary).clone();
    merged.fallback = Some(fallback);
    merged.fallback_role = Some(role);
    Arc::new(merged)
}

#[cfg(test)]
pub fn reset_override_warn() {
    OVERRIDE_MISS.store(false, Ordering::Relaxed);
}

pub fn override_warn_triggered() -> bool {
    OVERRIDE_MISS.load(Ordering::Relaxed)
}

pub fn register_dictionary(key: &str, dict: &FixDictionary) {
    let lookup = build_lookup_from_dict(key, dict);
    let mut guard = LOOKUPS.write().expect("dictionary cache poisoned");
    guard.insert(key.to_string(), Arc::new(lookup));

    drop_combo_entries_for(key, &mut guard);
}

fn build_lookup_from_dict(key: &str, dict: &FixDictionary) -> FixTagLookup {
    let mut lookup = FixTagLookup::from_dictionary(dict, key);

    if needs_session_merge(key)
        && let Some(session) = get_dictionary(SESSION_KEY)
    {
        lookup.fallback = Some(session);
        lookup.fallback_role = Some(FallbackKind::Session);
    }

    lookup
}

fn drop_combo_entries_for(key: &str, guard: &mut HashMap<String, Arc<FixTagLookup>>) {
    let stale: Vec<String> = guard
        .keys()
        .filter(|k| {
            k.split_once('+')
                .map(|(override_key, detected)| override_key == key || detected == key)
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    for combo in stale {
        guard.remove(&combo);
    }
}

fn build_message_defs(
    messages: &MessageContainer,
    components: &HashMap<String, ComponentDef>,
    name_to_tag: &HashMap<String, u32>,
) -> HashMap<String, MessageDef> {
    let mut map = HashMap::new();
    for msg in &messages.items {
        let (field_order, required) = expand_message_fields(msg, components, name_to_tag, true);
        let (groups, membership) = collect_group_specs(msg, components, name_to_tag);
        map.insert(
            msg.msg_type.clone(),
            MessageDef {
                _name: msg.name.clone(),
                _msg_type: msg.msg_type.clone(),
                field_order,
                required,
                groups,
                group_membership: membership,
            },
        );
    }
    map
}

fn expand_message_fields(
    msg: &Message,
    components: &HashMap<String, ComponentDef>,
    name_to_tag: &HashMap<String, u32>,
    include_header_trailer: bool,
) -> (Vec<u32>, Vec<u32>) {
    let mut order = Vec::new();
    let mut required = Vec::new();
    let mut stack = Vec::new();

    if include_header_trailer {
        append_component_fields(
            "Header",
            components,
            name_to_tag,
            &mut stack,
            &mut order,
            &mut required,
        );
    }
    append_field_refs(&msg.fields, name_to_tag, &mut order, &mut required);
    for comp in &msg.components {
        append_component_fields(
            &comp.name,
            components,
            name_to_tag,
            &mut stack,
            &mut order,
            &mut required,
        );
    }
    for group in &msg.groups {
        append_group_fields(
            group,
            components,
            name_to_tag,
            &mut stack,
            &mut order,
            &mut required,
        );
    }

    if include_header_trailer {
        append_component_fields(
            "Trailer",
            components,
            name_to_tag,
            &mut stack,
            &mut order,
            &mut required,
        );
    }

    dedupe(&mut required);
    (order, required)
}

fn append_field_refs(
    refs: &[crate::decoder::schema::FieldRef],
    name_to_tag: &HashMap<String, u32>,
    order: &mut Vec<u32>,
    required: &mut Vec<u32>,
) {
    for field in refs {
        if let Some(tag) = name_to_tag.get(&field.name) {
            order.push(*tag);
            if field.required.as_deref() == Some("Y") {
                required.push(*tag);
            }
        }
    }
}

fn append_component_fields(
    name: &str,
    components: &HashMap<String, ComponentDef>,
    name_to_tag: &HashMap<String, u32>,
    stack: &mut Vec<String>,
    order: &mut Vec<u32>,
    required: &mut Vec<u32>,
) {
    if stack.contains(&name.to_string()) {
        eprintln!("warning: component recursion detected at {name}, skipping nested expansion");
        return;
    }
    let Some(comp) = components.get(name) else {
        return;
    };
    stack.push(name.to_string());

    append_field_refs(&comp.fields, name_to_tag, order, required);
    for sub in &comp.components {
        append_component_fields(&sub.name, components, name_to_tag, stack, order, required);
    }
    for group in &comp.groups {
        append_group_fields(group, components, name_to_tag, stack, order, required);
    }

    stack.pop();
}

fn append_group_fields(
    group: &GroupDef,
    components: &HashMap<String, ComponentDef>,
    name_to_tag: &HashMap<String, u32>,
    stack: &mut Vec<String>,
    order: &mut Vec<u32>,
    required: &mut Vec<u32>,
) {
    append_field_refs(&group.fields, name_to_tag, order, required);
    for comp in &group.components {
        append_component_fields(&comp.name, components, name_to_tag, stack, order, required);
    }
    for sub in &group.groups {
        append_group_fields(sub, components, name_to_tag, stack, order, required);
    }
}

fn dedupe(values: &mut Vec<u32>) {
    let mut seen = HashSet::new();
    values.retain(|v| seen.insert(*v));
}

fn collect_group_specs(
    msg: &Message,
    components: &HashMap<String, ComponentDef>,
    name_to_tag: &HashMap<String, u32>,
) -> (HashMap<u32, GroupSpec>, HashMap<u32, u32>) {
    let mut specs = HashMap::new();
    let mut membership = HashMap::new();
    let mut group_stack = HashSet::new();
    let mut component_stack = HashSet::new();

    for group in &msg.groups {
        insert_group_spec(
            group,
            components,
            name_to_tag,
            &mut group_stack,
            &mut specs,
            &mut membership,
        );
    }

    collect_component_group_specs(
        "Header",
        components,
        name_to_tag,
        &mut component_stack,
        &mut group_stack,
        &mut specs,
        &mut membership,
    );
    for component in &msg.components {
        collect_component_group_specs(
            &component.name,
            components,
            name_to_tag,
            &mut component_stack,
            &mut group_stack,
            &mut specs,
            &mut membership,
        );
    }
    collect_component_group_specs(
        "Trailer",
        components,
        name_to_tag,
        &mut component_stack,
        &mut group_stack,
        &mut specs,
        &mut membership,
    );

    (specs, membership)
}

fn collect_component_group_specs(
    name: &str,
    components: &HashMap<String, ComponentDef>,
    name_to_tag: &HashMap<String, u32>,
    component_stack: &mut HashSet<String>,
    group_stack: &mut HashSet<String>,
    specs: &mut HashMap<u32, GroupSpec>,
    membership: &mut HashMap<u32, u32>,
) {
    if !component_stack.insert(name.to_string()) {
        return;
    }
    let Some(component) = components.get(name) else {
        component_stack.remove(name);
        return;
    };

    for group in &component.groups {
        insert_group_spec(
            group,
            components,
            name_to_tag,
            group_stack,
            specs,
            membership,
        );
    }
    for nested in &component.components {
        collect_component_group_specs(
            &nested.name,
            components,
            name_to_tag,
            component_stack,
            group_stack,
            specs,
            membership,
        );
    }

    component_stack.remove(name);
}

fn insert_group_spec(
    group: &GroupDef,
    components: &HashMap<String, ComponentDef>,
    name_to_tag: &HashMap<String, u32>,
    group_stack: &mut HashSet<String>,
    specs: &mut HashMap<u32, GroupSpec>,
    membership: &mut HashMap<u32, u32>,
) {
    if let Some(spec) = build_group_spec(group, components, name_to_tag, group_stack) {
        membership.extend(collect_memberships(&spec, spec.count_tag));
        specs.entry(spec.count_tag).or_insert(spec);
    }
}

fn build_group_spec(
    group: &GroupDef,
    components: &HashMap<String, ComponentDef>,
    name_to_tag: &HashMap<String, u32>,
    stack: &mut HashSet<String>,
) -> Option<GroupSpec> {
    let count_tag = *name_to_tag.get(&group.name)?;
    let delim = group
        .fields
        .first()
        .and_then(|f| name_to_tag.get(&f.name))
        .copied()
        .unwrap_or(count_tag);
    let mut order = Vec::new();
    let mut required = Vec::new();
    append_field_refs(&group.fields, name_to_tag, &mut order, &mut required);

    let mut nested = HashMap::new();
    for comp in &group.components {
        append_component_fields_for_spec(
            &comp.name,
            components,
            name_to_tag,
            stack,
            &mut order,
            &mut required,
            &mut nested,
        );
    }
    for sub in &group.groups {
        if let Some(spec) = build_group_spec(sub, components, name_to_tag, stack) {
            order.push(spec.count_tag);
            nested.insert(spec.count_tag, spec);
        }
    }

    dedupe(&mut order);
    let entry_tag_set: HashSet<u32> = order.iter().copied().collect();
    let entry_pos: HashMap<u32, usize> = order.iter().enumerate().map(|(i, t)| (*t, i)).collect();
    Some(GroupSpec {
        name: group.name.clone(),
        count_tag,
        delim,
        entry_order: order,
        entry_pos,
        entry_tag_set,
        nested,
    })
}

fn append_component_fields_for_spec(
    name: &str,
    components: &HashMap<String, ComponentDef>,
    name_to_tag: &HashMap<String, u32>,
    stack: &mut HashSet<String>,
    order: &mut Vec<u32>,
    required: &mut Vec<u32>,
    nested: &mut HashMap<u32, GroupSpec>,
) {
    if !stack.insert(name.to_string()) {
        return;
    }
    let Some(comp) = components.get(name) else {
        stack.remove(name);
        return;
    };

    append_field_refs(&comp.fields, name_to_tag, order, required);
    for sub_comp in &comp.components {
        append_component_fields_for_spec(
            &sub_comp.name,
            components,
            name_to_tag,
            stack,
            order,
            required,
            nested,
        );
    }
    for group in &comp.groups {
        if let Some(spec) = build_group_spec(group, components, name_to_tag, stack) {
            order.push(spec.count_tag);
            nested.insert(spec.count_tag, spec);
        }
    }

    stack.remove(name);
}

fn collect_memberships(spec: &GroupSpec, owner: u32) -> HashMap<u32, u32> {
    let mut map = HashMap::new();
    for tag in &spec.entry_tag_set {
        map.insert(*tag, owner);
    }
    for nested in spec.nested.values() {
        map.insert(nested.count_tag, nested.count_tag);
        map.extend(collect_memberships(nested, nested.count_tag));
    }
    map
}

fn collect_repeatable_from_specs(messages: &HashMap<String, MessageDef>) -> HashSet<u32> {
    fn walk(spec: &GroupSpec, acc: &mut HashSet<u32>) {
        acc.insert(spec.count_tag);
        for tag in &spec.entry_tag_set {
            acc.insert(*tag);
        }
        for nested in spec.nested.values() {
            walk(nested, acc);
        }
    }

    let mut repeatable = HashSet::new();
    for msg in messages.values() {
        for spec in msg.groups.values() {
            walk(spec, &mut repeatable);
        }
    }
    repeatable
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::schema::FixDictionary;
    use once_cell::sync::Lazy;
    use std::sync::{Arc, Mutex};

    static LOOKUP_TEST_GUARD: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    struct LookupCacheGuard {
        originals: Vec<(String, Option<Arc<FixTagLookup>>)>,
    }

    impl LookupCacheGuard {
        fn new(keys: &[&str]) -> Self {
            let mut originals = Vec::new();
            if let Ok(guard) = LOOKUPS.read() {
                for key in keys {
                    originals.push(((*key).to_string(), guard.get(*key).cloned()));
                }
            } else {
                for key in keys {
                    originals.push(((*key).to_string(), None));
                }
            }
            Self { originals }
        }
    }

    impl Drop for LookupCacheGuard {
        fn drop(&mut self) {
            if let Ok(mut guard) = LOOKUPS.write() {
                for (key, original) in &self.originals {
                    match original {
                        Some(existing) => {
                            guard.insert(key.clone(), existing.clone());
                        }
                        None => {
                            guard.remove(key);
                        }
                    }
                }
            }
            for (key, _) in &self.originals {
                clear_override_cache_for(key);
            }
        }
    }

    fn small_override_dictionary() -> FixDictionary {
        let xml = r#"
<fix type='FIX' major='4' minor='4'>
  <header>
    <field name='BeginString' required='Y'/>
  </header>
  <trailer>
    <field name='CheckSum' required='Y'/>
  </trailer>
  <messages>
    <message name='Heartbeat' msgtype='0' msgcat='admin'>
      <field name='MsgType' required='Y'/>
    </message>
  </messages>
  <components/>
  <fields>
    <field number='8' name='BeginString' type='STRING'/>
    <field number='10' name='CheckSum' type='STRING'/>
    <field number='35' name='MsgType' type='STRING'>
      <value enum='0' description='Heartbeat'/>
    </field>
  </fields>
</fix>
"#;
        FixDictionary::from_xml(xml).expect("override test dictionary parses")
    }

    fn small_detected_dictionary() -> FixDictionary {
        let xml = r#"
<fix type='FIX' major='5' minor='0' servicepack='2'>
  <header>
    <field name='BeginString' required='Y'/>
  </header>
  <trailer>
    <field name='CheckSum' required='Y'/>
  </trailer>
  <messages>
    <message name='Heartbeat' msgtype='0' msgcat='admin'>
      <field name='MsgType' required='Y'/>
    </message>
  </messages>
  <components/>
  <fields>
    <field number='8' name='BeginString' type='STRING'/>
    <field number='10' name='CheckSum' type='STRING'/>
    <field number='35' name='MsgType' type='STRING'>
      <value enum='0' description='Heartbeat'/>
    </field>
    <field number='1128' name='ApplVerID' type='STRING'>
      <value enum='9' description='FIX50SP2'/>
    </field>
  </fields>
</fix>
"#;
        FixDictionary::from_xml(xml).expect("detected test dictionary parses")
    }

    #[test]
    fn detects_schema_from_default_appl_ver_id() {
        let _lock = LOOKUP_TEST_GUARD.lock().unwrap();
        let msg = "8=FIXT.1.1\u{0001}35=D\u{0001}1137=8\u{0001}10=000\u{0001}";
        assert_eq!(detect_schema_key(msg), "FIX50SP1");
    }

    #[test]
    fn session_default_guides_fixt_messages_without_appl_ver_id() {
        let _lock = LOOKUP_TEST_GUARD.lock().unwrap();
        let msg = "8=FIXT.1.1\u{0001}35=0\u{0001}10=000\u{0001}";
        assert_eq!(
            detect_schema_key_with_default(msg, Some("FIX50SP2")),
            "FIX50SP2"
        );
    }

    #[test]
    fn load_dictionary_respects_override_key() {
        let _lock = LOOKUP_TEST_GUARD.lock().unwrap();
        reset_override_warn();
        let msg = "8=FIX.4.2\u{0001}35=D\u{0001}1128=9\u{0001}10=000\u{0001}";
        let overridden = load_dictionary_with_override(msg, Some("FIX50"));
        assert_eq!(
            overridden.field_name(1128),
            "ApplVerID",
            "override should still provide definitions from the selected dictionary"
        );
        assert!(
            !override_warn_triggered(),
            "a valid override should not trigger the warning flag"
        );
    }

    #[test]
    fn warns_and_falls_back_on_unknown_override() {
        let _lock = LOOKUP_TEST_GUARD.lock().unwrap();
        reset_override_warn();
        let msg = "8=FIX.4.4\u{0001}35=0\u{0001}10=000\u{0001}";
        let dict = load_dictionary_with_override(msg, Some("FIX00BAD"));
        assert!(override_warn_triggered(), "missing override should warn");
        assert_eq!(dict.field_name(35), "MsgType");
    }

    #[test]
    fn override_uses_fallback_dictionary_for_missing_tags() {
        let _lock = LOOKUP_TEST_GUARD.lock().unwrap();
        let _cache_guard = LookupCacheGuard::new(&["FIX44", "FIX50SP2"]);
        reset_override_warn();
        register_dictionary("FIX44", &small_override_dictionary());
        register_dictionary("FIX50SP2", &small_detected_dictionary());
        clear_override_cache_for("FIX44");
        clear_override_cache_for("FIX50SP2");
        let msg = "8=FIXT.1.1\u{0001}35=0\u{0001}1128=9\u{0001}10=000\u{0001}";
        let dict = load_dictionary_with_override(msg, Some("FIX44"));
        assert_eq!(
            dict.field_name(1128),
            "ApplVerID",
            "override should fall back to detected FIX version when a tag is absent"
        );
        assert!(
            !override_warn_triggered(),
            "successful fallback should not trigger override warning flag"
        );
    }

    #[test]
    fn repeatable_tags_include_nested_groups() {
        let _lock = LOOKUP_TEST_GUARD.lock().unwrap();
        let xml = r#"
<fix type='FIX' major='4' minor='4'>
  <header><field name='BeginString' required='Y'/></header>
  <trailer><field name='CheckSum' required='Y'/></trailer>
  <messages>
    <message name='Test' msgtype='T' msgcat='app'>
      <group name='NoOuter'>
        <field name='OuterField'/>
        <group name='NoInner'>
          <field name='InnerField'/>
        </group>
      </group>
    </message>
  </messages>
  <components/>
  <fields>
    <field number='8' name='BeginString' type='STRING'/>
    <field number='10' name='CheckSum' type='STRING'/>
    <field number='35' name='MsgType' type='STRING'>
      <value enum='T' description='Test'/>
    </field>
    <field number='900' name='NoOuter' type='NUMINGROUP'/>
    <field number='901' name='OuterField' type='STRING'/>
    <field number='910' name='NoInner' type='NUMINGROUP'/>
    <field number='911' name='InnerField' type='STRING'/>
  </fields>
</fix>
"#;
        let dict = FixDictionary::from_xml(xml).expect("dictionary parses");
        let lookup = FixTagLookup::from_dictionary(&dict, "TEST");
        assert!(lookup.is_repeatable(900), "outer group count tag tracked");
        assert!(lookup.is_repeatable(901), "outer field repeatable");
        assert!(lookup.is_repeatable(910), "nested group count tag tracked");
        assert!(lookup.is_repeatable(911), "nested field repeatable");
    }

    #[test]
    fn new_order_single_does_not_inherit_unreachable_group_memberships() {
        let _lock = LOOKUP_TEST_GUARD.lock().unwrap();
        let msg = "8=FIX.4.4\u{0001}35=D\u{0001}10=000\u{0001}";
        let dict = load_dictionary(msg);
        let message = dict.message_def("D").expect("new order single definition");

        assert!(
            !message.group_membership.contains_key(&11),
            "ClOrdID should not be treated as a repeating-group member"
        );
        assert!(
            !message.group_membership.contains_key(&40),
            "OrdType should not be treated as a repeating-group member"
        );
        assert!(
            !message.group_membership.contains_key(&54),
            "Side should not be treated as a repeating-group member"
        );
        assert!(
            !message.group_membership.contains_key(&60),
            "TransactTime should not be treated as a repeating-group member"
        );
    }
}
