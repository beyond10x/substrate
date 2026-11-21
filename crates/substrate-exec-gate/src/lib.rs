//! The executable gate of the `linux` confinement adapter (design 35).
//!
//! `substrate-exec-gate` is the last program a confined run executes before its own. It applies
//! one Landlock ruleset to itself and then executes the run's program, so every process of the
//! run inherits the ruleset and none can remove it (D4):
//!
//! | rule | right | why |
//! |---|---|---|
//! | `/` | refer | a ruleset denies every rename or link into another directory unless it grants refer (D6) |
//! | the program | execute | the literal program, as a file rule |
//! | the loader | execute | the kernel opens a dynamically linked program's loader with the execute right (D7) |
//! | each `--allow` path | execute | an entry of the run's allow-list: a directory and everything beneath it, or one file (D2) |
//!
//! The ruleset handles the execute and refer rights and nothing else: reading, writing and the
//! network behave as before.
//!
//! ```text
//! substrate-exec-gate --version
//! substrate-exec-gate <option>... -- <program> [<argument>...]
//!
//! <option>  = --program  <class> <root> <rel>
//!           | --loader   <path>
//!           | --allow    <class> <root> <rel>
//!           | --writable <root>
//! <class>   = follow | beneath
//! <root>    = the absolute path of a mount in the run's view
//! <rel>     = a normal relative path beneath <root>, or "." for <root> itself
//! <program> = the text of <root>/<rel> of --program
//! ```
//!
//! A `beneath` path is opened beneath its `<root>` without following any symbolic link, so a
//! link swapped in after the driver's check is refused by the kernel. A `follow` path, the loader
//! among them, is opened following symbolic links, and is refused when what it resolves to lies
//! in a `--writable` root: a mount of the view the run can write. That question is answered by
//! [place], from the view's mount table, and a place that cannot be established refuses as well.
//!
//! ```text
//! substrate-exec-gate: --allow /opt/ro/link resolves to /workspace/tools/bin, in the writable mount /workspace
//! ```
//!
//! **It fails closed.** The program is executed only after `landlock_restrict_self` returned 0.
//! Any earlier failure (an argument it does not accept, a path it cannot open, a kernel without
//! Landlock, a rule the kernel refuses) writes one line beginning `substrate-exec-gate:` to
//! standard error and exits [`REFUSED`]. An option this grammar does not name is such a failure.
//! When the program itself cannot be executed the status is 126, or 127 for a missing file, as
//! `env` answers in that position.
//!
//! **What the ruleset does not stop** (design 35 § 7): an allowed interpreter runs any script it
//! can read, the loader started directly runs any program file it can read, a memfd copy of a
//! program runs, and mapping a file as code is not execution.

use std::ffi::OsString;

#[cfg(target_os = "linux")]
mod landlock;
pub mod place;

/// The line `substrate-exec-gate --version` prints, which the driver compares at open.
pub const PROTOCOL: &str = "substrate-exec-gate/1";

/// The exit status of every failure before the ruleset is applied: nothing was executed.
pub const REFUSED: i32 = 125;

/// How a path is opened (design 35 D24).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// A mount no run can write: symbolic links are followed.
    Follow,
    /// A mount a run can write, in this or an earlier start: no part may be a symbolic link.
    Beneath,
}

impl Class {
    /// The word the argument list carries.
    pub const fn word(self) -> &'static str {
        match self {
            Self::Follow => "follow",
            Self::Beneath => "beneath",
        }
    }
}

/// One path of the argument list: the mount it lies in, and where beneath it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    class: Class,
    root: String,
    rel: String,
}

impl Located {
    /// A path beneath `root`, the absolute path of a mount; `rel` is a normal relative path, or
    /// `.` for the mount itself.
    ///
    /// # Errors
    ///
    /// A `root` that is not absolute and normal, or a `rel` that is absolute or not normal.
    pub fn new(class: Class, root: &str, rel: &str) -> Result<Self, String> {
        if !is_absolute_normal(root) {
            return Err(format!("{root:?} is not an absolute normal path"));
        }
        if rel != "." && (rel.starts_with('/') || !segments_are_normal(rel)) {
            return Err(format!("{rel:?} is not a normal relative path"));
        }
        Ok(Self {
            class,
            root: root.to_owned(),
            rel: rel.to_owned(),
        })
    }

    pub const fn class(&self) -> Class {
        self.class
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn rel(&self) -> &str {
        &self.rel
    }

    /// The text of `<root>/<rel>`: `<root>` itself for `.`.
    pub fn path(&self) -> String {
        match (self.root.as_str(), self.rel.as_str()) {
            (root, ".") => root.to_owned(),
            ("/", rel) => format!("/{rel}"),
            (root, rel) => format!("{root}/{rel}"),
        }
    }

    fn options(&self, option: &str) -> [String; 4] {
        [
            option.to_owned(),
            self.class.word().to_owned(),
            self.root.clone(),
            self.rel.clone(),
        ]
    }
}

/// Whether every `/`-separated part of `text` is a name: not empty, not `.`, not `..`, and
/// without a NUL byte.
fn segments_are_normal(text: &str) -> bool {
    !text.contains('\0')
        && text
            .split('/')
            .all(|segment| !matches!(segment, "" | "." | ".."))
}

/// Whether `text` is `/`, or `/` followed by names separated by single `/`.
fn is_absolute_normal(text: &str) -> bool {
    text == "/" || text.strip_prefix('/').is_some_and(segments_are_normal)
}

/// What one run is held to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lists {
    /// The run's literal program.
    pub program: Located,
    /// The host's ELF loader, where the base system's programs have one.
    pub loader: Option<String>,
    /// The entries of the allow-list.
    pub allow: Vec<Located>,
    /// The mounts of the view the run can write, which no `follow` path may resolve into.
    pub writable: Vec<String>,
}

impl Lists {
    /// The options that carry these lists, ending with the `--` the program follows.
    pub fn options(&self) -> Vec<String> {
        let mut options = self.program.options("--program").to_vec();
        if let Some(loader) = &self.loader {
            options.extend(["--loader".to_owned(), loader.clone()]);
        }
        for entry in &self.allow {
            options.extend(entry.options("--allow"));
        }
        for root in &self.writable {
            options.extend(["--writable".to_owned(), root.clone()]);
        }
        options.push("--".to_owned());
        options
    }
}

/// One parsed argument list.
#[derive(Debug, PartialEq, Eq)]
pub enum Invocation {
    /// `--version`, alone.
    Version,
    /// The lists, and the program with its arguments.
    Run {
        lists: Lists,
        command: Vec<OsString>,
    },
}

/// Parses the arguments after the helper's own name.
///
/// # Errors
///
/// What the argument list breaks, as the text of the diagnostic line: an unknown option, a
/// missing operand, a class other than the two words, a root or loader that is not an absolute
/// normal path, a `<rel>` that is absolute or not normal, no `--program` or a second one, a
/// second `--loader`, no `--`, no program, or a program that differs from its option.
pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Invocation, String> {
    let mut arguments = arguments.into_iter();
    let mut operand = |option: &str| -> Result<String, String> {
        arguments
            .next()
            .ok_or_else(|| format!("{option} lacks an operand"))?
            .into_string()
            .map_err(|_| format!("an operand of {option} is not UTF-8"))
    };
    let mut program = None;
    let mut loader = None;
    let mut allow = Vec::new();
    let mut writable = Vec::new();
    let mut first = true;
    loop {
        let Ok(option) = operand("the option list") else {
            return Err("the arguments do not end with -- and a program".to_owned());
        };
        match option.as_str() {
            "--version" if first => {
                return match operand("--version") {
                    Err(_) => Ok(Invocation::Version),
                    Ok(_) => Err("--version takes no other argument".to_owned()),
                };
            }
            "--" => break,
            "--program" | "--allow" => {
                let class = match operand(&option)?.as_str() {
                    "follow" => Class::Follow,
                    "beneath" => Class::Beneath,
                    other => return Err(format!("{other:?} is not a class of {option}")),
                };
                let located = Located::new(class, &operand(&option)?, &operand(&option)?)?;
                if option == "--allow" {
                    allow.push(located);
                } else if program.replace(located).is_some() {
                    return Err("--program occurs twice".to_owned());
                }
            }
            "--writable" => {
                let root = operand(&option)?;
                if !is_absolute_normal(&root) {
                    return Err(format!(
                        "the writable mount {root:?} is not an absolute normal path"
                    ));
                }
                writable.push(root);
            }
            "--loader" => {
                let path = operand(&option)?;
                if path == "/" || !is_absolute_normal(&path) {
                    return Err(format!(
                        "the loader {path:?} is not an absolute normal path"
                    ));
                }
                if loader.replace(path).is_some() {
                    return Err("--loader occurs twice".to_owned());
                }
            }
            other => return Err(format!("{other:?} is not an option of this helper")),
        }
        first = false;
    }
    let command: Vec<OsString> = arguments.collect();
    let program = program.ok_or("no --program names the program")?;
    let named = command.first().ok_or("no program follows --")?;
    if named.to_str() != Some(program.path().as_str()) {
        // Quoted, so no byte of a name breaks the one diagnostic line.
        return Err(format!(
            "the program {:?} is not the {:?} that --program names",
            named.to_string_lossy(),
            program.path()
        ));
    }
    Ok(Invocation::Run {
        lists: Lists {
            program,
            loader,
            allow,
            writable,
        },
        command,
    })
}

/// A little-endian integer of `width` bytes at `offset` of `image`.
fn field(image: &[u8], offset: usize, width: usize) -> Option<usize> {
    let bytes = image.get(offset..offset.checked_add(width)?)?;
    let mut value = [0_u8; 8];
    value[..width].copy_from_slice(bytes);
    usize::try_from(u64::from_le_bytes(value)).ok()
}

/// The `PT_INTERP` of an ELF image: the loader a dynamically linked program names. `None` for a
/// static program and for bytes that are not a little-endian ELF image.
pub fn elf_interpreter(image: &[u8]) -> Option<String> {
    const PT_INTERP: usize = 3;
    if image.get(..4)? != b"\x7fELF" || *image.get(5)? != 1 {
        return None;
    }
    // Where the header keeps the program header table, and a program header its segment, for
    // the 32-bit and the 64-bit class: (e_phoff, its width, e_phentsize, e_phnum, p_offset,
    // p_filesz).
    let (table, word, entry_size, count, segment_offset, segment_size) = match *image.get(4)? {
        1 => (28, 4, 42, 44, 4, 16),
        2 => (32, 8, 54, 56, 8, 32),
        _ => return None,
    };
    let table = field(image, table, word)?;
    let entry_size = field(image, entry_size, 2)?;
    for index in 0..field(image, count, 2)? {
        let header = table.checked_add(index.checked_mul(entry_size)?)?;
        if field(image, header, 4)? != PT_INTERP {
            continue;
        }
        let start = field(image, header.checked_add(segment_offset)?, word)?;
        let size = field(image, header.checked_add(segment_size)?, word)?;
        let text = image.get(start..start.checked_add(size)?)?;
        let text = text.split(|byte| *byte == 0).next()?;
        let path = std::str::from_utf8(text).ok()?;
        return (path != "/" && is_absolute_normal(path)).then(|| path.to_owned());
    }
    None
}

fn refuse(reason: &str) -> i32 {
    // One line: a path is written as a quoted string, so no byte of it breaks the line.
    eprintln!("substrate-exec-gate: {reason}");
    REFUSED
}

/// Applies the ruleset and executes the program. Returns only when it executed nothing.
#[cfg(target_os = "linux")]
fn run(lists: &Lists, command: &[OsString]) -> i32 {
    use std::os::unix::process::CommandExt as _;

    if let Err(reason) = landlock::restrict(lists) {
        return refuse(&reason);
    }
    // In this process, with its environment and descriptors: the program takes the helper's
    // place. `exec` restores the signal mask and `SIGPIPE`, which this runtime ignores.
    let error = std::process::Command::new(&command[0])
        .args(&command[1..])
        .exec();
    eprintln!(
        "substrate-exec-gate: cannot execute {:?}: {error}",
        command[0].to_string_lossy()
    );
    if error.kind() == std::io::ErrorKind::NotFound {
        127
    } else {
        126
    }
}

#[cfg(not(target_os = "linux"))]
fn run(_lists: &Lists, _command: &[OsString]) -> i32 {
    refuse("the executable gate needs Linux")
}

/// The whole helper: parses `arguments`, applies the ruleset and executes the program. Returns
/// the exit status when it did not execute one.
pub fn main(arguments: impl IntoIterator<Item = OsString>) -> i32 {
    use std::io::Write as _;

    match parse(arguments) {
        Ok(Invocation::Version) => match writeln!(std::io::stdout(), "{PROTOCOL}") {
            Ok(()) => 0,
            Err(error) => refuse(&format!("cannot write the protocol line: {error}")),
        },
        Ok(Invocation::Run { lists, command }) => run(&lists, &command),
        Err(reason) => refuse(&reason),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn located(class: Class, root: &str, rel: &str) -> Located {
        Located::new(class, root, rel).expect("a valid path")
    }

    #[test]
    fn version_is_accepted_alone() {
        assert_eq!(parse(arguments(&["--version"])), Ok(Invocation::Version));
        for list in [&["--version", "--"][..], &["--version", "x"], &[]] {
            parse(arguments(list)).expect_err("not the version form");
        }
    }

    #[test]
    fn a_run_carries_the_program_the_loader_and_every_entry_in_order() {
        let parsed = parse(arguments(&[
            "--program",
            "follow",
            "/usr",
            "bin/sh",
            "--loader",
            "/lib64/ld-linux-x86-64.so.2",
            "--allow",
            "beneath",
            "/workspace",
            "target/debug",
            "--allow",
            "follow",
            "/opt/tool",
            ".",
            "--writable",
            "/workspace",
            "--writable",
            "/tmp",
            "--",
            "/usr/bin/sh",
            "-c",
            "--allow",
        ]))
        .expect("parsed");
        let lists = Lists {
            program: located(Class::Follow, "/usr", "bin/sh"),
            loader: Some("/lib64/ld-linux-x86-64.so.2".to_owned()),
            allow: vec![
                located(Class::Beneath, "/workspace", "target/debug"),
                located(Class::Follow, "/opt/tool", "."),
            ],
            writable: vec!["/workspace".to_owned(), "/tmp".to_owned()],
        };
        assert_eq!(
            parsed,
            Invocation::Run {
                lists,
                command: arguments(&["/usr/bin/sh", "-c", "--allow"]),
            },
            "everything after -- is the program's, an option word included"
        );
    }

    #[test]
    fn a_path_is_its_root_for_a_dot_and_root_slash_rel_otherwise() {
        assert_eq!(located(Class::Follow, "/opt/tool", ".").path(), "/opt/tool");
        assert_eq!(
            located(Class::Beneath, "/workspace", "a/b").path(),
            "/workspace/a/b"
        );
        assert_eq!(located(Class::Follow, "/", "usr").path(), "/usr");
    }

    #[test]
    fn a_root_or_rel_that_is_not_normal_is_refused() {
        for (root, rel) in [
            ("usr", "bin"),
            ("", "bin"),
            ("/usr/", "bin"),
            ("/usr/../etc", "bin"),
            ("/usr/./x", "bin"),
            ("//usr", "bin"),
            ("/usr", ""),
            ("/usr", "/bin"),
            ("/usr", "bin/"),
            ("/usr", "bin//sh"),
            ("/usr", "bin/../sh"),
            ("/usr", "./bin"),
            ("/usr", ".."),
        ] {
            Located::new(Class::Follow, root, rel)
                .expect_err(&format!("{root:?} {rel:?} is not a path of the grammar"));
        }
    }

    /// The options the driver renders are exactly what the helper parses.
    #[test]
    fn the_options_of_a_list_parse_back_to_it() {
        for loader in [None, Some("/lib/ld-musl-x86_64.so.1".to_owned())] {
            let lists = Lists {
                program: located(Class::Beneath, "/workspace", "bin/tool"),
                loader,
                allow: vec![
                    located(Class::Follow, "/usr", "bin"),
                    located(Class::Beneath, "/workspace", "bin"),
                ],
                writable: vec!["/workspace".to_owned(), "/dev".to_owned()],
            };
            let mut list = lists.options();
            assert_eq!(list.last().map(String::as_str), Some("--"));
            assert_eq!(list[0], "--program", "the program comes first");
            list.push(lists.program.path());
            list.push("argument".to_owned());
            assert_eq!(
                parse(list.iter().map(OsString::from)),
                Ok(Invocation::Run {
                    lists,
                    command: arguments(&["/workspace/bin/tool", "argument"]),
                })
            );
        }
    }

    #[test]
    fn each_malformed_argument_list_is_refused() {
        const PROGRAM: [&str; 4] = ["--program", "follow", "/usr", "bin/sh"];
        let with = |extra: &[&str], command: &[&str]| {
            let mut list: Vec<&str> = PROGRAM.to_vec();
            list.extend_from_slice(extra);
            list.push("--");
            list.extend_from_slice(command);
            arguments(&list)
        };
        parse(with(&[], &["/usr/bin/sh"])).expect("the control parses");
        let refused: Vec<(&str, Vec<OsString>)> = vec![
            ("no option at all", arguments(&["--", "/usr/bin/sh"])),
            ("no --", arguments(&PROGRAM)),
            ("no program after --", with(&[], &[])),
            ("a program that differs", with(&[], &["/usr/bin/bash"])),
            (
                "a second --program",
                with(&["--program", "follow", "/usr", "bin/sh"], &["/usr/bin/sh"]),
            ),
            (
                "a second --loader",
                with(&["--loader", "/a", "--loader", "/a"], &["/usr/bin/sh"]),
            ),
            (
                "a relative loader",
                with(&["--loader", "ld.so"], &["/usr/bin/sh"]),
            ),
            (
                "an unknown class",
                with(&["--allow", "write", "/usr", "bin"], &["/usr/bin/sh"]),
            ),
            (
                "a missing operand",
                with(&["--allow", "follow", "/usr"], &["/usr/bin/sh"]),
            ),
            (
                "an absolute rel",
                with(&["--allow", "follow", "/usr", "/bin"], &["/usr/bin/sh"]),
            ),
            (
                "a rel with ..",
                with(
                    &["--allow", "beneath", "/workspace", "a/../b"],
                    &["/usr/bin/sh"],
                ),
            ),
            (
                "a relative root",
                with(&["--allow", "follow", "usr", "bin"], &["/usr/bin/sh"]),
            ),
            (
                "a relative writable mount",
                with(&["--writable", "workspace"], &["/usr/bin/sh"]),
            ),
            ("a writable mount without its operand", {
                let mut list = PROGRAM.to_vec();
                list.push("--writable");
                arguments(&list)
            }),
            ("an unknown option", with(&["--bogus"], &["/usr/bin/sh"])),
            // The deny-list is `story:executable-deny-list`: this helper refuses its options.
            (
                "--deny",
                with(&["--deny", "follow", "/usr", "bin/id"], &["/usr/bin/sh"]),
            ),
            (
                "--alias",
                with(&["--alias", "/usr/bin", "/bin"], &["/usr/bin/sh"]),
            ),
            ("a bare word before --", with(&["word"], &["/usr/bin/sh"])),
        ];
        for (what, list) in refused {
            assert!(parse(list).is_err(), "{what} was accepted");
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_operand_that_is_not_utf8_is_refused() {
        use std::os::unix::ffi::OsStringExt as _;
        let mut list = arguments(&["--program", "follow", "/usr"]);
        list.push(OsString::from_vec(vec![b'b', 0xff]));
        list.extend(arguments(&["--", "/usr/b"]));
        parse(list).expect_err("an option operand is text");
    }

    /// A 64-bit little-endian image with one `PT_INTERP` header naming `interpreter`.
    fn elf64(interpreter: Option<&str>) -> Vec<u8> {
        let mut image = vec![0_u8; 64];
        image[..4].copy_from_slice(b"\x7fELF");
        image[4] = 2; // ELFCLASS64
        image[5] = 1; // little-endian
        image[32..40].copy_from_slice(&64_u64.to_le_bytes()); // e_phoff
        image[54..56].copy_from_slice(&56_u16.to_le_bytes()); // e_phentsize
        image[56..58].copy_from_slice(&2_u16.to_le_bytes()); // e_phnum
        let mut load = vec![0_u8; 56];
        load[..4].copy_from_slice(&1_u32.to_le_bytes()); // PT_LOAD
        image.extend(load);
        let mut header = vec![0_u8; 56];
        let text = interpreter
            .map(|text| format!("{text}\0"))
            .unwrap_or_default();
        if interpreter.is_some() {
            header[..4].copy_from_slice(&3_u32.to_le_bytes()); // PT_INTERP
            header[8..16].copy_from_slice(&176_u64.to_le_bytes()); // p_offset
            header[32..40].copy_from_slice(&(text.len() as u64).to_le_bytes()); // p_filesz
        }
        image.extend(header);
        image.extend(text.bytes());
        image
    }

    #[test]
    fn the_interpreter_is_read_from_pt_interp() {
        assert_eq!(
            elf_interpreter(&elf64(Some("/lib64/ld-linux-x86-64.so.2"))).as_deref(),
            Some("/lib64/ld-linux-x86-64.so.2")
        );
        assert_eq!(elf_interpreter(&elf64(None)), None, "a static image");
        assert_eq!(elf_interpreter(b"#!/bin/sh\n"), None, "a script");
        let mut truncated = elf64(Some("/lib64/ld-linux-x86-64.so.2"));
        truncated.truncate(180);
        assert_eq!(elf_interpreter(&truncated), None, "a truncated image");
        let mut relative = elf64(Some("ld.so"));
        assert_eq!(elf_interpreter(&relative), None, "a relative interpreter");
        relative.clear();
        assert_eq!(elf_interpreter(&relative), None);
    }
}
