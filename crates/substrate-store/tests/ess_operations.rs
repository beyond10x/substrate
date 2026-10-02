//! Executes the committed ESS suite against the production `SQLite` Store.
//! Only the suite's closed step vocabulary is admitted. Unknown steps fail the run.
#![allow(clippy::too_many_lines)]
use serde_json::{Value, json};
use std::collections::BTreeMap;
use substrate_store::{NewOperation, Reservation, Scope, Store, StoreError};
use substrate_wire::{ErrorClass, ErrorDetail, OperationState};

const SUITE: &str = include_str!("../../../spec/operations-suite.json");
const PREFIX: &str = "substrate.operations.";
const VIEW: &str = "substrate.operations.Operations";
const TIME: &str = "2026-10-02T00:00:00Z";

struct Target {
    store: Store,
    scope: Scope,
    ids: Vec<String>,
    instances: BTreeMap<String, Value>,
    events: BTreeMap<String, Value>,
    command: String,
    outcome: String,
    error: Option<String>,
    rows: Vec<Value>,
    snapshot: Option<Value>,
}
fn text(value: &Value) -> &str {
    value.as_str().expect("suite requires text")
}
impl Target {
    fn new() -> Self {
        Self {
            store: Store::open(":memory:").unwrap(),
            scope: Scope {
                deployment: "ess".into(),
                subject: "local:ess".into(),
            },
            ids: vec![],
            instances: BTreeMap::new(),
            events: BTreeMap::new(),
            command: String::new(),
            outcome: String::new(),
            error: None,
            rows: vec![],
            snapshot: None,
        }
    }
    fn value(&self, expression: &Value) -> Value {
        match text(&expression["kind"]) {
            "literal" => expression["value"].clone(),
            "instance" => self.instances[text(&expression["instance"])].clone(),
            "observed" => {
                self.events[text(&expression["event"])][text(&expression["field"])].clone()
            }
            other => panic!("unsupported ESS expression {other}"),
        }
    }
    fn execute(&mut self, command: &str, expressions: &Value) {
        let input: BTreeMap<String, Value> = expressions
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), self.value(v)))
            .collect();
        self.command = command.into();
        self.events.clear();
        self.error = None;
        let before = self.store.stream_position(&self.scope).unwrap().2;
        let suffix = command.strip_prefix(PREFIX).unwrap();
        let id = input
            .get("operation")
            .map(text)
            .map_or_else(|| format!("ess-{}", self.ids.len()), str::to_owned);
        let result = match suffix {
            "Reserve" => {
                let new = NewOperation {
                    scope: self.scope.clone(),
                    operation: id.clone(),
                    operation_kind: text(&input["operation_kind"]).into(),
                    request_hash: text(&input["request_hash"]).into(),
                    actor: text(&input["actor"]).into(),
                    accepted_at: TIME.into(),
                    capability_snapshot: None,
                    principal: None,
                    grant_ref: None,
                    platform_principal: None,
                    resource: None,
                };
                assert_eq!(self.store.reserve(&new).unwrap(), Reservation::Accepted);
                self.ids.push(id.clone());
                self.outcome = "accepted".into();
                Ok(())
            }
            "MarkDispatchUnknown" => {
                self.outcome = "unknown".into();
                self.store
                    .mark_dispatch_unknown(&self.scope, &id, TIME, "operation", &id)
            }
            "CompleteSuccess" => {
                self.outcome = "terminal".into();
                self.store.complete_success(
                    &self.scope,
                    &id,
                    TIME,
                    200,
                    None,
                    &json!({"finished":true}),
                )
            }
            "CompleteError" => {
                self.outcome = "terminal".into();
                self.store.complete_error(
                    &self.scope,
                    &id,
                    TIME,
                    500,
                    None,
                    &ErrorDetail {
                        class: ErrorClass::Failed,
                        code: "ess.test-error".into(),
                        message: "test driver error".into(),
                        retriable: false,
                        address: None,
                        operation: Some(id.clone()),
                    },
                )
            }
            other => panic!("unsupported command {other}"),
        };
        if let Err(error) = result {
            assert!(
                matches!(error, StoreError::NotAccepted(_)),
                "unexpected error {error}"
            );
            self.error = Some(format!("{PREFIX}NotAccepted"));
            self.outcome = if self.store.operation(&self.scope, &id).unwrap().is_some() {
                "not-accepted"
            } else {
                "absent"
            }
            .into();
        }
        // Observe persisted events, never manufacture an event from the expected outcome.
        for event in self
            .store
            .events(&self.scope, None, 100)
            .unwrap()
            .unwrap()
            .items
            .into_iter()
            .filter(|e| e.seq > before)
        {
            let name = match event.transition.as_str() {
                "operation.accepted" => "Accepted",
                "operation.unknown" => "Unknown",
                "operation.terminal" => "Terminal",
                "operation.failed" => "Failed",
                other => panic!("unexpected event {other}"),
            };
            assert!(
                self.events
                    .insert(format!("{PREFIX}{name}"), event.observation)
                    .is_none(),
                "duplicate event"
            );
        }
    }
    fn query(&mut self, view: &str) {
        assert_eq!(view, VIEW);
        self.rows=self.ids.iter().map(|id|{
            let record=self.store.operation(&self.scope,id).unwrap().expect("durable row");
            let state=match record.state {OperationState::Accepted=>"Accepted",OperationState::Unknown=>"Unknown",OperationState::Terminal=>"Terminal",OperationState::Refused=>panic!("unexpected refused row")};
            json!({"operation":record.operation,"state":state,"actor":record.actor,"request_hash":record.request_hash,"operation_kind":record.operation_kind})
        }).collect();
    }
    fn step(&mut self, step: &Value) {
        match text(&step["step"]) {
            "execute_command" => self.execute(text(&step["command"]), &step["input"]),
            "expect_outcome" => {
                assert_eq!(self.command, text(&step["outcome"]["command"]));
                assert_eq!(self.outcome, text(&step["outcome"]["outcome"]));
            }
            "expect_error" => assert_eq!(self.error.as_deref(), Some(text(&step["error"]))),
            "capture_instance" => {
                assert_eq!(step["entity"], format!("{PREFIX}AcceptedOperation"));
                let value = self.events[text(&step["event"])][text(&step["field"])].clone();
                assert!(value.is_string());
                self.instances.insert(text(&step["instance"]).into(), value);
            }
            "query_view" => self.query(text(&step["view"])),
            "expect_event" => {
                let event = self
                    .events
                    .get(text(&step["event"]))
                    .expect("persisted event");
                for (name, shape) in step["shape"].as_object().unwrap() {
                    assert_eq!(shape, &json!({"holds":"primitive","kind":"string"}));
                    assert!(event[name].is_string(), "event field {name}");
                }
                if let Some(payload) = step.get("payload") {
                    for (name, value) in payload.as_object().unwrap() {
                        assert_eq!(&event[name], value);
                    }
                }
            }
            "expect_no_event" => assert!(!self.events.contains_key(text(&step["event"]))),
            "expect_no_events" => assert!(self.events.is_empty()),
            "expect_view" => {
                assert_eq!(step["view"], VIEW);
                assert_eq!(step["expectation"]["expect"], "contains");
                let expected = step["expectation"]["fields"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k, self.value(v)))
                    .collect::<BTreeMap<_, _>>();
                assert!(
                    self.rows
                        .iter()
                        .any(|row| expected.iter().all(|(k, v)| &row[*k] == v)),
                    "view does not contain {expected:?}; observed {:?}",
                    self.rows
                );
            }
            "snapshot_complete_subject" => {
                assert_eq!(step["view"], VIEW);
                assert_eq!(step["shape"]["identity_field"], "operation");
                let id = self.value(&step["subject"]["operation"]);
                let row = self
                    .rows
                    .iter()
                    .find(|r| r["operation"] == id)
                    .expect("snapshot subject");
                assert_eq!(
                    row.as_object().unwrap().len(),
                    step["shape"]["fields"].as_array().unwrap().len()
                );
                for field in step["shape"]["fields"].as_array().unwrap() {
                    let observed = &row[text(&field["name"])];
                    assert!(observed.is_string());
                    let kind = text(&field["type"]);
                    if kind != "String" {
                        assert_eq!(kind, "substrate.operations.AcceptedOperation.State");
                        let declaration = &step["shape"]["declarations"][kind];
                        assert_eq!(declaration["kind"], "enum");
                        assert!(
                            declaration["variants"]
                                .as_array()
                                .unwrap()
                                .contains(observed)
                        );
                    }
                }
                self.snapshot = Some(row.clone());
            }
            "expect_complete_subject_unchanged" => {
                assert_eq!(step["view"], VIEW);
                let before = self.snapshot.as_ref().expect("snapshot");
                assert_eq!(
                    self.rows
                        .iter()
                        .find(|r| r["operation"] == before["operation"]),
                    Some(before)
                );
            }
            other => panic!("unsupported ESS step {other}"),
        }
    }
}
#[test]
fn ess_operations_execute_the_committed_suite() {
    let suite: Value = serde_json::from_str(SUITE).unwrap();
    assert_eq!(suite["provenance"]["suite_version"], "ess-conformance/12");
    let scenarios = suite["scenarios"].as_object().unwrap();
    assert_eq!(
        scenarios.len(),
        14,
        "review a changed conformance inventory"
    );
    for (id, scenario) in scenarios {
        println!("ESS scenario: {id}");
        let mut target = Target::new();
        for step in scenario["steps"].as_array().unwrap() {
            target.step(step);
        }
    }
    println!("ESS accepted-operation ledger: 14 passed, 0 skipped, 0 failed");
}
