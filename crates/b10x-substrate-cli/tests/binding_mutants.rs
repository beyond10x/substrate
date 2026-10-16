//! Mutants of the clap command against the `cli_binding_validates` check (`tests/binding.rs`).
//!
//! `accepted_by_binding_check` is that test's own comparison (`tests/common/mod.rs`), returning
//! whether it would pass instead of asserting. Each `mutant_*` case feeds it `Cli::command()` with one
//! presented difference from `spec/generated/cli/binding.json` and asserts the check refuses it.
//! A red `mutant_*` case is a binary that presents something the binding does not name and still
//! passes `cli-binding-validates`. The `control_*` cases prove the predicate accepts the shipped
//! command and refuses the basic differences.

use b10x_substrate_cli::Cli;
use clap::{Arg, ArgAction, CommandFactory as _};

mod common;
use common::accepted_by_binding_check;

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
