// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2025 Steve Clarke <stephenlclarke@mac.com> - https://xyzzy.tools

//! FIX dictionary parsing and in-memory schema builder.
//! The code leans on serde for XML parsing, then uses a custom builder to
//! produce the immutable tree consumed by the CLI and renderers.

use anyhow::{Context, anyhow};
use rayon::prelude::*;
use roxmltree::{Document, Node};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename = "fix")]
pub struct FixDictionary {
    #[serde(rename = "@type", default)]
    pub typ: String,
    #[serde(rename = "@major")]
    pub major: String,
    #[serde(rename = "@minor")]
    pub minor: String,
    #[serde(rename = "@servicepack", default)]
    pub service_pack: Option<String>,
    #[serde(rename = "fields", default)]
    pub fields: FieldContainer,
    #[serde(rename = "messages", default)]
    pub messages: MessageContainer,
    #[serde(rename = "components", default)]
    pub components: ComponentContainer,
    #[serde(rename = "header")]
    pub header: ComponentDef,
    #[serde(rename = "trailer")]
    pub trailer: ComponentDef,
}

impl FixDictionary {
    pub fn from_xml(xml: &str) -> anyhow::Result<Self> {
        let doc = Document::parse(xml)?;
        let root = doc.root_element();

        let fields_node =
            find_child(root, "fields").ok_or_else(|| anyhow!("missing <fields> section"))?;
        let messages_node =
            find_child(root, "messages").ok_or_else(|| anyhow!("missing <messages> section"))?;
        let components_node = find_child(root, "components")
            .ok_or_else(|| anyhow!("missing <components> section"))?;
        let header_node =
            find_child(root, "header").ok_or_else(|| anyhow!("missing <header> section"))?;
        let trailer_node =
            find_child(root, "trailer").ok_or_else(|| anyhow!("missing <trailer> section"))?;

        Ok(FixDictionary {
            typ: root.attribute("type").unwrap_or("FIX").to_string(),
            major: root.attribute("major").unwrap_or_default().to_string(),
            minor: root.attribute("minor").unwrap_or_default().to_string(),
            service_pack: root
                .attribute("servicepack")
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string()),
            fields: FieldContainer {
                items: parse_fields(fields_node)?,
            },
            messages: MessageContainer {
                items: parse_messages(messages_node)?,
            },
            components: ComponentContainer {
                items: parse_components(components_node)?,
            },
            header: parse_component_def(header_node, false)?,
            trailer: parse_component_def(trailer_node, false)?,
        })
    }
}

fn find_child<'a, 'input>(node: Node<'a, 'input>, tag: &str) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|child| child.is_element() && child.has_tag_name(tag))
}

fn children_with_tag<'a, 'input>(
    node: Node<'a, 'input>,
    tag: &'static str,
) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children()
        .filter(move |child| child.is_element() && child.has_tag_name(tag))
}

fn sanitize_ascii(input: &str) -> String {
    input
        .chars()
        .map(|ch| if ch.is_ascii() { ch } else { '?' })
        .collect()
}

fn parse_fields(node: Node) -> anyhow::Result<Vec<Field>> {
    children_with_tag(node, "field").map(parse_field).collect()
}

fn parse_field(node: Node) -> anyhow::Result<Field> {
    let mut inline = Vec::new();
    let mut wrapper = Vec::new();

    for child in node.children().filter(|c| c.is_element()) {
        match child.tag_name().name() {
            "value" => inline.push(parse_value(child)?),
            "values" => {
                for value_node in children_with_tag(child, "value") {
                    wrapper.push(parse_value(value_node)?);
                }
            }
            _ => {}
        }
    }

    Ok(Field {
        name: attr(node, "name")?,
        number: attr(node, "number")?
            .parse()
            .context("invalid field number")?,
        field_type: attr(node, "type")?,
        values: inline,
        values_wrapper: ValuesWrapper { value: wrapper },
    })
}

fn parse_value(node: Node) -> anyhow::Result<Value> {
    Ok(Value {
        enumeration: attr(node, "enum")?,
        description: sanitize_ascii(node.attribute("description").unwrap_or("")),
    })
}

fn parse_messages(node: Node) -> anyhow::Result<Vec<Message>> {
    children_with_tag(node, "message")
        .map(parse_message)
        .collect()
}

fn parse_message(node: Node) -> anyhow::Result<Message> {
    let entries = parse_container_entries(node)?;
    Ok(Message {
        name: attr(node, "name")?,
        msg_type: attr(node, "msgtype")?,
        msg_cat: sanitize_ascii(node.attribute("msgcat").unwrap_or("")),
        fields: entries.fields,
        groups: entries.groups,
        components: entries.components,
        entries: entries.entries,
    })
}

fn parse_components(node: Node) -> anyhow::Result<Vec<ComponentDef>> {
    children_with_tag(node, "component")
        .map(|child| parse_component_def(child, true))
        .collect()
}

fn parse_component_def(node: Node, require_name: bool) -> anyhow::Result<ComponentDef> {
    let name = if require_name {
        attr(node, "name")?
    } else {
        node.attribute("name")
            .map(sanitize_ascii)
            .unwrap_or_default()
    };
    let entries = parse_container_entries(node)?;

    Ok(ComponentDef {
        name,
        fields: entries.fields,
        groups: entries.groups,
        components: entries.components,
        entries: entries.entries,
    })
}

fn parse_group(node: Node) -> anyhow::Result<GroupDef> {
    let entries = parse_container_entries(node)?;
    Ok(GroupDef {
        name: attr(node, "name")?,
        required: node.attribute("required").map(sanitize_ascii),
        fields: entries.fields,
        groups: entries.groups,
        components: entries.components,
        entries: entries.entries,
    })
}

#[derive(Default)]
struct ParsedEntries {
    fields: Vec<FieldRef>,
    groups: Vec<GroupDef>,
    components: Vec<ComponentRef>,
    entries: Vec<ContainerEntry>,
}

fn parse_container_entries(node: Node) -> anyhow::Result<ParsedEntries> {
    let mut parsed = ParsedEntries::default();

    for child in node.children().filter(|c| c.is_element()) {
        match child.tag_name().name() {
            "field" => {
                let field = FieldRef {
                    name: attr(child, "name")?,
                    required: child.attribute("required").map(sanitize_ascii),
                };
                parsed.entries.push(ContainerEntry::Field(field.clone()));
                parsed.fields.push(field);
            }
            "group" => {
                let group = parse_group(child)?;
                parsed.entries.push(ContainerEntry::Group(group.clone()));
                parsed.groups.push(group);
            }
            "component" => {
                let component = ComponentRef {
                    name: attr(child, "name")?,
                    _required: child.attribute("required").map(sanitize_ascii),
                };
                parsed
                    .entries
                    .push(ContainerEntry::Component(component.clone()));
                parsed.components.push(component);
            }
            _ => {}
        }
    }

    Ok(parsed)
}

fn attr<'a, 'input>(node: Node<'a, 'input>, name: &str) -> anyhow::Result<String> {
    let tag_name = node.tag_name().name().to_string();
    node.attribute(name)
        .map(sanitize_ascii)
        .ok_or_else(|| anyhow!("missing attribute @{name} on <{tag_name}>"))
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FieldContainer {
    #[serde(rename = "field", default)]
    pub items: Vec<Field>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct MessageContainer {
    #[serde(rename = "message", default)]
    pub items: Vec<Message>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ComponentContainer {
    #[serde(rename = "component", default)]
    pub items: Vec<ComponentDef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Field {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@number")]
    pub number: u32,
    #[serde(rename = "@type")]
    pub field_type: String,
    #[serde(rename = "value", default)]
    pub values: Vec<Value>,
    #[serde(rename = "values", default)]
    pub values_wrapper: ValuesWrapper,
}

impl Field {
    pub fn values_iter(&self) -> impl Iterator<Item = &Value> {
        self.values.iter().chain(self.values_wrapper.value.iter())
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ValuesWrapper {
    #[serde(rename = "value", default)]
    pub value: Vec<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Value {
    #[serde(rename = "@enum")]
    pub enumeration: String,
    #[serde(rename = "@description")]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FieldRef {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@required", default)]
    pub required: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ContainerEntry {
    Field(FieldRef),
    Group(GroupDef),
    Component(ComponentRef),
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupDef {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@required", default)]
    pub required: Option<String>,
    #[serde(rename = "field", default)]
    pub fields: Vec<FieldRef>,
    #[serde(rename = "group", default)]
    pub groups: Vec<GroupDef>,
    #[serde(rename = "component", default)]
    pub components: Vec<ComponentRef>,
    #[serde(skip)]
    pub entries: Vec<ContainerEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ComponentRef {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@required", default)]
    pub _required: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ComponentDef {
    #[serde(rename = "@name", default)]
    pub name: String,
    #[serde(rename = "field", default)]
    pub fields: Vec<FieldRef>,
    #[serde(rename = "group", default)]
    pub groups: Vec<GroupDef>,
    #[serde(rename = "component", default)]
    pub components: Vec<ComponentRef>,
    #[serde(skip)]
    pub entries: Vec<ContainerEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    #[serde(rename = "@name")]
    pub name: String,
    #[serde(rename = "@msgtype")]
    pub msg_type: String,
    #[serde(rename = "@msgcat")]
    pub msg_cat: String,
    #[serde(rename = "field", default)]
    pub fields: Vec<FieldRef>,
    #[serde(rename = "group", default)]
    pub groups: Vec<GroupDef>,
    #[serde(rename = "component", default)]
    pub components: Vec<ComponentRef>,
    #[serde(skip)]
    pub entries: Vec<ContainerEntry>,
}

#[derive(Debug, Clone)]
pub struct FieldNode {
    pub required: bool,
    pub field: Arc<Field>,
}

#[derive(Debug, Clone)]
pub struct ComponentNode {
    pub name: String,
    pub fields: Vec<FieldNode>,
    pub groups: Vec<GroupNode>,
    pub components: Vec<ComponentNode>,
}

#[derive(Debug, Clone)]
pub struct GroupNode {
    pub name: String,
    pub required: bool,
    pub fields: Vec<FieldNode>,
    pub components: Vec<ComponentNode>,
    pub groups: Vec<GroupNode>,
}

#[derive(Debug, Clone)]
pub struct MessageNode {
    pub name: String,
    pub msg_type: String,
    pub msg_cat: String,
    pub fields: Vec<FieldNode>,
    pub components: Vec<ComponentNode>,
    pub groups: Vec<GroupNode>,
}

#[derive(Debug, Clone)]
pub struct SchemaTree {
    pub fields: BTreeMap<String, Arc<Field>>,
    pub components: BTreeMap<String, ComponentNode>,
    pub messages: BTreeMap<String, MessageNode>,
    #[allow(dead_code)]
    pub version: String,
    pub service_pack: String,
}

impl SchemaTree {
    pub fn build(dict: FixDictionary) -> Self {
        let field_map: BTreeMap<_, _> = dict
            .fields
            .items
            .par_iter()
            .map(|field| (field.name.clone(), Arc::new(field.clone())))
            .collect();

        let mut component_defs = HashMap::new();
        for comp in dict.components.items.iter() {
            component_defs.insert(comp.name.clone(), comp.clone());
        }

        let mut header = dict.header.clone();
        header.name = "Header".to_string();
        component_defs.insert(header.name.clone(), header);

        let mut trailer = dict.trailer.clone();
        trailer.name = "Trailer".to_string();
        component_defs.insert(trailer.name.clone(), trailer);

        let mut builder = ComponentBuilder::new(&field_map, &component_defs);

        let mut component_names: Vec<_> = component_defs.keys().cloned().collect();
        component_names.sort();
        let mut components = BTreeMap::new();
        for name in component_names {
            if let Some(node) = builder.build_component(&name) {
                components.insert(name, node);
            }
        }

        let mut messages = BTreeMap::new();
        for msg in dict.messages.items.iter() {
            let node = build_message_node(msg, &field_map, &mut builder);
            messages.insert(msg.name.clone(), node);
        }

        let service_pack = dict
            .service_pack
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("-")
            .to_string();

        SchemaTree {
            fields: field_map,
            components,
            messages,
            version: format!("{} {}.{}", dict.typ, dict.major, dict.minor),
            service_pack,
        }
    }

    pub fn find_field_by_number(&self, number: u32) -> Option<&Field> {
        self.fields
            .values()
            .find(|f| f.number == number)
            .map(|arc| arc.as_ref())
    }
}

fn build_field_nodes(refs: &[FieldRef], fields: &BTreeMap<String, Arc<Field>>) -> Vec<FieldNode> {
    let mut nodes = Vec::with_capacity(refs.len());
    for field_ref in refs {
        if let Some(field) = fields.get(&field_ref.name) {
            let required = field_ref.required.as_deref() == Some("Y");
            nodes.push(FieldNode {
                required,
                field: field.clone(),
            });
        }
    }
    nodes
}

/// Internal helper that memoises component and group nodes so we don’t clone
/// the same structure repeatedly for every message.
struct ComponentBuilder<'a> {
    fields: &'a BTreeMap<String, Arc<Field>>,
    defs: &'a HashMap<String, ComponentDef>,
    cache: HashMap<String, ComponentNode>,
    stack: Vec<String>,
}

impl<'a> ComponentBuilder<'a> {
    fn new(
        fields: &'a BTreeMap<String, Arc<Field>>,
        defs: &'a HashMap<String, ComponentDef>,
    ) -> Self {
        Self {
            fields,
            defs,
            cache: HashMap::new(),
            stack: Vec::new(),
        }
    }

    fn build_component(&mut self, name: &str) -> Option<ComponentNode> {
        if let Some(node) = self.cache.get(name) {
            return Some(node.clone());
        }
        if self.stack.contains(&name.to_string()) {
            eprintln!("warning: recursive component detected at {name}, skipping");
            return None;
        }
        let def = self.defs.get(name)?;
        self.stack.push(name.to_string());
        let node = self.build_component_from_def(def);
        self.cache.insert(name.to_string(), node.clone());
        self.stack.pop();
        Some(node)
    }

    fn build_component_from_def(&mut self, comp: &ComponentDef) -> ComponentNode {
        let mut node = ComponentNode {
            name: comp.name.clone(),
            fields: build_field_nodes(&comp.fields, self.fields),
            groups: Vec::new(),
            components: Vec::new(),
        };

        for cref in comp.components.iter() {
            if let Some(child) = self.build_component(&cref.name) {
                node.components.push(child);
            }
        }

        for group in comp.groups.iter() {
            node.groups.push(self.build_group_from_def(group));
        }

        node
    }

    fn build_group_from_def(&mut self, group: &GroupDef) -> GroupNode {
        let mut node = GroupNode {
            name: group.name.clone(),
            required: group.required.as_deref() == Some("Y"),
            fields: build_field_nodes(&group.fields, self.fields),
            components: Vec::new(),
            groups: Vec::new(),
        };

        for cref in group.components.iter() {
            if let Some(child) = self.build_component(&cref.name) {
                node.components.push(child);
            }
        }

        for sub_group in group.groups.iter() {
            node.groups.push(self.build_group_from_def(sub_group));
        }

        node
    }
}

fn build_message_node(
    msg: &Message,
    fields: &BTreeMap<String, Arc<Field>>,
    builder: &mut ComponentBuilder,
) -> MessageNode {
    let mut node = MessageNode {
        name: msg.name.clone(),
        msg_type: msg.msg_type.clone(),
        msg_cat: msg.msg_cat.clone(),
        fields: build_field_nodes(&msg.fields, fields),
        components: Vec::new(),
        groups: Vec::new(),
    };

    for cref in msg.components.iter() {
        if let Some(sub) = builder.build_component(&cref.name) {
            node.components.push(sub);
        }
    }

    for group in msg.groups.iter() {
        node.groups.push(builder.build_group_from_def(group));
    }

    node
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_message_fields() {
        let xml = "<message name='Test' msgtype='T' msgcat='app'><field name='A' required='Y'/><field name='B' required='N'/></message>";
        let msg: Message =
            quick_xml::de::from_str(xml).expect("message should parse with repeated fields");
        assert_eq!(msg.fields.len(), 2);
    }

    #[test]
    fn parse_message_with_components() {
        let xml = r#"<message name='IOI' msgtype='6' msgcat='app'>
   <field name='IOIID' required='Y' />
   <field name='IOITransType' required='Y' />
   <component name='Instrument' required='Y' />
</message>"#;
        let msg: Message = quick_xml::de::from_str(xml).expect("message with components");
        assert_eq!(msg.fields.len(), 2);
        assert_eq!(msg.components.len(), 1);
    }

    #[derive(Debug, Deserialize)]
    struct SimpleRoot {
        #[serde(rename = "item", default)]
        items: Vec<SimpleItem>,
    }

    #[derive(Debug, Deserialize)]
    struct SimpleItem {
        #[serde(rename = "@name")]
        name: String,
    }

    #[test]
    fn parse_simple_vec() {
        let xml = r#"<root><item name='one'/><item name='two'/></root>"#;
        let root: SimpleRoot = quick_xml::de::from_str(xml).expect("simple vec");
        assert_eq!(root.items.len(), 2);
        assert_eq!(root.items[0].name, "one");
        assert_eq!(root.items[1].name, "two");
    }
}
