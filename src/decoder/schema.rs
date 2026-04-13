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
    #[allow(dead_code)]
    pub fields: Vec<FieldNode>,
    #[allow(dead_code)]
    pub groups: Vec<GroupNode>,
    #[allow(dead_code)]
    pub components: Vec<ComponentNode>,
    pub entries: Vec<ContainerNode>,
}

#[derive(Debug, Clone)]
pub struct GroupNode {
    pub name: String,
    pub required: bool,
    #[allow(dead_code)]
    pub fields: Vec<FieldNode>,
    #[allow(dead_code)]
    pub components: Vec<ComponentNode>,
    #[allow(dead_code)]
    pub groups: Vec<GroupNode>,
    pub entries: Vec<ContainerNode>,
}

#[derive(Debug, Clone)]
pub struct MessageNode {
    pub name: String,
    pub msg_type: String,
    pub msg_cat: String,
    #[allow(dead_code)]
    pub fields: Vec<FieldNode>,
    #[allow(dead_code)]
    pub components: Vec<ComponentNode>,
    #[allow(dead_code)]
    pub groups: Vec<GroupNode>,
    pub entries: Vec<ContainerNode>,
}

#[derive(Debug, Clone)]
pub enum ContainerNode {
    Field(FieldNode),
    Group(GroupNode),
    Component(ComponentNode),
}

pub trait ContainerNodeVisitor<'a> {
    type Error;

    fn visit_field(&mut self, field: &'a FieldNode, indent_level: usize)
    -> Result<(), Self::Error>;

    fn enter_component(
        &mut self,
        component: &'a ComponentNode,
        indent_level: usize,
    ) -> Result<(), Self::Error>;

    fn leave_component(
        &mut self,
        _component: &'a ComponentNode,
        _indent_level: usize,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    fn enter_group(&mut self, group: &'a GroupNode, indent_level: usize)
    -> Result<(), Self::Error>;

    fn leave_group(
        &mut self,
        _group: &'a GroupNode,
        _indent_level: usize,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

pub fn walk_container_nodes<'a, V: ContainerNodeVisitor<'a>>(
    entries: &'a [ContainerNode],
    indent_level: usize,
    child_indent: usize,
    visitor: &mut V,
) -> Result<(), V::Error> {
    for entry in entries {
        match entry {
            ContainerNode::Field(field) => visitor.visit_field(field, indent_level)?,
            ContainerNode::Component(component) => {
                visitor.enter_component(component, indent_level)?;
                walk_container_nodes(
                    &component.entries,
                    indent_level + child_indent,
                    child_indent,
                    visitor,
                )?;
                visitor.leave_component(component, indent_level)?;
            }
            ContainerNode::Group(group) => {
                visitor.enter_group(group, indent_level)?;
                walk_container_nodes(
                    &group.entries,
                    indent_level + child_indent,
                    child_indent,
                    visitor,
                )?;
                visitor.leave_group(group, indent_level)?;
            }
        }
    }
    Ok(())
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
        let mut entries = self.build_container_nodes(&comp.entries);
        let (fields, components, groups) = if entries.is_empty() {
            (
                build_field_nodes(&comp.fields, self.fields),
                comp.components
                    .iter()
                    .filter_map(|cref| self.build_component(&cref.name))
                    .collect(),
                comp.groups
                    .iter()
                    .map(|group| self.build_group_from_def(group))
                    .collect(),
            )
        } else {
            split_container_nodes(&entries)
        };
        if entries.is_empty() {
            entries = compose_container_nodes(&fields, &components, &groups);
        }
        ComponentNode {
            name: comp.name.clone(),
            fields,
            groups,
            components,
            entries,
        }
    }

    fn build_group_from_def(&mut self, group: &GroupDef) -> GroupNode {
        let mut entries = self.build_container_nodes(&group.entries);
        let (fields, components, groups) = if entries.is_empty() {
            (
                build_field_nodes(&group.fields, self.fields),
                group
                    .components
                    .iter()
                    .filter_map(|cref| self.build_component(&cref.name))
                    .collect(),
                group
                    .groups
                    .iter()
                    .map(|sub_group| self.build_group_from_def(sub_group))
                    .collect(),
            )
        } else {
            split_container_nodes(&entries)
        };
        if entries.is_empty() {
            entries = compose_container_nodes(&fields, &components, &groups);
        }
        GroupNode {
            name: group.name.clone(),
            required: group.required.as_deref() == Some("Y"),
            fields,
            components,
            groups,
            entries,
        }
    }

    fn build_container_nodes(&mut self, entries: &[ContainerEntry]) -> Vec<ContainerNode> {
        let mut nodes = Vec::with_capacity(entries.len());
        for entry in entries {
            match entry {
                ContainerEntry::Field(field_ref) => {
                    if let Some(field) = self.fields.get(&field_ref.name) {
                        nodes.push(ContainerNode::Field(FieldNode {
                            required: field_ref.required.as_deref() == Some("Y"),
                            field: field.clone(),
                        }));
                    }
                }
                ContainerEntry::Group(group) => {
                    nodes.push(ContainerNode::Group(self.build_group_from_def(group)));
                }
                ContainerEntry::Component(component) => {
                    if let Some(child) = self.build_component(&component.name) {
                        nodes.push(ContainerNode::Component(child));
                    }
                }
            }
        }
        nodes
    }
}

fn split_container_nodes(
    entries: &[ContainerNode],
) -> (Vec<FieldNode>, Vec<ComponentNode>, Vec<GroupNode>) {
    let mut fields = Vec::new();
    let mut components = Vec::new();
    let mut groups = Vec::new();

    for entry in entries {
        match entry {
            ContainerNode::Field(field) => fields.push(field.clone()),
            ContainerNode::Component(component) => components.push(component.clone()),
            ContainerNode::Group(group) => groups.push(group.clone()),
        }
    }

    (fields, components, groups)
}

fn build_message_node(
    msg: &Message,
    fields: &BTreeMap<String, Arc<Field>>,
    builder: &mut ComponentBuilder,
) -> MessageNode {
    let mut entries = builder.build_container_nodes(&msg.entries);
    let (message_fields, components, groups) = split_container_nodes(&entries);
    let fields = if entries.is_empty() {
        build_field_nodes(&msg.fields, fields)
    } else {
        message_fields
    };
    let components = if entries.is_empty() {
        msg.components
            .iter()
            .filter_map(|cref| builder.build_component(&cref.name))
            .collect()
    } else {
        components
    };
    let groups = if entries.is_empty() {
        msg.groups
            .iter()
            .map(|group| builder.build_group_from_def(group))
            .collect()
    } else {
        groups
    };
    if entries.is_empty() {
        entries = compose_container_nodes(&fields, &components, &groups);
    }

    MessageNode {
        name: msg.name.clone(),
        msg_type: msg.msg_type.clone(),
        msg_cat: msg.msg_cat.clone(),
        fields,
        components,
        groups,
        entries,
    }
}

fn compose_container_nodes(
    fields: &[FieldNode],
    components: &[ComponentNode],
    groups: &[GroupNode],
) -> Vec<ContainerNode> {
    let mut entries = Vec::with_capacity(fields.len() + components.len() + groups.len());
    entries.extend(fields.iter().cloned().map(ContainerNode::Field));
    entries.extend(components.iter().cloned().map(ContainerNode::Component));
    entries.extend(groups.iter().cloned().map(ContainerNode::Group));
    entries
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

    #[test]
    fn schema_tree_preserves_message_entry_order() {
        let xml = r#"
<fix type='FIX' major='4' minor='4'>
  <header>
    <field name='BeginString' required='Y'/>
  </header>
  <trailer>
    <field name='CheckSum' required='Y'/>
  </trailer>
  <messages>
    <message name='Ordered' msgtype='Z' msgcat='app'>
      <field name='FirstField' required='Y'/>
      <component name='Instrument'/>
      <field name='SecondField' required='N'/>
    </message>
  </messages>
  <components>
    <component name='Instrument'>
      <field name='Symbol' required='Y'/>
    </component>
  </components>
  <fields>
    <field number='8' name='BeginString' type='STRING'/>
    <field number='10' name='CheckSum' type='STRING'/>
    <field number='35' name='MsgType' type='STRING'/>
    <field number='55' name='Symbol' type='STRING'/>
    <field number='1001' name='FirstField' type='STRING'/>
    <field number='1002' name='SecondField' type='STRING'/>
  </fields>
</fix>
"#;

        let dict = FixDictionary::from_xml(xml).expect("dictionary parses");
        let schema = SchemaTree::build(dict);
        let message = schema.messages.get("Ordered").expect("ordered message");

        let names: Vec<&str> = message
            .entries
            .iter()
            .map(|entry| match entry {
                ContainerNode::Field(field) => field.field.name.as_str(),
                ContainerNode::Component(component) => component.name.as_str(),
                ContainerNode::Group(group) => group.name.as_str(),
            })
            .collect();

        assert_eq!(names, vec!["FirstField", "Instrument", "SecondField"]);
    }
}
