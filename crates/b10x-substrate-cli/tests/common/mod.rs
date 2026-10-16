//! The comparison of a clap command against the ESS CLI binding, shared by `binding.rs`
//! (`cli_binding_validates`, which asserts it on the shipped command) and `binding_mutants.rs`
//! (which asserts it refuses each presented difference). One function, so a mutant can never be
//! tested against a replica that has drifted from the real check.
//!
//! Nothing the binary presents is exempt except what clap generates itself: the `help` flag and
//! the root `version` flag, identified by their actions, and the auto-generated `help`
//! subcommand, identified by clap's own about text with the help subcommand not disabled.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use clap::{ArgAction, Command};
use serde_json::Value;

/// How one argument reaches the command, in the binding's own terms.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Option(String),
    Positional(usize),
    Trailing,
}

/// One argument of a leaf: its source, whether the invocation must supply it, and its enum values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg {
    pub source: Source,
    pub required: bool,
    pub values: Vec<String>,
}

/// One leaf command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leaf {
    pub about: String,
    pub aliases: Vec<String>,
    pub arguments: BTreeMap<String, Arg>,
}

pub type Leaves = BTreeMap<Vec<String>, Leaf>;

const CLAP_HELP_ABOUT: &str = "Print this message or the help of the given subcommand(s)";

pub fn binding() -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/generated/cli/binding.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).expect("binding.json is JSON")
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("a list")
        .iter()
        .map(|word| word.as_str().expect("a string").to_owned())
        .collect()
}

pub fn binding_leaves(binding: &Value) -> Leaves {
    let mut leaves = BTreeMap::new();
    for command in binding["commands"].as_array().expect("commands is a list") {
        let path = strings(&command["path"]);
        let callable = command["callable"].as_str().expect("callable");
        let fields = &binding["callables"][callable]["input"]["shape"]["fields"];
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
            let mut shape = &fields[&field];
            assert!(!shape.is_null(), "{path:?}: field {field} has no shape");
            let optional = shape["kind"] == "optional";
            if optional {
                shape = &shape["of"];
            }
            let values = if shape["kind"] == "enum" {
                strings(&shape["variants"])
            } else {
                Vec::new()
            };
            let argument = Arg {
                source,
                required: !optional,
                values,
            };
            assert!(
                arguments.insert(field.clone(), argument).is_none(),
                "{path:?}: field {field} is bound twice"
            );
        }
        let leaf = Leaf {
            about: command["about"].as_str().unwrap_or_default().to_owned(),
            aliases: strings(&command["aliases"]),
            arguments,
        };
        assert!(
            leaves.insert(path.clone(), leaf).is_none(),
            "{path:?} is bound twice"
        );
    }
    leaves
}

/// The arguments a command presents, less clap's own help flag, the root's version flag and the
/// process globals. A refusal names the first thing presented that the binding does not.
fn own_arguments<'a>(
    command: &'a Command,
    root: bool,
    globals: &BTreeSet<String>,
    at: &str,
) -> Result<Vec<&'a clap::Arg>, String> {
    let mut out = Vec::new();
    for argument in command.get_arguments() {
        let id = argument.get_id().as_str();
        let generated_help = id == "help"
            && matches!(
                argument.get_action(),
                ArgAction::Help | ArgAction::HelpShort | ArgAction::HelpLong
            );
        let generated_version =
            root && id == "version" && matches!(argument.get_action(), ArgAction::Version);
        if generated_help || generated_version || globals.contains(id) {
            continue;
        }
        out.push(argument);
    }
    if command.is_allow_external_subcommands_set() {
        return Err(format!("{at}: external subcommands are allowed"));
    }
    Ok(out)
}

fn clap_leaves(
    command: &Command,
    prefix: &[String],
    globals: &BTreeSet<String>,
    out: &mut Leaves,
) -> Result<(), String> {
    for sub in command.get_subcommands() {
        let generated_help = sub.get_name() == "help"
            && !command.is_disable_help_subcommand_set()
            && sub.get_about().map(ToString::to_string).as_deref() == Some(CLAP_HELP_ABOUT);
        if generated_help {
            continue;
        }
        let mut path = prefix.to_vec();
        path.push(sub.get_name().to_owned());
        let at = format!("{path:?}");
        let aliases: Vec<String> = sub.get_all_aliases().map(str::to_owned).collect();
        let arguments = own_arguments(sub, false, globals, &at)?;
        if let Some(group) = constraining_group(sub) {
            return Err(format!("{at}: the group {group} requires or excludes"));
        }
        if sub.has_subcommands() {
            if !aliases.is_empty() {
                return Err(format!("{at}: a group presents aliases {aliases:?}"));
            }
            if let Some(argument) = arguments.first() {
                return Err(format!(
                    "{at}: a group presents the argument {}",
                    argument.get_id()
                ));
            }
            clap_leaves(sub, &path, globals, out)?;
            continue;
        }
        let mut observed = BTreeMap::new();
        let mut positional = 0;
        for argument in arguments {
            let id = argument.get_id().as_str().to_owned();
            if argument.get_short().is_some() || argument.get_all_aliases().is_some() {
                return Err(format!("{at}: {id} presents a short flag or alias"));
            }
            if let Some(relation) = relation(argument) {
                return Err(format!("{at}: {id} presents the relation {relation}"));
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
                    None => return Err(format!("{at}: option {id} has no long name")),
                }
            };
            observed.insert(
                id,
                Arg {
                    source,
                    required: argument.is_required_set(),
                    values: argument
                        .get_possible_values()
                        .iter()
                        .map(|value| value.get_name().to_owned())
                        .collect(),
                },
            );
        }
        out.insert(
            path,
            Leaf {
                about: sub.get_about().map(ToString::to_string).unwrap_or_default(),
                aliases,
                arguments: observed,
            },
        );
    }
    Ok(())
}

/// The whole comparison. `Ok` when the command presents exactly the binding, `Err` naming the
/// first difference.
pub fn check(mut command: Command) -> Result<(), String> {
    let binding = binding();
    if binding["format"] != "ess-cli-plan/2" {
        return Err("the binding format is not ess-cli-plan/2".to_owned());
    }
    command.build();
    let expect = |ok: bool, message: &str| if ok { Ok(()) } else { Err(message.to_owned()) };
    expect(
        command.get_name() == binding["binary"].as_str().expect("binary"),
        "binary name differs",
    )?;
    expect(
        command.get_about().map(ToString::to_string).as_deref() == binding["about"].as_str(),
        "root about differs",
    )?;

    let state = binding["globals"]["state"].as_str().expect("state global");
    let output = binding["globals"]["output"]
        .as_str()
        .expect("output global");
    let globals: BTreeSet<String> = [state.to_owned(), output.to_owned()].into();
    let socket = command
        .get_arguments()
        .find(|argument| argument.get_id() == state)
        .ok_or("the state global is not an argument")?;
    expect(
        socket.is_global_set() && socket.get_long() == Some(state),
        "the state global is not --socket and global",
    )?;
    let output_arg = command
        .get_arguments()
        .find(|argument| argument.get_id() == output)
        .ok_or("the output global is not an argument")?;
    expect(
        output_arg.is_global_set() && output_arg.get_long() == Some(output),
        "the output global is not --output and global",
    )?;
    let values: Vec<String> = output_arg
        .get_possible_values()
        .iter()
        .map(|value| value.get_name().to_owned())
        .collect();
    expect(values == ["human", "json"], "output values differ")?;
    let defaults: Vec<String> = output_arg
        .get_default_values()
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    expect(defaults == ["human"], "output default differs")?;

    let root_extra = own_arguments(&command, true, &globals, "root")?;
    if let Some(argument) = root_extra.first() {
        return Err(format!("root presents the argument {}", argument.get_id()));
    }

    let expected = binding_leaves(&binding);
    let mut observed = BTreeMap::new();
    clap_leaves(&command, &[], &globals, &mut observed)?;
    let expected_paths: BTreeSet<_> = expected.keys().cloned().collect();
    let observed_paths: BTreeSet<_> = observed.keys().cloned().collect();
    if observed_paths != expected_paths {
        return Err(format!(
            "leaf paths differ: observed {observed_paths:?}, binding {expected_paths:?}"
        ));
    }
    expect(
        expected_paths.len() == 12,
        "the binding names twelve commands",
    )?;
    for (path, leaf) in &expected {
        if &observed[path] != leaf {
            return Err(format!(
                "{path:?}: presents {:?}, the binding names {leaf:?}",
                observed[path]
            ));
        }
    }
    Ok(())
}

/// `check`, as a predicate.
pub fn accepted_by_binding_check(command: Command) -> bool {
    check(command).is_ok()
}

/// The first relation `argument` holds to another argument, if any: a conflict, an override, a
/// requirement, a conditional requirement or a conditional default. The binding states none of
/// these, so each would make an invocation the binding admits fail to parse, or one it refuses
/// parse. clap exposes them through no getter, so this reads its `Debug` form; `r_ifs_all` and
/// `r_unless_all` are absent from that form, and are not seen.
fn relation(argument: &clap::Arg) -> Option<&'static str> {
    let debug = format!("{argument:?}");
    [
        "conflicts",
        "overrides",
        "requires",
        "r_ifs",
        "r_unless",
        "default_vals_ifs",
    ]
    .into_iter()
    .find(|field| !debug.contains(&format!("{field}: []")))
}

/// The first group of `command` that requires, excludes or conflicts, read from its `Debug`
/// form as [`relation`] reads an argument's. The derive's own group per `#[derive(Args)]` struct
/// does none of these.
fn constraining_group(command: &Command) -> Option<String> {
    command
        .get_groups()
        .find(|group| {
            let debug = format!("{group:?}");
            !(debug.contains("required: false")
                && debug.contains("requires: []")
                && debug.contains("conflicts: []")
                && debug.contains("multiple: true"))
        })
        .map(|group| group.get_id().to_string())
}
