#![forbid(unsafe_code)]

use std::process::ExitCode;

use b10x_substrate_mcp::run;
use b10x_substrate_sdk::run_daemon_child_if_requested;

#[tokio::main]
async fn main() -> ExitCode {
    match run_daemon_child_if_requested().await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => match run().await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("substrate-mcp: {error}");
                ExitCode::FAILURE
            }
        },
        Err(_) => {
            eprintln!("substrate-mcp: linked daemon child failed");
            ExitCode::FAILURE
        }
    }
}
