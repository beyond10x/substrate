//! `cli-binding-validates`: the hand-written clap derive command line is held to the ESS CLI
//! binding. `cargo xtask check-ess` proves `spec/client/cli.yaml` valid and
//! `spec/generated/cli/binding.json` its exact resolved form; this test proves the binary presents
//! exactly that binding: the same leaf paths, and per leaf the same arguments with the same
//! sources, plus the two process globals.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use b10x_substrate_cli::Cli;
use clap::CommandFactory as _;
use serde_json::Value;

/// How one argument reaches the command, in the binding's own terms.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Source {
    Option(String),
    Positional(usize),
    Trailing,
}

fn binding() -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/generated/cli/binding.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).expect("binding.json is JSON")
}

fn binding_leaves(binding: &Value) -> BTreeMap<Vec<String>, (String, BTreeMap<String, Source>)> {
    let mut leaves = BTreeMap::new();
    for command in binding["commands"].as_array().expect("commands is a list") {
        let path: Vec<String> = command["path"]
            .as_array()
            .expect("path is a list")
            .iter()
            .map(|word| word.as_str().expect("path word").to_owned())
            .collect();
        let about = command["about"].as_str().unwrap_or_default().to_owned();
        let mut arguments = BTreeMap::new();
        for argument in command["arguments"].as_array().expect("arguments") {
            let field = argument["field"].as_str().expect("field").to_owned();
            let source = &argument["source"];
            let source = match source["kind"].as_str().expect("source kind") {
                "option" => Source::Option(source["long"].as_str().expect("long").to_owned()),
                "positional" => Source::Positional(
                    usize::try_from(source["index"].as_u64().expect("index")).expect("index"),
                ),
                "trailing" => Source::Trailing,
                other => panic!("binding source kind {other} is not presented by this CLI"),
            };
            assert!(
                arguments.insert(field.clone(), source).is_none(),
                "{path:?}: field {field} is bound twice"
            );
        }
        assert!(
            leaves.insert(path.clone(), (about, arguments)).is_none(),
            "{path:?} is bound twice"
        );
    }
    leaves
}

fn clap_leaves(
    command: &clap::Command,
    prefix: &[String],
    globals: &BTreeSet<String>,
    out: &mut BTreeMap<Vec<String>, (String, BTreeMap<String, Source>)>,
) {
    for sub in command.get_subcommands() {
        let mut path = prefix.to_vec();
        if sub.get_name() == "help" {
            continue;
        }
        path.push(sub.get_name().to_owned());
        if sub.has_subcommands() {
            clap_leaves(sub, &path, globals, out);
            continue;
        }
        let about = sub.get_about().map(ToString::to_string).unwrap_or_default();
        let mut arguments = BTreeMap::new();
        let mut positional = 0;
        for argument in sub.get_arguments() {
            let id = argument.get_id().as_str().to_owned();
            if id == "help" || id == "version" || globals.contains(&id) {
                continue;
            }
            let source = if argument.is_positional() {
                if argument.is_last_set() || argument.is_trailing_var_arg_set() {
                    Source::Trailing
                } else {
                    positional += 1;
                    Source::Positional(positional)
                }
            } else {
                Source::Option(
                    argument
                        .get_long()
                        .unwrap_or_else(|| panic!("{path:?}: option {id} has no long name"))
                        .to_owned(),
                )
            };
            arguments.insert(id, source);
        }
        out.insert(path, (about, arguments));
    }
}

#[test]
fn cli_binding_validates() {
    let binding = binding();
    assert_eq!(binding["format"], "ess-cli-plan/2");

    let mut command = Cli::command();
    command.build();
    assert_eq!(
        command.get_name(),
        binding["binary"].as_str().expect("binary")
    );
    assert_eq!(
        command.get_about().map(ToString::to_string).as_deref(),
        binding["about"].as_str()
    );

    // The two process globals: `state` spent on the socket, and the output selector.
    let state = binding["globals"]["state"].as_str().expect("state global");
    let output = binding["globals"]["output"]
        .as_str()
        .expect("output global");
    let globals: BTreeSet<String> = [state.to_owned(), output.to_owned()].into();
    let socket = command
        .get_arguments()
        .find(|argument| argument.get_id() == state)
        .expect("the state global is an argument");
    assert!(socket.is_global_set(), "--{state} is global");
    assert_eq!(socket.get_long(), Some(state));
    let output_arg = command
        .get_arguments()
        .find(|argument| argument.get_id() == output)
        .expect("the output global is an argument");
    assert!(output_arg.is_global_set(), "--{output} is global");
    assert_eq!(output_arg.get_long(), Some(output));
    let values: Vec<String> = output_arg
        .get_possible_values()
        .iter()
        .map(|value| value.get_name().to_owned())
        .collect();
    assert_eq!(values, ["human", "json"]);
    assert_eq!(
        output_arg
            .get_default_values()
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        ["human"]
    );

    let expected = binding_leaves(&binding);
    let mut observed = BTreeMap::new();
    clap_leaves(&command, &[], &globals, &mut observed);

    let expected_paths: BTreeSet<_> = expected.keys().cloned().collect();
    let observed_paths: BTreeSet<_> = observed.keys().cloned().collect();
    assert_eq!(
        observed_paths, expected_paths,
        "clap leaf paths must equal the binding's command paths"
    );
    assert_eq!(
        expected_paths.len(),
        12,
        "the binding names twelve commands"
    );
    for (path, (about, arguments)) in &expected {
        let (observed_about, observed_arguments) = &observed[path];
        assert_eq!(
            observed_arguments, arguments,
            "{path:?}: arguments and their sources must equal the binding"
        );
        assert_eq!(
            observed_about, about,
            "{path:?}: about must equal the binding"
        );
    }
}
