# Security Policy

## Reporting a vulnerability

Please do **not** open a public issue for a security vulnerability. Instead,
report it privately:

- **Preferred:** use GitHub's private vulnerability reporting on the
  [Security tab](https://github.com/lnorton89/rustaman/security/advisories/new).
- **By email:** [lnorton89@gmail.com](mailto:lnorton89@gmail.com). If you
  encrypt, ask for a key first; otherwise plain text is fine.

Include what you found, how to reproduce it, and which version or commit you
tested against. Reports are acknowledged within a few days, and fixes land in
the next release.

## Supported versions

Only the latest release is supported. Fixes ship in a new release; there is
no backport window for older versions.

## Scope

Rustaman inspects and controls processes on the machine it runs on. Ending,
suspending, or reprioritising a process you asked it to is the product, not a
vulnerability, and neither is the fact that running it as administrator lets
it act on other users' processes — that is what administrator means.

What *is* in scope:

- **Acting on the wrong process.** Every action is addressed by
  `(pid, creation_time)` and re-verified immediately before it is carried
  out, precisely so a PID reused between the click and the call cannot
  redirect a kill. A way around that check is a real finding.
- **Misuse of an `unsafe` block.** The whole Win32 surface is hand-wrapped
  in `src/win/`; a handle leak, a missing bounds check on a variable-length
  system structure, or a struct layout that disagrees with what the kernel
  writes all belong here.
- **Privilege handling.** `SeDebugPrivilege` is enabled once at startup when
  available and never re-enabled per action; anything that widens what an
  unelevated instance can do is in scope.
- **Untrusted input reaching a privileged path.** Process names, command
  lines, service display names, and startup-entry values are all attacker-
  influenced strings that this app reads and displays.

Everything else — bug reports, feature requests, and questions — belongs on
the [issue tracker](https://github.com/lnorton89/rustaman/issues); see
[`CONTRIBUTING.md`](CONTRIBUTING.md).
