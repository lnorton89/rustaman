# Which Windows API answers which question

Every system fact this app shows comes from one of the interfaces below.
This file records *which* call, what it costs, why it is that one rather
than the obvious alternative, and — the last section — what none of them
can tell you.

The floor is **Windows 10 1809 (build 17763)**, 64-bit. Anything
introduced after that is probed at runtime rather than assumed, so a
missing entry point costs a column rather than the app.

---

## Processes

### The process table — `NtQuerySystemInformation(SystemProcessInformation)`

`src/win/nt/`. One call. It returns a variable-length chain of records,
one per process, each carrying: image name, PID, parent PID, creation
time, base priority, thread count, handle count, session id, every
working-set and commit counter, kernel and user CPU time, and cumulative
read/write/other I/O — plus an array of per-thread records that follows
each process record.

The documented alternative is `EnumProcesses`, then per process
`OpenProcess` + `QueryFullProcessImageNameW` + `GetProcessTimes` +
`GetProcessMemoryInfo` + `GetProcessIoCounters` + `CloseHandle`. On a
desktop with 350 processes that is around 2,500 syscalls per sample, at
one sample a second, forever. It also fails on protected processes: they
appear with blank columns, which reads as a broken app rather than as an
access check.

Two costs come with the undocumented route, and both are handled in
`nt/types.rs`:

1. **`windows-sys` ships a redacted struct.** The public SDK's
   `SYSTEM_PROCESS_INFORMATION` omits most fields. The real layout is
   declared by hand and pinned with `const _: () = assert!(...)` on
   `size_of` and `offset_of` for the fields that are read. A layout that
   disagrees with what the kernel writes does not crash — it produces
   plausible wrong numbers — so the assertions are what make it a build
   failure instead.
2. **The chain is offset-linked, not an array.** Each record names the
   byte offset of the next; a zero offset ends it. The walk is bounds
   checked against the buffer length on every step, because the buffer
   size is whatever the previous call said it needed and the machine can
   grow processes in between.

The buffer is sized by asking, then retried on
`STATUS_INFO_LENGTH_MISMATCH` with headroom, a bounded number of times.

### CPU percentage

`src/win/nt/cpu.rs`. From `SystemProcessorPerformanceInformation`, one
record per logical processor: `IdleTime`, `KernelTime`, `UserTime`,
`DpcTime`, `InterruptTime`.

The trap: **`KernelTime` already includes `IdleTime`.** So total
utilisation is `1 - idle_delta / total_delta`, not
`(kernel_delta + user_delta) / total_delta` — the second reads 100% on a
machine doing nothing. Per-process CPU is a different calculation
entirely: the process's own kernel+user delta over the wall-clock delta,
divided by the logical processor count.

Both are deltas between two samples with the same `ProcessKey`. See
[`ARCHITECTURE.md`](ARCHITECTURE.md) on why that matters.

A second trap, in the query itself rather than the arithmetic: **this
class rejects a buffer that is *larger* than it needs, not just a
smaller one.** Every other class here follows the documented contract —
`STATUS_INFO_LENGTH_MISMATCH` means "too small", and a bigger buffer than
strictly necessary still succeeds — which is what `nt::query`'s
grow-and-retry protocol assumes. `SystemProcessorPerformanceInformation`
does not: an oversized buffer mismatches identically to an undersized
one, every attempt, so `nt::query`'s "ask for more each time" schedule
never converges — confirmed against a real machine, where the call
reported needing exactly 768 bytes (16 cores × 48) while rejecting a
512 KB buffer and every doubling of it in turn. This is not documented
anywhere; it was found by instrumenting the call and reading the status
and reported size back. `src/win/nt/cpu.rs::read` goes through
`nt::query_exact` instead, which resizes to exactly what the kernel
reports rather than growing past it — see that function's docs for why
that is safe (the logical processor count does not change mid-process).
If a future `NtQuerySystemInformation` class added here starts behaving
strangely under `nt::query`, check this first before assuming the buffer
math is wrong.

### Identity — owner, session, elevation, integrity

`src/win/identity.rs`. `OpenProcessToken` → `GetTokenInformation` for
`TokenUser` (then `LookupAccountSidW`), `TokenElevation`, and
`TokenIntegrityLevel`; session id comes from the process record.

This is the part that needs privilege. Unelevated, `OpenProcess` with
`PROCESS_QUERY_LIMITED_INFORMATION` succeeds for your own processes and
fails for other accounts' — so the owner, path and architecture columns
are blank for them. `src/win/privilege.rs` enables `SeDebugPrivilege`
once at startup when the token has it, which is what fills those in for
an administrator. Nothing re-enables it per action.

Results are cached per `ProcessKey`: a process's owner does not change,
so this is one token open per process per lifetime rather than per
sample.

### Suspend and resume — `NtSuspendProcess` / `NtResumeProcess`

`src/win/control.rs`. Both are ntdll exports with no import library and
no documentation, declared with `#[link(name = "ntdll")] unsafe extern
"system"`. There is no documented Win32 equivalent: the alternative is
enumerating threads with ToolHelp and calling `SuspendThread` on each,
which races a process that is creating threads and leaves it in a
half-suspended state when it loses.

### Ending a process

`TerminateProcess` on a handle opened with `PROCESS_TERMINATE`. "End
process tree" walks the forest children-first, so a parent cannot respawn
a child that has not been ended yet.

Every action re-reads the target's creation time from the opened handle
and compares it against the `ProcessKey` it was asked for, before doing
anything. Between the click and the call the process can exit and the PID
can be reused; without the check the kill lands on whatever took the PID.

---

## Performance

### Memory

`src/win/memory.rs`. `GlobalMemoryStatusEx` for the totals,
`GetPerformanceInfo` for the kernel paged and non-paged pools, commit
charge and limit, and the system-wide handle and thread counts.

`GetPerformanceInfo` reports pages, not bytes; the page size is in the
same structure and is the one to multiply by. Assuming 4 KiB is right on
every machine anyone will run this on and wrong in principle, which is
the kind of wrong that survives for years.

### Disk — active time, not just throughput

`src/win/disk.rs`. `DeviceIoControl(IOCTL_DISK_PERFORMANCE)` against
`\\.\PhysicalDriveN`, which returns `BytesRead`, `BytesWritten`,
`ReadTime`, `WriteTime`, `IdleTime` and `QueryTime` as 100 ns counters.

**Active time** — the number Task Manager shows, and the one that
actually tells you the disk is the bottleneck — is
`1 - idle_delta / query_time_delta`. Throughput alone does not: a disk at
100% active time doing 2 MB/s of small random reads is saturated, and a
throughput graph makes it look idle.

Opening `\\.\PhysicalDriveN` needs no elevation for this IOCTL, but the
counters must be enabled — they are on by default on every modern
Windows, and a drive that returns `ERROR_NOT_SUPPORTED` is skipped rather
than shown as zero.

### Network

`src/win/net.rs`. `GetIfTable2` for per-adapter octets in and out; rates
are deltas as everywhere else. Loopback and tunnel adapters are excluded
by `IfType`, not by name.

Per-process connection *counts* come from `GetExtendedTcpTable` and
`GetExtendedUdpTable` with `TCP_TABLE_OWNER_PID_ALL`, which is the only
Win32 interface that attributes a network object to a PID at all.

**IPv4 only, deliberately.** The IPv6 tables are a second pair of calls
with a second pair of row layouts, and a process holding an IPv6 endpoint
almost always holds an IPv4 one too — so supporting them roughly doubles
the work in this module to change a count from "some" to "slightly more
some".

### GPU

`src/win/gpu.rs`. There is no Win32 call for GPU utilisation. The
counters are PDH: `\GPU Engine(*)\Utilization Percentage` and
`\GPU Process Memory(*)\Dedicated Usage`, with the PID embedded in the
instance name (`pid_1234_luid_..._engtype_3D`). Task Manager reads the
same ones.

PDH is a stateful API — `PdhOpenQuery`, `PdhAddCounterW`,
`PdhCollectQueryData` twice for a rate counter, then
`PdhGetFormattedCounterArrayW` — so the query is opened once and lives
for the process, wrapped in an owning `Drop` type like everything else.
A machine with no GPU counters (a VM, a very old driver) loses the GPU
section and nothing else.

Note the handle types: PDH query and counter handles are `*mut c_void` in
`windows-sys` 0.61, not `isize`.

---

## Services and startup

### Services — the SCM

`src/win/services.rs`. `OpenSCManagerW(SC_MANAGER_ENUMERATE_SERVICE)` →
`EnumServicesStatusExW(SC_ENUM_PROCESS_INFO)`, which returns name,
display name, current state, and **the hosting PID** in one pass. That
last field is what makes "go to process" possible: it is how you get from
a `svchost.exe` eating a core to which of its fifteen services is doing
it.

Enumeration is unprivileged. Start and stop are not — `OpenServiceW` with
`SERVICE_START` / `SERVICE_STOP` fails without elevation, and the UI
reports the access denial rather than silently doing nothing.

This is re-read on demand (`F5`), not every tick: the enumeration is
expensive and the answer changes on a human timescale.

### Startup — six locations

`src/win/startup.rs`. A program can register to run at logon in six
places, and a tool that reads fewer is quietly lying:

| Location | |
|---|---|
| `HKCU\...\CurrentVersion\Run` | per-user |
| `HKLM\...\CurrentVersion\Run` | machine-wide, 64-bit view |
| `HKLM\...\Wow6432Node\...\Run` | machine-wide, **32-bit view** — the one everyone forgets |
| `HKCU\...\CurrentVersion\RunOnce` | per-user, fires once |
| `HKLM\...\CurrentVersion\RunOnce` | machine-wide, fires once |
| The Startup folders | per-user and All Users |

Whether an entry is *enabled* is not in the `Run` key. It is a binary
blob under `...\Explorer\StartupApproved\Run`, whose first byte is the
flag — even values enabled, odd disabled. An entry with no blob has never
been touched, and is enabled.

**This module is read-only, deliberately.** Toggling an entry means
writing that blob, and removing one means deleting another program's
registry value. Both are destructive changes to state this app does not
own, and neither is something to add without a confirmation flow designed
around it. The UI offers "open file location" and "copy path" instead,
which is enough to act on an entry deliberately.

---

## The window itself

- **Dark title bar** — `DwmSetWindowAttribute` with
  `DWMWA_USE_IMMERSIVE_DARK_MODE`. The attribute number changed between
  1809 and 1903 (19 → 20), so `src/win/dwm.rs` tries the newer one and
  falls back. Both fail harmlessly on a build that has neither.
- **Dragging the custom title bar** — `ViewportCommand::StartDrag`, which
  hands the drag to the window manager. A hand-rolled move loop that
  repositions the window on each mouse event works, and silently loses
  Aero Snap: no half-screen snap, no snap layouts, no maximise on
  drag-to-top.
- **"Open file location"** — `ShellExecuteW` with the `explore` verb and
  `/select,"<path>"` as the parameter, rather than building a command
  line for `explorer.exe`. A path containing a space or a quote becomes
  part of the command in the second form, and a program's install
  directory is exactly where those live.
- **The window icon is drawn, not loaded.** `gui::icons::app_icon`
  rasterises the mark from `src/brand.rs` into a 64×64 buffer at startup.
  The `.ico` compiled into the `.exe` by `build.rs` comes from the same
  definition, via `cargo run --example brand_assets`, so the taskbar icon
  and the title-bar icon cannot drift apart.
- **DPI** — declared in `assets/rustaman.manifest` as PerMonitorV2, which
  is what stops Windows bitmap-scaling the window into a blur on a 150%
  display.

---

## What no Windows API will tell you

These are the questions people ask of a task manager that this one
answers with "no" rather than with a guess. They are here so the omission
is a decision on the record rather than something that looks like an
oversight.

### Per-process network throughput

There is no Win32 call for it. Task Manager gets it from an **ETW kernel
session** — a continuous, system-wide trace with the privileges and the
overhead that implies, plus the fact that only one kernel logger session
of some classes can exist at a time, so starting one can fail because
something else got there first.

Rustaman shows open TCP and UDP endpoint counts instead: cheap, exact,
unprivileged, and honest about being a different measurement.

### Which window belongs to which process, reliably

`EnumWindows` + `GetWindowThreadProcessId` gets you most of the way, and
`src/win/windows.rs` uses it to tell an app from a background task. It is
a heuristic, not a fact: UWP apps run their UI in `ApplicationFrameHost`
and their code somewhere else, so the window and the process that owns
the work are genuinely two processes.

### Why a process is using the CPU

Not a question any of these interfaces answer. It needs a stack walk of
another process, which needs a symbol path, `dbghelp`, and privileges —
and is a profiler, not a task manager.

### Whether a driver is the problem

`InterruptTime` and `DpcTime` are in the per-processor record and are
read, but attributing them to a *driver* needs an ETW session again.
High DPC time shows up in the CPU view as a band; naming the culprit does
not.

### Exact per-process disk throughput at the device level

`ReadTransferCount` and `WriteTransferCount` in the process record count
bytes through the I/O manager, which includes cache hits that never
touched a disk. That is the right number for "what is this process
asking for" and the wrong one for "what is hitting the platter". Both
views are shown, from different sources, rather than one number pretending
to be both.
