// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2025 Steve Clarke <stephenlclarke@mac.com> - https://xyzzy.tools

//! Lightweight FIX tag obfuscator for sensitive identifiers.
//! Only the tags listed in `sensitive.rs` are touched, and replacements
//! remain stable for the lifetime of the process to keep logs consistent.

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

const SOH: char = '\u{0001}';
static FIX_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"8=FIX.*?10=\d{3}\u{0001}").expect("valid regex"));

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

    pub fn is_enabled(&self) -> bool {
        self.enabled
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

        let mut output = String::with_capacity(line.len());
        let mut last = 0usize;
        let mut changed = false;

        for matched in FIX_REGEX.find_iter(line) {
            output.push_str(&line[last..matched.start()]);
            let obfuscated = self.obfuscate_fix_message(matched.as_str());
            changed |= obfuscated != matched.as_str();
            output.push_str(&obfuscated);
            last = matched.end();
        }

        if last > 0 {
            output.push_str(&line[last..]);
            return if changed { output } else { line.to_string() };
        }

        self.obfuscate_fix_message(line)
    }

    fn obfuscate_fix_message(&self, msg: &str) -> String {
        let mut changed = false;
        let mut fragments = Vec::new();
        let trailing_soh = msg.ends_with(SOH);

        for fragment in msg.split(SOH) {
            if fragment.is_empty() {
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
            return msg.to_string();
        }

        refresh_fix_lengths(&mut fragments);

        let delim = SOH.to_string();
        let mut output = fragments.join(&delim);
        if trailing_soh {
            output.push(SOH);
        }
        output
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

fn refresh_fix_lengths(fragments: &mut [String]) {
    let body_idx = fragments
        .iter()
        .position(|fragment| tag_key(fragment) == Some("9"));
    let checksum_idx = fragments
        .iter()
        .rposition(|fragment| tag_key(fragment) == Some("10"));
    let (Some(body_idx), Some(checksum_idx)) = (body_idx, checksum_idx) else {
        return;
    };
    if checksum_idx <= body_idx {
        return;
    }

    let body_len: usize = fragments[body_idx + 1..checksum_idx]
        .iter()
        .map(|fragment| fragment.len() + 1)
        .sum();
    fragments[body_idx] = format!("9={body_len}");

    let checksum = calculate_checksum(fragments, checksum_idx);
    fragments[checksum_idx] = format!("10={checksum:03}");
}

fn tag_key(fragment: &str) -> Option<&str> {
    fragment.split_once('=').map(|(tag, _)| tag)
}

fn calculate_checksum(fragments: &[String], checksum_idx: usize) -> i32 {
    let mut total = 0i32;
    for fragment in &fragments[..checksum_idx] {
        total += fragment.bytes().map(i32::from).sum::<i32>();
        total += SOH as i32;
    }
    total % 256
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

    fn fix_message(fields: &[(&str, &str)]) -> String {
        let mut fragments = vec!["8=FIX.4.4".to_string(), "9=000".to_string()];
        fragments.extend(fields.iter().map(|(tag, value)| format!("{tag}={value}")));
        fragments.push("10=000".to_string());
        refresh_fix_lengths(&mut fragments);
        let mut msg = fragments.join(&SOH.to_string());
        msg.push(SOH);
        msg
    }

    #[test]
    fn disabled_obfuscator_returns_original_line_and_reset_is_noop() {
        let obfuscator = Obfuscator::from_sensitive_tags(&SENSITIVE_TAG_NAMES, false);
        let line = fix_message(&[("35", "0"), ("49", "ABC"), ("56", "DEF")]);
        assert_eq!(obfuscator.enabled_line(&line), line);
        assert_eq!(obfuscator.obfuscate_line(&line), line);
        obfuscator.reset();
        assert_eq!(obfuscator.enabled_line(&line), line);
    }

    #[test]
    fn obfuscate_line_leaves_non_sensitive_and_malformed_fragments_unchanged() {
        let obfuscator = Obfuscator::from_sensitive_tags(&SENSITIVE_TAG_NAMES, true);
        let line = "plain log line without FIX";
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
        let first = obfuscator.obfuscate_line(&fix_message(&[("35", "0"), ("49", "ABC")]));
        let second = obfuscator.obfuscate_line(&fix_message(&[("35", "0"), ("49", "DEF")]));
        assert_ne!(first, second);
        obfuscator.reset();
        let third = obfuscator.obfuscate_line(&fix_message(&[("35", "0"), ("49", "ABC")]));
        assert_eq!(first, third, "aliases should restart after reset");
    }

    #[test]
    fn obfuscate_line_preserves_mixed_log_context_and_repairs_fix_lengths() {
        let obfuscator = Obfuscator::from_sensitive_tags(&SENSITIVE_TAG_NAMES, true);
        let line = format!(
            "ts=2025-01-01T00:00:00Z {} trailing context",
            fix_message(&[("35", "A"), ("49", "BUY1"), ("56", "SELL1"), ("98", "0"),])
        );
        let obfuscated = obfuscator.obfuscate_line(&line);

        assert!(obfuscated.starts_with("ts=2025-01-01T00:00:00Z 8=FIX.4.4"));
        assert!(obfuscated.contains("49=SenderCompID0001"));
        assert!(obfuscated.contains("56=TargetCompID0001"));
        assert!(obfuscated.ends_with(" trailing context"));

        let start = obfuscated.find("8=FIX").expect("find FIX start");
        let end = obfuscated[start..]
            .find(&format!("{SOH} trailing context"))
            .expect("find FIX end");
        let fix_msg = &obfuscated[start..start + end + 1];
        let declared_len = fix_msg
            .split(SOH)
            .find_map(|fragment| fragment.strip_prefix("9="))
            .and_then(|value| value.parse::<usize>().ok())
            .expect("body length");
        let actual_len: usize = fix_msg
            .split(SOH)
            .skip_while(|fragment| !fragment.starts_with("9="))
            .skip(1)
            .take_while(|fragment| !fragment.starts_with("10="))
            .map(|fragment| fragment.len() + 1)
            .sum();
        let checksum = fix_msg
            .split(SOH)
            .find_map(|fragment| fragment.strip_prefix("10="))
            .and_then(|value| value.parse::<i32>().ok())
            .expect("checksum");

        assert_eq!(declared_len, actual_len);
        assert_eq!(
            checksum,
            calculate_checksum(
                &fix_msg
                    .split(SOH)
                    .filter(|fragment| !fragment.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>(),
                fix_msg
                    .split(SOH)
                    .position(|fragment| fragment.starts_with("10="))
                    .expect("checksum index")
            )
        );
    }
}
