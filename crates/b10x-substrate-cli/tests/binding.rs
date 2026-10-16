//! `cli-binding-validates`: the hand-written clap derive command line is held to the ESS CLI
//! binding. `cargo xtask check-ess` proves `spec/client/cli.yaml` valid and
//! `spec/generated/cli/binding.json` its exact resolved form; this test proves the binary presents
//! exactly that binding: the same leaf paths, and per leaf the same aliases, arguments, sources,
//! required-ness and enum values, the two process globals, and nothing else beyond what clap
//! generates itself. The comparison is `common::check`, which `binding_mutants.rs` also uses.

mod common;

use b10x_substrate_cli::Cli;
use clap::CommandFactory as _;

#[test]
fn cli_binding_validates() {
    if let Err(difference) = common::check(Cli::command()) {
        panic!("the command line differs from the binding: {difference}");
    }
}
