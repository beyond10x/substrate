//! `substrate-exec-gate`: applies a run's executable allow-list and executes the run's program
//! (design 35 D4, D5, D24). Every failure before the ruleset is applied exits 125 and executes
//! nothing.

fn main() {
    std::process::exit(substrate_exec_gate::main(std::env::args_os().skip(1)));
}
