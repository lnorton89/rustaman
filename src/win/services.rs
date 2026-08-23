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
    SC_MANAGER_ENUMERATE_SERVICE, SERVICE_CONTINUE_PENDING, SERVICE_CONTROL_STOP, SERVICE_PAUSED,
    SERVICE_PAUSE_PENDING, SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START,
    SERVICE_START_PENDING, SERVICE_STATE_ALL, SERVICE_STATUS, SERVICE_STOP, SERVICE_STOPPED,
    SERVICE_STOP_PENDING, SERVICE_WIN32,
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
        close_service_handle(self.0);
    }
}

/// Closes the service handle owned by a `ServiceHandle`.
fn close_service_handle(handle: windows_sys::Win32::System::Services::SC_HANDLE) {
    // SAFETY: `handle` is non-null (checked by `ServiceHandle::new`),
    // exclusively owned, and this function is called once from `Drop`.
    let _ = unsafe { CloseServiceHandle(handle) };
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
    /// Paused.
    Paused,
    /// Pausing, continuing, or an unrecognised transition.
    Transitioning,
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
            Self::Paused => "Paused",
            Self::Transitioning => "Transitioning",
        }
    }

    /// Whether this state is one a stop request makes sense from.
    #[must_use]
    pub fn can_stop(self) -> bool {
        matches!(self, Self::Running | Self::Paused)
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
            SERVICE_PAUSED => Self::Paused,
            SERVICE_PAUSE_PENDING | SERVICE_CONTINUE_PENDING => Self::Transitioning,
            _ => Self::Transitioning,
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

/// Runs the enumeration, retrying when the mutable service table grows.
fn enumerate_buffer(manager: &ServiceHandle) -> Option<Enumeration> {
    let mut needed = 0u32;
    let mut returned = 0u32;
    let mut resume = 0u32;

    // First call: a null buffer asks for the size.
    let _ = enumerate_services_call(manager, None, &mut needed, &mut returned, &mut resume);
    for _ in 0..3 {
        let size = usize::try_from(needed).unwrap_or(0);
        if size == 0 {
            return None;
        }
        let mut bytes = vec![0u8; size];
        returned = 0;
        resume = 0;
        let ok = enumerate_services_call(
            manager,
            Some(&mut bytes),
            &mut needed,
            &mut returned,
            &mut resume,
        );
        if ok != 0 {
            return Some(Enumeration {
                bytes,
                count: returned,
            });
        }
    }
    None
}

/// Performs one sizing or filling call to `EnumServicesStatusExW`.
fn enumerate_services_call(
    manager: &ServiceHandle,
    bytes: Option<&mut [u8]>,
    needed: &mut u32,
    returned: &mut u32,
    resume: &mut u32,
) -> i32 {
    let (buffer, capacity) = bytes.map_or((std::ptr::null_mut(), 0), |bytes| {
        (
            bytes.as_mut_ptr(),
            u32::try_from(bytes.len()).unwrap_or(u32::MAX),
        )
    });
    // SAFETY: `manager` is live and has enumeration access; `buffer` is
    // either null for sizing or names `capacity` writable bytes. Every
    // out-parameter is uniquely borrowed for this synchronous call.
    unsafe {
        EnumServicesStatusExW(
            manager.raw(),
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            buffer,
            capacity,
            std::ptr::from_mut(needed),
            std::ptr::from_mut(returned),
            std::ptr::from_mut(resume),
            std::ptr::null(),
        )
    }
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
        let Some(entry) = read_service_entry(slice) else {
            break;
        };

        let Some(name) = wide_from_buffer(entry.lpServiceName, bytes) else {
            continue;
        };
        let Some(display_name) = wide_from_buffer(entry.lpDisplayName, bytes) else {
            continue;
        };

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

/// Copies one possibly unaligned enumeration entry from a byte slice.
fn read_service_entry(slice: &[u8]) -> Option<ENUM_SERVICE_STATUS_PROCESSW> {
    if slice.len() < std::mem::size_of::<ENUM_SERVICE_STATUS_PROCESSW>() {
        return None;
    }
    // SAFETY: callers pass exactly one entry's readable bytes;
    // `read_unaligned` deliberately imposes no alignment requirement.
    Some(unsafe { std::ptr::read_unaligned(slice.as_ptr().cast()) })
}

/// Copies a string only when its pointer and terminator are inside the
/// owning SCM buffer.
fn wide_from_buffer(pointer: *const u16, bytes: &[u8]) -> Option<String> {
    if pointer.is_null() {
        return Some(String::new());
    }
    let start = pointer as usize;
    let owner = bytes.as_ptr() as usize;
    let end = owner.checked_add(bytes.len())?;
    if start < owner || start >= end || !start.is_multiple_of(std::mem::align_of::<u16>()) {
        return None;
    }
    let remaining = end.checked_sub(start)? / 2;
    // SAFETY: alignment and bounds were checked above.
    let wide = unsafe { std::slice::from_raw_parts(pointer, remaining) };
    let length = wide.iter().position(|unit| *unit == 0)?;
    Some(String::from_utf16_lossy(&wide[..length]))
}

/// Asks a service to start.
///
/// Normally requires administrator; see the module docs.
pub fn start(name: &str) -> Result<(), super::control::ActionError> {
    let service = open_service(name, SERVICE_START)?;
    let ok = start_service(&service);
    if ok == 0 {
        return Err(super::control::ActionError::Failed(last_error()));
    }
    Ok(())
}

/// Starts a service without command-line arguments.
fn start_service(service: &ServiceHandle) -> i32 {
    // SAFETY: `service` is live and opened with `SERVICE_START`; a zero
    // count with a null vector is the documented no-arguments form.
    unsafe { StartServiceW(service.raw(), 0, std::ptr::null()) }
}

/// Asks a service to stop.
///
/// Returns as soon as the request is accepted, not when the service has
/// stopped; see the module docs.
pub fn stop(name: &str) -> Result<(), super::control::ActionError> {
    let service = open_service(name, SERVICE_STOP | SERVICE_QUERY_STATUS)?;
    let mut status = SERVICE_STATUS::default();
    let ok = stop_service(&service, &mut status);
    if ok == 0 {
        return Err(super::control::ActionError::Failed(last_error()));
    }
    Ok(())
}

/// Sends one stop control to a service.
fn stop_service(service: &ServiceHandle, status: &mut SERVICE_STATUS) -> i32 {
    // SAFETY: `service` is live and opened with `SERVICE_STOP`; `status`
    // is a live uniquely borrowed out-parameter for this call.
    unsafe {
        ControlService(
            service.raw(),
            SERVICE_CONTROL_STOP,
            std::ptr::from_mut(status),
        )
    }
}

/// Opens one service with the given rights.
fn open_service(name: &str, access: u32) -> Result<ServiceHandle, super::control::ActionError> {
    let Some(manager) = open_manager(SC_MANAGER_CONNECT) else {
        return Err(super::control::ActionError::Denied(last_error()));
    };
    let wide = strings::to_wide(name);
    let raw = open_service_call(&manager, &wide, access);
    ServiceHandle::new(raw).ok_or_else(|| super::control::ActionError::Denied(last_error()))
}

/// Opens a named service under a live manager.
fn open_service_call(
    manager: &ServiceHandle,
    name: &[u16],
    access: u32,
) -> windows_sys::Win32::System::Services::SC_HANDLE {
    // SAFETY: `manager` is live; `name` is a live NUL-terminated UTF-16
    // buffer and the returned handle is immediately given an owner.
    unsafe { OpenServiceW(manager.raw(), name.as_ptr(), access) }
}

/// Opens the Service Control Manager with the given rights.
fn open_manager(access: u32) -> Option<ServiceHandle> {
    let raw = open_manager_call(access);
    ServiceHandle::new(raw)
}

/// Opens the local machine's active Service Control Manager database.
fn open_manager_call(access: u32) -> windows_sys::Win32::System::Services::SC_HANDLE {
    // SAFETY: null names select the local machine and active database;
    // the returned handle is immediately given to `ServiceHandle`.
    unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), access) }
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
    #[ignore = "environment smoke test"]
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
    #[ignore = "environment smoke test"]
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
            ServiceState::Transitioning,
        ] {
            assert!(!state.label().is_empty());
        }
    }

    #[test]
    fn an_unknown_state_constant_does_not_claim_to_be_running() {
        // Falling back to `Running` for an unrecognised state would put a
        // stop button on something that is not running.
        assert_eq!(ServiceState::from_raw(9999), ServiceState::Transitioning);
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
