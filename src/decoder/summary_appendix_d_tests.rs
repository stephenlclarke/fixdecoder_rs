use super::*;
use crate::decoder::fixparser::parse_fix;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpectedOrderedEvent {
    time: Option<String>,
    msg_type: Option<String>,
    state: String,
}

#[derive(Debug, Clone)]
struct ExpectedEventEntry {
    time: Option<String>,
    sequence: usize,
    fingerprint: String,
    event: ExpectedOrderedEvent,
}

#[derive(Debug, Clone)]
struct ExpectedRecord {
    key: String,
    order_id: Option<String>,
    cl_ord_id: Option<String>,
    orig_cl_ord_id: Option<String>,
    last_sequence: usize,
    events: Vec<ExpectedEventEntry>,
}

impl ExpectedRecord {
    fn new(key: String) -> Self {
        Self {
            key,
            order_id: None,
            cl_ord_id: None,
            orig_cl_ord_id: None,
            last_sequence: 0,
            events: Vec::new(),
        }
    }

    fn merge_ids(
        &mut self,
        order_id: Option<String>,
        cl_ord_id: Option<String>,
        orig_cl_ord_id: Option<String>,
    ) {
        if self.order_id.is_none() {
            self.order_id = order_id;
        }
        if self.cl_ord_id.is_none() {
            self.cl_ord_id = cl_ord_id;
        }
        if self.orig_cl_ord_id.is_none() {
            self.orig_cl_ord_id = orig_cl_ord_id;
        }
    }

    fn display_id(&self) -> String {
        self.order_id
            .clone()
            .or_else(|| self.cl_ord_id.clone())
            .unwrap_or_else(|| self.key.clone())
    }

    fn ordered_events(&self) -> Vec<ExpectedOrderedEvent> {
        let mut seen = HashSet::new();
        let mut events = self.events.clone();
        events.sort_by(|left, right| {
            compare_event_position(
                left.time.as_deref(),
                left.sequence,
                right.time.as_deref(),
                right.sequence,
            )
        });
        events
            .into_iter()
            .filter(|event| seen.insert(event.fingerprint.clone()))
            .map(|event| event.event)
            .collect()
    }

    fn state_path(&self) -> Vec<String> {
        collapsed_state_path(&self.ordered_events())
    }
}

fn appendix_fixture_messages(relative_path: &str) -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("examples")
        .join("appendix_d")
        .join(relative_path);

    fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read Appendix D fixture {}: {err}", path.display()))
        .lines()
        .filter(|line| line.starts_with("8=FIX"))
        .map(str::to_string)
        .collect()
}

fn parsed_field_map(msg: &str) -> HashMap<u32, String> {
    parse_fix(msg)
        .into_iter()
        .map(|field| (field.tag, field.value))
        .collect()
}

fn expected_event_fingerprint(fields: &HashMap<u32, String>) -> String {
    let time = fields
        .get(&60)
        .cloned()
        .or_else(|| fields.get(&52).cloned())
        .unwrap_or_default();

    [
        time,
        fields.get(&35).cloned().unwrap_or_default(),
        String::new(),
        fields.get(&150).cloned().unwrap_or_default(),
        fields.get(&39).cloned().unwrap_or_default(),
        fields.get(&1036).cloned().unwrap_or_default(),
        fields.get(&14).cloned().unwrap_or_default(),
        fields.get(&151).cloned().unwrap_or_default(),
        fields.get(&32).cloned().unwrap_or_default(),
        fields.get(&31).cloned().unwrap_or_default(),
        fields.get(&6).cloned().unwrap_or_default(),
        fields.get(&58).cloned().unwrap_or_default(),
        fields.get(&11).cloned().unwrap_or_default(),
        fields.get(&41).cloned().unwrap_or_default(),
    ]
    .join("\u{1f}")
}

fn collapsed_state_path(events: &[ExpectedOrderedEvent]) -> Vec<String> {
    let mut states = Vec::new();
    for event in events {
        if states.last() == Some(&event.state) {
            continue;
        }
        states.push(event.state.clone());
    }
    states
}

fn actual_ordered_events(record: &OrderRecord) -> Vec<ExpectedOrderedEvent> {
    record
        .ordered_events()
        .into_iter()
        .map(|event| ExpectedOrderedEvent {
            time: event.time.clone(),
            msg_type: event.msg_type.clone(),
            state: event.state.clone(),
        })
        .collect()
}

fn expected_resolve_key(
    aliases: &mut HashMap<String, String>,
    unknown_counter: &mut usize,
    order_id: Option<&str>,
    cl_ord_id: Option<&str>,
    orig_cl_ord_id: Option<&str>,
) -> String {
    for candidate in [order_id, cl_ord_id, orig_cl_ord_id].into_iter().flatten() {
        if let Some(key) = aliases.get(candidate) {
            return key.clone();
        }
    }

    if let Some(id) = order_id.or(cl_ord_id) {
        return id.to_string();
    }

    *unknown_counter += 1;
    format!("UNKNOWN-{}", *unknown_counter)
}

fn expected_records(messages: &[String]) -> Vec<ExpectedRecord> {
    let mut aliases = HashMap::new();
    let mut unknown_counter = 0usize;
    let mut records: HashMap<String, ExpectedRecord> = HashMap::new();

    for (sequence, msg) in messages.iter().enumerate() {
        let fields = parsed_field_map(msg);
        let dict = load_dictionary_with_override(msg, Some("FIX44"));
        let order_id = fields.get(&37).map(String::as_str);
        let cl_ord_id = fields.get(&11).map(String::as_str);
        let orig_cl_ord_id = fields.get(&41).map(String::as_str);

        if should_ignore_summary_message(
            fields.get(&35).map(String::as_str),
            order_id,
            cl_ord_id,
            orig_cl_ord_id,
            &dict,
            &aliases,
        ) {
            continue;
        }

        let key = expected_resolve_key(
            &mut aliases,
            &mut unknown_counter,
            order_id,
            cl_ord_id,
            orig_cl_ord_id,
        );

        for id in [
            fields.get(&37).cloned(),
            fields.get(&11).cloned(),
            fields.get(&41).cloned(),
        ]
        .into_iter()
        .flatten()
        {
            aliases.entry(id).or_insert_with(|| key.clone());
        }

        let time = fields
            .get(&60)
            .cloned()
            .or_else(|| fields.get(&52).cloned());
        let state = derive_state(
            fields.get(&150).map(String::as_str),
            fields.get(&39).map(String::as_str),
            fields.get(&151).map(String::as_str),
            fields.get(&1036).map(String::as_str),
        );

        let record = records
            .entry(key.clone())
            .or_insert_with(|| ExpectedRecord::new(key.clone()));
        record.merge_ids(
            fields.get(&37).cloned(),
            fields.get(&11).cloned(),
            fields.get(&41).cloned(),
        );
        record.last_sequence = sequence + 1;
        record.events.push(ExpectedEventEntry {
            time: time.clone(),
            sequence: sequence + 1,
            fingerprint: expected_event_fingerprint(&fields),
            event: ExpectedOrderedEvent {
                time,
                msg_type: fields.get(&35).cloned(),
                state,
            },
        });
    }

    let mut ordered: Vec<ExpectedRecord> = records.into_values().collect();
    ordered.sort_by(|left, right| left.key.cmp(&right.key));
    ordered
}

fn assert_appendix_summary_fixture(case_name: &str, relative_path: &str) {
    let messages = appendix_fixture_messages(relative_path);
    assert!(
        !messages.is_empty(),
        "{case_name}: expected Appendix D fixture to contain FIX messages"
    );

    let mut summary = OrderSummary::new('\u{0001}');
    let mut processed = Vec::new();

    for (index, msg) in messages.into_iter().enumerate() {
        processed.push(msg.clone());
        summary.record_message(&msg, Some("FIX44"));

        let expected_records = expected_records(&processed);
        let records = summary.ordered_records();
        assert_eq!(
            records.len(),
            expected_records.len(),
            "{case_name} after message {} should track the expected number of logical orders",
            index + 1
        );

        let actual_by_key: HashMap<&str, &OrderRecord> = records
            .iter()
            .map(|record| (record.key.as_str(), *record))
            .collect();
        for expected in &expected_records {
            let actual = actual_by_key.get(expected.key.as_str()).unwrap_or_else(|| {
                panic!(
                    "{case_name} after message {} should contain summary record {}",
                    index + 1,
                    expected.key
                )
            });
            let expected_events = expected.ordered_events();
            let expected_state_path = expected.state_path();

            assert_eq!(
                actual_ordered_events(actual),
                expected_events,
                "{case_name} after message {} should order timeline events correctly for {}",
                index + 1,
                expected.key
            );
            assert_eq!(
                actual.state_path(),
                expected_state_path,
                "{case_name} after message {} should build the expected state path for {}",
                index + 1,
                expected.key
            );
            assert_eq!(
                actual.ordered_messages().len(),
                expected_events.len(),
                "{case_name} after message {} should retain one rendered message per ordered event for {}",
                index + 1,
                expected.key
            );
        }

        let mut rendered = Vec::new();
        summary.render(&mut rendered).expect("render summary");
        let rendered = String::from_utf8(rendered).expect("summary utf8");

        assert!(
            rendered.contains("Order Summary"),
            "{case_name} after message {} should render the summary footer: {rendered}",
            index + 1
        );
        assert!(
            rendered.contains("Timeline:"),
            "{case_name} after message {} should render the detailed timeline: {rendered}",
            index + 1
        );
        for expected in &expected_records {
            let expected_flow = flow_label(&expected.state_path());
            assert!(
                rendered.contains(&expected.display_id()),
                "{case_name} after message {} should render order {}: {rendered}",
                index + 1,
                expected.display_id()
            );
            assert!(
                rendered.contains(&expected_flow),
                "{case_name} after message {} should render flow label {}: {rendered}",
                index + 1,
                expected_flow
            );
        }

        let sections = summary
            .build_paged_sections()
            .expect("build paged summary sections");
        assert_eq!(
            sections.len(),
            expected_records.len(),
            "{case_name} after message {} should expose the expected number of paged sections",
            index + 1
        );
        for expected in &expected_records {
            let expected_flow = flow_label(&expected.state_path());
            let section = sections.iter().find(|section| {
                section.summary.contains(&expected.display_id())
                    && section.summary.contains(&expected_flow)
            });
            let section = section.unwrap_or_else(|| {
                panic!(
                    "{case_name} after message {} should expose pager section for {}",
                    index + 1,
                    expected.display_id()
                )
            });
            assert!(
                section.summary.contains("Current Message"),
                "{case_name} after message {} should render the left-pane summary block",
                index + 1
            );
            assert!(
                section.detail.contains("Timeline:"),
                "{case_name} after message {} should render the detail pane timeline",
                index + 1
            );
        }

        let total_message_counts: usize = sections
            .last()
            .expect("paged section")
            .message_counts
            .iter()
            .map(|count| count.count)
            .sum();
        assert_eq!(
            total_message_counts,
            processed.len(),
            "{case_name} after message {} should count every processed Appendix D message",
            index + 1
        );
    }
}

macro_rules! appendix_summary_cases {
    ($(($name:ident, $relative_path:literal),)+) => {
        $(
            #[test]
            fn $name() {
                assert_appendix_summary_fixture(stringify!($name), $relative_path);
            }
        )+
    };
}

appendix_summary_cases! {
    (EXCHANGE_A_1_a_alt01, "exchange/a_1_a_alt01_a_1_a_filled_order_after_order_rests_on_book.fix"),
    (EXCHANGE_A_1_a_alt02, "exchange/a_1_a_alt02_a_1_a_filled_order_after_order_rests_on_book.fix"),
    (EXCHANGE_A_1_a_main, "exchange/a_1_a_main_a_1_a_filled_order_after_order_rests_on_book.fix"),
    (EXCHANGE_A_1_b_alt01, "exchange/a_1_b_alt01_a_1_b_part_filled_day_order_after_order_rests_on_book_done_for_day.fix"),
    (EXCHANGE_A_1_b_main, "exchange/a_1_b_main_a_1_b_part_filled_day_order_after_order_rests_on_book_done_for_day.fix"),
    (EXCHANGE_A_1_c_alt01, "exchange/a_1_c_alt01_a_1_c_order_filled_upon_hitting_the_book.fix"),
    (EXCHANGE_A_1_c_main, "exchange/a_1_c_main_a_1_c_order_filled_upon_hitting_the_book.fix"),
    (EXCHANGE_A_1_d_alt01, "exchange/a_1_d_alt01_a_1_d_order_partially_filled_upon_hitting_the_book.fix"),
    (EXCHANGE_A_1_d_main, "exchange/a_1_d_main_a_1_d_order_partially_filled_upon_hitting_the_book.fix"),
    (EXCHANGE_I_1_a_alt01, "exchange/i_1_a_alt01_i_1_a_fill_or_kill_order_that_cannot_be_filled.fix"),
    (EXCHANGE_I_1_a_alt02, "exchange/i_1_a_alt02_i_1_a_fill_or_kill_order_that_cannot_be_filled.fix"),
    (EXCHANGE_I_1_a_main, "exchange/i_1_a_main_i_1_a_fill_or_kill_order_that_cannot_be_filled.fix"),
    (EXCHANGE_I_1_b_alt01, "exchange/i_1_b_alt01_i_1_b_immediate_or_cancel_order_that_cannot_immediately_be_hit_completely.fix"),
    (EXCHANGE_I_1_b_alt02, "exchange/i_1_b_alt02_i_1_b_immediate_or_cancel_order_that_cannot_immediately_be_hit_completely.fix"),
    (EXCHANGE_I_1_b_main, "exchange/i_1_b_main_i_1_b_immediate_or_cancel_order_that_cannot_immediately_be_hit_completely.fix"),
    (GENERAL_A_1_a_alt01, "general/a_1_a_alt01_a_1_a_filled_order.fix"),
    (GENERAL_A_1_a_alt02, "general/a_1_a_alt02_a_1_a_filled_order.fix"),
    (GENERAL_A_1_a_main, "general/a_1_a_main_a_1_a_filled_order.fix"),
    (GENERAL_A_1_b_alt01, "general/a_1_b_alt01_a_1_b_part_filled_day_order_done_for_day.fix"),
    (GENERAL_A_1_b_main, "general/a_1_b_main_a_1_b_part_filled_day_order_done_for_day.fix"),
    (GENERAL_B_1_a_alt01, "general/b_1_a_alt01_b_1_a_cancel_request_issued_for_a_zero_filled_order.fix"),
    (GENERAL_B_1_a_alt02, "general/b_1_a_alt02_b_1_a_cancel_request_issued_for_a_zero_filled_order.fix"),
    (GENERAL_B_1_a_alt03, "general/b_1_a_alt03_b_1_a_cancel_request_issued_for_a_zero_filled_order.fix"),
    (GENERAL_B_1_a_main, "general/b_1_a_main_b_1_a_cancel_request_issued_for_a_zero_filled_order.fix"),
    (GENERAL_B_1_b_alt01, "general/b_1_b_alt01_b_1_b_cancel_request_issued_for_a_part_filled_order_executions_occur_whilst_cancel_request_is_active.fix"),
    (GENERAL_B_1_b_alt02, "general/b_1_b_alt02_b_1_b_cancel_request_issued_for_a_part_filled_order_executions_occur_whilst_cancel_request_is_active.fix"),
    (GENERAL_B_1_b_alt03, "general/b_1_b_alt03_b_1_b_cancel_request_issued_for_a_part_filled_order_executions_occur_whilst_cancel_request_is_active.fix"),
    (GENERAL_B_1_b_main, "general/b_1_b_main_b_1_b_cancel_request_issued_for_a_part_filled_order_executions_occur_whilst_cancel_request_is_active.fix"),
    (GENERAL_B_1_c_alt01, "general/b_1_c_alt01_b_1_c_cancel_request_issued_for_an_order_that_becomes_filled_before_cancel_request_can_be_accepted.fix"),
    (GENERAL_B_1_c_alt02, "general/b_1_c_alt02_b_1_c_cancel_request_issued_for_an_order_that_becomes_filled_before_cancel_request_can_be_accepted.fix"),
    (GENERAL_B_1_c_main, "general/b_1_c_main_b_1_c_cancel_request_issued_for_an_order_that_becomes_filled_before_cancel_request_can_be_accepted.fix"),
    (GENERAL_B_1_d_main, "general/b_1_d_main_b_1_d_cancel_request_issued_for_an_order_that_has_not_yet_been_acknowledged.fix"),
    (GENERAL_B_1_e_main, "general/b_1_e_main_b_1_e_cancel_request_issued_for_an_order_that_has_not_yet_been_acknowledged_the_acknowledgment_and_the_cancel_request_cross.fix"),
    (GENERAL_B_1_f_main, "general/b_1_f_main_b_1_f_cancel_request_issued_for_an_unknown_order.fix"),
    (GENERAL_C_1_a_alt01, "general/c_1_a_alt01_c_1_a_zero_filled_order_cancel_replace_request_issued_to_increase_order_qty.fix"),
    (GENERAL_C_1_a_alt02, "general/c_1_a_alt02_c_1_a_zero_filled_order_cancel_replace_request_issued_to_increase_order_qty.fix"),
    (GENERAL_C_1_a_alt03, "general/c_1_a_alt03_c_1_a_zero_filled_order_cancel_replace_request_issued_to_increase_order_qty.fix"),
    (GENERAL_C_1_a_main, "general/c_1_a_main_c_1_a_zero_filled_order_cancel_replace_request_issued_to_increase_order_qty.fix"),
    (GENERAL_C_1_b_alt01, "general/c_1_b_alt01_c_1_b_part_filled_order_followed_by_cancel_replace_request_to_increase_order_qty_execution_occurs_whilst_order_is_pending_replace.fix"),
    (GENERAL_C_1_b_alt02, "general/c_1_b_alt02_c_1_b_part_filled_order_followed_by_cancel_replace_request_to_increase_order_qty_execution_occurs_whilst_order_is_pending_replace.fix"),
    (GENERAL_C_1_b_alt03, "general/c_1_b_alt03_c_1_b_part_filled_order_followed_by_cancel_replace_request_to_increase_order_qty_execution_occurs_whilst_order_is_pending_replace.fix"),
    (GENERAL_C_1_b_main, "general/c_1_b_main_c_1_b_part_filled_order_followed_by_cancel_replace_request_to_increase_order_qty_execution_occurs_whilst_order_is_pending_replace.fix"),
    (GENERAL_C_1_c_alt01, "general/c_1_c_alt01_c_1_c_filled_order_followed_by_cancel_replace_request_to_increase_order_quantity.fix"),
    (GENERAL_C_1_c_alt02, "general/c_1_c_alt02_c_1_c_filled_order_followed_by_cancel_replace_request_to_increase_order_quantity.fix"),
    (GENERAL_C_1_c_alt03, "general/c_1_c_alt03_c_1_c_filled_order_followed_by_cancel_replace_request_to_increase_order_quantity.fix"),
    (GENERAL_C_1_c_main, "general/c_1_c_main_c_1_c_filled_order_followed_by_cancel_replace_request_to_increase_order_quantity.fix"),
    (GENERAL_C_2_a_alt01, "general/c_2_a_alt01_c_2_a_cancel_replace_request_not_for_quantity_change_is_rejected_as_a_fill_has_occurred.fix"),
    (GENERAL_C_2_a_main, "general/c_2_a_main_c_2_a_cancel_replace_request_not_for_quantity_change_is_rejected_as_a_fill_has_occurred.fix"),
    (GENERAL_C_3_a_alt01, "general/c_3_a_alt01_c_3_a_cancel_replace_request_sent_whilst_execution_is_being_reported_the_requested_order_qty_exceeds_the_cum_qty_order_is_replaced_then_filled.fix"),
    (GENERAL_C_3_a_alt02, "general/c_3_a_alt02_c_3_a_cancel_replace_request_sent_whilst_execution_is_being_reported_the_requested_order_qty_exceeds_the_cum_qty_order_is_replaced_then_filled.fix"),
    (GENERAL_C_3_a_alt03, "general/c_3_a_alt03_c_3_a_cancel_replace_request_sent_whilst_execution_is_being_reported_the_requested_order_qty_exceeds_the_cum_qty_order_is_replaced_then_filled.fix"),
    (GENERAL_C_3_a_main, "general/c_3_a_main_c_3_a_cancel_replace_request_sent_whilst_execution_is_being_reported_the_requested_order_qty_exceeds_the_cum_qty_order_is_replaced_then_filled.fix"),
    (GENERAL_C_3_b_alt01, "general/c_3_b_alt01_c_3_b_cancel_replace_request_sent_whilst_execution_is_being_reported_the_requested_order_qty_equals_the_cum_qty_order_qty_is_amended_to_cum_qty.fix"),
    (GENERAL_C_3_b_main, "general/c_3_b_main_c_3_b_cancel_replace_request_sent_whilst_execution_is_being_reported_the_requested_order_qty_equals_the_cum_qty_order_qty_is_amended_to_cum_qty.fix"),
    (GENERAL_C_3_c_alt01, "general/c_3_c_alt01_c_3_c_cancel_replace_request_sent_whilst_execution_is_being_reported_the_requested_order_qty_is_below_cum_qty_order_qty_is_amended_to_cum_qty.fix"),
    (GENERAL_C_3_c_main, "general/c_3_c_main_c_3_c_cancel_replace_request_sent_whilst_execution_is_being_reported_the_requested_order_qty_is_below_cum_qty_order_qty_is_amended_to_cum_qty.fix"),
    (GENERAL_D_1_a_alt01, "general/d_1_a_alt01_d_1_a_one_cancel_replace_request_is_issued_which_is_accepted_another_one_is_issued_which_is_also_accepted.fix"),
    (GENERAL_D_1_a_main, "general/d_1_a_main_d_1_a_one_cancel_replace_request_is_issued_which_is_accepted_another_one_is_issued_which_is_also_accepted.fix"),
    (GENERAL_D_1_b_alt01, "general/d_1_b_alt01_d_1_b_one_cancel_replace_request_is_issued_which_is_rejected_before_order_becomes_pending_replace_then_another_one_is_issued_which_is_accepted.fix"),
    (GENERAL_D_1_b_main, "general/d_1_b_main_d_1_b_one_cancel_replace_request_is_issued_which_is_rejected_before_order_becomes_pending_replace_then_another_one_is_issued_which_is_accepted.fix"),
    (GENERAL_D_1_c_alt01, "general/d_1_c_alt01_d_1_c_one_cancel_replace_request_is_issued_which_is_rejected_after_it_is_in_pending_replace_then_another_one_is_issued_which_is_accepted.fix"),
    (GENERAL_D_1_c_alt02, "general/d_1_c_alt02_d_1_c_one_cancel_replace_request_is_issued_which_is_rejected_after_it_is_in_pending_replace_then_another_one_is_issued_which_is_accepted.fix"),
    (GENERAL_D_1_c_main, "general/d_1_c_main_d_1_c_one_cancel_replace_request_is_issued_which_is_rejected_after_it_is_in_pending_replace_then_another_one_is_issued_which_is_accepted.fix"),
    (GENERAL_D_2_a_main, "general/d_2_a_main_d_2_a_one_cancel_replace_request_is_issued_followed_immediately_by_another_broker_processes_sequentially.fix"),
    (GENERAL_D_2_b_main, "general/d_2_b_main_d_2_b_one_cancel_replace_request_is_issued_followed_immediately_by_another_broker_processes_pending_replaces_before_replaces.fix"),
    (GENERAL_D_2_c_main, "general/d_2_c_main_d_2_c_one_cancel_replace_request_is_issued_followed_immediately_by_another_both_are_rejected.fix"),
    (GENERAL_D_2_d_main, "general/d_2_d_main_d_2_d_one_cancel_replace_request_is_issued_followed_immediately_by_another_broker_rejects_the_second_as_order_is_pending_replace.fix"),
    (GENERAL_E_1_a_main, "general/e_1_a_main_e_1_a_telephoned_order.fix"),
    (GENERAL_E_1_b_alt01, "general/e_1_b_alt01_e_1_b_unsolicited_cancel_of_a_part_filled_order.fix"),
    (GENERAL_E_1_b_main, "general/e_1_b_main_e_1_b_unsolicited_cancel_of_a_part_filled_order.fix"),
    (GENERAL_E_1_c_alt01, "general/e_1_c_alt01_e_1_c_unsolicited_replacement_of_a_part_filled_order.fix"),
    (GENERAL_E_1_c_main, "general/e_1_c_main_e_1_c_unsolicited_replacement_of_a_part_filled_order.fix"),
    (GENERAL_E_1_d_alt01, "general/e_1_d_alt01_e_1_d_unsolicited_reduction_of_order_quantity_by_sell_side_e_g_for_us_ecns_to_communicate_nasdaq_selectnet_declines.fix"),
    (GENERAL_E_1_d_main, "general/e_1_d_main_e_1_d_unsolicited_reduction_of_order_quantity_by_sell_side_e_g_for_us_ecns_to_communicate_nasdaq_selectnet_declines.fix"),
    (GENERAL_E_1_e_alt01, "general/e_1_e_alt01_e_1_e_unsolicited_cancel_of_a_cancel_if_not_best_order.fix"),
    (GENERAL_E_1_e_main, "general/e_1_e_main_e_1_e_unsolicited_cancel_of_a_cancel_if_not_best_order.fix"),
    (GENERAL_E_1_f_alt01, "general/e_1_f_alt01_e_1_f_order_is_sent_to_exchange_held_waiting_for_activation_and_then_activated.fix"),
    (GENERAL_E_1_f_main, "general/e_1_f_main_e_1_f_order_is_sent_to_exchange_held_waiting_for_activation_and_then_activated.fix"),
    (GENERAL_F_1_a_main, "general/f_1_a_main_f_1_a_order_rejected_due_to_duplicate_clordid.fix"),
    (GENERAL_F_1_b_main, "general/f_1_b_main_f_1_b_possresend_and_duplicate_clordid.fix"),
    (GENERAL_F_1_c_main, "general/f_1_c_main_f_1_c_order_rejected_because_the_order_has_already_been_verbally_submitted.fix"),
    (GENERAL_G_1_a_main, "general/g_1_a_main_g_1_a_order_status_request_rejected_for_unknown_order.fix"),
    (GENERAL_G_1_b_alt01, "general/g_1_b_alt01_g_1_b_transmitting_a_cms_style_nothing_done_in_response_to_a_status_request.fix"),
    (GENERAL_G_1_b_main, "general/g_1_b_main_g_1_b_transmitting_a_cms_style_nothing_done_in_response_to_a_status_request.fix"),
    (GENERAL_G_1_c_alt01, "general/g_1_c_alt01_g_1_c_order_sent_immediately_followed_by_a_status_request_subsequent_status_requests_sent_during_life_of_order.fix"),
    (GENERAL_G_1_c_main, "general/g_1_c_main_g_1_c_order_sent_immediately_followed_by_a_status_request_subsequent_status_requests_sent_during_life_of_order.fix"),
    (GENERAL_H_1_a_main, "general/h_1_a_main_h_1_a_gtc_order_partially_filled_restated_renewed_and_partially_filled_the_following_day.fix"),
    (GENERAL_H_1_b_main, "general/h_1_b_main_h_1_b_gtc_order_with_partial_fill_a_2_1_stock_split_then_a_partial_fill_and_fill_the_following_day.fix"),
    (GENERAL_H_1_c_alt01, "general/h_1_c_alt01_h_1_c_gtc_order_partially_filled_restated_renewed_and_canceled_the_following_day.fix"),
    (GENERAL_H_1_c_alt02, "general/h_1_c_alt02_h_1_c_gtc_order_partially_filled_restated_renewed_and_canceled_the_following_day.fix"),
    (GENERAL_H_1_c_main, "general/h_1_c_main_h_1_c_gtc_order_partially_filled_restated_renewed_and_canceled_the_following_day.fix"),
    (GENERAL_H_1_d_alt01, "general/h_1_d_alt01_h_1_d_gtc_order_partially_filled_restated_renewed_followed_by_replace_request_to_increase_quantity.fix"),
    (GENERAL_H_1_d_alt02, "general/h_1_d_alt02_h_1_d_gtc_order_partially_filled_restated_renewed_followed_by_replace_request_to_increase_quantity.fix"),
    (GENERAL_H_1_d_main, "general/h_1_d_main_h_1_d_gtc_order_partially_filled_restated_renewed_followed_by_replace_request_to_increase_quantity.fix"),
    (GENERAL_I_1_a_alt01, "general/i_1_a_alt01_i_1_a_fill_or_kill_order_cannot_be_filled.fix"),
    (GENERAL_I_1_a_main, "general/i_1_a_main_i_1_a_fill_or_kill_order_cannot_be_filled.fix"),
    (GENERAL_I_1_b_alt01, "general/i_1_b_alt01_i_1_b_immediate_or_cancel_order_that_cannot_be_immediately_hit.fix"),
    (GENERAL_I_1_b_main, "general/i_1_b_main_i_1_b_immediate_or_cancel_order_that_cannot_be_immediately_hit.fix"),
    (GENERAL_J_1_a_alt01, "general/j_1_a_alt01_j_1_a_filled_order_followed_by_correction_and_cancellation_of_executions.fix"),
    (GENERAL_J_1_a_main, "general/j_1_a_main_j_1_a_filled_order_followed_by_correction_and_cancellation_of_executions.fix"),
    (GENERAL_J_1_b_main, "general/j_1_b_main_j_1_b_a_canceled_order_followed_by_a_busted_execution_and_a_new_execution.fix"),
    (GENERAL_J_1_c_main, "general/j_1_c_main_j_1_c_gtc_order_partially_filled_restated_renewed_and_partially_filled_the_following_day_with_corrections_of_quantity_on_both_executions.fix"),
    (GENERAL_J_1_d_main, "general/j_1_d_main_j_1_d_part_filled_order_done_for_day_followed_by_trade_correction_and_bust.fix"),
    (GENERAL_K_1_a_alt01, "general/k_1_a_alt01_k_1_a_trading_halt_reinstate.fix"),
    (GENERAL_K_1_a_main, "general/k_1_a_main_k_1_a_trading_halt_reinstate.fix"),
    (GENERAL_K_1_b_alt01, "general/k_1_b_alt01_k_1_b_trading_halt_cancel.fix"),
    (GENERAL_K_1_b_main, "general/k_1_b_main_k_1_b_trading_halt_cancel.fix"),
    (GENERAL_L_1_a_alt01, "general/l_1_a_alt01_l_1_a_transmitting_a_guarantee_of_execution_prior_to_execution.fix"),
    (GENERAL_L_1_a_main, "general/l_1_a_main_l_1_a_transmitting_a_guarantee_of_execution_prior_to_execution.fix"),
    (GENERAL_L_1_b_alt01, "general/l_1_b_alt01_l_1_b_use_of_cashorderqty.fix"),
    (GENERAL_L_1_b_main, "general/l_1_b_main_l_1_b_use_of_cashorderqty.fix"),
}
