// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2025 Steve Clarke <stephenlclarke@mac.com> - https://xyzzy.tools

pub mod colours;
pub mod display;
pub mod fixparser;
pub mod layout;
pub mod message_groups;
pub mod prettifier;
pub mod schema;
pub mod summary;
pub mod summary_pager;
pub mod tag_lookup;
pub mod validator;

pub use display::{
    DisplayStyle, display_component, display_message, list_all_components, list_all_messages,
    list_all_tags, print_component_columns, print_message_columns, print_tag_details,
    print_tags_in_columns,
};
pub use prettifier::{OutputStyle, PrettifyContext, disable_output_colours, prettify_files};
pub use schema::FixDictionary;
pub use tag_lookup::register_dictionary as register_fix_dictionary;
