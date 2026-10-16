//! Generated typed CLI adapter.
pub mod wire;
pub mod runtime;
pub use runtime::*;
pub fn plan() -> wire::Plan { serde_json::from_str(include_str!("../binding.json")).expect("compiled CLI plan") }
pub fn command() -> clap::Command { runtime::command(&plan()) }
pub fn run(args: Vec<std::ffi::OsString>, sources: &mut dyn Sources, handler: &mut dyn Handler, dynamic: Option<&mut dyn DynamicValidator>) -> ProcessOutput { runtime::run(&plan(), args, sources, handler, dynamic) }
