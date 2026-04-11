// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2025 Steve Clarke <stephenlclarke@mac.com> - https://xyzzy.tools

//! Lightweight FIX tag obfuscator for sensitive identifiers.
//! Only the tags listed in `sensitive.rs` are touched, and replacements
//! remain stable for the lifetime of the process to keep logs consistent.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

const SOH: char = '\u{0001}';

/// Shared mutable state for the obfuscator.  Holds the mapping between
/// original FIX tag values and their aliases so outputs remain consistent.
#[derive(Default)]
struct ObfuscatorState {
    alias_map: HashMap<(u32, String), String>,
    counters: HashMap<u32, u32>,
}

/// Public obfuscator facade wrapping the sensitive tag map and alias state.
pub struct Obfuscator {
    enabled: bool,
    tags: HashMap<u32, String>,
    state: Mutex<ObfuscatorState>,
}

impl Obfuscator {
    /// Build a new obfuscator from the generated sensitive-tag list and the
    /// user’s chosen on/off flag.
    pub fn from_sensitive_tags(tags: &BTreeMap<u32, &'static str>, enabled: bool) -> Self {
        let mut copy = HashMap::with_capacity(tags.len());
        for (tag, name) in tags {
            copy.insert(*tag, (*name).to_string());
        }
        Self {
            enabled,
            tags: copy,
            state: Mutex::new(ObfuscatorState::default()),
        }
    }

    /// Process a FIX line and return either the original content (when
    /// obfuscation is disabled) or a redacted version.
    pub fn enabled_line(&self, line: &str) -> String {
        if !self.enabled {
            return line.to_string();
        }
        self.obfuscate_line(line)
    }

    /// Clear all cached aliases to start a new obfuscation session (e.g. per file).
    pub fn reset(&self) {
        if !self.enabled {
            return;
        }
        let mut state = self.state.lock().expect("obfuscator mutex poisoned");
        state.alias_map.clear();
        state.counters.clear();
    }

    /// Core obfuscation routine shared by the public wrapper.  Keeps the
    /// state machine private whilst making it easy to test.
    pub fn obfuscate_line(&self, line: &str) -> String {
        if !self.enabled {
            return line.to_string();
        }

        let mut changed = false;
        let mut fragments: Vec<String> = Vec::new();

        for fragment in line.split(SOH) {
            if fragment.is_empty() {
                fragments.push(String::new());
                continue;
            }

            if let Some((tag_str, value)) = split_once(fragment)
                && let Ok(tag) = tag_str.parse::<u32>()
                && let Some(name) = self.tags.get(&tag)
            {
                let alias = self.next_alias(tag, value, name);
                fragments.push(format!("{tag}={alias}"));
                changed = true;
                continue;
            }

            fragments.push(fragment.to_string());
        }

        if !changed {
            return line.to_string();
        }

        let delim = SOH.to_string();
        fragments.join(&delim)
    }

    /// Return the alias for a tag/value pair, creating a new entry the first
    /// time we see that combination.
    fn next_alias(&self, tag: u32, value: &str, name: &str) -> String {
        let mut state = self.state.lock().expect("obfuscator mutex poisoned");
        let key = (tag, value.to_string());

        if let Some(alias) = state.alias_map.get(&key) {
            return alias.clone();
        }

        let counter = state.counters.entry(tag).or_insert(0);
        *counter += 1;
        let alias = format!("{name}{:04}", counter);
        state.alias_map.insert(key, alias.clone());

        alias
    }
}

/// Tiny helper that splits a FIX fragment on `=` or SOH so we can extract
/// tag/value pairs without extra allocations.
fn split_once(fragment: &str) -> Option<(&str, &str)> {
    if let Some(idx) = fragment.find('=') {
        return Some((&fragment[..idx], &fragment[idx + 1..]));
    }
    if let Some(idx) = fragment.find(SOH) {
        return Some((&fragment[..idx], &fragment[idx + 1..]));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fix::SENSITIVE_TAG_NAMES;

    #[test]
    fn disabled_obfuscator_returns_original_line_and_reset_is_noop() {
        let obfuscator = Obfuscator::from_sensitive_tags(&SENSITIVE_TAG_NAMES, false);
        let line = "49=ABC\u{0001}999=plain\u{0001}";
        assert_eq!(obfuscator.enabled_line(line), line);
        assert_eq!(obfuscator.obfuscate_line(line), line);
        obfuscator.reset();
        assert_eq!(obfuscator.enabled_line(line), line);
    }

    #[test]
    fn obfuscate_line_leaves_non_sensitive_and_malformed_fragments_unchanged() {
        let obfuscator = Obfuscator::from_sensitive_tags(&SENSITIVE_TAG_NAMES, true);
        let line = "foo\u{0001}999=plain\u{0001}";
        assert_eq!(obfuscator.obfuscate_line(line), line);
    }

    #[test]
    fn split_once_accepts_equals_and_soh_delimiters() {
        assert_eq!(split_once("49=ABC"), Some(("49", "ABC")));
        assert_eq!(split_once("49\u{0001}ABC"), Some(("49", "ABC")));
        assert_eq!(split_once("no-delimiter"), None);
    }

    #[test]
    fn reset_starts_aliases_over() {
        let obfuscator = Obfuscator::from_sensitive_tags(&SENSITIVE_TAG_NAMES, true);
        let first = obfuscator.obfuscate_line("49=ABC\u{0001}");
        let second = obfuscator.obfuscate_line("49=DEF\u{0001}");
        assert_ne!(first, second);
        obfuscator.reset();
        let third = obfuscator.obfuscate_line("49=ABC\u{0001}");
        assert_eq!(first, third, "aliases should restart after reset");
    }
}
