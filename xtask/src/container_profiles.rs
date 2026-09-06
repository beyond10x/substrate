//! Offline classification and safety checks for the explicit OCI container profile.

#[cfg(test)]
mod tests {
    use serde_json::Value;

    #[test]
    fn container_seccomp_has_one_valid_schema_and_keeps_privileged_syscalls_absent() {
        let schema: Value = serde_json::from_str(include_str!(
            "../../deploy/seccomp/schemas/host-execution.schema.json"
        ))
        .expect("schema JSON");
        jsonschema::draft202012::meta::validate(&schema).expect("meta-schema validation");
        let profile: Value =
            serde_json::from_str(include_str!("../../deploy/seccomp/host-exec-v1-amd64.json"))
                .expect("profile JSON");
        jsonschema::validator_for(&schema)
            .expect("profile schema")
            .validate(&profile)
            .expect("classified OCI profile");
        let rules = profile["syscalls"].as_array().expect("syscall rules");
        for forbidden in [
            "bpf",
            "perf_event_open",
            "open_by_handle_at",
            "init_module",
            "finit_module",
            "delete_module",
            "syslog",
            "settimeofday",
            "clock_settime",
            "reboot",
            "kexec_load",
        ] {
            assert!(
                !rules.iter().any(|rule| rule["action"] == "SCMP_ACT_ALLOW"
                    && rule["names"]
                        .as_array()
                        .expect("names")
                        .iter()
                        .any(|name| name == forbidden)),
                "unexpected outer authority: {forbidden}"
            );
        }
    }
}
