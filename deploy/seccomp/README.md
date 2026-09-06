# Container execution seccomp profile

`host-exec-v1-amd64.json` is an explicit amd64 OCI seccomp profile for design 25. Other
architectures are unserved. Its local schema is checked by the repository gate; the `$schema`
annotation does not alter OCI runtime behavior.

The ordinary baseline derives from the Apache-2.0
[Moby default profile at 61eaf326](https://github.com/moby/profiles/blob/61eaf32614c7c71b60bd8927d3e6a4ffc8ff1f31/seccomp/default.json).
Moby Project contributors retain their copyright; the repository's Apache-2.0 LICENSE applies.
Architecture conditions were resolved for amd64 and the supported modern cgroup-v2 kernel.
Every capability-conditional allow rule was omitted. The original argument restrictions and
clone3 ENOSYS fallback remain. The final rule adds only the namespace, mount, root-switch,
hostname and project-quota operations needed by the explicit bootstrap and bubblewrap.

The default is EPERM. No bpf, performance monitoring, module loading, kernel-log access, host clock
mutation or file-handle-open permission is added. A separate inherited Substrate seccomp filter
restricts each confined child further. No profile is downloaded or widened during daemon startup.

The versioned profile is immutable after its first release. A policy change requires a new name,
fresh real-container and node evidence, and an explicit deployment selection.
