# b10x-substrate-exec-gate

`substrate-exec-gate` is the helper the `linux` confinement adapter of `b10x-substrate-host` runs
inside a sandbox, as the last program before the run's own, when the run's profile carries an
executable allow-list (`ConfinementProfile.exec_subpaths`). It applies one Landlock ruleset to
itself and executes the run's program, so every process of the run inherits the ruleset:
`execve` succeeds only for the program, the host's ELF loader and files that are, or lie beneath,
a listed path. Any other execution fails inside the run with `EACCES`.

```text
substrate-exec-gate --version
substrate-exec-gate <option>... -- <program> [<argument>...]

<option>  = --program  <class> <root> <rel>
          | --loader   <path>
          | --allow    <class> <root> <rel>
          | --writable <root>
          | --env      <name> <value>
<class>   = follow | beneath
```

- `--version` prints the protocol line `substrate-exec-gate/1`, which the driver checks when it
  opens.
- A `beneath` path is opened beneath its `<root>` with no symbolic link followed. A `follow` path
  is opened following links, and refused when what it resolves to lies in a `--writable` root, a
  mount of the view the run can write, or in a mount that is mounted beneath one. So is one
  that resolves through a symbolic link lying there, or passes through a directory there on
  its way elsewhere: the helper resolves a `follow` path one name at a time, places every
  link it reads and every directory it passes through, and steps back for `..` to the
  directory it came through, without asking the kernel. Where a path, a link or a directory
  lies is read from the view's mount table (`/proc/self/mountinfo`), never from the text of
  two paths, and a place, or where a mount beneath a `--writable` root is attached, that
  cannot be established refuses as well: nothing on a file-system type other than `ext4`,
  `overlay` and `tmpfs` has one. A path of either class is placed where it ends.
- `--env` carries one variable of the program's environment. **The program starts with the
  `--env` operands as its whole environment, and nothing of the helper's own.** An empty name, a
  name that holds `=` and a name given twice are refused.
- **Started with a variable of its own other than `PWD`, the helper executes nothing** and names
  the variable. The driver gives a run's variables as operands, so that none reaches a process
  that runs before the ruleset: a loader acts on `LD_PRELOAD` and its like before a program's
  first instruction, and a helper that found such a variable in its own environment was started
  by a caller that let it reach that far. `PWD` is the one variable bubblewrap sets itself.
- Every failure before the ruleset is applied exits 125 with one line on standard error that
  begins `substrate-exec-gate:`, and executes nothing. An option the grammar does not name is such
  a failure.
- When the program cannot be executed the status is 126, or 127 for a missing file.

## Using it

Cargo does not build a dependency's binary, so an embedder of `b10x-substrate-host` builds this
package from the revision it pins the host crate at, and names the file in `HostConfig.exec_gate`
as an absolute path:

```console
cargo build --release -p b10x-substrate-exec-gate --target x86_64-unknown-linux-musl
```

**The helper must be statically linked: build it for a musl target, as above.** The driver
tests the file's bytes when it opens and refuses `config.exec-gate-invalid` unless they are an
ELF image of the driver's own class, byte order and machine, with a header the kernel's loader
takes, whose program header table it could read whole and which holds no `PT_INTERP`. Cargo's
default build has one, and a script is no ELF image. The kernel's loader takes the type `ET_EXEC`
or `ET_DYN`, the program header size of the class, and at least one program header in at most
one 4 KiB page; for any other image the kernel answers `ENOEXEC`, and bubblewrap would then run
a shell of the view with the file as its script. A loader would run before the helper's first
instruction and read
files a run's profile can mount into its view, `/etc/ld.so.preload` among them, so an object
named there would run before the list is applied. A static-pie image has no `PT_INTERP` and is
taken. For the same reason a run with a list is given no controlling terminal
(`session.pty-unserved`). The file must lie
outside the driver's workspace root and every declared writable root, and have one link:
**install a copy, not cargo's own output**, which cargo hard-links to a second name under
`deps/`. The driver reads it once, when it opens, and runs those bytes in every later start.

The lists are served on `ext4`, `overlay` and `tmpfs`. Where the file, the workspace root, a
declared writable root, `/usr`, `/bin`, `/lib`, `/lib64` or the temporary directory lies on
another file-system type, the driver opens without the helper: the adapter proves no
executable-list fact, `facts().withheld` names the path and the type, and a profile with a list
is refused `confinement.fact-unserved` with that reason.

## Writing a list

An entry names a directory, which admits every file beneath it, or one file, in the paths the run
sees. **List the directory that holds the programs, not one above it**: with `/usr` listed,
`/bin/sh` is denied on a host whose `/bin` is a link into `/usr`, because the rule does not reach
through the view's second mount of `/usr/bin`. With `/usr/bin` listed both names run.

**A listed directory admits the files beneath it, not what a symbolic link in it points to.**
The kernel decides on the file a path resolves to. On a distribution that keeps a tool's name in
`/usr/bin` as a link to a file elsewhere, the tool is refused with `/usr/bin` listed: on Ubuntu
26.04, `/usr/bin/ln` is a link to `/usr/lib/cargo/bin/coreutils/ln`. List the tool by its own
path, which puts the rule on the file the link resolves to, or list the directory the links
point into as well.

**A listed path must not pass through a directory the run can write**, even to leave it again:
a link to `/workspace/../usr/bin` is refused, and `/usr/bin` listed in its place runs.

**One limit of the comparison of mounts.** Where one file system shows under two device
numbers (a network file system mounted twice with different options, two FUSE mounts of one
directory), the comparison cannot see that a read-only root and a mount the run can write hold
the same files. The only guard there is that nothing on a file system other than `ext4`,
`overlay` and `tmpfs` is followed: a listed path that ends on one, reads a symbolic link on
one or passes through a directory on one is refused.

## What the list does not stop

The ruleset restricts which files `execve` accepts. It does not restrict what code an allowed
program runs: an allowed interpreter runs any script it can read, the loader started directly
runs any program file it can read, and a memfd copy of a program runs. The run's variables are
not filtered: an object that `LD_PRELOAD` names is loaded into every allowed dynamic program and
runs its code there, and an `execve` it makes of an unlisted path still fails. The lists keep an honest
tool chain to the tools a step declares. Containment of hostile code stays with the namespaces,
the seccomp program and the read-only view.

The package depends on `libc` and nothing else, and needs Landlock ABI 2 (Linux 5.19).

This package is under development. See the
[public Substrate documentation](https://beyond10x.github.io/substrate/docs/guides/confinement-profiles)
for its current status.
