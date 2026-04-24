# Repeating Group Samples

This directory contains a small checked-in set of valid FIX 4.4 sample
messages that exercise common repeating-group patterns.

Included examples:

- `new_order_single_parties.fix`: `NoPartyIDs(453)` with nested
  `NoPartySubIDs(802)`
- `new_order_single_preallocs.fix`: `NoAllocs(78)` in `PreAllocGrp`
- `allocation_instruction_orders.fix`: `NoOrders(73)` in `OrdAllocGrp`
- `market_data_snapshot_full_refresh.fix`: `NoMDEntries(268)` in `MDFullGrp`
- `all.fixlog`: aggregate corpus containing all of the above
- `manifest.json`: metadata for the generated files

Source material:

- FIX TagValue Encoding:
  <https://www.fixtrading.org/standards/tagvalue-online/>
- FIXimate `Parties`:
  <https://fiximate.fixtrading.org/legacy/en/FIX.4.4/body_49484950.html?find=PartyID>
- FIXimate `NewOrderSingle`:
  <https://fiximate.fixtrading.org/legacy/en/FIX.4.4/body_495268.html?find=Text>
- FIXimate `PreAllocGrp`:
  <https://fiximate.fixtrading.org/legacy/en/FIX.4.4/body_50485157.html?find=AllocQty>
- FIXimate `AllocationInstruction`:
  <https://fiximate.fixtrading.org/legacy/en/FIX.4.4/body_495774.html?find=AllocID>
- FIXimate `MarketDataSnapshotFullRefresh`:
  <https://fiximate.fixtrading.org/legacy/en/FIX.4.4/body_514887.html?find=MDFullGrp>
- FIXimate `MDFullGrp`:
  <https://fiximate.fixtrading.org/legacy/en/FIX.4.4/body_50485149.html?find=NoMDEntries>
- FIX JSON sample message:
  <https://www.fixtrading.org/standards/json-online/>

The messages are synthetic, but they are valid tag-value FIX messages with
recalculated `BodyLength (9)` and `CheckSum (10)`.

Regenerate the corpus with:

```bash
python3 ./ci/generate_repeating_group_samples.py
```
