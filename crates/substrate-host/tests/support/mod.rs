//! Executes authored scenarios of the committed confinement suite (`spec/confinement-suite.json`)
//! against real adapters opened through `substrate_host::confinement::open`.
//!
//! Only the suite's closed step vocabulary is admitted; an unknown step, expression or command
//! fails the run. Every event and view row the runner compares is built from what the adapter
//! returned (its `facts()`, its admission record), never from the expected outcome.
//!
//! Synthesized scenarios check the model against ESS's interpreted reference and are not run
//! here: they open adapter rows with arbitrary fact sets no real adapter proves.
//!
//! A scenario belongs to a lane by its name's `-on-<lane>` suffix; a runner selects by that suffix
//! and by the adapters the scenario opens, so a renamed or added scenario is picked up without an
//! edit here, and the count each lane executes is read from the suite.

#![allow(dead_code, clippy::too_many_lines)]

use std::collections::BTreeMap;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{Value, json};
use substrate_host::confinement::{
    self, AdapterFacts, AdapterName, AuditRecord, AuditSink, ConfinementAdapter, ConfinementError,
    ConfinementProfile, ContractFamily, Fact, InnerAdapter,
};

pub const SUITE: &str = include_str!("../../../../spec/confinement-suite.json");
const PREFIX: &str = "substrate.confinement.";
const AUTHORED: &str = "substrate.confinement/authored/";

/// What a runner needs to open a real adapter for one scenario's open step.
pub struct OpenStep {
    pub adapter: String,
    pub inner: Option<String>,
    pub served_families: Vec<String>,
    pub facts: Vec<String>,
}

pub type Opener<'a> =
    dyn FnMut(&OpenStep) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> + 'a;

/// The authored scenarios of `lane` whose every open step names an adapter `admits` accepts.
pub fn scenarios(lane: &str, admits: impl Fn(&str) -> bool) -> Vec<(String, Value)> {
    let suite: Value = serde_json::from_str(SUITE).expect("the committed suite is JSON");
    assert_eq!(suite["provenance"]["scenario_initial_state"], "empty");
    let suffix = format!("-on-{lane}");
    suite["scenarios"]
        .as_object()
        .expect("scenarios")
        .iter()
        .filter(|(id, _)| id.starts_with(AUTHORED) && id.ends_with(&suffix))
        .filter(|(_, scenario)| {
            scenario["steps"]
                .as_array()
                .expect("steps")
                .iter()
                .filter(|step| step["command"] == format!("{PREFIX}OpenAdapter"))
                .all(|step| admits(text(&step["input"]["adapter"]["value"])))
        })
        .map(|(id, scenario)| (id.clone(), scenario.clone()))
        .collect()
}

/// Every authored scenario carrying the `-on-<lane>` suffix, whatever it opens.
pub fn lane_size(lane: &str) -> usize {
    scenarios(lane, |_| true).len()
}

fn text(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("suite requires text, got {value}"))
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("suite requires a list, got {value}"))
        .iter()
        .map(|item| text(item).to_owned())
        .collect()
}

fn family(name: &str) -> ContractFamily {
    serde_json::from_value(json!(name)).unwrap_or_else(|_| panic!("unknown family {name}"))
}

fn fact(name: &str) -> Fact {
    serde_json::from_value(json!(name)).unwrap_or_else(|_| panic!("unknown fact {name}"))
}

fn inner(name: &str) -> InnerAdapter {
    serde_json::from_value(json!(name)).unwrap_or_else(|_| panic!("unknown inner adapter {name}"))
}

fn adapter_name(name: &str) -> AdapterName {
    serde_json::from_value(json!(name)).unwrap_or_else(|_| panic!("unknown adapter {name}"))
}

fn kebab(name: &str) -> String {
    name.replace('_', "-")
}

/// The model's outcome name and error (name, fields) for a refusal the adapter returned.
fn refusal(error: &ConfinementError) -> (String, String, Value) {
    match error {
        ConfinementError::AdapterUnserved { adapter, .. } => (
            "adapter-unserved".into(),
            "AdapterUnserved".into(),
            json!({"adapter": adapter.name()}),
        ),
        ConfinementError::FamilyUnserved { adapter, family } => (
            format!("family-unserved-{}", kebab(&family.name())),
            "FamilyUnserved".into(),
            json!({"adapter": adapter.name(), "family": family.name()}),
        ),
        ConfinementError::FactUnserved { adapter, fact } => (
            format!("fact-unserved-{}", kebab(&fact.name())),
            "FactUnserved".into(),
            json!({"adapter": adapter.name(), "fact": fact.name()}),
        ),
        ConfinementError::LocalRefused { adapter } => (
            "local-refused".into(),
            "LocalRefused".into(),
            json!({"adapter": adapter.name()}),
        ),
        ConfinementError::LocalReasonMissing { adapter } => (
            "local-reason-missing".into(),
            "LocalReasonMissing".into(),
            json!({"adapter": adapter.name()}),
        ),
        ConfinementError::ImageUnpinned { adapter } => (
            "image-unpinned".into(),
            "ImageUnpinned".into(),
            json!({"adapter": adapter.name()}),
        ),
        ConfinementError::ReservedFieldUnserved { field } => (
            format!("{}-reserved", field.name()),
            "ReservedFieldUnserved".into(),
            json!({"field": field.name()}),
        ),
        ConfinementError::Driver(error) => panic!("the model names no driver refusal: {error}"),
    }
}

mod erased {
    use serde_json::Value;

    /// A suite name for any of the contract's serializable enums.
    pub trait Named {
        fn name(&self) -> String;
    }

    impl<T: serde::Serialize> Named for T {
        fn name(&self) -> String {
            match serde_json::to_value(self).expect("serializable") {
                Value::String(name) => name,
                other => panic!("not a unit enum: {other}"),
            }
        }
    }
}

use erased::Named as _;

/// One scenario's state: the adapters it opened, what the last command answered, and the rows
/// the two views read.
pub struct Target<'a> {
    opener: &'a mut Opener<'a>,
    opened: BTreeMap<String, Arc<dyn ConfinementAdapter>>,
    forced: BTreeMap<String, String>,
    command: String,
    outcome: String,
    error: Option<(String, Value)>,
    events: BTreeMap<String, Value>,
    instances: BTreeMap<String, Value>,
    adapters: Vec<Value>,
    profiles: Vec<Value>,
    rows: Vec<Value>,
    view: String,
    audit: Arc<Mutex<Vec<AuditRecord>>>,
    /// Refusal audit records asserted, across the scenario.
    pub refusals_audited: usize,
    /// Opens refused by `confinement::check_open` on the scenario's declared set.
    pub open_checks_refused: usize,
}

impl<'a> Target<'a> {
    pub fn new(opener: &'a mut Opener<'a>) -> Self {
        Self {
            opener,
            opened: BTreeMap::new(),
            forced: BTreeMap::new(),
            command: String::new(),
            outcome: String::new(),
            error: None,
            events: BTreeMap::new(),
            instances: BTreeMap::new(),
            adapters: Vec::new(),
            profiles: Vec::new(),
            rows: Vec::new(),
            view: String::new(),
            audit: Arc::new(Mutex::new(Vec::new())),
            refusals_audited: 0,
            open_checks_refused: 0,
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

    fn input(&self, expressions: &Value) -> BTreeMap<String, Value> {
        expressions
            .as_object()
            .expect("command input")
            .iter()
            .map(|(name, expression)| (name.clone(), self.value(expression)))
            .collect()
    }

    fn execute(&mut self, command: &str, expressions: &Value) {
        let input = self.input(expressions);
        command.clone_into(&mut self.command);
        self.events.clear();
        self.error = None;
        match command.strip_prefix(PREFIX).expect("confinement command") {
            "OpenAdapter" => self.open(&input),
            "AdmitProfile" => self.admit(&input),
            other => panic!("unsupported command {other}"),
        }
        if let Some(forced) = self.forced.remove(command) {
            assert_eq!(
                self.outcome, forced,
                "the scenario's setup forces {forced}; the real adapter answered {}",
                self.outcome
            );
        }
    }

    fn open(&mut self, input: &BTreeMap<String, Value>) {
        let step = OpenStep {
            adapter: text(&input["adapter"]).to_owned(),
            inner: input.get("inner").map(|value| text(value).to_owned()),
            served_families: strings(&input["served_families"]),
            facts: strings(&input["facts"]),
        };
        // The open's input is the proved set. A set lacking a fact the adapter itself must prove is
        // refused by the same check `confinement::open` applies to every real proof, before any
        // adapter is asked to open: no real `local` can be made to prove less.
        let declared = AdapterFacts::new(
            adapter_name(&step.adapter),
            step.inner.as_deref().map(inner),
            &step
                .served_families
                .iter()
                .map(|name| (family(name), true))
                .collect::<Vec<_>>(),
            &step
                .facts
                .iter()
                .map(|name| (fact(name), true))
                .collect::<Vec<_>>(),
        );
        if let Err(error) = confinement::check_open(&declared) {
            let (_, name, fields) = refusal(&error);
            assert_eq!(
                name, "FactUnserved",
                "an open check refuses by fact: {error}"
            );
            assert_eq!(fields["adapter"], json!(step.adapter));
            self.open_checks_refused += 1;
            self.outcome = match (step.adapter.as_str(), step.inner.as_deref()) {
                ("local", _) => "local-not-declared-insecure",
                ("systemd", Some("local")) => "systemd-local-not-declared-insecure",
                ("oci" | "pod", _) => "image-not-digest-pinned",
                (other, _) => panic!("the model names no open check for {other}"),
            }
            .into();
            self.error = Some((format!("{PREFIX}{name}"), fields));
            return;
        }
        match (self.opener)(&step) {
            Ok(adapter) => {
                let facts = adapter.facts();
                let observed = serde_json::to_value(facts).expect("adapter facts serialize");
                // The scenario's open step is the adapter's declared set; the real adapter must
                // have proved exactly that, in the contract's order.
                assert_eq!(
                    observed["served_families"],
                    json!(step.served_families),
                    "{} serves other families than the scenario declares",
                    step.adapter
                );
                assert_eq!(
                    observed["facts"],
                    json!(step.facts),
                    "{} proved other facts than the scenario declares",
                    step.adapter
                );
                assert_eq!(observed["adapter"], json!(step.adapter));
                assert_eq!(
                    observed.get("inner").cloned().unwrap_or(Value::Null),
                    step.inner
                        .as_ref()
                        .map_or(Value::Null, |inner| json!(inner))
                );
                self.adapters
                    .retain(|row| row["adapter"] != observed["adapter"]);
                let mut row = observed.clone();
                row["state"] = json!("Open");
                self.adapters.push(row);
                self.events
                    .insert(format!("{PREFIX}AdapterOpened"), observed);
                self.opened.insert(step.adapter.clone(), adapter);
                self.outcome = "opened".into();
            }
            Err(error) => {
                let (outcome, name, fields) = refusal(&error);
                assert_eq!(
                    outcome, "adapter-unserved",
                    "an open refuses only as adapter-unserved: {error}"
                );
                assert_eq!(fields["adapter"], json!(step.adapter));
                self.outcome = "unserved".into();
                self.error = Some((format!("{PREFIX}{name}"), fields));
            }
        }
    }

    fn admit(&mut self, input: &BTreeMap<String, Value>) {
        let name = text(&input["adapter"]).to_owned();
        let sink_records = Arc::clone(&self.audit);
        let profile = ConfinementProfile {
            families: strings(&input["required_families"])
                .iter()
                .map(|name| family(name))
                .collect(),
            facts: strings(&input["required_facts"])
                .iter()
                .map(|name| fact(name))
                .collect(),
            inner: input.get("inner").map(|value| inner(text(value))),
            local_reason: input
                .get("local_reason")
                .map(|value| text(value).to_owned()),
            image: input.get("image").map(|value| text(value).to_owned()),
            ingress_routes: input.get("ingress_routes").cloned(),
            rate_bounds: input.get("rate_bounds").cloned(),
            audit: Some(AuditSink::new(move |record: &AuditRecord| {
                sink_records.lock().push(record.clone());
            })),
            ..ConfinementProfile::default()
        };
        self.audit.lock().clear();
        let opened = self.opened.contains_key(&name);
        let result = match self.opened.get(&name) {
            Some(adapter) => adapter.admit(&profile),
            None => Err(confinement::admit_unopened(&adapter_name(&name), &profile)),
        };
        let audited = std::mem::take(&mut *self.audit.lock());
        assert_eq!(
            audited.len(),
            1,
            "an admission writes exactly one audit record: {audited:?}"
        );
        let record = serde_json::to_value(&audited[0]).expect("audit record serializes");
        match result {
            Ok(admitted) => {
                let mut event =
                    serde_json::to_value(admitted.record()).expect("admission record serializes");
                assert_eq!(record, event, "the sink received another record");
                assert_eq!(event["event"], "profile_admitted");
                event.as_object_mut().expect("record").remove("event");
                self.outcome = if admitted.declared_insecure() {
                    "admitted-insecure"
                } else {
                    "admitted"
                }
                .into();
                let mut row = event.clone();
                row["state"] = json!("Admitted");
                self.profiles.push(row);
                self.events
                    .insert(format!("{PREFIX}ProfileAdmitted"), event);
            }
            Err(error) => {
                let (outcome, error_name, fields) = refusal(&error);
                // ESS cannot attach an event to an error outcome; the refusal's audit record is
                // asserted here instead: one `profile_refused`, naming the code and what it names.
                assert_eq!(record["event"], "profile_refused", "{record}");
                assert_eq!(record["code"], json!(error.code()), "{record}");
                for (field, value) in fields.as_object().expect("fields") {
                    assert_eq!(
                        &record[field], value,
                        "refusal record field {field}: {record}"
                    );
                }
                self.refusals_audited += 1;
                // The model names `confinement.adapter-unserved` by why: no opened row, or an
                // opened row whose `inner` is not the requested one.
                self.outcome = if !opened && outcome == "adapter-unserved" {
                    "adapter-not-open".into()
                } else {
                    outcome
                };
                self.error = Some((format!("{PREFIX}{error_name}"), fields));
            }
        }
    }

    fn query(&mut self, view: &str) {
        self.rows = match view.strip_prefix(PREFIX).expect("confinement view") {
            "Adapters" => self.adapters.clone(),
            "Profiles" => self.profiles.clone(),
            other => panic!("unsupported view {other}"),
        };
        view.clone_into(&mut self.view);
    }

    fn shape(event: &Value, name: &str, shape: &Value) {
        let field = event.get(name);
        let optional = shape["optional"] == json!(true);
        let Some(field) = field.filter(|value| !value.is_null()) else {
            assert!(optional, "event field {name} is absent and not optional");
            return;
        };
        match text(&shape["holds"]) {
            "enum" => assert!(
                shape["variants"]
                    .as_array()
                    .expect("variants")
                    .contains(field),
                "event field {name} = {field} is not a declared variant"
            ),
            "list" => assert!(field.is_array(), "event field {name} is not a list"),
            "primitive" => match text(&shape["kind"]) {
                "boolean" => assert!(field.is_boolean(), "event field {name}"),
                "string" => assert!(field.is_string(), "event field {name}"),
                "uuid" => {
                    let value = text(field);
                    let groups: Vec<&str> = value.split('-').collect();
                    assert_eq!(
                        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
                        [8, 4, 4, 4, 12],
                        "{name} = {value} is not a UUID"
                    );
                    assert!(
                        groups
                            .iter()
                            .all(|group| group.bytes().all(|byte| byte.is_ascii_hexdigit())),
                        "{name} = {value} is not a UUID"
                    );
                }
                other => panic!("unsupported primitive {other}"),
            },
            other => panic!("unsupported shape {other}"),
        }
    }

    pub fn step(&mut self, step: &Value) {
        match text(&step["step"]) {
            "configure_external_outcome" => {
                let force = &step["force"];
                self.forced.insert(
                    text(&force["command"]).to_owned(),
                    text(&force["outcome"]).to_owned(),
                );
            }
            "execute_command" => self.execute(text(&step["command"]), &step["input"]),
            "expect_outcome" => {
                assert_eq!(self.command, text(&step["outcome"]["command"]));
                assert_eq!(
                    self.outcome,
                    text(&step["outcome"]["outcome"]),
                    "error: {:?}",
                    self.error
                );
            }
            "expect_error" => {
                let (name, fields) = self.error.as_ref().expect("an error");
                assert_eq!(name, text(&step["error"]));
                if let Some(expected) = step.get("fields") {
                    for (field, value) in expected.as_object().expect("fields") {
                        assert_eq!(&fields[field], value, "error field {field}");
                    }
                }
            }
            "expect_no_error" => assert!(self.error.is_none(), "{:?}", self.error),
            "capture_instance" => {
                let value = self.events[text(&step["event"])][text(&step["field"])].clone();
                assert!(
                    !value.is_null(),
                    "captured {} is absent",
                    text(&step["field"])
                );
                self.instances
                    .insert(text(&step["instance"]).to_owned(), value);
            }
            kind @ ("expect_event" | "expect_event_values") => {
                let event = self
                    .events
                    .get(text(&step["event"]))
                    .unwrap_or_else(|| panic!("no event {}", text(&step["event"])))
                    .clone();
                if let Some(shape) = step.get("shape") {
                    for (name, field_shape) in shape.as_object().expect("shape") {
                        Self::shape(&event, name, field_shape);
                    }
                }
                if let Some(payload) = step.get("payload") {
                    for (name, value) in payload.as_object().expect("payload") {
                        let expected = if kind == "expect_event_values" {
                            self.value(value)
                        } else {
                            value.clone()
                        };
                        assert_eq!(event[name], expected, "event field {name}");
                    }
                }
            }
            "expect_no_event" => assert!(
                !self.events.contains_key(text(&step["event"])),
                "unexpected {}",
                text(&step["event"])
            ),
            "query_view" => self.query(text(&step["view"])),
            "expect_view" => {
                assert_eq!(self.view, text(&step["view"]));
                let expectation = &step["expectation"];
                match text(&expectation["expect"]) {
                    "contains" => {
                        let expected: BTreeMap<&String, Value> = expectation["fields"]
                            .as_object()
                            .expect("fields")
                            .iter()
                            .map(|(name, expression)| (name, self.value(expression)))
                            .collect();
                        assert!(
                            self.rows.iter().any(|row| expected
                                .iter()
                                .all(|(name, value)| &row[*name] == value)),
                            "view does not contain {expected:?}; observed {:?}",
                            self.rows
                        );
                    }
                    "counts" => {
                        if let Some(at_most) = expectation.get("at_most") {
                            assert!(
                                self.rows.len() as u64 <= at_most.as_u64().expect("count"),
                                "view holds {} rows: {:?}",
                                self.rows.len(),
                                self.rows
                            );
                        }
                        if let Some(at_least) = expectation.get("at_least") {
                            assert!(self.rows.len() as u64 >= at_least.as_u64().expect("count"));
                        }
                    }
                    other => panic!("unsupported view expectation {other}"),
                }
            }
            other => panic!("unsupported ESS step {other}"),
        }
    }
}

/// Runs each scenario with a fresh target, and returns how many ran, how many refusal audit
/// records were asserted and how many opens `confinement::check_open` refused.
pub fn run(
    scenarios: &[(String, Value)],
    mut opener: impl FnMut(&OpenStep) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError>,
) -> (usize, usize, usize) {
    let mut executed = 0;
    let mut audited = 0;
    let mut open_checks = 0;
    for (id, scenario) in scenarios {
        println!("ESS scenario: {id}");
        let opener: &mut Opener<'_> = &mut opener;
        let mut target = Target::new(opener);
        for step in scenario["steps"].as_array().expect("steps") {
            target.step(step);
        }
        audited += target.refusals_audited;
        open_checks += target.open_checks_refused;
        executed += 1;
    }
    (executed, audited, open_checks)
}
