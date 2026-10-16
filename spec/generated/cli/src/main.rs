fn main() {
    let output = cli_contract::run(std::env::args_os().collect(), &mut cli_contract::OsSources, &mut cli_contract::UnavailableHandler, None);
    print!("{}", output.stdout);
    eprint!("{}", output.stderr);
    std::process::exit(output.exit_code);
}
