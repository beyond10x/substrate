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
//!           | --env      <name> <value>
//! <class>   = follow | beneath
//! <root>    = the absolute path of a mount in the run's view
//! <rel>     = a normal relative path beneath <root>, or "." for <root> itself
//! <program> = the text of <root>/<rel> of --program
//! ```
//!
//! **The program's environment is the `--env` operands and nothing else** (D28). A variable of
//! the run must reach no process that runs before the ruleset is applied: the loader of such a
//! process would act on `LD_PRELOAD` and its like, and what it then starts is held to no list.
//! So the driver sets no variable of the run for bubblewrap, and hands each to the helper as an
//! operand, which only the `execve` of the program receives.
//!
//! - The helper never passes its own environment on.
//! - The helper refuses to run when it was started with any variable but `PWD`, which
//!   bubblewrap sets after `--clearenv`. A caller that leaks the run's variables into the
//!   helper's environment is thereby refused at every start, not trusted.
//! - What the helper cannot do is undo what a loader did before its first instruction: a
//!   dynamically linked build started with `LD_PRELOAD` naming a loadable object has already
//!   run that object. The driver never starts it so; a static build has no loader to ask.
//!
//! A `beneath` path is opened beneath its `<root>` without following any symbolic link, so a
//! link swapped in after the driver's check is refused by the kernel. A `follow` path, the loader
//! among them, is opened following symbolic links, and is refused when what it resolves to lies
//! in a `--writable` root: a mount of the view the run can write, with every mount that is
//! mounted beneath it (D30). Every symbolic link the path resolves through is asked the same
//! question, and refused there too. It is answered by [place], from the view's mount table,
//! and a place, or where such a mount is attached, that cannot be established refuses as well:
//! a link on a file-system type outside the list has no place.
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
    /// The options that carry these lists and no variable, ending with the `--` the program
    /// follows: the program then starts with an empty environment.
    pub fn options(&self) -> Vec<String> {
        self.options_with(&[])
    }

    /// The options that carry these lists and the program's whole environment (D28), ending
    /// with the `--` the program follows.
    pub fn options_with(&self, environment: &[(String, String)]) -> Vec<String> {
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
        for (name, value) in environment {
            options.extend(["--env".to_owned(), name.clone(), value.clone()]);
        }
        options.push("--".to_owned());
        options
    }
}

/// The one variable the helper may be started with: bubblewrap sets it after `--clearenv`, and
/// no loader reads it.
const OWN_VARIABLE: &str = "PWD";

/// The variable the helper was started with that it must not have (D28): any but
/// [`OWN_VARIABLE`]. Decided on the names it is given.
pub fn foreign_variable<N: AsRef<std::ffi::OsStr>>(
    names: impl IntoIterator<Item = N>,
) -> Option<String> {
    names
        .into_iter()
        .find(|name| name.as_ref() != OWN_VARIABLE)
        .map(|name| name.as_ref().to_string_lossy().into_owned())
}

/// One parsed argument list.
#[derive(Debug, PartialEq, Eq)]
pub enum Invocation {
    /// `--version`, alone.
    Version,
    /// The lists, the program's whole environment, and the program with its arguments.
    Run {
        lists: Lists,
        environment: Vec<(String, String)>,
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
/// second `--loader`, a variable name that is empty or holds `=`, a variable named twice, a NUL
/// byte in a name or value, no `--`, no program, or a program that differs from its option.
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
    let mut environment: Vec<(String, String)> = Vec::new();
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
            "--env" => {
                let name = operand(&option)?;
                let value = operand(&option)?;
                if name.is_empty() || name.contains(['=', '\0']) || value.contains('\0') {
                    return Err(format!("{name:?} does not name a variable of --env"));
                }
                // The driver hands over one value per name; a second is not chosen between.
                if environment.iter().any(|(known, _)| *known == name) {
                    return Err(format!("--env names {name:?} twice"));
                }
                environment.push((name, value));
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
        environment,
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

/// The program header type of the loader a dynamically linked image names.
const PT_INTERP: usize = 3;

/// The class, byte order and machine of an ELF image: `EI_CLASS`, `EI_DATA` and `e_machine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElfTarget {
    /// `EI_CLASS`: 1 for a 32-bit image, 2 for a 64-bit one.
    pub class: u8,
    /// `EI_DATA`: 1 for little-endian, 2 for big-endian.
    pub data: u8,
    /// `e_machine`.
    pub machine: u16,
}

impl ElfTarget {
    /// What the kernel under this build executes natively: the target this crate was compiled
    /// for, which is the driver's own where the driver links it. `None` on an architecture
    /// this table does not name, where no image is taken for native.
    pub const NATIVE: Option<Self> = {
        let machine = if cfg!(target_arch = "x86_64") {
            Some(62)
        } else if cfg!(target_arch = "aarch64") {
            Some(183)
        } else if cfg!(target_arch = "x86") {
            Some(3)
        } else if cfg!(target_arch = "arm") {
            Some(40)
        } else if cfg!(target_arch = "riscv64") {
            Some(243)
        } else {
            None
        };
        match machine {
            Some(machine) => Some(Self {
                class: if cfg!(target_pointer_width = "64") {
                    2
                } else {
                    1
                },
                data: if cfg!(target_endian = "little") { 1 } else { 2 },
                machine,
            }),
            None => None,
        }
    };
}

/// How the kernel starts an executable image, read from its ELF header and program headers
/// and from nothing else (design 35 D29, rule 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Linkage {
    /// A native ELF image with no `PT_INTERP` header: the kernel starts it directly, and no
    /// loader runs before its first instruction. A static-pie image is one, though it has a
    /// `PT_DYNAMIC` header.
    Static,
    /// A native ELF image with a `PT_INTERP` header, whatever the header names: a loader runs
    /// first.
    Interpreted,
    /// Not an ELF image, or one of another class, byte order or machine than the native one. A
    /// `#!` script is one, and what runs first there is its interpreter; an image of another
    /// machine is run by whatever the host registered for it.
    Foreign,
    /// A native ELF image the kernel's loader refuses before it maps anything (`ENOEXEC`): its
    /// type is neither `ET_EXEC` nor `ET_DYN`, its `e_phentsize` is not the size of a program
    /// header of its class, or it declares no program header or more than the kernel reads,
    /// which the escape value of a count kept elsewhere (`PN_XNUM`) is. A caller that starts a
    /// program with `execvp` then runs a shell with the file as its script.
    Unstartable,
    /// A native ELF image whose header or program header table was not read whole: one of
    /// them lies outside the bytes.
    Unreadable,
}

/// Whether the kernel starts `image` directly: [`Linkage::Static`] exactly when it is an ELF
/// image of [`ElfTarget::NATIVE`], its header is one the kernel's loader takes
/// ([`Linkage::Unstartable`] otherwise), its whole program header table was read, and no
/// header has the type `PT_INTERP`. The four are asked in that order. The contents of a
/// `PT_INTERP` header are not read, and `PT_DYNAMIC` is not asked.
pub fn elf_linkage(image: &[u8]) -> Linkage {
    linkage_for(image, ElfTarget::NATIVE)
}

fn linkage_for(image: &[u8], native: Option<ElfTarget>) -> Linkage {
    // Only a little-endian image is read here, so a big-endian host takes none.
    let identity = || {
        let found = ElfTarget {
            class: *image.get(4)?,
            data: *image.get(5)?,
            machine: u16::try_from(field(image, 18, 2)?).ok()?,
        };
        (image.get(..4)? == b"\x7fELF" && found.data == 1 && Some(found) == native).then_some(())
    };
    if identity().is_none() {
        return Linkage::Foreign;
    }
    let Some(table) = ProgramHeaderTable::of(image) else {
        return Linkage::Unreadable;
    };
    if !table.is_one_the_kernel_loads() {
        return Linkage::Unstartable;
    }
    match table.types(image) {
        Some(types) if types.contains(&PT_INTERP) => Linkage::Interpreted,
        Some(_) => Linkage::Static,
        None => Linkage::Unreadable,
    }
}

/// The largest program header table every kernel this crate runs on reads: 4096 bytes, which
/// is 73 headers of the 64-bit class and 128 of the 32-bit one.
///
/// From `load_elf_phdrs` in `fs/binfmt_elf.c`, where `size` is `sizeof(struct elf_phdr) *
/// e_phnum`. Linux v5.19 (line 478) and v6.8 (line 523) refuse a table with
/// `size == 0 || size > 65536 || size > ELF_MIN_ALIGN`, and `ELF_MIN_ALIGN` is the larger of
/// `ELF_EXEC_PAGESIZE` and `PAGE_SIZE` (lines 81 to 85 of v6.18), so 4096 where a page is
/// 4 KiB. Linux v6.18 (line 537) refuses `size == 0 || size > 65536` only: measured on
/// 6.18.55 with 4 KiB pages, an image with 74 headers and one with 1170 start, and one with
/// 1171 is `ENOEXEC`. The bound here is the smallest of these, so an image this crate takes
/// is one each of those kernels reads; an image with more headers is refused on every host,
/// also where the kernel would start it.
const PROGRAM_HEADER_TABLE_BYTES: usize = 4096;

/// What the header of a little-endian ELF image, 32-bit or 64-bit, says of its type and its
/// program header table.
struct ProgramHeaderTable {
    /// `e_type`.
    kind: usize,
    /// `e_phoff`.
    offset: usize,
    /// `e_phentsize`.
    entry_size: usize,
    /// `e_phnum`.
    count: usize,
    /// The size of one program header of the image's class.
    header_size: usize,
}

impl ProgramHeaderTable {
    /// `None` when the header ends outside the bytes, and for a class that is neither.
    fn of(image: &[u8]) -> Option<Self> {
        // (e_phoff, its width, e_phentsize, e_phnum, the size of one program header).
        let (offset, word, entry_size, count, header_size) = match *image.get(4)? {
            1 => (28, 4, 42, 44, 32),
            2 => (32, 8, 54, 56, 56),
            _ => return None,
        };
        Some(Self {
            kind: field(image, 16, 2)?,
            offset: field(image, offset, word)?,
            entry_size: field(image, entry_size, 2)?,
            count: field(image, count, 2)?,
            header_size,
        })
    }

    /// What the kernel's loader asks of the header before it maps anything, and answers
    /// `ENOEXEC` without (`load_elf_binary` and `load_elf_phdrs` of `fs/binfmt_elf.c`): the type
    /// is `ET_EXEC` or `ET_DYN`, `e_phentsize` is the size of a program header of the class,
    /// and the table is neither empty nor over [`PROGRAM_HEADER_TABLE_BYTES`]. The last refuses
    /// a count kept elsewhere (`PN_XNUM`, 0xffff), which the kernel does not follow.
    fn is_one_the_kernel_loads(&self) -> bool {
        const ET_EXEC: usize = 2;
        const ET_DYN: usize = 3;
        if !matches!(self.kind, ET_EXEC | ET_DYN) {
            return false;
        }
        if self.entry_size != self.header_size {
            return false;
        }
        self.count != 0 && self.count * self.header_size <= PROGRAM_HEADER_TABLE_BYTES
    }

    /// The `p_type` of every program header, or `None` when one of them lies outside the bytes.
    fn types(&self, image: &[u8]) -> Option<Vec<usize>> {
        (0..self.count)
            .map(|index| {
                let header = self
                    .offset
                    .checked_add(index.checked_mul(self.entry_size)?)?;
                image.get(header..header.checked_add(self.entry_size)?)?;
                field(image, header, 4)
            })
            .collect()
    }
}

/// The `PT_INTERP` of an ELF image: the loader a dynamically linked program names. `None` for a
/// static program and for bytes that are not a little-endian ELF image.
pub fn elf_interpreter(image: &[u8]) -> Option<String> {
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
fn run(lists: &Lists, environment: &[(String, String)], command: &[OsString]) -> i32 {
    use std::os::unix::process::CommandExt as _;

    // D28: a variable of the run in this process's own environment reached a process that runs
    // before the ruleset, this one at the least. Nothing is executed.
    if let Some(name) = foreign_variable(std::env::vars_os().map(|(name, _)| name)) {
        return refuse(&format!(
            "was started with the variable {name:?}; the run's variables are passed with --env, \
             so that none reaches a process that runs before the ruleset"
        ));
    }
    if let Err(reason) = landlock::restrict(lists) {
        return refuse(&reason);
    }
    // In this process, with its descriptors: the program takes the helper's place. Its
    // environment is the `--env` operands and nothing of the helper's own. `exec` restores the
    // signal mask and `SIGPIPE`, which this runtime ignores.
    let error = std::process::Command::new(&command[0])
        .args(&command[1..])
        .env_clear()
        .envs(environment.iter().map(|(name, value)| (name, value)))
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
fn run(_lists: &Lists, _environment: &[(String, String)], _command: &[OsString]) -> i32 {
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
        Ok(Invocation::Run {
            lists,
            environment,
            command,
        }) => run(&lists, &environment, &command),
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

    /// `--version` is the first argument or it is not the version form: after any option it is
    /// an unknown option, so a list that ends in it runs nothing and prints no protocol line.
    #[test]
    fn version_after_an_option_is_refused() {
        for list in [
            &["--writable", "/workspace", "--version"][..],
            &["--program", "follow", "/usr", "bin/sh", "--version"],
            &["--loader", "/lib/ld.so", "--version"],
            &["--env", "A", "b", "--version"],
        ] {
            let refused = parse(arguments(list)).expect_err("--version after an option");
            assert!(
                refused.contains("--version"),
                "{list:?} is refused for its --version: {refused}"
            );
        }
    }

    /// D28: the program's environment is the `--env` operands, in the order given; each is a
    /// name and a value of its own, so no byte of a value reads as an option or a separator.
    #[test]
    fn the_environment_is_the_env_operands_and_nothing_else() {
        let lists = Lists {
            program: located(Class::Follow, "/usr", "bin/sh"),
            loader: None,
            allow: Vec::new(),
            writable: Vec::new(),
        };
        let environment = vec![
            ("LD_PRELOAD".to_owned(), "/workspace/pre.so".to_owned()),
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("EMPTY".to_owned(), String::new()),
            ("OPTION".to_owned(), "--".to_owned()),
            ("NAMED".to_owned(), "--env".to_owned()),
            ("SPACED".to_owned(), "two words".to_owned()),
            ("EQUALS".to_owned(), "a=b --env X y".to_owned()),
        ];
        let mut list = lists.options_with(&environment);
        assert_eq!(list.last().map(String::as_str), Some("--"));
        assert_eq!(
            list.len(),
            lists.options().len() + 3 * environment.len(),
            "one --env with a name and a value per variable: {list:?}"
        );
        list.push("/usr/bin/sh".to_owned());
        assert_eq!(
            parse(list.iter().map(OsString::from)),
            Ok(Invocation::Run {
                lists: lists.clone(),
                environment,
                command: arguments(&["/usr/bin/sh"]),
            })
        );
        // Without a variable the options are what they were, and the environment is empty.
        assert_eq!(lists.options(), lists.options_with(&[]));
        assert!(!lists.options().iter().any(|part| part == "--env"));
    }

    #[test]
    fn a_malformed_variable_is_refused() {
        const PROGRAM: [&str; 4] = ["--program", "follow", "/usr", "bin/sh"];
        for (what, extra) in [
            ("an empty name", &["--env", "", "value"][..]),
            ("a name with =", &["--env", "A=B", "value"]),
            ("a name twice", &["--env", "A", "1", "--env", "A", "2"]),
            ("a name without its value", &["--env", "A"]),
        ] {
            let mut list = PROGRAM.to_vec();
            list.extend_from_slice(extra);
            list.extend_from_slice(&["--", "/usr/bin/sh"]);
            assert!(parse(arguments(&list)).is_err(), "{what} was accepted");
        }
        // `--env A --` reads `--` as the value: then nothing ends the options.
        let mut list = PROGRAM.to_vec();
        list.extend_from_slice(&["--env", "A", "--", "/usr/bin/sh"]);
        assert!(parse(arguments(&list)).is_err());
    }

    /// D28: the helper may be started with `PWD`, which bubblewrap sets after `--clearenv`, and
    /// with nothing else.
    #[test]
    fn a_variable_of_its_own_other_than_pwd_is_foreign() {
        assert_eq!(foreign_variable::<&str>([]), None);
        assert_eq!(foreign_variable(["PWD"]), None);
        assert_eq!(
            foreign_variable(["PWD", "LD_PRELOAD"]).as_deref(),
            Some("LD_PRELOAD")
        );
        assert_eq!(foreign_variable(["PATH"]).as_deref(), Some("PATH"));
        assert_eq!(foreign_variable(["pwd"]).as_deref(), Some("pwd"));
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
                environment: Vec::new(),
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
                    environment: Vec::new(),
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

    const X86_64: u16 = 62;
    const AARCH64: u16 = 183;
    const I386: u16 = 3;

    /// A little-endian image of `class` (1 or 2) for `machine`, of the ELF type `kind`, whose
    /// program headers have the types `segments`, each header empty but for its type.
    fn image_with(class: u8, machine: u16, kind: u16, segments: &[u32]) -> Vec<u8> {
        let (header, offset_at, size_at, count_at, entry) = match class {
            1 => (52, 28, 42, 44, 32_usize),
            _ => (64, 32, 54, 56, 56_usize),
        };
        let mut image = vec![0_u8; header];
        image[..4].copy_from_slice(b"\x7fELF");
        image[4] = class;
        image[5] = 1;
        image[16..18].copy_from_slice(&kind.to_le_bytes());
        image[18..20].copy_from_slice(&machine.to_le_bytes());
        image[offset_at..offset_at + 2]
            .copy_from_slice(&u16::try_from(header).expect("small").to_le_bytes());
        image[size_at..size_at + 2]
            .copy_from_slice(&u16::try_from(entry).expect("small").to_le_bytes());
        image[count_at..count_at + 2]
            .copy_from_slice(&u16::try_from(segments.len()).expect("few").to_le_bytes());
        for segment in segments {
            let mut one = vec![0_u8; entry];
            one[..4].copy_from_slice(&segment.to_le_bytes());
            image.extend(one);
        }
        image
    }

    const ET_EXEC: u16 = 2;
    const ET_DYN: u16 = 3;
    const PT_LOAD: u32 = 1;
    const PT_DYNAMIC: u32 = 2;
    const INTERP: u32 = 3;
    const PT_PHDR: u32 = 6;
    const PT_TLS: u32 = 7;
    const PT_GNU_STACK: u32 = 0x6474_e551;

    /// D29, rule 1: among native images the helper's linkage is the presence of a `PT_INTERP`
    /// header and nothing else. A static image and a static-pie image, which has a
    /// `PT_DYNAMIC` header, are static; an image with `PT_INTERP` is not, wherever the header
    /// stands and whatever it names. Asked for a 64-bit and a 32-bit native target.
    #[test]
    fn linkage_is_the_presence_of_pt_interp() {
        for (class, machine) in [(2, X86_64), (2, AARCH64), (1, I386)] {
            let native = Some(ElfTarget {
                class,
                data: 1,
                machine,
            });
            let linkage = |kind: u16, segments: &[u32]| {
                linkage_for(&image_with(class, machine, kind, segments), native)
            };
            assert_eq!(
                linkage(ET_EXEC, &[PT_LOAD, PT_LOAD, PT_TLS, PT_GNU_STACK]),
                Linkage::Static,
                "class {class}: a static image that is not position independent"
            );
            assert_eq!(
                linkage(
                    ET_DYN,
                    &[PT_LOAD, PT_LOAD, PT_DYNAMIC, PT_TLS, PT_GNU_STACK]
                ),
                Linkage::Static,
                "class {class}: a static-pie image has PT_DYNAMIC and no PT_INTERP"
            );
            assert_eq!(
                linkage(
                    ET_DYN,
                    &[PT_PHDR, INTERP, PT_LOAD, PT_LOAD, PT_DYNAMIC, PT_GNU_STACK]
                ),
                Linkage::Interpreted,
                "class {class}: a dynamically linked image"
            );
            assert_eq!(
                linkage(ET_EXEC, &[PT_LOAD, PT_GNU_STACK, INTERP]),
                Linkage::Interpreted,
                "class {class}: PT_INTERP as the last header"
            );
        }
        // The interpreter's text is not what decides: `elf_interpreter` reads none here.
        let native = Some(ElfTarget {
            class: 2,
            data: 1,
            machine: X86_64,
        });
        let unnamed = image_with(2, X86_64, ET_DYN, &[PT_LOAD, INTERP]);
        assert_eq!(elf_interpreter(&unnamed), None);
        assert_eq!(linkage_for(&unnamed, native), Linkage::Interpreted);
        // Nor is its form: the reader of D7 takes a loader from the first of these only, and
        // every one of them is a loader the kernel would start, or fail to.
        for (path, read) in [
            (&b"/lib64/ld-linux-x86-64.so.2"[..], true),
            (b"ld.so", false),
            (b"/lib/../lib64/ld.so", false),
            (b"/lib/\xff", false),
            (b"", false),
            (b"/", false),
        ] {
            let mut image = image_with(2, X86_64, ET_DYN, &[PT_LOAD, INTERP]);
            let start = u64::try_from(image.len()).expect("small");
            let size = u64::try_from(path.len() + 1).expect("small");
            // The second header, 64 + 56 bytes in: p_offset at 8, p_filesz at 32.
            image[128..136].copy_from_slice(&start.to_le_bytes());
            image[152..160].copy_from_slice(&size.to_le_bytes());
            image.extend_from_slice(path);
            image.push(0);
            assert_eq!(
                elf_interpreter(&image).is_some(),
                read,
                "{:?}",
                String::from_utf8_lossy(path)
            );
            assert_eq!(
                linkage_for(&image, native),
                Linkage::Interpreted,
                "{:?}",
                String::from_utf8_lossy(path)
            );
        }
    }

    /// D29, rule 1, first point: an image of another machine, class or byte order than the
    /// native one is foreign, with or without a `PT_INTERP`, and so are bytes that are no ELF
    /// image: a `#!` script, nothing, the magic alone. On a target the table does not name,
    /// every image is.
    #[test]
    fn an_image_of_another_machine_and_a_script_are_foreign() {
        let native = Some(ElfTarget {
            class: 2,
            data: 1,
            machine: X86_64,
        });
        let fixed = image_with(2, X86_64, ET_EXEC, &[PT_LOAD]);
        assert_eq!(linkage_for(&fixed, native), Linkage::Static, "the control");
        for (what, image) in [
            (
                "another machine",
                image_with(2, AARCH64, ET_EXEC, &[PT_LOAD]),
            ),
            (
                "another machine, dynamically linked",
                image_with(2, AARCH64, ET_DYN, &[INTERP, PT_LOAD]),
            ),
            ("another class", image_with(1, I386, ET_EXEC, &[PT_LOAD])),
            (
                "the machine in another class",
                image_with(1, X86_64, ET_EXEC, &[PT_LOAD]),
            ),
        ] {
            assert_eq!(linkage_for(&image, native), Linkage::Foreign, "{what}");
        }
        let mut big_endian = fixed.clone();
        big_endian[5] = 2;
        assert_eq!(linkage_for(&big_endian, native), Linkage::Foreign);
        for (what, bytes) in [
            ("a script", &b"#!/bin/sh\nexec \"$@\"\n"[..]),
            ("nothing", b""),
            ("the magic alone", b"\x7fELF"),
            ("text", b"gate"),
        ] {
            assert_eq!(linkage_for(bytes, native), Linkage::Foreign, "{what}");
        }
        assert_eq!(
            linkage_for(&fixed, None),
            Linkage::Foreign,
            "no native target"
        );
        // This build's own target is one of the table's on every host the driver runs on.
        if cfg!(target_os = "linux") {
            let own = ElfTarget::NATIVE.expect("a Linux target the table names");
            let image = image_with(own.class, own.machine, ET_EXEC, &[PT_LOAD]);
            assert_eq!(elf_linkage(&image), Linkage::Static);
            assert_eq!(elf_linkage(b"#!/bin/sh\n"), Linkage::Foreign);
        }
    }

    /// D29, rule 1: a native image whose header or program header table was not read whole is
    /// refused as such, whatever the part that was read holds.
    #[test]
    fn a_truncated_image_is_unreadable() {
        for (class, machine) in [(2, X86_64), (1, I386)] {
            let native = Some(ElfTarget {
                class,
                data: 1,
                machine,
            });
            let whole = image_with(class, machine, ET_EXEC, &[PT_LOAD, PT_GNU_STACK]);
            assert_eq!(linkage_for(&whole, native), Linkage::Static);
            let mut truncated = whole.clone();
            truncated.pop();
            assert_eq!(
                linkage_for(&truncated, native),
                Linkage::Unreadable,
                "class {class}: the last header ends outside the bytes"
            );
            // The header that was cut off is the one that names a loader.
            let mut hidden = image_with(class, machine, ET_EXEC, &[PT_LOAD, INTERP]);
            hidden.truncate(hidden.len() - 8);
            assert_eq!(linkage_for(&hidden, native), Linkage::Unreadable);
            let mut header_only = whole.clone();
            header_only.truncate(if class == 1 { 52 } else { 64 });
            assert_eq!(linkage_for(&header_only, native), Linkage::Unreadable);
            let mut half_a_header = whole.clone();
            half_a_header.truncate(40);
            assert_eq!(
                linkage_for(&half_a_header, native),
                Linkage::Unreadable,
                "class {class}: the ELF header itself ends outside the bytes"
            );
        }
    }

    /// The two native targets the cases below ask: (class, machine, where `e_phentsize` is,
    /// the size of one program header, the most headers a kernel with 4 KiB pages reads).
    const TARGETS: [(u8, u16, usize, u16, usize); 2] =
        [(2, X86_64, 54, 56, 73), (1, I386, 42, 32, 128)];

    /// D29, rule 1: a count kept elsewhere (`PN_XNUM`, 0xffff in `e_phnum`) is not started. The
    /// kernel does not follow the escape value; it reads it as a count, over its bound. The
    /// image holds all 0xffff headers, none of them `PT_INTERP`, so nothing but the count
    /// refuses it.
    #[test]
    fn a_program_header_count_kept_elsewhere_is_unstartable() {
        const PN_XNUM: usize = 0xffff;
        for (class, machine, _, _, _) in TARGETS {
            let native = Some(ElfTarget {
                class,
                data: 1,
                machine,
            });
            let image = image_with(class, machine, ET_EXEC, &vec![PT_LOAD; PN_XNUM]);
            let count_at = if class == 1 { 44 } else { 56 };
            assert_eq!(image[count_at..count_at + 2], [0xff, 0xff]);
            assert_eq!(
                linkage_for(&image, native),
                Linkage::Unstartable,
                "class {class}"
            );
        }
    }

    /// D29, rule 1: a count the kernel's loader does not read is not started. The kernel reads
    /// a table of at most one page and refuses an empty one. Each image holds every header
    /// its count names, so nothing but the count refuses it.
    #[test]
    fn a_program_header_count_the_kernel_does_not_read_is_unstartable() {
        for (class, machine, _, _, most) in TARGETS {
            let native = Some(ElfTarget {
                class,
                data: 1,
                machine,
            });
            let counting = |count: usize| {
                linkage_for(
                    &image_with(class, machine, ET_EXEC, &vec![PT_LOAD; count]),
                    native,
                )
            };
            assert_eq!(counting(1), Linkage::Static, "class {class}: one header");
            assert_eq!(
                counting(most),
                Linkage::Static,
                "class {class}: one page of headers"
            );
            assert_eq!(
                counting(most + 1),
                Linkage::Unstartable,
                "class {class}: one header more than a page holds"
            );
            assert_eq!(
                counting(0),
                Linkage::Unstartable,
                "class {class}: no program header"
            );
        }
    }

    /// D29, rule 1: an `e_phentsize` other than the size of a program header of the class is
    /// not started, smaller or larger, as the kernel refuses it. Each image is long enough for
    /// its table at the size it states, so nothing but the size refuses it.
    #[test]
    fn a_program_header_size_other_than_the_classs_is_unstartable() {
        for (class, machine, size_at, size, _) in TARGETS {
            let native = Some(ElfTarget {
                class,
                data: 1,
                machine,
            });
            let whole = image_with(class, machine, ET_EXEC, &[PT_LOAD, PT_GNU_STACK, PT_LOAD]);
            assert_eq!(linkage_for(&whole, native), Linkage::Static, "the control");
            for (what, other) in [
                ("eight bytes smaller", size - 8),
                ("one byte smaller", size - 1),
                ("none", 0),
                ("one byte larger", size + 1),
                ("eight bytes larger", size + 8),
            ] {
                let mut image = whole.clone();
                image[size_at..size_at + 2].copy_from_slice(&other.to_le_bytes());
                image.extend([0_u8; 64]);
                assert_eq!(
                    linkage_for(&image, native),
                    Linkage::Unstartable,
                    "class {class}: e_phentsize {what}"
                );
            }
        }
    }

    /// D29, rule 1: an image whose type is neither `ET_EXEC` nor `ET_DYN` is not started,
    /// whatever its program headers hold: a relocatable object, a core file, no type, and the
    /// values an operating system or a processor keeps.
    #[test]
    fn an_image_of_a_type_the_kernel_does_not_execute_is_unstartable() {
        for (class, machine, _, _, _) in TARGETS {
            let native = Some(ElfTarget {
                class,
                data: 1,
                machine,
            });
            for kind in [ET_EXEC, ET_DYN] {
                assert_eq!(
                    linkage_for(&image_with(class, machine, kind, &[PT_LOAD]), native),
                    Linkage::Static,
                    "class {class}: type {kind}"
                );
            }
            for (what, kind) in [
                ("ET_NONE", 0_u16),
                ("ET_REL", 1),
                ("ET_CORE", 4),
                ("the first after ET_CORE", 5),
                ("ET_LOOS", 0xfe00),
                ("ET_HIPROC", 0xffff),
            ] {
                for segments in [&[PT_LOAD, PT_GNU_STACK][..], &[INTERP, PT_LOAD]] {
                    assert_eq!(
                        linkage_for(&image_with(class, machine, kind, segments), native),
                        Linkage::Unstartable,
                        "class {class}: {what} with the headers {segments:?}"
                    );
                }
            }
        }
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
