// ============================================================================
// Module:       gui::app
// Description:  The window's whole state — the latest snapshot, the view, the
//               selection, the history rings — and the eframe entry point.
//
// Dependencies: eframe/egui; crate::engine, crate::model, crate::theme
// ============================================================================

//! The application state.
//!
//! One struct holding everything the window knows, and the `eframe::App`
//! implementation that drives it. The draw code in [`super::ui`] takes
//! `&mut App` and reads from it; nothing else owns state.
//!
//! ## State lives in the struct that owns the view
//!
//! [`App`] does not have forty flat fields prefixed `process_` and
//! `service_`. Each view's state is its own struct — [`ProcessView`],
//! [`ServicesView`], [`PerformanceView`] — and a new field goes in its
//! group. That is the difference between a struct someone can read and
//! one that has to be searched.
//!
//! ## Nothing tree-sized happens in a draw call
//!
//! `ui::draw` runs in full every frame, at up to sixty frames a second.
//! A machine can have four hundred processes; sorting, filtering, and
//! flattening them is O(n log n) work that must not happen sixty times a
//! second when the data changes once.
//!
//! So the visible rows are **cached** on [`ProcessView`] and rebuilt only
//! when something they depend on changes — see [`rows::RowKey`]. If you
//! add a field that affects which rows are shown or their order, add it
//! to that key, or the table will quietly stop responding to it.

pub mod actions;
pub mod background;
pub mod rows;

use crate::config::Config;
use crate::engine::Engine;
use crate::icon::Icon;
use crate::model::columns::ColumnOrder;
use crate::model::filter::Query;
use crate::model::history::Series;
use crate::model::sort::{compare_text, SortKey};
use crate::model::tree::Entry;
use crate::model::{ProcessKey, ProcessKind, Snapshot};
use crate::theme::{Catalog, Palette};
use std::collections::{HashMap, HashSet};

/// How many samples of history each graph keeps.
///
/// At the default one-second interval that is four minutes, which is long
/// enough to see a build start and finish and short enough that the ring
/// costs nothing. Sixty-four cores at four minutes is 240 f32s each —
/// about 60 KB in total.
pub const HISTORY: usize = 240;

/// Which view is on screen.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum View {
    /// The process tree.
    #[default]
    Processes,
    /// CPU, memory, disk, network and GPU graphs.
    Performance,
    /// Per-process memory: what is holding it, and how.
    Memory,
    /// A flat, technical table of every process.
    Details,
    /// Windows services.
    Services,
    /// Programs that run at logon.
    Startup,
    /// Machine, Windows, firmware, and hardware facts.
    System,
    /// Theme, interval, and the about panel.
    Settings,
}

impl View {
    /// Every view, in the order the navigation rail lists them.
    pub const ALL: [Self; 8] = [
        Self::Processes,
        Self::Performance,
        Self::Memory,
        Self::Details,
        Self::Services,
        Self::Startup,
        Self::System,
        Self::Settings,
    ];

    /// The rail's label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Processes => "Processes",
            Self::Performance => "Performance",
            Self::Memory => "Memory",
            Self::Details => "Details",
            Self::Services => "Services",
            Self::Startup => "Startup",
            Self::System => "System",
            Self::Settings => "Settings",
        }
    }

    /// The rail's icon.
    ///
    /// These were Unicode glyphs — `U+25A4`, `U+2699` and so on — on the
    /// reasoning that egui's bundled fonts made them free. They are not
    /// in those fonts: the whole rail shipped as a column of empty boxes
    /// beside its labels. See [`crate::icon`] for what replaced them and
    /// why an icon font was not the answer either.
    #[must_use]
    pub fn icon(self) -> Icon {
        match self {
            Self::Processes => Icon::Processes,
            Self::Performance => Icon::Performance,
            Self::Memory => Icon::Memory,
            Self::Details => Icon::Details,
            Self::Services => Icon::Services,
            Self::Startup => Icon::Startup,
            Self::System => Icon::SystemInfo,
            Self::Settings => Icon::Settings,
        }
    }

    /// The stable name persisted in the config file.
    ///
    /// Separate from [`View::label`] so the label can be reworded without
    /// invalidating everyone's saved view.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Processes => "processes",
            Self::Performance => "performance",
            Self::Memory => "memory",
            Self::Details => "details",
            Self::Services => "services",
            Self::Startup => "startup",
            Self::System => "system",
            Self::Settings => "settings",
        }
    }

    /// The view a persisted id names, or the default.
    #[must_use]
    pub fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|view| view.id() == id)
            .unwrap_or_default()
    }
}

/// The process list's own state.
pub struct ProcessView {
    /// The column being sorted on.
    pub sort: SortKey,
    /// Whether that sort is descending.
    pub descending: bool,
    /// Whether the list groups into a tree with category headings.
    pub grouped: bool,
    /// The search box's text.
    pub search: String,
    /// The parsed form of it, rebuilt only when the text changes.
    pub query: Query,
    /// Which rows have their children shown.
    pub expanded: HashSet<ProcessKey>,
    /// Which categories are collapsed.
    pub collapsed: HashSet<ProcessKind>,
    /// The selected row, if any.
    pub selected: Option<ProcessKey>,
    /// The order the columns are drawn in, as the user dragged them.
    ///
    /// Persisted, and reconciled against the build's own column set on
    /// load — see [`crate::model::columns`], which is where the cases
    /// that matter (a column added or removed by a later release) are
    /// handled.
    pub columns: ColumnOrder,
    /// The flattened rows to draw, and the state they were built from.
    pub rows: rows::Cache,
    /// GPU textures uploaded from the sampler's per-image shell icons.
    pub icons: HashMap<std::path::PathBuf, egui::TextureHandle>,
    /// Snapshot sequence whose icons were last reconciled.
    pub icon_sequence: u64,
}

impl Default for ProcessView {
    fn default() -> Self {
        Self {
            // CPU descending is what a task manager is opened for nine
            // times in ten: "what is making this machine slow".
            sort: SortKey::Cpu,
            descending: true,
            grouped: true,
            search: String::new(),
            query: Query::default(),
            expanded: HashSet::new(),
            collapsed: HashSet::new(),
            selected: None,
            columns: ColumnOrder::new(&crate::gui::ui::processes::DEFAULT_COLUMNS),
            rows: rows::Cache::default(),
            icons: HashMap::new(),
            icon_sequence: 0,
        }
    }
}

/// The Details view's own state.
pub struct DetailsView {
    /// The column being sorted on.
    pub sort: SortKey,
    /// Whether that sort is descending.
    pub descending: bool,
    /// The search box's text.
    pub search: String,
    /// The parsed query.
    pub query: Query,
    /// The selected row.
    pub selected: Option<ProcessKey>,
    /// The flattened rows.
    pub rows: rows::Cache,
}

impl Default for DetailsView {
    fn default() -> Self {
        Self {
            sort: SortKey::Pid,
            descending: false,
            search: String::new(),
            query: Query::default(),
            selected: None,
            rows: rows::Cache::default(),
        }
    }
}

/// The Performance view's history rings.
///
/// Every graph draws from one of these. They are on the app rather than
/// rebuilt per frame because a graph is a *history*, and the snapshot
/// only carries the present.
pub struct PerformanceView {
    /// Overall CPU utilisation.
    pub cpu: Series,
    /// Kernel-mode share of it, drawn as a band beneath.
    pub cpu_kernel: Series,
    /// Per-logical-processor utilisation, one ring each.
    pub cores: Vec<Series>,
    /// Physical memory in use, as a percentage.
    pub memory: Series,
    /// Combined disk throughput, in bytes per second.
    ///
    /// Kept alongside the two halves below because the disk *tile* in
    /// the picker is forty points tall and has room for one line, and
    /// the question it answers there is only "is this disk busy".
    pub disk: Series,
    /// Bytes read per second, across every disk.
    pub disk_read: Series,
    /// Bytes written per second, across every disk.
    ///
    /// Read and write are held apart because summing them discards the
    /// one thing that distinguishes a machine that is paging from a
    /// machine that is writing a backup — and those want opposite
    /// responses from whoever is looking at the graph.
    pub disk_write: Series,
    /// Combined network throughput, in bytes per second. See
    /// [`crate::model::SystemSample::network_rate`] on what "combined"
    /// means here — it is not the sum of the list.
    pub network: Series,
    /// The send half of it.
    pub network_send: Series,
    /// The receive half.
    ///
    /// Stored rather than derived as total-minus-send: the total is
    /// hardware adapters only, so on a machine whose send traffic goes
    /// over a virtual adapter the subtraction is negative, and a
    /// negative rate plots below the axis.
    pub network_receive: Series,
    /// One ring per adapter, keyed by interface LUID.
    ///
    /// Keyed rather than indexed, and keyed on the LUID rather than the
    /// name, for the reason every other identity in this app is: a
    /// `Vec` parallel to the snapshot's adapter list gives an adapter
    /// the history of whichever adapter used to occupy its slot the
    /// moment one is added, removed or renamed.
    pub adapters: HashMap<u64, Series>,
    /// Busiest GPU engine, as a percentage.
    pub gpu: Series,
    /// Which sub-panel the Performance view has selected.
    pub focus: PerformanceFocus,
    /// Which adapter the Network panel's graph is showing, or `None`
    /// for the machine's total.
    pub network_selected: Option<u64>,
    /// Whether the Network panel's virtual-adapter group is expanded.
    ///
    /// Collapsed by default: a dev machine running Hyper-V, WSL or a VPN
    /// client reports a couple of dozen virtual adapters, and opening
    /// the page to all of them drawn out is the thing this field exists
    /// to avoid.
    ///
    /// Note what the group is: adapters with no hardware behind them,
    /// which is a *fixed* property. It used to be adapters carrying no
    /// traffic, which is not — so an adapter with intermittent traffic
    /// moved between the list and the drawer every second, and the row
    /// someone was reaching for was gone by the time they clicked.
    pub network_virtual_expanded: bool,
}

impl Default for PerformanceView {
    /// Every ring at [`HISTORY`] capacity.
    ///
    /// Written out rather than derived because a `Series`'s capacity is
    /// its whole point, and a derived `Default` would give every graph a
    /// one-sample ring — which draws as nothing, on every graph, with no
    /// error anywhere.
    fn default() -> Self {
        Self {
            cpu: Series::new(HISTORY),
            cpu_kernel: Series::new(HISTORY),
            cores: Vec::new(),
            memory: Series::new(HISTORY),
            disk: Series::new(HISTORY),
            disk_read: Series::new(HISTORY),
            disk_write: Series::new(HISTORY),
            network: Series::new(HISTORY),
            network_send: Series::new(HISTORY),
            network_receive: Series::new(HISTORY),
            adapters: HashMap::new(),
            gpu: Series::new(HISTORY),
            focus: PerformanceFocus::default(),
            network_selected: None,
            network_virtual_expanded: false,
        }
    }
}

/// The Memory view's own state.
///
/// Small on purpose: everything it draws comes from the snapshot the
/// sampler already produced, so there is nothing to cache here — only
/// what the *reader* has chosen.
#[derive(Default)]
pub struct MemoryView {
    /// The process whose breakdown is shown, if one has been picked.
    ///
    /// Keyed on [`ProcessKey`] rather than a PID or a tile index: the
    /// treemap is rebuilt from a fresh snapshot every second and tiles
    /// move, so an index would select whatever landed in that spot.
    pub selected: Option<ProcessKey>,
    /// Whether the treemap is sized by private working set or by total
    /// commit. Two genuinely different questions — see
    /// [`MemoryMeasure`].
    pub measure: MemoryMeasure,
}

/// What the Memory view's treemap sizes its tiles by.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MemoryMeasure {
    /// Private working set: the resident pages a process shares with
    /// nobody.
    ///
    /// The default, and the only one of the two that adds up. Summed
    /// across the machine it approaches the memory actually in use,
    /// because no page is counted twice.
    #[default]
    Resident,
    /// Private commit: everything the process has been promised,
    /// resident or paged out.
    ///
    /// Larger than the resident figure, and the one that answers "what
    /// will this cost when it is all touched at once" rather than "what
    /// is it costing now".
    Committed,
}

impl MemoryMeasure {
    /// Both, in the order the switch offers them.
    pub const ALL: [Self; 2] = [Self::Resident, Self::Committed];

    /// The word on the switch.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Resident => "In RAM",
            Self::Committed => "Committed",
        }
    }

    /// What this measure says one process is holding.
    #[must_use]
    pub fn of(self, row: &crate::model::ProcessRow) -> u64 {
        match self {
            Self::Resident => row.private_working_set,
            Self::Committed => row.private_bytes,
        }
    }
}

/// Which resource the Performance view is showing in detail.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum PerformanceFocus {
    /// Processor.
    #[default]
    Cpu,
    /// Physical memory.
    Memory,
    /// Physical disks.
    Disk,
    /// Network adapters.
    Network,
    /// Graphics adapters.
    Gpu,
}

/// The Services view's own state.
#[derive(Default)]
pub struct ServicesView {
    /// The services read at the last refresh.
    pub services: Vec<crate::win::services::Service>,
    /// The search box's text.
    pub search: String,
    /// The selected service's short name.
    pub selected: Option<String>,
    /// When the list was last read. Services are not read by the sampler
    /// — see [`crate::engine::sampler`] — so the view refreshes them on
    /// its own schedule.
    pub refreshed: Option<std::time::Instant>,
    /// The column being sorted on.
    pub sort: ServiceSortKey,
    /// Whether that sort is descending.
    pub descending: bool,
    /// A background read of the service list, if one is in flight.
    ///
    /// `EnumServicesStatusExW` is a real syscall and must not run on the
    /// paint thread — see [`background::BackgroundRead`].
    pub pending: Option<background::BackgroundRead<Vec<crate::win::services::Service>>>,
    /// The search text, sort and read this filtered, sorted list was
    /// built from — recomputed only when one of those actually changes,
    /// the same reasoning `rows::Cache` applies to the process table.
    pub visible_key: Option<(String, ServiceSortKey, bool, Option<std::time::Instant>)>,
    /// The filtered, sorted list [`Self::visible_key`] was built for.
    pub visible: Vec<crate::win::services::Service>,
}

/// A column the Services table can sort by.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ServiceSortKey {
    /// The display name, falling back to the short name — what the Name
    /// column actually shows.
    #[default]
    Name,
    /// The short name — what `sc` and `net` take.
    ShortName,
    /// Running, stopped, or a transition between them.
    Status,
    /// The hosting process's PID.
    Pid,
}

impl ServiceSortKey {
    /// The column heading.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::ShortName => "Service",
            Self::Status => "Status",
            Self::Pid => "PID",
        }
    }

    /// Which way this column sorts when it is first clicked.
    ///
    /// None of these is a magnitude worth finding the biggest of, unlike
    /// the process table's CPU or memory — so every column opens
    /// ascending, the same as [`crate::model::sort::SortKey`]'s own
    /// non-magnitude columns.
    #[must_use]
    pub const fn defaults_descending(self) -> bool {
        false
    }

    /// Compares two services by this column, breaking a tie on the short
    /// name — the one field the SCM guarantees is unique, so the table
    /// has a determined order rather than reshuffling on every refresh.
    /// See [`crate::model::sort::SortKey::compare`] for why a tie-break
    /// matters at all.
    #[must_use]
    pub fn compare_directed(
        self,
        a: &crate::win::services::Service,
        b: &crate::win::services::Service,
        descending: bool,
    ) -> std::cmp::Ordering {
        let primary = match self {
            Self::Name => compare_text(service_label(a), service_label(b)),
            Self::ShortName => compare_text(&a.name, &b.name),
            Self::Status => a.state.cmp(&b.state),
            Self::Pid => a.pid.cmp(&b.pid),
        };
        let primary = if descending {
            primary.reverse()
        } else {
            primary
        };
        primary.then_with(|| compare_text(&a.name, &b.name))
    }
}

/// A service's Name-column text: the display name, falling back to the
/// short one for the services that do not register a friendly name.
#[must_use]
pub fn service_label(service: &crate::win::services::Service) -> &str {
    if service.display_name.is_empty() {
        &service.name
    } else {
        &service.display_name
    }
}

/// The Startup view's own state.
#[derive(Default)]
pub struct StartupView {
    /// The entries read at the last refresh.
    pub entries: Vec<crate::win::startup::StartupEntry>,
    /// The search box's text.
    pub search: String,
    /// The selected entry's name.
    pub selected: Option<String>,
    /// The column being sorted on.
    pub sort: StartupSortKey,
    /// Whether that sort is descending.
    pub descending: bool,
    /// When the list was last read.
    pub refreshed: Option<std::time::Instant>,
    /// A background read of the startup list, if one is in flight. See
    /// [`ServicesView::pending`].
    pub pending: Option<background::BackgroundRead<Vec<crate::win::startup::StartupEntry>>>,
    /// See [`ServicesView::visible_key`].
    pub visible_key: Option<(String, StartupSortKey, bool, Option<std::time::Instant>)>,
    /// See [`ServicesView::visible`].
    pub visible: Vec<crate::win::startup::StartupEntry>,
}

/// A column the Startup table can sort by.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum StartupSortKey {
    /// The registered name.
    #[default]
    Name,
    /// Enabled or disabled.
    Status,
    /// Which registry location or startup folder it came from.
    Location,
}

impl StartupSortKey {
    /// The column heading.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Status => "Status",
            Self::Location => "Location",
        }
    }

    /// Which way this column sorts when it is first clicked.
    ///
    /// See [`ServiceSortKey::defaults_descending`] — the same reasoning
    /// applies: nothing here is a magnitude.
    #[must_use]
    pub const fn defaults_descending(self) -> bool {
        false
    }

    /// Compares two entries by this column.
    ///
    /// The tie-break is `(name, location)` rather than the name alone: an
    /// entry can legitimately be registered under the same name in more
    /// than one location — a per-user copy and an all-users one — so the
    /// name by itself is not always unique.
    #[must_use]
    pub fn compare_directed(
        self,
        a: &crate::win::startup::StartupEntry,
        b: &crate::win::startup::StartupEntry,
        descending: bool,
    ) -> std::cmp::Ordering {
        let primary = match self {
            Self::Name => compare_text(&a.name, &b.name),
            Self::Status => a.enabled.cmp(&b.enabled),
            Self::Location => a.location.cmp(b.location),
        };
        let primary = if descending {
            primary.reverse()
        } else {
            primary
        };
        primary
            .then_with(|| compare_text(&a.name, &b.name).then_with(|| a.location.cmp(b.location)))
    }
}

/// A message shown in the status bar for a few seconds.
///
/// Actions report through this rather than through a modal: "Ended
/// chrome.exe" does not warrant a dialog with a button, and a dialog per
/// action makes ending five processes into ten clicks.
pub struct Toast {
    /// What happened.
    pub message: String,
    /// Whether it was a failure, which decides the colour.
    pub failed: bool,
    /// When it was raised.
    pub raised: std::time::Instant,
}

/// How long a toast stays on screen.
pub const TOAST_SECONDS: f32 = 5.0;

/// Whether monitoring data is arriving with believable freshness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SamplerHealth {
    /// Thread has started but no first value is due yet.
    Starting,
    /// A snapshot arrived within the freshness window.
    Live,
    /// Thread exists but sequence progress is overdue.
    Stale,
    /// Thread exited or never started.
    Stopped,
}

fn sampler_health(
    running: bool,
    since_last: Option<std::time::Duration>,
    since_start: std::time::Duration,
    interval: std::time::Duration,
) -> SamplerHealth {
    if !running {
        return SamplerHealth::Stopped;
    }
    let deadline = interval
        .saturating_mul(3)
        .max(std::time::Duration::from_secs(3));
    match since_last {
        Some(age) if age > deadline => SamplerHealth::Stale,
        Some(_) => SamplerHealth::Live,
        None if since_start > deadline => SamplerHealth::Stale,
        None => SamplerHealth::Starting,
    }
}

/// A confirmation the user has to answer before something destructive
/// happens.
pub enum Pending {
    /// End one process.
    EndTask(ProcessKey, String),
    /// End a process and everything under it.
    EndTree(ProcessKey, String, usize),
    /// Set a process to realtime priority, which can make a machine stop
    /// answering the keyboard. See [`crate::model::Priority::is_dangerous`].
    Realtime(ProcessKey, String),
    /// Stop a service.
    StopService(String, String),
}

/// Everything the window knows.
pub struct App {
    /// The sampler.
    pub engine: Engine,
    /// The most recent snapshot, or `None` before the first arrives.
    pub snapshot: Option<std::sync::Arc<Snapshot>>,
    /// Every available theme.
    pub catalog: Catalog,
    /// The theme in force.
    pub theme: Palette,
    /// Persisted preferences, saved on exit.
    pub config: Config,
    /// Which view is on screen.
    pub view: View,
    /// The process list's state.
    pub processes: ProcessView,
    /// The Details view's state.
    pub details: DetailsView,
    /// The Performance view's history.
    pub performance: PerformanceView,
    /// The Services view's state.
    pub services: ServicesView,
    /// The Startup view's state.
    pub startup: StartupView,
    /// The Memory view's state.
    pub memory_view: MemoryView,
    /// A confirmation awaiting an answer, if any.
    pub pending: Option<Pending>,
    /// Whether the about panel is open.
    pub about: bool,
    /// The most recent status message.
    pub toast: Option<Toast>,
    /// Whether the window draws its own title bar.
    pub custom_chrome: bool,
    /// Whether the window is maximised, tracked so the title bar's own
    /// button can show the right glyph.
    pub maximised: bool,
    /// The border colour last handed to DWM, so a theme change can be
    /// noticed.
    ///
    /// The Windows 11 border is set once when the window is created. It
    /// is not a property the window keeps in step with anything — DWM
    /// holds whatever colour it was last given — so switching theme left
    /// a dark border framing a light window until the app was restarted.
    /// Only a *change* re-applies it; this is a system call and it has no
    /// business running on a frame where nothing about the theme moved.
    dressed_border: Option<crate::color::Rgb>,
    /// Numeric Win32 window handle, attached by the native launcher.
    pub(crate) native_window: Option<isize>,
    /// Whether `SeDebugPrivilege` was granted, which the Settings view
    /// reports — it is the difference between a full process list and one
    /// missing half its identity columns.
    pub elevated: bool,
    /// When the last snapshot reached the UI, for stale-data detection.
    pub last_snapshot_at: Option<std::time::Instant>,
    /// When monitoring started, so a missing first sample can go stale.
    pub engine_started_at: std::time::Instant,
    /// Efficiency-mode changes this app has made and the sampler has not
    /// caught up with yet.
    ///
    /// The sampler reads quality of service on a rolling sweep rather
    /// than for every row every second, so a process's state can be a
    /// few seconds stale — which is fine for a flag somebody else
    /// changed and not fine at all for the one the user just clicked.
    /// An entry here says "this is what it is now", and it is dropped
    /// the moment a snapshot agrees.
    ///
    /// Not a cache and not a source of truth: it never invents a state
    /// for a process nobody touched, so the worst it can be wrong about
    /// is a toggle that failed silently between the sweep's passes.
    pub efficiency_overrides: HashMap<ProcessKey, bool>,
}

impl App {
    /// Builds the window's state and starts the sampler.
    #[must_use]
    pub fn new(config: Config) -> Self {
        let catalog = Catalog::load();
        let theme = catalog.get(config.theme.as_deref()).clone();
        // Token privilege is process-wide. Ask once and pass the result
        // into the sampler rather than adjusting it again on that thread.
        let elevated = crate::win::privilege::enable();
        let engine = Engine::start(config.interval(), elevated);

        let mut processes = ProcessView::default();
        if let Some(sort) = config.sort {
            processes.sort = sort;
            processes.descending = config
                .sort_descending
                .unwrap_or_else(|| sort.defaults_descending());
        }
        if let Some(grouped) = config.grouped {
            processes.grouped = grouped;
        }
        if let Some(saved) = config.columns.as_deref() {
            // Reconciled, never trusted. A saved order written by an
            // older build does not mention a column added since, and a
            // table that used the list as it stands would silently draw
            // one fewer column than this build has — which reads as the
            // feature never having shipped.
            processes.columns =
                ColumnOrder::reconcile(saved, &crate::gui::ui::processes::DEFAULT_COLUMNS);
        }

        Self {
            engine,
            snapshot: None,
            theme,
            catalog,
            view: config
                .view
                .as_deref()
                .map_or(View::default(), View::from_id),
            processes,
            details: DetailsView::default(),
            performance: PerformanceView::default(),
            services: ServicesView::default(),
            startup: StartupView::default(),
            memory_view: MemoryView::default(),
            pending: None,
            about: false,
            toast: None,
            // Custom chrome by default: the system title bar on Windows 10
            // is a light grey caption that no amount of theming can reach
            // (DWM's dark-mode attribute only darkens it, and only from
            // 1809). Drawing our own is the difference between a window
            // that looks designed and one that looks like a dark app
            // wearing someone else's hat.
            custom_chrome: config.custom_chrome.unwrap_or(true),
            maximised: false,
            dressed_border: None,
            native_window: None,
            elevated,
            last_snapshot_at: None,
            engine_started_at: std::time::Instant::now(),
            efficiency_overrides: HashMap::new(),
            config,
        }
    }

    /// A process's efficiency mode, with any change this app has made
    /// since the last sweep folded in.
    ///
    /// Every read of the flag goes through here rather than at the
    /// row — the context menu's tick, the row's mark and the details
    /// pane all have to agree, and three call sites reaching into the
    /// override map by hand is how two of them end up disagreeing.
    #[must_use]
    pub fn efficiency_of(&self, row: &crate::model::ProcessRow) -> crate::model::Efficiency {
        match self.efficiency_overrides.get(&row.key()) {
            Some(true) => crate::model::Efficiency::Reduced,
            Some(false) => crate::model::Efficiency::Standard,
            None => row.efficiency,
        }
    }

    /// Drops overrides the sampler has caught up with, and any whose
    /// process has exited.
    ///
    /// Called once per snapshot rather than per frame: an override that
    /// outlives its process is a leak, and one that outlives the sweep
    /// that confirmed it would pin the row to a state it no longer has
    /// if something else changed it.
    fn settle_efficiency_overrides(&mut self, snapshot: &Snapshot) {
        if self.efficiency_overrides.is_empty() {
            return;
        }
        let observed: HashMap<ProcessKey, crate::model::Efficiency> = snapshot
            .processes
            .iter()
            .map(|row| (row.key(), row.efficiency))
            .collect();
        self.efficiency_overrides.retain(|key, wanted| {
            match observed.get(key) {
                // Gone: the process exited.
                None => false,
                // The sweep has not reached it since the change.
                Some(crate::model::Efficiency::Unknown) => true,
                Some(seen) => seen.is_reduced() != *wanted,
            }
        });
    }

    /// Takes any new snapshot and folds it into the history rings.
    ///
    /// Called once per frame, before drawing.
    pub fn poll(&mut self) {
        let Some(snapshot) = self.engine.latest() else {
            return;
        };
        self.record_history(&snapshot);
        self.settle_efficiency_overrides(&snapshot);
        self.snapshot = Some(snapshot);
        self.last_snapshot_at = Some(std::time::Instant::now());
    }

    /// Appends one snapshot's figures to the graphs.
    fn record_history(&mut self, snapshot: &Snapshot) {
        let performance = &mut self.performance;
        performance
            .cpu
            .push(snapshot.system.cpu.total_percent as f32);
        performance
            .cpu_kernel
            .push(snapshot.system.cpu.kernel_percent as f32);

        // The core count can change — a VM with hot-add, or a machine
        // whose parked cores come back. Resizing rather than rebuilding
        // keeps the history of the cores that were already there.
        let cores = snapshot.system.cpu.per_core.len();
        if performance.cores.len() != cores {
            performance
                .cores
                .resize_with(cores, || Series::new(HISTORY));
        }
        for (series, value) in performance
            .cores
            .iter_mut()
            .zip(snapshot.system.cpu.per_core.iter())
        {
            series.push(*value as f32);
        }

        performance
            .memory
            .push(snapshot.system.memory.used_percent() as f32);
        let (read, write) = snapshot
            .system
            .disks
            .iter()
            .fold((0.0, 0.0), |(read, write), disk| {
                (read + disk.read_rate, write + disk.write_rate)
            });
        performance.disk.push((read + write) as f32);
        performance.disk_read.push(read as f32);
        performance.disk_write.push(write as f32);

        let network = snapshot.system.network_rate();
        let sent = snapshot.system.network_send_rate();
        performance.network.push(network as f32);
        performance.network_send.push(sent as f32);
        // Clamped at zero rather than trusted: see the field's docs.
        performance
            .network_receive
            .push((network - sent).max(0.0) as f32);

        // One ring per adapter, so the Network panel can graph a single
        // adapter rather than only the machine's total — and so a row
        // can carry its own sparkline, which is what makes twenty rows
        // scannable rather than twenty numbers to read.
        //
        // Rings for adapters that are gone are dropped rather than kept
        // against a possible return: an adapter that comes back gets a
        // fresh LUID from the driver stack, so a kept ring is a ring
        // nothing will ever claim.
        performance
            .adapters
            .retain(|luid, _| snapshot.system.adapters.iter().any(|a| a.luid == *luid));
        for adapter in &snapshot.system.adapters {
            performance
                .adapters
                .entry(adapter.luid)
                .or_insert_with(|| Series::new(HISTORY))
                .push(adapter.total_rate() as f32);
        }

        performance.gpu.push(
            snapshot
                .system
                .gpus
                .iter()
                .map(|gpu| gpu.utilisation)
                .fold(0.0f64, f64::max) as f32,
        );
    }

    /// Ensures every history ring exists at the right capacity.
    ///
    /// Called when the sampling interval changes: the samples in a ring
    /// were taken at the old spacing, so a graph drawn from both would
    /// silently compress part of its own history without saying so.
    pub fn reset_history(&mut self) {
        let performance = &mut self.performance;
        for series in [
            &mut performance.cpu,
            &mut performance.cpu_kernel,
            &mut performance.memory,
            &mut performance.disk,
            &mut performance.network,
            &mut performance.network_send,
            &mut performance.gpu,
        ] {
            series.clear();
        }
        for series in &mut performance.cores {
            series.clear();
        }
        for series in performance.adapters.values_mut() {
            series.clear();
        }
    }

    /// Raises a status message.
    pub fn notify(&mut self, message: impl Into<String>, failed: bool) {
        self.toast = Some(Toast {
            message: message.into(),
            failed,
            raised: std::time::Instant::now(),
        });
    }

    /// The selected process's row in the current snapshot, if it is still
    /// running.
    ///
    /// Resolved fresh each frame rather than being held as a reference:
    /// the snapshot is replaced wholesale every interval, and a selection
    /// that outlived its process must resolve to `None` rather than to
    /// whatever now sits at that index.
    #[must_use]
    pub fn selected_row(&self) -> Option<&crate::model::ProcessRow> {
        let key = self.selected_key()?;
        self.snapshot
            .as_ref()?
            .processes
            .iter()
            .find(|row| row.key() == key)
    }

    /// Whether the Details view is the one being drawn.
    ///
    /// The Details view keeps its own sort, filter, selection and row
    /// cache; every other view either shares the process list's or has
    /// none. One method decides that, rather than a `match` repeated at
    /// six call sites where a new view could be added to five of them.
    #[must_use]
    pub fn on_details(&self) -> bool {
        matches!(self.view, View::Details)
    }

    /// The selected process, in whichever view owns a selection.
    #[must_use]
    pub fn selected_key(&self) -> Option<ProcessKey> {
        if self.on_details() {
            self.details.selected
        } else {
            self.processes.selected
        }
    }

    /// The row cache the current view is drawing from.
    #[must_use]
    pub fn active_rows(&self) -> &rows::Cache {
        if self.on_details() {
            &self.details.rows
        } else {
            &self.processes.rows
        }
    }

    /// Collects the current state into a [`Config`] for saving.
    #[must_use]
    pub fn to_config(&self) -> Config {
        Config {
            theme: Some(self.theme.id.clone()),
            view: Some(self.view.id().to_string()),
            interval_ms: Some(
                u64::try_from(self.engine.interval().as_millis())
                    .unwrap_or(crate::config::DEFAULT_INTERVAL_MS),
            ),
            sort: Some(self.processes.sort),
            sort_descending: Some(self.processes.descending),
            grouped: Some(self.processes.grouped),
            columns: Some(self.processes.columns.as_slice().to_vec()),
            custom_chrome: Some(self.custom_chrome),
            ..self.config.clone()
        }
    }

    /// Whether the process list currently has a filter.
    #[must_use]
    pub fn is_filtering(&self) -> bool {
        if self.on_details() {
            !self.details.query.is_empty()
        } else {
            !self.processes.query.is_empty()
        }
    }

    /// The rows the process list is currently drawing.
    #[must_use]
    pub fn visible_entries(&self) -> &[Entry] {
        self.active_rows().entries()
    }

    /// Current sampler state, based on thread state and data freshness.
    #[must_use]
    pub fn sampler_health(&self) -> SamplerHealth {
        sampler_health(
            self.engine.is_running(),
            self.last_snapshot_at.map(|at| at.elapsed()),
            self.engine_started_at.elapsed(),
            self.engine.interval(),
        )
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let viewport = ui.ctx().input(|input| input.viewport().clone());
        if let Some(maximised) = viewport.maximized {
            self.maximised = maximised;
        }
        if let Some(size) = restored_window_size(viewport.inner_rect, self.maximised) {
            self.config.window_size = Some(size);
        }
        // Native move/resize enters Windows' own modal interaction loop, so
        // egui is not guaranteed to observe the pointer-up event. Checking on
        // every ordinary frame is cheap (read-only unless correction is
        // needed) and repairs the window immediately after that loop returns.
        if !self.maximised {
            if let Some(window) = self.native_window {
                let _ = crate::win::window::fit_to_work_area(window);
            }
        }
        // The Windows 11 border follows the theme, which it cannot do on
        // its own: DWM keeps whatever colour it was last given, so a
        // theme switched at runtime left the old border framing the new
        // window until a restart. Gated on the colour actually changing —
        // this is a system call, and a frame where the theme did not move
        // has no business making one.
        if self.dressed_border != Some(self.theme.border) {
            super::dress_window_for_windows_11(self.native_window, &self.theme, self.custom_chrome);
            self.dressed_border = Some(self.theme.border);
        }

        self.poll();
        super::ui::draw(self, ui);

        // Repaint on the sampler's schedule rather than continuously.
        // egui redraws on demand; without this the window would go still
        // between input events and the graphs would stop moving — and
        // with a naive `request_repaint()` it would redraw at the
        // monitor's refresh rate, burning a core to animate data that
        // changes once a second.
        //
        // egui schedules its own animation frames. This wake exists only
        // to poll the sampler, so it follows the sampling interval rather
        // than forcing a permanent 20 Hz idle loop.
        ui.ctx().request_repaint_after(self.engine.interval());
    }

    fn on_exit(&mut self) {
        // Saved on a normal quit. A failure here is reported nowhere —
        // the window is already closing — but it is also the one moment
        // where there is nothing useful to do about it.
        let _ = crate::config::save(&self.to_config());
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // The window's own background, painted before egui draws
        // anything. Left as the theme's app colour so a frame that has
        // not finished laying out does not flash white — which is
        // exactly what happens during a resize, and is very visible on a
        // dark theme.
        let color = super::ui::theme::rgb(self.theme.app);
        egui::Rgba::from(color).to_array()
    }
}

fn restored_window_size(rect: Option<egui::Rect>, maximised: bool) -> Option<[f32; 2]> {
    let size = rect?.size();
    (!maximised && size.x.is_finite() && size.y.is_finite() && size.x > 0.0 && size.y > 0.0)
        .then_some([size.x, size.y])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampler_health_is_based_on_freshness_not_only_a_thread_handle() {
        let interval = std::time::Duration::from_secs(1);
        assert_eq!(
            sampler_health(true, None, std::time::Duration::from_secs(1), interval),
            SamplerHealth::Starting
        );
        assert_eq!(
            sampler_health(
                true,
                Some(std::time::Duration::from_secs(5)),
                interval,
                interval
            ),
            SamplerHealth::Stale
        );
        assert_eq!(
            sampler_health(false, None, std::time::Duration::ZERO, interval),
            SamplerHealth::Stopped
        );
    }

    #[test]
    fn only_a_restored_window_size_is_persisted() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1234.0, 777.0));
        assert_eq!(
            restored_window_size(Some(rect), false),
            Some([1234.0, 777.0])
        );
        assert_eq!(restored_window_size(Some(rect), true), None);
    }

    #[test]
    fn every_view_has_a_label_and_a_stable_id() {
        for view in View::ALL {
            assert!(!view.label().is_empty(), "{view:?}");
            assert!(!view.id().is_empty(), "{view:?}");
            assert!(
                view.id().chars().all(|c| c.is_ascii_lowercase()),
                "{view:?}: a persisted id should be a plain lowercase token, \
                 got {:?}",
                view.id()
            );
        }
    }

    #[test]
    fn view_ids_are_unique_and_round_trip() {
        // A duplicate id would make one view unreachable from a saved
        // config, silently.
        let mut ids: Vec<&str> = View::ALL.iter().map(|view| view.id()).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "two views share an id");

        for view in View::ALL {
            assert_eq!(View::from_id(view.id()), view);
        }
    }

    #[test]
    fn an_unknown_view_id_falls_back_to_the_default() {
        // Views come and go between releases; a saved id that no longer
        // exists should cost the view, not the config.
        assert_eq!(View::from_id("no-such-view"), View::default());
        assert_eq!(View::from_id(""), View::default());
    }

    #[test]
    fn service_label_falls_back_to_the_short_name() {
        let named = crate::win::services::Service {
            name: "Spooler".to_string(),
            display_name: "Print Spooler".to_string(),
            ..Default::default()
        };
        let unnamed = crate::win::services::Service {
            name: "Spooler".to_string(),
            display_name: String::new(),
            ..Default::default()
        };
        assert_eq!(service_label(&named), "Print Spooler");
        assert_eq!(
            service_label(&unnamed),
            "Spooler",
            "a service with no display name should fall back to the short one"
        );
    }

    #[test]
    fn services_tied_on_the_sorted_column_break_on_the_short_name() {
        // Two stopped services tie on Status; the SCM's short name is the
        // one field guaranteed unique, so it has to be what settles the
        // order, or the table's order would depend on enumeration order.
        let a = crate::win::services::Service {
            name: "b".to_string(),
            ..Default::default()
        };
        let b = crate::win::services::Service {
            name: "a".to_string(),
            ..Default::default()
        };
        assert_eq!(
            ServiceSortKey::Status.compare_directed(&a, &b, false),
            std::cmp::Ordering::Greater,
            "tied on status, the short name should place \"a\" before \"b\""
        );
    }

    #[test]
    fn startup_entries_tied_on_name_break_on_location() {
        // A machine can register the same name under both a per-user and
        // an all-users location, so the name alone is not always unique.
        let a = crate::win::startup::StartupEntry {
            name: "X".to_string(),
            location: "HKCU Run",
            ..Default::default()
        };
        let b = crate::win::startup::StartupEntry {
            name: "X".to_string(),
            location: "HKLM Run",
            ..Default::default()
        };
        assert_eq!(
            StartupSortKey::Status.compare_directed(&a, &b, false),
            std::cmp::Ordering::Less,
            "tied on name and status, the location should break the tie"
        );
    }

    #[test]
    fn the_process_list_opens_on_the_question_it_is_opened_for() {
        // "What is making this machine slow" is nine of ten reasons
        // anyone opens a task manager.
        let view = ProcessView::default();
        assert_eq!(view.sort, SortKey::Cpu);
        assert!(view.descending);
        assert!(view.grouped, "the tree is the useful default");
    }

    #[test]
    fn a_view_that_was_saved_is_restored() {
        let config = Config {
            view: Some("performance".to_string()),
            sort: Some(SortKey::Memory),
            sort_descending: Some(false),
            grouped: Some(false),
            ..Config::default()
        };
        let app = App::new(config);
        assert_eq!(app.view, View::Performance);
        assert_eq!(app.processes.sort, SortKey::Memory);
        assert!(!app.processes.descending);
        assert!(!app.processes.grouped);
    }

    #[test]
    fn the_saved_config_round_trips_through_the_app() {
        let app = App::new(Config::default());
        let saved = app.to_config();
        let restored = App::new(saved.clone());
        assert_eq!(restored.to_config().view, saved.view);
        assert_eq!(restored.to_config().sort, saved.sort);
        assert_eq!(restored.to_config().theme, saved.theme);
    }

    #[test]
    fn a_selection_that_outlived_its_process_resolves_to_nothing() {
        // The snapshot is replaced wholesale every interval; a stale
        // selection must not resolve to whatever now sits at that index.
        let mut app = App::new(Config::default());
        app.processes.selected = Some(ProcessKey {
            pid: 999_999,
            started_at: 1,
        });
        app.snapshot = Some(std::sync::Arc::new(Snapshot::default()));
        assert!(app.selected_row().is_none());
    }

    #[test]
    fn history_rings_follow_a_changing_core_count() {
        // A VM with CPU hot-add, or a machine whose parked cores come
        // back. Rebuilding rather than resizing would throw away the
        // history of the cores that were already there.
        let mut app = App::new(Config::default());
        let mut snapshot = Snapshot::default();
        snapshot.system.cpu.per_core = vec![10.0, 20.0];
        app.record_history(&snapshot);
        assert_eq!(app.performance.cores.len(), 2);

        snapshot.system.cpu.per_core = vec![10.0, 20.0, 30.0, 40.0];
        app.record_history(&snapshot);
        assert_eq!(app.performance.cores.len(), 4);
        assert_eq!(
            app.performance.cores.first().map(Series::len),
            Some(2),
            "an existing core keeps its history across the resize"
        );
    }

    #[test]
    fn resetting_the_history_clears_every_ring() {
        // Called when the interval changes: the old samples were taken at
        // a different spacing, so a graph drawn from both would compress
        // part of its own history without saying so.
        let mut app = App::new(Config::default());
        let mut snapshot = Snapshot::default();
        snapshot.system.cpu.per_core = vec![50.0];
        snapshot.system.cpu.total_percent = 50.0;
        app.record_history(&snapshot);
        assert!(!app.performance.cpu.is_empty());

        app.reset_history();
        assert!(app.performance.cpu.is_empty());
        assert!(app.performance.cores.iter().all(Series::is_empty));
    }

    #[test]
    fn a_notification_records_whether_it_was_a_failure() {
        let mut app = App::new(Config::default());
        assert!(app.toast.is_none());
        app.notify("Ended chrome.exe", false);
        assert!(app.toast.as_ref().is_some_and(|toast| !toast.failed));
        app.notify("Access denied", true);
        assert!(app.toast.as_ref().is_some_and(|toast| toast.failed));
    }

    #[test]
    fn custom_chrome_is_the_default_but_can_be_turned_off() {
        // The Windows 10 system caption is a light grey bar no amount of
        // theming reaches, which is why this defaults on.
        assert!(App::new(Config::default()).custom_chrome);
        let native = App::new(Config {
            custom_chrome: Some(false),
            ..Config::default()
        });
        assert!(!native.custom_chrome);
    }

    #[test]
    fn the_history_window_is_long_enough_to_be_useful() {
        // Four minutes at the default interval: long enough to watch a
        // build start and finish.
        let minutes = HISTORY as f64 * crate::config::DEFAULT_INTERVAL_MS as f64 / 60_000.0;
        assert!(
            minutes >= 3.0,
            "a {minutes}-minute history is too short to see anything develop"
        );
    }
}
