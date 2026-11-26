//! Where the tools a lane case runs lie, as entries of an executable allow-list.
//!
//! An entry admits a file by where the file itself lies (design 35 § 6): a symbolic link in a
//! listed directory that points outside it is not admitted by that directory. A host may keep a
//! base tool as such a link (Ubuntu 26.04 keeps `/usr/bin/ln` as a link into
//! `/usr/lib/cargo/bin/coreutils`), so a case lists the directory that holds each tool's file:
//! the parent of the tool's path with every symbolic link followed.

use std::path::{Path, PathBuf};

/// The first of `searched` that holds `name`, as a path in that directory.
fn found(searched: &[&Path], name: &str) -> Option<PathBuf> {
    searched
        .iter()
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
}

/// The directories that hold the files of `names`, each tool taken from the first of `searched`
/// that has it, sorted and each once, as list entries.
///
/// # Panics
///
/// Where none of `searched` has a tool, or its path does not resolve to a file in a directory
/// named in UTF-8.
pub fn holding_directories(searched: &[&Path], names: &[&str]) -> Vec<String> {
    let mut directories: Vec<String> = names
        .iter()
        .map(|name| {
            let path =
                found(searched, name).unwrap_or_else(|| panic!("no {name} in any of {searched:?}"));
            let file = std::fs::canonicalize(&path)
                .unwrap_or_else(|error| panic!("{} does not resolve: {error}", path.display()));
            let directory = file.parent().expect("a directory");
            directory
                .to_str()
                .unwrap_or_else(|| panic!("{} is not UTF-8", directory.display()))
                .to_owned()
        })
        .collect();
    directories.sort();
    directories.dedup();
    directories
}
