//! Mutants of the clap command against the `cli_binding_validates` check (`tests/binding.rs`).
//!
//! `accepted_by_binding_check` is that test's comparison, line for line, returning whether it
//! would pass instead of asserting. Each `mutant_*` case feeds it `Cli::command()` with one
//! presented difference from `spec/generated/cli/binding.json` and asserts the check refuses it.
//! A red `mutant_*` case is a binary that presents something the binding does not name and still
//! passes `cli-binding-validates`. The `control_*` cases prove the replica refuses what the
//! original refuses, so a red mutant is the check's blind spot and not the replica's.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use b10x_substrate_cli::Cli;
use clap::{Arg, ArgAction, CommandFactory as _};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Source {
    Option(String),
    Positional(usize),
    Trailing,
}

type Leaves = BTreeMap<Vec<String>, (String, BTreeMap<String, Source>)>;

fn binding() -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/generated/cli/binding.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("binding")).expect("JSON")
}

fn binding_leaves(binding: &Value) -> Leaves {
    let mut leaves = BTreeMap::new();
    for command in binding["commands"].as_array().expect("commands") {
        let path: Vec<String> = command["path"]
            .as_array()
            .expect("path")
            .iter()
            .map(|word| word.as_str().expect("word").to_owned())
            .collect();
        let about = command["about"].as_str().unwrap_or_default().to_owned();
        let mut arguments = BTreeMap::new();
        for argument in command["arguments"].as_array().expect("arguments") {
            let field = argument["field"].as_str().expect("field").to_owned();
            let source = &argument["source"];
            let source = match source["kind"].as_str().expect("kind") {
                "option" => Source::Option(source["long"].as_str().expect("long").to_owned()),
                "positional" => Source::Positional(
                    usize::try_from(source["index"].as_u64().expect("index")).expect("index"),
                ),
                "trailing" => Source::Trailing,
                other => panic!("{other}"),
            };
            arguments.insert(field, source);
        }
        leaves.insert(path, (about, arguments));
    }
    leaves
}

fn clap_leaves(
    command: &clap::Command,
    prefix: &[String],
    globals: &BTreeSet<String>,
    out: &mut Leaves,
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
                match argument.get_long() {
                    Some(long) => Source::Option(long.to_owned()),
                    None => return,
                }
            };
            arguments.insert(id, source);
        }
        out.insert(path, (about, arguments));
    }
}

/// `cli_binding_validates`, as a predicate.
fn accepted_by_binding_check(mut command: clap::Command) -> bool {
    let binding = binding();
    command.build();
    if command.get_name() != binding["binary"].as_str().expect("binary") {
        return false;
    }
    if command.get_about().map(ToString::to_string).as_deref() != binding["about"].as_str() {
        return false;
    }
    let state = binding["globals"]["state"].as_str().expect("state");
    let output = binding["globals"]["output"].as_str().expect("output");
    let globals: BTreeSet<String> = [state.to_owned(), output.to_owned()].into();
    let Some(socket) = command.get_arguments().find(|a| a.get_id() == state) else {
        return false;
    };
    if !socket.is_global_set() || socket.get_long() != Some(state) {
        return false;
    }
    let Some(output_arg) = command.get_arguments().find(|a| a.get_id() == output) else {
        return false;
    };
    if !output_arg.is_global_set() || output_arg.get_long() != Some(output) {
        return false;
    }
    let values: Vec<String> = output_arg
        .get_possible_values()
        .iter()
        .map(|value| value.get_name().to_owned())
        .collect();
    if values != ["human", "json"] {
        return false;
    }
    let defaults: Vec<String> = output_arg
        .get_default_values()
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    if defaults != ["human"] {
        return false;
    }
    let expected = binding_leaves(&binding);
    let mut observed = BTreeMap::new();
    clap_leaves(&command, &[], &globals, &mut observed);
    let expected_paths: BTreeSet<_> = expected.keys().cloned().collect();
    let observed_paths: BTreeSet<_> = observed.keys().cloned().collect();
    if observed_paths != expected_paths || expected_paths.len() != 12 {
        return false;
    }
    expected.iter().all(|(path, (about, arguments))| {
        let (observed_about, observed_arguments) = &observed[path];
        observed_arguments == arguments && observed_about == about
    })
}

fn in_workspace(
    name: &'static str,
    mutate: impl FnOnce(clap::Command) -> clap::Command,
) -> clap::Command {
    Cli::command().mut_subcommand("workspace", |group| group.mut_subcommand(name, mutate))
}

#[test]
fn control_the_shipped_command_is_accepted() {
    assert!(accepted_by_binding_check(Cli::command()));
}

#[test]
fn control_a_renamed_leaf_is_refused() {
    let mutant = in_workspace("destroy", |leaf| leaf.name("remove"));
    assert!(!accepted_by_binding_check(mutant));
}

#[test]
fn control_an_added_leaf_argument_is_refused() {
    let mutant = in_workspace("show", |leaf| leaf.arg(Arg::new("verbose").long("verbose")));
    assert!(!accepted_by_binding_check(mutant));
}

#[test]
fn control_a_renamed_option_is_refused() {
    let mutant = in_workspace("renew", |leaf| {
        leaf.mut_arg("ttl_ms", |arg| arg.long("ttl"))
    });
    assert!(!accepted_by_binding_check(mutant));
}

/// The binding gives every command `"aliases": []`; the check never reads the field.
#[test]
fn mutant_an_unbound_command_alias_is_refused() {
    let mutant = in_workspace("destroy", |leaf| leaf.visible_alias("rm"));
    assert!(
        !accepted_by_binding_check(mutant),
        "`workspace rm` is presented, the binding names no alias, and the check passes"
    );
}

/// Arguments of a command that has subcommands are never compared.
#[test]
fn mutant_an_argument_on_a_group_is_refused() {
    let mutant = Cli::command().mut_subcommand("exec", |group| {
        group.arg(
            Arg::new("verbose")
                .long("verbose")
                .action(ArgAction::SetTrue),
        )
    });
    assert!(
        !accepted_by_binding_check(mutant),
        "`exec --verbose` is presented and the check passes"
    );
}

/// Root arguments other than the two globals are never compared.
#[test]
fn mutant_a_root_argument_is_refused() {
    let mutant = Cli::command().arg(Arg::new("token").long("token"));
    assert!(
        !accepted_by_binding_check(mutant),
        "`--token` at the root is presented and the check passes"
    );
}

/// The `help` skip is by name, at every level: a hand-written `help` leaf is skipped too.
#[test]
fn mutant_a_user_help_leaf_is_refused() {
    let mutant = Cli::command().mut_subcommand("workspace", |group| {
        group
            .disable_help_subcommand(true)
            .subcommand(clap::Command::new("help").about("Not clap's help"))
    });
    assert!(
        !accepted_by_binding_check(mutant),
        "a user `workspace help` leaf is presented and the check passes"
    );
}

/// The `version` skip is by id on every leaf, though clap generates `--version` only at the root.
#[test]
fn mutant_a_leaf_version_option_is_refused() {
    let mutant = Cli::command().mut_subcommand("machine", |leaf| {
        leaf.arg(
            Arg::new("version")
                .long("version")
                .action(ArgAction::SetTrue),
        )
    });
    assert!(
        !accepted_by_binding_check(mutant),
        "`machine --version` is presented and the check passes"
    );
}

/// The binding's `SessionSignalRequest.signal` is the enum `int | term | kill`; the check reads
/// no possible value of a leaf argument.
#[test]
fn mutant_a_renamed_enum_value_is_refused() {
    let mutant = Cli::command().mut_subcommand("session", |group| {
        group.mut_subcommand("signal", |leaf| {
            leaf.mut_arg("signal", |arg| {
                arg.value_parser(["interrupt", "terminate", "kill"])
            })
        })
    });
    assert!(
        !accepted_by_binding_check(mutant),
        "`--signal interrupt` replaces the binding's `int` and the check passes"
    );
}

/// The binding's `ExecRunRequest.workspace` is a required string; the check reads no
/// required-ness.
#[test]
fn mutant_a_required_argument_made_optional_is_refused() {
    let mutant = Cli::command().mut_subcommand("exec", |group| {
        group.mut_subcommand("run", |leaf| {
            leaf.mut_arg("workspace", |arg| arg.required(false))
        })
    });
    assert!(
        !accepted_by_binding_check(mutant),
        "`exec run` without `--workspace` parses and the check passes"
    );
}

/// An external-subcommand catch-all presents every unknown word as a command.
#[test]
fn mutant_external_subcommands_are_refused() {
    let mutant = Cli::command().allow_external_subcommands(true);
    assert!(
        !accepted_by_binding_check(mutant),
        "any unknown command word is accepted and the check passes"
    );
}
