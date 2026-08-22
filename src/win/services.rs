// ============================================================================
// Module:       win::services
// Description:  Enumerating Windows services and their state, and starting or
//               stopping one.
//
// Dependencies: windows-sys (Service Control Manager); super::strings
// ============================================================================

//! Windows services.
//!
//! ## Read and write want different rights
//!
//! The Service Control Manager is opened with the rights the operation
//! needs and no more. [`enumerate`] asks for `SC_MANAGER_ENUMERATE_SERVICE`,
//! which any user has — so the Services view populates on a normal,
//! non-elevated run. [`start`] and [`stop`] open the individual service
//! with `SERVICE_START` / `SERVICE_STOP`, which normally require
//! administrator.
//!
//! Asking for `SC_MANAGER_ALL_ACCESS` up front — the obvious thing to
//! write — makes the *read* path fail for every non-administrator, so the
//! view is empty on the runs where it is most likely to be used. The
//! split is what lets a user see the service list and be told, only when
//! they try to stop something, that this particular action needs
//! elevation.
//!
//! ## Stopping is a request, not a command
//!
//! `ControlService(SERVICE_CONTROL_STOP)` asks a service to stop. It
//! returns as soon as the request is *accepted*, not when the service has
//! stopped — a service can take many seconds to shut down, and some
//! refuse. [`stop`] therefore reports that the request was sent, and the
//! next sample shows the real state. Blocking the UI thread on a
//! service's shutdown would freeze the window for as long as the service
//! felt like taking.

use super::strings;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, ControlService, EnumServicesStatusExW, OpenSCManagerW, OpenServiceW,
    StartServiceW, ENUM_SERVICE_STATUS_PROCESSW, SC_ENUM_PROCESS_INFO, SC_MANAGER_CONNECT,
    SC_MANAGER_ENUMERATE_SERVICE, SERVICE_CONTROL_STOP, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
    SERVICE_START, SERVICE_START_PENDING, SERVICE_STATE_ALL, SERVICE_STATUS, SERVICE_STOP,
    SERVICE_STOPPED, SERVICE_STOP_PENDING, SERVICE_WIN32,
};

/// An open SCM or service handle, closed on drop.
///
/// A service handle is **not** a kernel handle: it is closed with
/// `CloseServiceHandle`, not `CloseHandle`, and passing one to the wrong
/// closer is a bug that does not fail loudly. Hence a separate type from
/// [`super::handle::OwnedHandle`] rather than a reuse of it.
struct ServiceHandle(windows_sys::Win32::System::Services::SC_HANDLE);

impl ServiceHandle {
    /// Takes ownership of a handle from `OpenSCManagerW` or
    /// `OpenServiceW`.
    fn new(handle: windows_sys::Win32::System::Services::SC_HANDLE) -> Option<Self> {
        (!handle.is_null()).then_some(Self(handle))
    }

    /// The raw handle, valid while `self` is alive.
    fn raw(&self) -> windows_sys::Win32::System::Services::SC_HANDLE {
        self.0
    }
}

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        // SAFETY: `self.0` is a non-null service handle (checked in
        // `new`) owned exclusively by this value, and `CloseServiceHandle`
        // is its documented closer.
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

/// What state a service is in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ServiceState {
    /// Not running.
    #[default]
    Stopped,
    /// Starting.
    Starting,
    /// Running.
    Running,
    /// Stopping.
    Stopping,
    /// Paused, or in one of the pause transitions.
    Other,
}

impl ServiceState {
    /// The word shown in the Status column.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Stopping => "Stopping",
            Self::Other => "Paused",
        }
    }

    /// Whether this state is one a stop request makes sense from.
    #[must_use]
    pub fn can_stop(self) -> bool {
        matches!(self, Self::Running | Self::Other)
    }

    /// Whether this state is one a start request makes sense from.
    #[must_use]
    pub fn can_start(self) -> bool {
        matches!(self, Self::Stopped)
    }

    /// Maps the Win32 state constant.
    fn from_raw(state: u32) -> Self {
        match state {
            SERVICE_STOPPED => Self::Stopped,
            SERVICE_START_PENDING => Self::Starting,
            SERVICE_RUNNING => Self::Running,
            SERVICE_STOP_PENDING => Self::Stopping,
            _ => Self::Other,
        }
    }
}

/// One service.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Service {
    /// The short name, e.g. `Spooler`. What `sc` and `net` take.
    pub name: String,
    /// The display name, e.g. "Print Spooler".
    pub display_name: String,
    /// Current state.
    pub state: ServiceState,
    /// The hosting process's PID, or `None` when the service is not
    /// running.
    ///
    /// This is what makes the Services view useful next to the process
    /// list: it is the link from "svchost.exe is using 40% CPU" to which
    /// of the fifteen services inside it is responsible.
    pub pid: Option<u32>,
}

/// Every Win32 service on the machine.
///
/// Returns an empty list rather than an error when the SCM cannot be
/// opened — the Services view then shows its empty state, which is the
/// right outcome for a machine or an account where this is not available.
#[must_use]
pub fn enumerate() -> Vec<Service> {
    let Some(manager) = open_manager(SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE) else {
        return Vec::new();
    };
    let Some(buffer) = enumerate_buffer(&manager) else {
        return Vec::new();
    };
    parse_services(&buffer.bytes, buffer.count)
}

/// The raw result of an enumeration.
struct Enumeration {
    /// The filled buffer.
    bytes: Vec<u8>,
    /// How many entries the call reported.
    count: u32,
}

/// Runs the enumeration, growing the buffer once as the API directs.
fn enumerate_buffer(manager: &ServiceHandle) -> Option<Enumeration> {
    let mut needed = 0u32;
    let mut returned = 0u32;
    let mut resume = 0u32;

    // First call: a null buffer asks for the size.
    //
    // SAFETY: `manager` is a live SCM handle opened with
    // SC_MANAGER_ENUMERATE_SERVICE. A null buffer with a zero length is
    // the documented way to ask for the required size; the three
    // out-parameters are live, uniquely-borrowed `u32`s. The two null
    // trailing arguments decline the resume handle and the group filter,
    // which the call permits.
    let _ = unsafe {
        EnumServicesStatusExW(
            manager.raw(),
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            std::ptr::null_mut(),
            0,
            std::ptr::from_mut(&mut needed),
            std::ptr::from_mut(&mut returned),
            std::ptr::from_mut(&mut resume),
            std::ptr::null(),
        )
    };
    let size = usize::try_from(needed).unwrap_or(0);
    if size == 0 {
        return None;
    }

    let mut bytes = vec![0u8; size];
    // SAFETY: as above, with `bytes` now a live, uniquely-borrowed
    // allocation of exactly `needed` bytes — which is what the length
    // argument states and what the call just reported it needs.
    let ok = unsafe {
        EnumServicesStatusExW(
            manager.raw(),
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            bytes.as_mut_ptr(),
            needed,
            std::ptr::from_mut(&mut needed),
            std::ptr::from_mut(&mut returned),
            std::ptr::from_mut(&mut resume),
            std::ptr::null(),
        )
    };
    (ok != 0).then_some(Enumeration {
        bytes,
        count: returned,
    })
}

/// Parses an enumeration buffer into services.
///
/// The entries are a fixed-size array at the head of the buffer, but each
/// one's two name fields are *pointers into the same buffer* rather than
/// inline strings — so the buffer has to stay alive while the names are
/// read, and the strings are copied out before it is dropped. That is why
/// this takes the buffer by reference and returns owned `String`s.
fn parse_services(bytes: &[u8], count: u32) -> Vec<Service> {
    let stride = std::mem::size_of::<ENUM_SERVICE_STATUS_PROCESSW>();
    let entries = usize::try_from(count).unwrap_or(0);
    let mut found = Vec::with_capacity(entries);

    for index in 0..entries {
        let Some(base) = index.checked_mul(stride) else {
            break;
        };
        let Some(end) = base.checked_add(stride) else {
            break;
        };
        // The count comes from the call, but the buffer is what bounds
        // it: a count the buffer cannot back ends the walk.
        let Some(slice) = bytes.get(base..end) else {
            break;
        };
        // SAFETY: `slice` is exactly one entry long and valid for reads.
        // `read_unaligned` imposes no alignment requirement, which
        // matters because the buffer is a `Vec<u8>`.
        let entry = unsafe {
            std::ptr::read_unaligned(slice.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>())
        };

        // SAFETY: both name pointers point into `bytes`, which is
        // borrowed for the whole of this function, and both are
        // NUL-terminated strings the SCM wrote. The strings are copied
        // out here, before the borrow ends.
        let name = unsafe { wide_from_pointer(entry.lpServiceName) };
        // SAFETY: as above.
        let display_name = unsafe { wide_from_pointer(entry.lpDisplayName) };

        let status = entry.ServiceStatusProcess;
        found.push(Service {
            name,
            display_name,
            state: ServiceState::from_raw(status.dwCurrentState),
            // A stopped service reports PID 0, which is the idle process
            // rather than a host — so it is reported as "no process".
            pid: (status.dwProcessId != 0).then_some(status.dwProcessId),
        });
    }
    found
}

/// Reads a NUL-terminated wide string from a raw pointer.
///
/// # Safety
///
/// `pointer` must be null or point at a NUL-terminated UTF-16 string
/// alive for this call.
unsafe fn wide_from_pointer(pointer: *const u16) -> String {
    /// A guard against a missing terminator. Service names are short;
    /// display names are at most a couple of hundred characters.
    const MAX_UNITS: usize = 1024;
    if pointer.is_null() {
        return String::new();
    }
    let mut length = 0usize;
    while length < MAX_UNITS {
        // SAFETY: the caller guarantees NUL-termination, so every unit
        // up to the terminator is within the allocation; the cap stops
        // the scan if it is not.
        let unit = unsafe { *pointer.add(length) };
        if unit == 0 {
            break;
        }
        length += 1;
    }
    // SAFETY: `length` units were just confirmed readable.
    let slice = unsafe { std::slice::from_raw_parts(pointer, length) };
    String::from_utf16_lossy(slice)
}

/// Asks a service to start.
///
/// Normally requires administrator; see the module docs.
pub fn start(name: &str) -> Result<(), super::control::ActionError> {
    let service = open_service(name, SERVICE_START)?;
    // SAFETY: `service` is a live service handle opened with
    // SERVICE_START. A zero argument count with a null argument vector
    // is the documented "no arguments".
    let ok = unsafe { StartServiceW(service.raw(), 0, std::ptr::null()) };
    if ok == 0 {
        return Err(super::control::ActionError::Failed(last_error()));
    }
    Ok(())
}

/// Asks a service to stop.
///
/// Returns as soon as the request is accepted, not when the service has
/// stopped; see the module docs.
pub fn stop(name: &str) -> Result<(), super::control::ActionError> {
    let service = open_service(name, SERVICE_STOP | SERVICE_QUERY_STATUS)?;
    // SAFETY: `SERVICE_STATUS` is plain integers, so all-zero is a valid
    // starting value; the call overwrites it.
    let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: `service` is a live handle opened with SERVICE_STOP.
    // `status` is a live, uniquely-borrowed out-parameter the call writes
    // the service's state into.
    let ok = unsafe {
        ControlService(
            service.raw(),
            SERVICE_CONTROL_STOP,
            std::ptr::from_mut(&mut status),
        )
    };
    if ok == 0 {
        return Err(super::control::ActionError::Failed(last_error()));
    }
    Ok(())
}

/// Opens one service with the given rights.
fn open_service(name: &str, access: u32) -> Result<ServiceHandle, super::control::ActionError> {
    let Some(manager) = open_manager(SC_MANAGER_CONNECT) else {
        return Err(super::control::ActionError::Denied(last_error()));
    };
    let wide = strings::to_wide(name);
    // SAFETY: `manager` is a live SCM handle. `wide` is a live,
    // NUL-terminated UTF-16 buffer bound to a local that outlives the
    // call. The returned handle goes straight into `ServiceHandle`,
    // which rejects null and closes it on drop.
    let raw = unsafe { OpenServiceW(manager.raw(), wide.as_ptr(), access) };
    ServiceHandle::new(raw).ok_or_else(|| super::control::ActionError::Denied(last_error()))
}

/// Opens the Service Control Manager with the given rights.
fn open_manager(access: u32) -> Option<ServiceHandle> {
    // SAFETY: two null name pointers select the local machine and the
    // active database, which is what the call documents. The returned
    // handle goes straight into `ServiceHandle`.
    let raw = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), access) };
    ServiceHandle::new(raw)
}

/// `GetLastError`, wrapped.
fn last_error() -> u32 {
    // SAFETY: takes no arguments, reads thread-local state, cannot fail.
    unsafe { GetLastError() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn services_enumerate_without_elevation() {
        // The reason the read path asks only for
        // SC_MANAGER_ENUMERATE_SERVICE. Asking for ALL_ACCESS would make
        // this empty on every non-administrator run.
        let services = enumerate();
        assert!(
            services.len() > 20,
            "a Windows machine has well over twenty services; got {}",
            services.len()
        );
    }

    #[test]
    fn a_well_known_service_is_present_and_named() {
        let services = enumerate();
        let spooler = services.iter().find(|service| service.name == "Spooler");
        let Some(spooler) = spooler else {
            // The print spooler can be removed on a hardened install.
            return;
        };
        assert!(
            !spooler.display_name.is_empty(),
            "a service should carry its display name"
        );
        assert_ne!(
            spooler.display_name, spooler.name,
            "the display name is the human-readable one"
        );
    }

    #[test]
    fn a_running_service_reports_its_hosting_process() {
        // The link from "svchost.exe is using 40% CPU" to which service
        // is responsible — the main reason this view exists.
        let services = enumerate();
        let running: Vec<&Service> = services
            .iter()
            .filter(|service| service.state == ServiceState::Running)
            .collect();
        assert!(!running.is_empty(), "a machine has running services");
        assert!(
            running.iter().any(|service| service.pid.is_some()),
            "a running service should name its hosting process"
        );
    }

    #[test]
    fn a_stopped_service_reports_no_process_rather_than_pid_zero() {
        // PID 0 is the idle process, not a host.
        for service in enumerate() {
            if service.state == ServiceState::Stopped {
                assert_eq!(
                    service.pid, None,
                    "{} is stopped but claims a hosting process",
                    service.name
                );
            }
        }
    }

    #[test]
    fn state_transitions_gate_the_right_actions() {
        assert!(ServiceState::Running.can_stop());
        assert!(!ServiceState::Running.can_start());
        assert!(ServiceState::Stopped.can_start());
        assert!(!ServiceState::Stopped.can_stop());
        // A service that is already changing state should offer neither,
        // or a double-click sends a second request into a transition.
        assert!(!ServiceState::Starting.can_start());
        assert!(!ServiceState::Starting.can_stop());
        assert!(!ServiceState::Stopping.can_start());
        assert!(!ServiceState::Stopping.can_stop());
    }

    #[test]
    fn every_state_has_a_label() {
        for state in [
            ServiceState::Stopped,
            ServiceState::Starting,
            ServiceState::Running,
            ServiceState::Stopping,
            ServiceState::Other,
        ] {
            assert!(!state.label().is_empty());
        }
    }

    #[test]
    fn an_unknown_state_constant_does_not_claim_to_be_running() {
        // Falling back to `Running` for an unrecognised state would put a
        // stop button on something that is not running.
        assert_eq!(ServiceState::from_raw(9999), ServiceState::Other);
        assert_eq!(
            ServiceState::from_raw(SERVICE_RUNNING),
            ServiceState::Running
        );
    }

    #[test]
    fn a_service_that_does_not_exist_is_reported_rather_than_silently_ignored() {
        let result = start("RustamanNoSuchServiceExists");
        assert!(
            result.is_err(),
            "starting a nonexistent service must not report success"
        );
    }

    #[test]
    fn an_entry_count_the_buffer_cannot_back_ends_the_walk() {
        let bytes = vec![0u8; 8];
        assert!(
            parse_services(&bytes, 500).is_empty(),
            "a count past the buffer must not read past it"
        );
    }
}
