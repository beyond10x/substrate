---
format: aep.planning-md/3
id: approval-record:wave-2026-11-18-box-friction
kind: approval-record
status: active
title: 'Wave 2026-11-18: box friction #223 and #224 under the operator''s standing grant'
relations:
- decides: story:base-view-carries-etc-alternatives
- decides: story:tmp-takes-an-operator-size
revision: 3
transitions:
- {from: "draft", to: "active", at: "2026-11-18T08:23:22Z", actor: "human:unknown", revision: 3}
---
# Wave 2026-11-18: box friction #223 and #224 under the operator's standing grant

Charter `substrate` (dispatch DSP-20261118-02), goal `box-friction-fixed`: the operator's
decision of 2026-11-17 lets the controller decide and merge without asking.

Stories: `story:base-view-carries-etc-alternatives`, `story:tmp-takes-an-operator-size`.

Width: 1 tree, both stories edit the one bubblewrap argv in `crates/substrate-host/src/process.rs`.
