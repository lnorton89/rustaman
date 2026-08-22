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
use std::collections::HashSet;

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
    /// A flat, technical table of every process.
    Details,
    /// Windows services.
    Services,
    /// Programs that run at logon.
    Startup,
    /// Theme, interval, and the about panel.
    Settings,
}

impl View {
    /// Every view, in the order the navigation rail lists them.
    pub const ALL: [Self; 6] = [
        Self::Processes,
        Self::Performance,
        Self::Details,
        Self::Services,
        Self::Startup,
        Self::Settings,
    ];

    /// The rail's label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Processes => "Processes",
            Self::Performance => "Performance",
            Self::Details => "Details",
            Self::Services => "Services",
            Self::Startup => "Startup",
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
            Self::Details => Icon::Details,
            Self::Services => Icon::Services,
            Self::Startup => Icon::Startup,
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
            Self::Details => "details",
            Self::Services => "services",
            Self::Startup => "startup",
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
    pub disk: Series,
    /// Combined network throughput, in bytes per second.
    pub network: Series,
    /// Busiest GPU engine, as a percentage.
    pub gpu: Series,
    /// Which sub-panel the Performance view has selected.
    pub focus: PerformanceFocus,
    /// Whether the Network panel's idle-adapter list is expanded.
    ///
    /// Collapsed by default: a dev machine running Hyper-V, WSL or a VPN
    /// client reports a couple of dozen throughput-less adapters, and
    /// opening the page to all of them drawn out is the thing this field
    /// exists to avoid.
    pub network_idle_expanded: bool,
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
            network: Series::new(HISTORY),
            gpu: Series::new(HISTORY),
            focus: PerformanceFocus::default(),
            network_idle_expanded: false,
        }
    }
}

/// Which resource the Performance view is showing in detail.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
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
    pub snapshot: Option<Snapshot>,
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
    /// Whether `SeDebugPrivilege` was granted, which the Settings view
    /// reports — it is the difference between a full process list and one
    /// missing half its identity columns.
    pub elevated: bool,
}

impl App {
    /// Builds the window's state and starts the sampler.
    #[must_use]
    pub fn new(config: Config) -> Self {
        let catalog = Catalog::load();
        let theme = catalog.get(config.theme.as_deref()).clone();
        let engine = Engine::start(config.interval());

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
            elevated: false,
            config,
        }
    }

    /// Takes any new snapshot and folds it into the history rings.
    ///
    /// Called once per frame, before drawing.
    pub fn poll(&mut self) {
        let Some(snapshot) = self.engine.latest() else {
            return;
        };
        self.record_history(&snapshot);
        self.snapshot = Some(snapshot);
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
        performance.disk.push(
            snapshot
                .system
                .disks
                .iter()
                .map(crate::model::DiskSample::total_rate)
                .sum::<f64>() as f32,
        );
        performance.network.push(
            snapshot
                .system
                .adapters
                .iter()
                .map(crate::model::AdapterSample::total_rate)
                .sum::<f64>() as f32,
        );
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
            &mut performance.gpu,
        ] {
            series.clear();
        }
        for series in &mut performance.cores {
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
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        super::ui::draw(self, ui);

        // Repaint on the sampler's schedule rather than continuously.
        // egui redraws on demand; without this the window would go still
        // between input events and the graphs would stop moving — and
        // with a naive `request_repaint()` it would redraw at the
        // monitor's refresh rate, burning a core to animate data that
        // changes once a second.
        //
        // A shorter tick than the interval so a hover animation, which
        // runs at HOVER_SECONDS, is not quantised to the sample rate.
        let tick = self
            .engine
            .interval()
            .min(std::time::Duration::from_millis(50));
        ui.ctx().request_repaint_after(tick);
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

#[cfg(test)]
mod tests {
    use super::*;

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
        app.snapshot = Some(Snapshot::default());
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
