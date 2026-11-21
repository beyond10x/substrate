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
<class>   = follow | beneath
```

- `--version` prints the protocol line `substrate-exec-gate/1`, which the driver checks when it
  opens.
- A `beneath` path is opened beneath its `<root>` with no symbolic link followed. A `follow` path
  is opened following links, and refused when what it resolves to lies in a `--writable` root, a
  mount of the view the run can write. Where a path lies is read from the view's mount table
  (`/proc/self/mountinfo`), never from the text of two paths, and a place that cannot be
  established refuses as well.
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

A static build starts in every sandbox. A default build needs the build host's loader and a
compatible `libc` beneath `/usr`, `/lib` or `/lib64` of every host it runs on. The file must lie
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

## What the list does not stop

The ruleset restricts which files `execve` accepts. It does not restrict what code an allowed
program runs: an allowed interpreter runs any script it can read, the loader started directly
runs any program file it can read, and a memfd copy of a program runs. The lists keep an honest
tool chain to the tools a step declares. Containment of hostile code stays with the namespaces,
the seccomp program and the read-only view.

The package depends on `libc` and nothing else, and needs Landlock ABI 2 (Linux 5.19).

This package is under development. See the
[public Substrate documentation](https://beyond10x.github.io/substrate/docs/guides/confinement-profiles)
for its current status.
