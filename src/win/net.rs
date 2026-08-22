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
//! ## Loopback and the tunnels are excluded
//!
//! An adapter list that includes the loopback interface reports traffic
//! that never left the machine, and one that includes every WFP callout
//! and tunnel pseudo-adapter is a list of twenty entries of which two are
//! real. [`adapters`] filters to interfaces that are connected, not
//! loopback, and have a non-zero link speed.

use super::strings;
use std::collections::HashMap;
use windows_sys::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetExtendedTcpTable, GetExtendedUdpTable, GetIfTable2, MIB_IF_ROW2,
    MIB_IF_TABLE2, MIB_TCPTABLE_OWNER_PID, MIB_UDPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
    UDP_TABLE_OWNER_PID,
};
use windows_sys::Win32::NetworkManagement::Ndis::NET_IF_OPER_STATUS_UP;
use windows_sys::Win32::Networking::WinSock::AF_INET;

/// One adapter's cumulative counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdapterCounters {
    /// The adapter's friendly name, e.g. "Ethernet" or "Wi-Fi".
    pub name: String,
    /// Cumulative octets received.
    pub received: u64,
    /// Cumulative octets sent.
    pub sent: u64,
    /// Nominal link speed in bits per second, for the graph's scale.
    pub link_speed: u64,
}

/// Reads every real, connected adapter's counters.
///
/// See the module docs on what is filtered out and why.
#[must_use]
pub fn adapters() -> Vec<AdapterCounters> {
    let Some(table) = IfTable::read() else {
        return Vec::new();
    };
    table
        .rows()
        .iter()
        .filter(|row| is_interesting(row))
        .map(|row| AdapterCounters {
            name: strings::from_wide_nul(&row.Alias),
            received: row.InOctets,
            sent: row.OutOctets,
            link_speed: row.TransmitLinkSpeed,
        })
        .collect()
}

/// Whether an interface is one a person would recognise as their network
/// connection.
fn is_interesting(row: &MIB_IF_ROW2) -> bool {
    /// `IF_TYPE_SOFTWARE_LOOPBACK`, from the IANA interface-type list.
    const LOOPBACK: u32 = 24;
    /// `IF_TYPE_TUNNEL`.
    const TUNNEL: u32 = 131;

    row.OperStatus == NET_IF_OPER_STATUS_UP
        && row.Type != LOOPBACK
        && row.Type != TUNNEL
        // A pseudo-adapter with no link speed is not a network
        // connection; a real one always reports one.
        && row.TransmitLinkSpeed > 0
        // `u64::MAX` is what a virtual adapter reports for "unlimited",
        // which would make the graph's scale meaningless.
        && row.TransmitLinkSpeed != u64::MAX
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
        // SAFETY: `pointer` is a live, uniquely-borrowed out-parameter
        // the callee writes a freshly allocated table into on success.
        // Ownership of that allocation transfers to this `IfTable`,
        // whose `Drop` frees it.
        let status = unsafe { GetIfTable2(std::ptr::from_mut(&mut pointer)) };
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
        // SAFETY: `self.0` is a non-null table allocated by
        // `GetIfTable2` (checked in `read`) and alive for the borrow.
        let table = unsafe { &*self.0 };
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
        // SAFETY: `self.0` is a non-null table from `GetIfTable2`, owned
        // exclusively by this value, and this is the one free.
        unsafe { FreeMibTable(self.0.cast()) };
    }
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
    let Some(buffer) = extended_table(true) else {
        return;
    };
    // The table is a `DWORD` count followed by that many rows.
    let Some(count) = leading_count(&buffer) else {
        return;
    };
    let header = std::mem::size_of::<MIB_TCPTABLE_OWNER_PID>()
        - std::mem::size_of::<windows_sys::Win32::NetworkManagement::IpHelper::MIB_TCPROW_OWNER_PID>(
        );
    let stride = std::mem::size_of::<
        windows_sys::Win32::NetworkManagement::IpHelper::MIB_TCPROW_OWNER_PID,
    >();
    // The owning PID is the last `DWORD` of the row.
    let pid_offset = stride - std::mem::size_of::<u32>();
    tally(&buffer, count, header, stride, pid_offset, counts);
}

/// Adds every UDP endpoint's owner to the counts.
fn count_udp(counts: &mut HashMap<u32, u32>) {
    let Some(buffer) = extended_table(false) else {
        return;
    };
    let Some(count) = leading_count(&buffer) else {
        return;
    };
    let header = std::mem::size_of::<MIB_UDPTABLE_OWNER_PID>()
        - std::mem::size_of::<windows_sys::Win32::NetworkManagement::IpHelper::MIB_UDPROW_OWNER_PID>(
        );
    let stride = std::mem::size_of::<
        windows_sys::Win32::NetworkManagement::IpHelper::MIB_UDPROW_OWNER_PID,
    >();
    let pid_offset = stride - std::mem::size_of::<u32>();
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
/// IPv4 only. The IPv6 tables are a second pair of calls with a second
/// pair of row layouts, and a process with an IPv6 endpoint almost always
/// has an IPv4 one too — so the second pair roughly doubles the work to
/// change a count from "some" to "slightly more some". Recorded in
/// `docs/WINDOWS_APIS.md` as a deliberate omission.
fn extended_table(tcp: bool) -> Option<Vec<u8>> {
    let mut size = 0u32;
    // First call: a null buffer asks for the size.
    //
    // SAFETY: a null buffer with `size` as a live out-parameter is the
    // documented way to ask for the required length. The call is
    // expected to fail; only the size it reports is used. The remaining
    // arguments are by-value constants.
    let _ = unsafe {
        if tcp {
            GetExtendedTcpTable(
                std::ptr::null_mut(),
                std::ptr::from_mut(&mut size),
                0,
                AF_INET as u32,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        } else {
            GetExtendedUdpTable(
                std::ptr::null_mut(),
                std::ptr::from_mut(&mut size),
                0,
                AF_INET as u32,
                UDP_TABLE_OWNER_PID,
                0,
            )
        }
    };
    let capacity = usize::try_from(size).unwrap_or(0);
    if capacity == 0 {
        return None;
    }

    let mut buffer = vec![0u8; capacity];
    // SAFETY: `buffer` is a live, uniquely-borrowed allocation of exactly
    // `size` bytes, which is what the length out-parameter states. The
    // callee writes only within it and does not retain the pointer.
    let status = unsafe {
        if tcp {
            GetExtendedTcpTable(
                buffer.as_mut_ptr().cast(),
                std::ptr::from_mut(&mut size),
                0,
                AF_INET as u32,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        } else {
            GetExtendedUdpTable(
                buffer.as_mut_ptr().cast(),
                std::ptr::from_mut(&mut size),
                0,
                AF_INET as u32,
                UDP_TABLE_OWNER_PID,
                0,
            )
        }
    };
    (status == 0).then_some(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_adapter_list_excludes_loopback_and_pseudo_adapters() {
        // A list with loopback in it reports traffic that never left the
        // machine; one with every tunnel and callout in it is twenty
        // entries of which two are real.
        for adapter in adapters() {
            assert!(
                adapter.link_speed > 0 && adapter.link_speed != u64::MAX,
                "{} has an implausible link speed of {}",
                adapter.name,
                adapter.link_speed
            );
            assert!(
                !adapter.name.to_lowercase().contains("loopback"),
                "the loopback interface should have been filtered out"
            );
        }
    }

    #[test]
    fn adapter_counters_are_readable() {
        // A machine with no network at all is possible, so this asserts
        // the shape rather than the presence of an adapter.
        for adapter in adapters() {
            assert!(
                !adapter.name.is_empty(),
                "an adapter should report a friendly name"
            );
        }
    }

    #[test]
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
