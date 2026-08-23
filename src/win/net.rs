// ============================================================================
// Module:       win::net
// Description:  Per-adapter throughput counters and the per-process TCP/UDP
//               endpoint counts shown in the Network column.
//
// Dependencies: windows-sys (GetIfTable2, GetExtendedTcpTable/UdpTable);
//               super::strings
// ============================================================================

//! Network activity.
//!
//! Two separate things, from two separate APIs, because Windows does not
//! offer them together:
//!
//! - **Per-adapter throughput**, from `GetIfTable2`. Cumulative octet
//!   counters per interface, which a delta turns into a rate.
//! - **Per-process endpoints**, from `GetExtendedTcpTable` and
//!   `GetExtendedUdpTable` with the owning-PID variants. A *count* of
//!   open connections, not a byte rate.
//!
//! ## Why the process column is connections and not bytes
//!
//! There is no Win32 call that reports per-process network throughput.
//! None. Task Manager gets that column from an ETW kernel session
//! (`Microsoft-Windows-Kernel-Network`), which means starting a trace
//! session, consuming a real-time event stream, and attributing every
//! packet event to a PID — a background thread doing continuous work,
//! plus the privileges an ETW session needs.
//!
//! That is a great deal of machinery and ongoing cost for one column, and
//! it is the sort of thing that turns a task manager into the busiest
//! process on the machine. So this app shows **open endpoints** per
//! process instead: cheap, exact, needs no privilege, and answers the
//! question that is actually being asked most of the time — "what is this
//! process talking to, and how much". `docs/WINDOWS_APIS.md` records the
//! decision so it does not get re-litigated as an oversight.
//!
//! ## A filter module is not an adapter, and this is the bug it caused
//!
//! `GetIfTable2` returns a row for every NDIS **filter module** bound to
//! an adapter as well as for the adapter itself. A machine with Npcap,
//! the QoS packet scheduler and the two WFP lightweight filters
//! installed — which is to say a stock Windows machine with Wireshark on
//! it — reports five rows for one network card:
//!
//! ```text
//! Ethernet 2
//! Ethernet 2-Npcap Packet Driver (NPCAP)-0000
//! Ethernet 2-QoS Packet Scheduler-0000
//! Ethernet 2-WFP 802.3 MAC Layer LightWeight Filter-0000
//! Ethernet 2-WFP Native MAC Layer LightWeight Filter-0000
//! ```
//!
//! Every one of them carries the *same* octet counters as the adapter it
//! is bound to, because they are the same bytes seen at a different
//! layer of the same stack. Listing them is a list of near-identical
//! cards showing near-identical numbers; **summing** them, which the
//! Network panel's total did, reports five times the traffic the machine
//! actually moved.
//!
//! The `FilterInterface` flag in `InterfaceAndOperStatusFlags` is what
//! names them, and [`adapters`] drops them on it. Loopback goes too —
//! traffic that never left the machine is not throughput.
//!
//! ## Everything else stays, in whatever state it is in
//!
//! This function used to also require `OperStatus == Up` and a non-zero
//! link speed. That is a filter on a *live* property, and a list built
//! from one is a list whose rows appear and vanish as the machine
//! changes: unplug the cable and the row goes, so the panel answers
//! "what is connected right now" when the question being asked is "what
//! does this machine have". The state comes back as a field instead —
//! see [`crate::model::AdapterState`] — and the row stays put.

use super::strings;
use crate::model::{AdapterKind, AdapterState};
use std::collections::HashMap;
use windows_sys::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetExtendedTcpTable, GetExtendedUdpTable, GetIfTable2, MIB_IF_ROW2,
    MIB_IF_TABLE2, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID,
    MIB_TCPTABLE_OWNER_PID, MIB_UDP6ROW_OWNER_PID, MIB_UDP6TABLE_OWNER_PID, MIB_UDPROW_OWNER_PID,
    MIB_UDPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL, UDP_TABLE_OWNER_PID,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

/// One adapter's cumulative counters and the facts that identify it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdapterCounters {
    /// The interface LUID — this adapter's identity across samples.
    pub luid: u64,
    /// The adapter's friendly name, e.g. "Ethernet" or "Wi-Fi".
    pub name: String,
    /// The hardware description, e.g. "Realtek Gaming 2.5GbE Family
    /// Controller". What tells "Ethernet" and "Ethernet 2" apart.
    pub description: String,
    /// What kind of adapter this is.
    pub kind: AdapterKind,
    /// Whether it is up, and if not, why.
    pub state: AdapterState,
    /// Whether there is hardware behind it, from the `HardwareInterface`
    /// flag.
    pub hardware: bool,
    /// Cumulative octets received.
    pub received: u64,
    /// Cumulative octets sent.
    pub sent: u64,
    /// Nominal link speed in bits per second, or zero where the adapter
    /// reports none or reports the "unlimited" sentinel.
    pub link_speed: u64,
}

/// Reads every adapter the machine has.
///
/// See the module docs on the two things filtered out — filter modules
/// and loopback — and on why nothing else is.
#[must_use]
pub fn adapters() -> Vec<AdapterCounters> {
    let Some(table) = IfTable::read() else {
        return Vec::new();
    };
    table
        .rows()
        .iter()
        .filter(|row| is_an_adapter(row))
        .map(|row| AdapterCounters {
            luid: luid_of(row),
            name: strings::from_wide_nul(&row.Alias),
            description: strings::from_wide_nul(&row.Description),
            kind: kind_of(row),
            state: state_of(row),
            hardware: flags(row).hardware,
            received: row.InOctets,
            sent: row.OutOctets,
            link_speed: link_speed(row),
        })
        .collect()
}

/// `IF_TYPE_SOFTWARE_LOOPBACK`, from the IANA interface-type list.
const IF_TYPE_LOOPBACK: u32 = 24;
/// `IF_TYPE_TUNNEL`.
const IF_TYPE_TUNNEL: u32 = 131;
/// `IF_TYPE_IEEE80211` — an 802.11 wireless interface.
const IF_TYPE_WIRELESS: u32 = 71;
/// `IF_TYPE_IEEE80216_WMAN` — WiMAX mobile broadband.
const IF_TYPE_WIMAX: u32 = 237;
/// `IF_TYPE_WWANPP` — GSM mobile broadband.
const IF_TYPE_WWANPP: u32 = 243;
/// `IF_TYPE_WWANPP2` — CDMA mobile broadband.
const IF_TYPE_WWANPP2: u32 = 244;
/// `IF_TYPE_ETHERNET_CSMACD`.
const IF_TYPE_ETHERNET: u32 = 6;

/// An interface's LUID as a plain integer.
///
/// `NET_LUID_LH` is a union of a `u64` and a bitfield view of the same
/// eight bytes — the interface index and the IANA type unpacked out of
/// it. Reading the `u64` arm is the documented way to get an opaque
/// identifier, which is the only use it has here.
fn luid_of(row: &MIB_IF_ROW2) -> u64 {
    // SAFETY: `NET_LUID_LH` is a union over eight bytes and `Value` is
    // its `u64` arm, so every bit pattern the kernel can write into the
    // field is a valid `u64`. The row was filled by `GetIfTable2`, which
    // initialises the whole struct, so the bytes are not uninitialised.
    unsafe { row.InterfaceLuid.Value }
}

/// Whether a row describes an adapter rather than a layer of one.
fn is_an_adapter(row: &MIB_IF_ROW2) -> bool {
    !flags(row).filter && row.Type != IF_TYPE_LOOPBACK
}

/// The `InterfaceAndOperStatusFlags` bitfield, unpacked.
///
/// Only the two bits anything here reads. The field is a C bitfield of
/// eight `BOOLEAN : 1` members, which MSVC packs from the least
/// significant bit up in declaration order — `HardwareInterface` first,
/// `FilterInterface` second. `windows-sys` exposes it as one opaque
/// `u8`, so the shifts are stated here rather than generated, and the
/// order is the one `MIB_IF_ROW2`'s own declaration gives.
struct Flags {
    /// `HardwareInterface`: there is a physical device behind this.
    hardware: bool,
    /// `FilterInterface`: this is an NDIS filter module bound to some
    /// other interface, not an interface in its own right.
    filter: bool,
}

/// Unpacks the flags a row carries.
fn flags(row: &MIB_IF_ROW2) -> Flags {
    /// `HardwareInterface`, the first member of the bitfield.
    const HARDWARE: u8 = 1 << 0;
    /// `FilterInterface`, the second.
    const FILTER: u8 = 1 << 1;

    let bits = row.InterfaceAndOperStatusFlags._bitfield;
    Flags {
        hardware: bits & HARDWARE != 0,
        filter: bits & FILTER != 0,
    }
}

/// Classifies an adapter for the label beside its name.
///
/// Reads the IANA type first and the NDIS physical medium second: the
/// type is what distinguishes a tunnel and mobile broadband, and the
/// medium is what distinguishes Wi-Fi and Bluetooth from the Ethernet
/// they both emulate to the rest of the stack. An adapter with no
/// hardware behind it is `Virtual` whatever it claims to be, which is
/// how a Hyper-V vSwitch — an `IF_TYPE_ETHERNET_CSMACD` with an Ethernet
/// medium, indistinguishable from a network card by those two fields
/// alone — ends up labelled honestly.
fn kind_of(row: &MIB_IF_ROW2) -> AdapterKind {
    /// `NdisPhysicalMediumWirelessLan`.
    const MEDIUM_WIRELESS_LAN: i32 = 1;
    /// `NdisPhysicalMediumWirelessWan`.
    const MEDIUM_WIRELESS_WAN: i32 = 8;
    /// `NdisPhysicalMediumNative802_11`.
    const MEDIUM_NATIVE_802_11: i32 = 9;
    /// `NdisPhysicalMediumBluetooth`.
    const MEDIUM_BLUETOOTH: i32 = 10;

    match row.Type {
        IF_TYPE_TUNNEL => return AdapterKind::Tunnel,
        IF_TYPE_WIRELESS => return AdapterKind::WiFi,
        IF_TYPE_WIMAX | IF_TYPE_WWANPP | IF_TYPE_WWANPP2 => return AdapterKind::Cellular,
        _ => {}
    }
    match row.PhysicalMediumType {
        MEDIUM_WIRELESS_LAN | MEDIUM_NATIVE_802_11 => return AdapterKind::WiFi,
        MEDIUM_BLUETOOTH => return AdapterKind::Bluetooth,
        MEDIUM_WIRELESS_WAN => return AdapterKind::Cellular,
        _ => {}
    }
    if !flags(row).hardware {
        return AdapterKind::Virtual;
    }
    if row.Type == IF_TYPE_ETHERNET {
        AdapterKind::Ethernet
    } else {
        AdapterKind::Other
    }
}

/// Reads an adapter's state, preferring the *reason* it is down.
///
/// `OperStatus` alone says "down" for a disabled adapter, an unplugged
/// cable and a missing device alike, and those are three different
/// things to be told. `AdminStatus` distinguishes the disabled one and
/// `MediaConnectState` the unplugged one, so both are consulted before
/// falling back.
fn state_of(row: &MIB_IF_ROW2) -> AdapterState {
    /// `IfOperStatusUp`.
    const OPER_UP: i32 = 1;
    /// `IfOperStatusDormant`.
    const OPER_DORMANT: i32 = 5;
    /// `IfOperStatusNotPresent`.
    const OPER_NOT_PRESENT: i32 = 6;
    /// `IfOperStatusLowerLayerDown`.
    const OPER_LOWER_LAYER_DOWN: i32 = 7;
    /// `NET_IF_ADMIN_STATUS_DOWN`.
    const ADMIN_DOWN: i32 = 2;
    /// `MediaConnectStateDisconnected`.
    const MEDIA_DISCONNECTED: i32 = 2;

    if row.OperStatus == OPER_UP {
        return AdapterState::Up;
    }
    if row.AdminStatus == ADMIN_DOWN {
        return AdapterState::Disabled;
    }
    match row.OperStatus {
        OPER_DORMANT => AdapterState::Dormant,
        OPER_LOWER_LAYER_DOWN => AdapterState::LowerLayerDown,
        OPER_NOT_PRESENT => AdapterState::NotPresent,
        _ if row.MediaConnectState == MEDIA_DISCONNECTED => AdapterState::Disconnected,
        _ => AdapterState::NotPresent,
    }
}

/// An adapter's link speed, with the sentinels flattened to zero.
///
/// `u64::MAX` is what a virtual adapter reports for "unlimited", and a
/// down adapter reports nothing at all. Both mean "there is no figure to
/// show here", and a caller that has to know about two of them is a
/// caller that will check for one.
fn link_speed(row: &MIB_IF_ROW2) -> u64 {
    if row.TransmitLinkSpeed == u64::MAX {
        0
    } else {
        row.TransmitLinkSpeed
    }
}

/// An owned `MIB_IF_TABLE2`, freed on drop.
///
/// `GetIfTable2` allocates the table itself and requires `FreeMibTable`,
/// which is exactly the pattern the owning-wrapper rule exists for: the
/// interesting path here is the one where the table is read and the
/// filter finds nothing, and an early return there would leak the whole
/// allocation on every sample.
struct IfTable(*mut MIB_IF_TABLE2);

impl IfTable {
    /// Reads the interface table.
    fn read() -> Option<Self> {
        let mut pointer: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        let status = get_if_table(&mut pointer);
        if status != 0 || pointer.is_null() {
            return None;
        }
        Some(Self(pointer))
    }

    /// The rows of the table.
    ///
    /// The table is a header followed by a variable-length array, so the
    /// slice is built from the header's stated count. The lifetime ties
    /// it to `self`, which is what stops a row outliving the allocation.
    fn rows(&self) -> &[MIB_IF_ROW2] {
        let table = self.table();
        self.rows_from_table(table)
    }

    /// Borrows the allocation header guarded by this owner.
    fn table(&self) -> &MIB_IF_TABLE2 {
        // SAFETY: `self.0` is a non-null table allocated by
        // `GetIfTable2` (checked in `read`) and alive for the borrow.
        unsafe { &*self.0 }
    }

    /// Borrows the variable-length row array in this owned allocation.
    fn rows_from_table<'a>(&'a self, table: &'a MIB_IF_TABLE2) -> &'a [MIB_IF_ROW2] {
        let count = usize::try_from(table.NumEntries).unwrap_or(0);
        // SAFETY: the table's `Table` field is the first element of a
        // trailing array of `NumEntries` rows, which is the layout
        // `GetIfTable2` documents and fills. The slice's lifetime is
        // tied to `&self`, so it cannot outlive the allocation.
        unsafe { std::slice::from_raw_parts(table.Table.as_ptr(), count) }
    }
}

impl Drop for IfTable {
    fn drop(&mut self) {
        free_if_table(self.0);
    }
}

/// Allocates an interface table into `pointer`.
fn get_if_table(pointer: &mut *mut MIB_IF_TABLE2) -> u32 {
    // SAFETY: `pointer` is a live, uniquely-borrowed out-parameter. On
    // success the allocation is transferred immediately to `IfTable`.
    unsafe { GetIfTable2(std::ptr::from_mut(pointer)) }
}

/// Releases the allocation exclusively owned by an `IfTable`.
fn free_if_table(pointer: *mut MIB_IF_TABLE2) {
    // SAFETY: callers pass the non-null allocation returned by
    // `GetIfTable2` exactly once, from `IfTable::drop`.
    unsafe { FreeMibTable(pointer.cast()) };
}

/// Counts open TCP and UDP endpoints per process.
///
/// Returns a map from PID to endpoint count. A PID missing from the map
/// has no open endpoints, which the caller renders as an em dash rather
/// than a zero.
#[must_use]
pub fn connections_by_pid() -> HashMap<u32, u32> {
    let mut counts: HashMap<u32, u32> = HashMap::new();
    count_tcp(&mut counts);
    count_udp(&mut counts);
    counts
}

/// Adds every TCP endpoint's owner to the counts.
fn count_tcp(counts: &mut HashMap<u32, u32>) {
    count_table::<MIB_TCPTABLE_OWNER_PID, MIB_TCPROW_OWNER_PID>(true, AF_INET as u32, counts);
    count_table::<MIB_TCP6TABLE_OWNER_PID, MIB_TCP6ROW_OWNER_PID>(true, AF_INET6 as u32, counts);
}

/// Adds every UDP endpoint's owner to the counts.
fn count_udp(counts: &mut HashMap<u32, u32>) {
    count_table::<MIB_UDPTABLE_OWNER_PID, MIB_UDPROW_OWNER_PID>(false, AF_INET as u32, counts);
    count_table::<MIB_UDP6TABLE_OWNER_PID, MIB_UDP6ROW_OWNER_PID>(false, AF_INET6 as u32, counts);
}

/// Reads and tallies one address-family/protocol table.
fn count_table<Table, Row>(tcp: bool, family: u32, counts: &mut HashMap<u32, u32>) {
    let Some(buffer) = extended_table(tcp, family) else {
        return;
    };
    let Some(count) = leading_count(&buffer) else {
        return;
    };
    let header = std::mem::size_of::<Table>().saturating_sub(std::mem::size_of::<Row>());
    let stride = std::mem::size_of::<Row>();
    let pid_offset = stride.saturating_sub(std::mem::size_of::<u32>());
    tally(&buffer, count, header, stride, pid_offset, counts);
}

/// Walks a table's rows and tallies the PID at `pid_offset` in each.
///
/// Shared by TCP and UDP because the two tables differ only in their row
/// size and the offset of the owning PID within it — the walk, and every
/// bounds check in it, is identical.
fn tally(
    buffer: &[u8],
    count: u32,
    header: usize,
    stride: usize,
    pid_offset: usize,
    counts: &mut HashMap<u32, u32>,
) {
    let rows = usize::try_from(count).unwrap_or(0);
    for index in 0..rows {
        let Some(base) = index
            .checked_mul(stride)
            .and_then(|shift| header.checked_add(shift))
        else {
            break;
        };
        let Some(start) = base.checked_add(pid_offset) else {
            break;
        };
        let Some(end) = start.checked_add(std::mem::size_of::<u32>()) else {
            break;
        };
        // The row count comes from the buffer; a count that overruns it
        // ends the walk rather than reading past the allocation.
        let Some(bytes) = buffer.get(start..end) else {
            break;
        };
        let Ok(word) = <[u8; 4]>::try_from(bytes) else {
            break;
        };
        let pid = u32::from_le_bytes(word);
        *counts.entry(pid).or_insert(0) += 1;
    }
}

/// The `DWORD` row count at the start of an extended table.
fn leading_count(buffer: &[u8]) -> Option<u32> {
    let bytes = buffer.get(..std::mem::size_of::<u32>())?;
    let word = <[u8; 4]>::try_from(bytes).ok()?;
    Some(u32::from_le_bytes(word))
}

/// Reads an extended connection table into a byte buffer.
///
/// `tcp` selects between the TCP and UDP variants; they share the
/// ask-then-fetch protocol and differ only in the function called.
///
/// IPv4 and IPv6 use different row layouts, selected by `family`.
fn extended_table(tcp: bool, family: u32) -> Option<Vec<u8>> {
    let mut size = 0u32;
    // First call: a null buffer asks for the size.
    //
    let _ = if tcp {
        query_tcp_table(std::ptr::null_mut(), &mut size, family)
    } else {
        query_udp_table(std::ptr::null_mut(), &mut size, family)
    };
    for _ in 0..3 {
        let capacity = usize::try_from(size).unwrap_or(0);
        if capacity == 0 {
            return None;
        }
        let mut buffer = vec![0u8; capacity];
        let status = if tcp {
            query_tcp_table(buffer.as_mut_ptr().cast(), &mut size, family)
        } else {
            query_udp_table(buffer.as_mut_ptr().cast(), &mut size, family)
        };
        if status == 0 {
            return Some(buffer);
        }
    }
    None
}

/// Queries or fills one extended TCP table.
fn query_tcp_table(buffer: *mut core::ffi::c_void, size: &mut u32, family: u32) -> u32 {
    // SAFETY: `buffer` is either null for the documented sizing call or
    // points to the caller's live `*size`-byte allocation. `size` is a
    // unique live out-parameter and the remaining values are constants.
    unsafe {
        GetExtendedTcpTable(
            buffer,
            std::ptr::from_mut(size),
            0,
            family,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        )
    }
}

/// Queries or fills one extended UDP table.
fn query_udp_table(buffer: *mut core::ffi::c_void, size: &mut u32, family: u32) -> u32 {
    // SAFETY: `buffer` is either null for the documented sizing call or
    // points to the caller's live `*size`-byte allocation. `size` is a
    // unique live out-parameter and the remaining values are constants.
    unsafe {
        GetExtendedUdpTable(
            buffer,
            std::ptr::from_mut(size),
            0,
            family,
            UDP_TABLE_OWNER_PID,
            0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_adapter_list_excludes_loopback_and_filter_modules() {
        // The regression this exists for: an NDIS filter module bound to
        // an adapter gets its own row, carrying the *same* octet
        // counters as the adapter it filters. On a machine with Npcap
        // and the two WFP lightweight filters installed that is five
        // rows for one network card, all reading the same throughput —
        // and a total that sums them reports five times the traffic the
        // machine moved.
        //
        // A filter module's alias is always the underlying adapter's
        // alias with the filter's name appended, so a row whose name is
        // a strict extension of another row's is the tell. Checked by
        // shape rather than by looking for "WFP", because the set of
        // filters installed is whatever the machine happens to have.
        let adapters = adapters();
        for adapter in &adapters {
            assert!(
                !adapter.name.to_lowercase().contains("loopback"),
                "the loopback interface should have been filtered out"
            );
            for other in &adapters {
                if other.luid == adapter.luid {
                    continue;
                }
                assert!(
                    !adapter.name.starts_with(&format!("{}-", other.name)),
                    "{} looks like a filter module bound to {} — its \
                     counters are that adapter's counters counted twice",
                    adapter.name,
                    other.name
                );
            }
        }
    }

    #[test]
    fn every_adapter_has_a_name_and_a_distinct_identity() {
        // A machine with no network at all is possible, so this asserts
        // the shape rather than the presence of an adapter. The LUID is
        // the part that matters: it keys the rate delta, the history
        // ring and the selection, so two adapters sharing one would
        // subtract one adapter's counters from another's.
        let adapters = adapters();
        let mut seen = std::collections::HashSet::new();
        for adapter in &adapters {
            assert!(
                !adapter.name.is_empty(),
                "an adapter should report a friendly name"
            );
            assert!(
                seen.insert(adapter.luid),
                "{} shares a LUID with another adapter",
                adapter.name
            );
        }
    }

    #[test]
    fn a_machine_with_a_network_card_reports_one_as_hardware() {
        // The `HardwareInterface` flag is read out of a hand-unpacked
        // bitfield, and getting the bit order wrong does not fail — it
        // silently files every physical adapter under "virtual", which
        // moves it into the collapsed group and drops it out of the
        // machine's throughput total. This is the assertion that would
        // notice.
        //
        // Guarded on the list being non-empty rather than asserting a
        // card exists: a VM guest genuinely has only synthetic adapters.
        let adapters = adapters();
        if adapters.iter().any(|adapter| {
            matches!(adapter.kind, AdapterKind::Ethernet | AdapterKind::WiFi)
                && adapter.link_speed > 0
        }) {
            assert!(
                adapters.iter().any(|adapter| adapter.hardware),
                "a machine with a live Ethernet or Wi-Fi adapter has at \
                 least one hardware interface, got {:?}",
                adapters
                    .iter()
                    .map(|adapter| (&adapter.name, adapter.kind, adapter.hardware))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn nothing_is_filtered_out_for_being_down() -> anyhow::Result<()> {
        let mut row = MIB_IF_ROW2 {
            Type: IF_TYPE_ETHERNET,
            ..MIB_IF_ROW2::default()
        };
        assert!(is_an_adapter(&row));
        row.OperStatus = 0;
        row.AdminStatus = 0;
        row.MediaConnectState = 0;
        row.TransmitLinkSpeed = 0;
        row.ReceiveLinkSpeed = 0;
        assert!(
            is_an_adapter(&row),
            "live state must describe an adapter, not remove it"
        );
        Ok(())
    }

    #[test]
    #[ignore = "environment smoke test"]
    fn this_process_can_be_found_among_the_connection_owners() {
        // Every Windows machine has open endpoints; the map should not be
        // empty, and every PID in it should be plausible.
        let counts = connections_by_pid();
        assert!(
            !counts.is_empty(),
            "a running machine has open TCP or UDP endpoints"
        );
        for (pid, count) in &counts {
            assert!(*count > 0, "PID {pid} was recorded with no endpoints");
        }
    }

    #[test]
    fn a_row_count_that_overruns_the_buffer_ends_the_walk() {
        // The count comes from the buffer itself, so it is untrusted.
        let buffer = vec![0u8; 8];
        let mut counts = HashMap::new();
        tally(&buffer, 9_999, 4, 24, 20, &mut counts);
        assert!(
            counts.is_empty(),
            "a count the buffer cannot back must not produce entries"
        );
    }

    #[test]
    fn a_well_formed_table_is_tallied() {
        // Two rows of 24 bytes after a 4-byte header, PID in the last
        // DWORD of each row.
        let mut buffer = vec![0u8; 4 + 24 * 2];
        if let Some(slot) = buffer.get_mut(0..4) {
            slot.copy_from_slice(&2u32.to_le_bytes());
        }
        if let Some(slot) = buffer.get_mut(4 + 20..4 + 24) {
            slot.copy_from_slice(&1234u32.to_le_bytes());
        }
        if let Some(slot) = buffer.get_mut(4 + 24 + 20..4 + 48) {
            slot.copy_from_slice(&1234u32.to_le_bytes());
        }
        let mut counts = HashMap::new();
        tally(&buffer, 2, 4, 24, 20, &mut counts);
        assert_eq!(
            counts.get(&1234),
            Some(&2),
            "both rows belong to the same process"
        );
    }

    #[test]
    fn an_empty_buffer_has_no_leading_count() {
        assert!(leading_count(&[]).is_none());
        assert!(leading_count(&[1, 2]).is_none(), "less than a DWORD");
        assert_eq!(leading_count(&[7, 0, 0, 0]), Some(7));
    }
}
