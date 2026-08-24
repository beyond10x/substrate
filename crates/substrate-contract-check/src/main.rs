#![forbid(unsafe_code)]

use std::io::{Read as _, Write as _};

use serde_json::{Value, json};
use std::collections::BTreeSet;

const DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";
const MAX_PROTOCOL_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RECORDS: usize = 10_000;
const MAX_RESOURCES: usize = 10_000;

fn main() {
    let request = read_request();
    let failures = validate_request(&request);
    let output = serde_json::to_vec(&json!({ "failures": failures })).expect("serialize result");
    std::io::stdout()
        .write_all(&output)
        .expect("write validation result");
    if !failures.is_empty() {
        std::process::exit(1);
    }
}

fn read_request() -> Value {
    let mut input = Vec::new();
    if let Err(error) = std::io::stdin()
        .take(MAX_PROTOCOL_BYTES + 1)
        .read_to_end(&mut input)
    {
        fail_protocol(&format!("read stdin: {error}"));
    }
    if input.len() as u64 > MAX_PROTOCOL_BYTES {
        fail_protocol("request exceeds 16 MiB protocol ceiling");
    }
    match serde_json::from_slice(&input) {
        Ok(value) => value,
        Err(error) => fail_protocol(&format!("decode request: {error}")),
    }
}

fn validate_request(request: &Value) -> Vec<String> {
    require_exact_keys(request, &["records", "resources"], "request");
    let Some(records) = request.get("records").and_then(Value::as_array) else {
        fail_protocol("records must be an array");
    };
    let Some(resources) = request.get("resources").and_then(Value::as_array) else {
        fail_protocol("resources must be an array");
    };
    if records.len() > MAX_RECORDS || resources.len() > MAX_RESOURCES {
        fail_protocol("records/resources exceed protocol item ceiling");
    }
    let mut registry = jsonschema::Registry::new();
    let mut resource_uris = BTreeSet::new();
    for resource in resources {
        require_exact_keys(resource, &["schema", "uri"], "resource");
        let Some(uri) = resource.get("uri").and_then(Value::as_str) else {
            fail_protocol("resource URI must be a string");
        };
        if !uri.starts_with("https://b10x.invalid/") || !resource_uris.insert(uri) {
            fail_protocol("resource URI must be unique under https://b10x.invalid/");
        }
        let Some(schema) = resource.get("schema") else {
            fail_protocol("resource schema is absent");
        };
        registry = match registry.add(uri, schema.clone()) {
            Ok(value) => value,
            Err(error) => fail_protocol(&format!("register {uri}: {error}")),
        };
    }
    let registry = match registry.prepare() {
        Ok(value) => value,
        Err(error) => fail_protocol(&format!("prepare registry: {error}")),
    };
    let mut failures = Vec::new();
    let mut labels = BTreeSet::new();
    for record in records {
        let Some(label) = record.get("label").and_then(Value::as_str) else {
            fail_protocol("record label must be a string");
        };
        if label.is_empty() || label.len() > 4096 || !labels.insert(label) {
            fail_protocol("record labels must be non-empty, bounded, and unique");
        }
        match record.get("kind").and_then(Value::as_str) {
            Some("meta") => {
                require_exact_keys(record, &["kind", "label", "schema"], "meta record");
                let Some(schema) = record.get("schema") else {
                    fail_protocol("meta record schema is absent");
                };
                if schema.get("$schema").and_then(Value::as_str) != Some(DRAFT_2020_12) {
                    failures.push(format!(
                        "{label}: must declare the pinned Draft 2020-12 meta-schema"
                    ));
                    continue;
                }
                if let Err(error) = jsonschema::draft202012::meta::validate(schema) {
                    failures.push(format!("{label}: {error}"));
                }
            }
            Some("instance") => {
                require_exact_keys(
                    record,
                    &["instance", "kind", "label", "schema_uri"],
                    "instance record",
                );
                let Some(schema_uri) = record.get("schema_uri").and_then(Value::as_str) else {
                    fail_protocol("instance record schema_uri must be a string");
                };
                let Some(instance) = record.get("instance") else {
                    fail_protocol("instance record document is absent");
                };
                if !resource_uris.contains(schema_uri) {
                    fail_protocol("instance schema_uri is not a registered exact resource URI");
                }
                let schema = json!({ "$ref": schema_uri });
                let validator = match jsonschema::draft202012::options()
                    .with_registry(&registry)
                    .build(&schema)
                {
                    Ok(value) => value,
                    Err(error) => {
                        failures.push(format!("{label}: schema compile failed: {error}"));
                        continue;
                    }
                };
                if let Err(error) = validator.validate(instance) {
                    failures.push(format!("{label}: {error}"));
                }
            }
            _ => fail_protocol("record kind must be meta or instance"),
        }
    }
    failures
}

fn require_exact_keys(value: &Value, expected: &[&str], label: &str) {
    let Some(object) = value.as_object() else {
        fail_protocol(&format!("{label} must be an object"));
    };
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    if actual != expected {
        fail_protocol(&format!("{label} has unexpected or missing fields"));
    }
}

fn fail_protocol(message: &str) -> ! {
    let output = serde_json::to_vec(&json!({ "protocol_error": message }))
        .expect("serialize protocol error");
    let _ = std::io::stdout().write_all(&output);
    std::process::exit(2);
}
