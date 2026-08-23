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

### Efficiency mode — `Get`/`SetProcessInformation(ProcessPowerThrottling)`

`src/win/control.rs`. Windows 11's EcoQoS, and the one genuinely new
thing 11 added to the process table.

Turning it on is two calls, not one. `SetProcessInformation` with
`PROCESS_POWER_THROTTLING_EXECUTION_SPEED` in both the control and state
masks is the QoS request — on a hybrid part it keeps the process on the
E-cores, on any part it lets the power manager clock it down — and
`SetPriorityClass(IDLE_PRIORITY_CLASS)` is what stops it competing for
the cores it does get. Task Manager sets both; either alone gets a
fraction of the effect.

Turning it **off** clears the control mask rather than setting it with a
clear state mask. A clear control mask means "the system decides", which
is where a process starts life; an explicit opt-out would pin the process
at full speed even where Windows would have throttled it anyway.

The read side is the asymmetric part, and it is worth knowing before
testing on a Windows 10 machine:

| | Windows 10 | Windows 11 |
|---|---|---|
| `SetProcessInformation` | works (1709+) | works |
| `GetProcessInformation` | `ERROR_INVALID_PARAMETER` | works |

So on 10 every process reads as `Efficiency::Unknown`, no marks are
drawn, and the menu item is not offered — gated on the build number, not
on the call failing, because a menu item that reports success and changes
nothing observable is worse than one that is absent.

**The read costs a handle per process**, which is why it is not a column
computed every sample. `crate::engine::sampler::EfficiencySweep` reads a
fixed slice of the process list per pass and caches the answers, so the
per-sample cost is a constant rather than a multiple of the process
count — the same objection that rules out `EnumProcesses` above. See
`docs/PERFORMANCE.md`.

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

**Per-process memory needs no call at all.** The Memory view shows a
private working set, both peaks, both pool quotas and both fault counts
for every process, and every one of those is a field of the
`SYSTEM_PROCESS_INFORMATION` that `NtQuerySystemInformation` already
returns for the process table. `GetProcessMemoryInfo` would be one
`OpenProcess` and one call *per process, per sample* for the same
numbers. Before reaching for a per-process memory API, check
`src/win/nt/types.rs` — the struct is declared by hand there and most of
it was going unread.

**`WorkingSetPrivateSize` is the field that matters**, and it is the one
the SDK's redacted struct does not have. A plain working set counts
every DLL and mapped file against every process holding it open, so
summing working sets across a machine gives a total several times its
RAM. The private figure is what adds up, and it is why the treemap is
sized by it.

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

**A drive letter can block the thread that asks about it, and can put a
dialog in front of it.** `GetDiskFreeSpaceExW` talks to the device: on a
network drive whose server has gone it blocks for the redirector's whole
timeout, and on a drive with no media Windows raises a *hard error* —
the "There is no disk in the drive" box — which is modal and holds the
calling thread until somebody dismisses it. From the sampler thread that
is the app hanging, with the dialog nowhere near its window.

Two guards, both required: `win::system::suppress_device_error_dialogs`
sets `SEM_FAILCRITICALERRORS` on the sampler thread so the dialog becomes
an error return, and `volumes()` asks `GetDriveTypeW` — which reads the
mount table and cannot block — before it asks the device anything.
Anything added here that touches a device by letter needs the same
treatment.

### Network

`src/win/net.rs`. `GetIfTable2` for per-adapter octets in and out; rates
are deltas as everywhere else.

**A filter module gets its own row, and it is not an adapter.**
`GetIfTable2` returns a row per NDIS filter module bound to an interface
as well as one for the interface. A machine with Npcap, the QoS packet
scheduler and the two WFP lightweight filters installed reports five rows
for one network card — `Ethernet 2`, then `Ethernet 2-Npcap Packet Driver
(NPCAP)-0000`, `Ethernet 2-QoS Packet Scheduler-0000`, and so on — each
carrying **the same octet counters**, because they are the same bytes
seen at different layers of one stack. Listing them duplicates the card;
summing them reports five times the machine's real throughput. The
`FilterInterface` bit of `InterfaceAndOperStatusFlags` is what names
them.

That field is a C bitfield of eight `BOOLEAN : 1` members and
`windows-sys` exposes it as one opaque `u8`, so the bits are unpacked by
hand in `win::net::flags`. MSVC packs from the least significant bit up
in declaration order: `HardwareInterface` is bit 0, `FilterInterface`
bit 1. Getting that order wrong does not fail — it files every physical
adapter under "virtual" and drops it out of the throughput total.

**The list is not filtered by state.** Loopback is excluded by `IfType`;
everything else is reported whatever condition it is in, with the reason
in `AdapterState`. Filtering on `OperStatus` made rows appear and vanish
as the machine changed, which answers "what is connected right now" when
the question is "what does this machine have".

**Identity is the LUID.** `InterfaceIndex` is documented as changing when
an adapter is disabled and re-enabled, and the alias is a label a user
can edit — so both were wrong to key the rate delta on. `NET_LUID_LH` is
a union; the `Value` arm is the opaque 64-bit identifier.

**Summing the adapter list is not the machine's throughput.** Windows
counts a byte once per interface it crosses, and on a box with Hyper-V,
WSL or a VPN a packet crosses the virtual switch, the tunnel adapter and
the physical card. `model::SystemSample::network_rate` sums hardware
interfaces only, falling back to everything on a guest VM that has none.

Per-process connection *counts* come from `GetExtendedTcpTable` and
`GetExtendedUdpTable` with `TCP_TABLE_OWNER_PID_ALL`, which is the only
Win32 interface that attributes a network object to a PID at all.

Both `AF_INET` and `AF_INET6` owner tables are queried for TCP and UDP.
The mutable tables use bounded resize/retry loops so a connection opened
between the size query and fetch does not blank the whole sample.

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

### Startup locations

`src/win/startup.rs` covers the canonical Run/RunOnce registry mechanisms
and both Startup folders:

| Location | |
|---|---|
| `HKCU\...\CurrentVersion\Run` | per-user |
| `HKLM\...\CurrentVersion\Run` | machine-wide, 64-bit view |
| `HKLM\...\Run` through `KEY_WOW64_32KEY` | machine-wide, **32-bit view** |
| `HKCU\...\CurrentVersion\RunOnce` | per-user, fires once |
| `HKLM\...\CurrentVersion\RunOnce` | machine-wide, fires once |
| `HKLM\...\RunOnce` through `KEY_WOW64_32KEY` | machine-wide, 32-bit view, fires once |
| The Startup folders | per-user and All Users |

Whether an entry is *enabled* is not in the `Run` key. It is a binary
blob under `...\Explorer\StartupApproved\Run` / `StartupFolder`, whose first byte is the
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

- **Rounded corners** — `DwmSetWindowAttribute` with
  `DWMWA_WINDOW_CORNER_PREFERENCE` (33), Windows 11 only. Windows 11
  rounds a window's corners for you, but only a window that has a frame;
  the app's own title bar means an *undecorated* window, and those stay
  square unless the preference is set explicitly. Without it this is the
  one square window on a rounded desktop.
- **Border colour** — `DWMWA_BORDER_COLOR` (34), Windows 11 only. An
  undecorated window has no frame, so on a dark desktop it ends where its
  background stops; the one-pixel line DWM draws is what separates it
  from what is behind it, and left alone that line is the user's accent
  colour — the only part of the window the theme cannot otherwise reach.
  The value is a `COLORREF`, which is `0x00BBGGRR` and not the RGB order
  every other colour in this codebase is written in.
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

Rustaman shows open IPv4 and IPv6 TCP/UDP endpoint counts instead: cheap,
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
