// ============================================================================
// Module:       screenshot (example)
// Description:  Renders any view of the app to a PNG, headlessly, against a
//               fabricated machine — the harness for looking at the UI.
//
// Dependencies: egui_kittest (wgpu), image, clap, anyhow; rustaman::gui
// ============================================================================

//! Draws the app and writes the result to a file.
//!
//! ```text
//! cargo run --example screenshot -- --scene network
//! cargo run --example screenshot -- --list
//! cargo run --example screenshot -- --scene network --size 900x700 --theme light
//! ```
//!
//! ## Why this exists
//!
//! Every visual rule this codebase enforces — the margins, the theme
//! contrast, the icon geometry — is checked by a test that cannot see the
//! window. A test asserts that a rect fits inside another rect; it cannot
//! tell you that a panel is a wall of near-identical boxes, that a
//! column is unreadable, or that a list is reordering itself. Those need
//! a picture, and getting one used to mean building the app, launching
//! it, finding the panel, and having whatever the machine happened to be
//! doing at that moment be the test case.
//!
//! That last part is the real problem. The UI has to handle a machine
//! with twenty network adapters, a disconnected NIC, sixty-four cores and
//! no GPU counters — and the machine in front of you has one of those
//! configurations, permanently. So this renders against a **fabricated**
//! snapshot: every scene below is a machine that would otherwise have to
//! be found rather than described.
//!
//! ## How it renders without a window
//!
//! `egui_kittest` drives the same `gui::ui::draw` the real app does,
//! against an offscreen wgpu target. There is no window, no event loop
//! and no sampler — the app's snapshot is assigned rather than sampled —
//! so it runs from a terminal, over a remote session, and in CI.
//!
//! What it therefore does **not** prove: that the real window opens, that
//! the sampler produces these numbers, or that the compositor draws what
//! wgpu handed it. It renders the same tessellation the app does, which
//! covers everything about layout, spacing, colour and text, and nothing
//! about the machine.

#[cfg(not(windows))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!(
        "the window is Windows-only — `src/gui` compiles to nothing on \
         this platform, so there is nothing here to draw"
    )
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    windows::main()
}

#[cfg(windows)]
mod windows {
    use anyhow::{Context, Result};
    use rustaman::gui::app::{App, PerformanceFocus, View};
    use rustaman::model::history::Series;
    use rustaman::model::{
        AdapterKind, AdapterSample, AdapterState, CoreKind, CpuSample, DiskSample, Efficiency,
        GpuSample, MemorySample, Snapshot, SystemSample,
    };
    use std::path::PathBuf;

    /// Where a scene lands when `--out` is not given.
    ///
    /// Under `target/` rather than in the repository: these are build
    /// output, they are regenerated on demand, and a directory of stale
    /// PNGs is the thing that makes someone trust a picture that is three
    /// changes out of date.
    const DEFAULT_DIRECTORY: &str = "target/screenshots";

    /// The window size a scene is drawn at unless `--size` says otherwise.
    ///
    /// Taken from `gui::DEFAULT_SIZE` rather than written down again.
    /// This used to say `(1440.0, 900.0)` on the belief that it was the
    /// app's default — it is not, and never was: 1440x900 appears in
    /// `config.rs` only inside a round-trip *test fixture*. Every
    /// screenshot taken against it was 260 points wider than the window
    /// the app actually opens, which is exactly the direction that hides
    /// a crowding bug rather than showing one.
    const DEFAULT_SIZE: (f32, f32) = (
        rustaman::gui::DEFAULT_SIZE[0],
        rustaman::gui::DEFAULT_SIZE[1],
    );

    #[derive(clap::Parser, Debug)]
    #[command(name = "screenshot", about = "Render a view of the app to a PNG")]
    struct Cli {
        /// Which scene to draw. See `--list`.
        #[arg(long, default_value = "network")]
        scene: String,

        /// List the scenes and exit.
        #[arg(long)]
        list: bool,

        /// Draw every scene.
        #[arg(long)]
        all: bool,

        /// Where to write the PNG. Defaults to
        /// `target/screenshots/<scene>.png`.
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,

        /// Window size, as `WIDTHxHEIGHT`.
        #[arg(long, value_name = "WxH")]
        size: Option<String>,

        /// Theme id, e.g. `light`. Defaults to the app's own default.
        #[arg(long, value_name = "ID")]
        theme: Option<String>,
    }

    /// One thing worth looking at: a view, a panel within it, and the
    /// machine it is drawn against.
    struct Scene {
        /// The name `--scene` takes.
        name: &'static str,
        /// What it is for, in `--list`.
        about: &'static str,
        /// The view to open.
        view: View,
        /// The Performance panel to select, where the view is that one.
        focus: PerformanceFocus,
        /// Whether the Network panel's virtual-adapter group starts open.
        expanded: bool,
        /// The size to draw at, overriding [`DEFAULT_SIZE`].
        size: Option<(f32, f32)>,
        /// Whether to draw this machine instead of the fabricated one.
        live: bool,
        /// Whether to raise the end-task confirmation over the view.
        modal: bool,
        /// Where to park the pointer, in window points. Hover is a third
        /// of a row's visual states and the one no static screenshot
        /// shows — the app drew it in two different colours either side
        /// of the first column for as long as nobody could see it.
        hover: Option<(f32, f32)>,
        /// Whether to select a row — the third of the app's three row
        /// states, after resting and hovered, and the one that opens the
        /// Details view's inspector.
        select: bool,
    }

    /// Every scene, in the order `--list` prints them.
    const SCENES: [Scene; 27] = [
        Scene {
            name: "live-system",
            about: "System Information on THIS machine, really sampled",
            view: View::System,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: true,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "live-network",
            about: "Performance › Network on THIS machine, really sampled",
            view: View::Performance,
            focus: PerformanceFocus::Network,
            expanded: true,
            size: Some((1280.0, 1500.0)),
            live: true,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "live-memory",
            about: "The Memory view on THIS machine, really sampled",
            view: View::Memory,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: true,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "live-processes",
            about: "The process tree on THIS machine, really sampled",
            view: View::Processes,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: true,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "network",
            about: "Performance › Network on a machine with 21 adapters",
            view: View::Performance,
            focus: PerformanceFocus::Network,
            expanded: false,
            size: None,
            live: false,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "network-open",
            about: "The same, with the virtual-adapter group expanded",
            view: View::Performance,
            focus: PerformanceFocus::Network,
            expanded: true,
            size: None,
            live: false,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "network-narrow",
            about: "The Network panel at the width where rows drop their sparkline",
            view: View::Performance,
            focus: PerformanceFocus::Network,
            expanded: false,
            size: Some((900.0, 720.0)),
            live: false,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "cpu",
            about: "Performance › CPU, sixteen cores",
            view: View::Performance,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "perf-memory",
            about: "Performance › Memory — the machine's own totals",
            view: View::Performance,
            focus: PerformanceFocus::Memory,
            expanded: false,
            size: None,
            live: false,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "disk",
            about: "Performance › Disk, two drives",
            view: View::Performance,
            focus: PerformanceFocus::Disk,
            expanded: false,
            size: None,
            live: false,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "gpu",
            about: "Performance › GPU",
            view: View::Performance,
            focus: PerformanceFocus::Gpu,
            expanded: false,
            size: None,
            live: false,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "processes",
            about: "The process tree",
            view: View::Processes,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "details",
            about: "The flat process table",
            view: View::Details,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "services",
            about: "The services list",
            view: View::Services,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "startup",
            about: "The startup entries",
            view: View::Startup,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "details-selected",
            about: "The flat table with a row selected, so the inspector opens",
            view: View::Details,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
            select: true,
            modal: false,
            hover: None,
        },
        Scene {
            name: "processes-selected",
            about: "The process tree with a row selected",
            view: View::Processes,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
            select: true,
            modal: false,
            hover: None,
        },
        Scene {
            name: "modal",
            about: "The end-task confirmation over the process tree",
            view: View::Processes,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
            select: true,
            modal: true,
            hover: None,
        },
        Scene {
            name: "processes-hover",
            about: "The process tree with the pointer resting on a row",
            view: View::Processes,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
            select: false,
            modal: false,
            hover: Some((700.0, 392.0)),
        },
        // The three tables that paint their row background from the
        // first cell only, one scene each. `egui_extras` paints its own
        // hover fill per cell *underneath* the app's, so a row filled
        // once from the leading cell comes out in two colours with the
        // seam at that column's edge — and it is only visible while the
        // pointer is on the row, which is why it survived so long.
        Scene {
            name: "details-hover",
            about: "The flat table with the pointer resting on a row",
            view: View::Details,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
            select: false,
            modal: false,
            hover: Some((700.0, 392.0)),
        },
        Scene {
            name: "services-hover",
            about: "The services list with the pointer resting on a row",
            view: View::Services,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
            select: false,
            modal: false,
            hover: Some((700.0, 302.0)),
        },
        Scene {
            name: "startup-hover",
            about: "The startup list with the pointer resting on a row",
            view: View::Startup,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
            select: false,
            modal: false,
            hover: Some((700.0, 212.0)),
        },
        // A window tall enough that a panel sizing its graphs by a
        // constant leaves a third of the pane empty below them.
        Scene {
            name: "cpu-tall",
            about: "Performance › CPU on a tall window, where fixed heights show",
            view: View::Performance,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: Some((1440.0, 1600.0)),
            live: false,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "memory",
            about: "The Memory view: a treemap of what is holding memory",
            view: View::Memory,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "memory-picked",
            about: "The same, with a tile picked so its breakdown shows",
            view: View::Memory,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
            select: true,
            modal: false,
            hover: None,
        },
        Scene {
            name: "system",
            about: "Windows, firmware, and hardware information",
            view: View::System,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
            select: false,
            modal: false,
            hover: None,
        },
        Scene {
            name: "settings",
            about: "Theme, interval and the about panel",
            view: View::Settings,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
            select: false,
            modal: false,
            hover: None,
        },
    ];

    /// Every scene's name is the one `--scene` matches on, and matching
    /// takes the *first*. A duplicate therefore does not collide
    /// loudly — it silently shadows, and `--scene memory` quietly draws
    /// a different view than the one asked for. Checked here rather than
    /// left to whoever notices the wrong picture.
    fn names_are_unique() -> Result<()> {
        let mut seen: Vec<&str> = SCENES.iter().map(|scene| scene.name).collect();
        let total = seen.len();
        seen.sort_unstable();
        seen.dedup();
        anyhow::ensure!(
            seen.len() == total,
            "two scenes share a name, so one of them cannot be reached"
        );
        Ok(())
    }

    pub(crate) fn main() -> Result<()> {
        names_are_unique()?;
        use clap::Parser;
        let cli = Cli::parse();

        if cli.list {
            for scene in &SCENES {
                println!("{:<16} {}", scene.name, scene.about);
            }
            return Ok(());
        }

        let size = match cli.size.as_deref() {
            Some(text) => Some(parse_size(text)?),
            None => None,
        };

        let chosen: Vec<&Scene> = if cli.all {
            // The fabricated scenes only. The live ones take a second
            // each waiting on the sampler, and they draw whatever this
            // machine happens to be doing — so a `--all` that included
            // them would be slow *and* would produce two pictures that
            // cannot be compared with the same two taken yesterday.
            // Name a live scene to get one.
            SCENES.iter().filter(|scene| !scene.live).collect()
        } else {
            let one = SCENES
                .iter()
                .find(|scene| scene.name == cli.scene)
                .with_context(|| {
                    format!(
                        "no scene called {:?} — run with --list to see them",
                        cli.scene
                    )
                })?;
            vec![one]
        };

        for scene in chosen {
            let path = match (&cli.out, chosen_is_single(&cli)) {
                (Some(path), true) => path.clone(),
                _ => PathBuf::from(DEFAULT_DIRECTORY).join(format!("{}.png", scene.name)),
            };
            render(scene, size, cli.theme.as_deref(), &path)?;
            println!("wrote {}", path.display());
        }
        Ok(())
    }

    /// Whether `--out` names the one file being written.
    ///
    /// `--all --out one.png` would otherwise write every scene over the
    /// same file and report success once per scene.
    fn chosen_is_single(cli: &Cli) -> bool {
        !cli.all
    }

    /// Parses a `WIDTHxHEIGHT` argument.
    fn parse_size(text: &str) -> Result<(f32, f32)> {
        let (width, height) = text
            .split_once(['x', 'X'])
            .with_context(|| format!("expected WIDTHxHEIGHT, got {text:?}"))?;
        Ok((
            width.trim().parse().context("the width is not a number")?,
            height
                .trim()
                .parse()
                .context("the height is not a number")?,
        ))
    }

    /// Draws one scene and writes it out.
    fn render(
        scene: &Scene,
        size: Option<(f32, f32)>,
        theme: Option<&str>,
        path: &PathBuf,
    ) -> Result<()> {
        let (width, height) = size.or(scene.size).unwrap_or(DEFAULT_SIZE);

        let mut config = rustaman::config::Config::default();
        if let Some(theme) = theme {
            config.theme = Some(theme.to_string());
        }
        let mut app = App::new(config);
        app.view = scene.view;
        app.performance.focus = scene.focus;
        app.performance.network_virtual_expanded = scene.expanded;
        // The window's own title bar, so a screenshot is the whole app
        // rather than the app minus its chrome.
        app.custom_chrome = true;

        if scene.live {
            sample_this_machine(&mut app)?;
            app.services.services = rustaman::win::services::enumerate();
            app.startup.entries = rustaman::win::startup::enumerate();
        } else {
            let snapshot = fabricate();
            fill_history(&mut app, &snapshot);
            app.snapshot = Some(snapshot.into());
            app.services.services = services();
            app.startup.entries = startup();
        }
        if scene.select {
            // A row a long way down the list and with something to show:
            // the selection bar, the hover ramp underneath it, and the
            // inspector's own content all draw differently from an empty
            // one, and none of them appears in a screenshot of a table
            // nobody has touched.
            let chosen = app.snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .processes
                    .iter()
                    .find(|process| process.name == "chrome.exe")
                    .map(rustaman::model::ProcessRow::key)
            });
            app.processes.selected = chosen;
            app.details.selected = chosen;
            app.memory_view.selected = chosen;
        }

        if scene.modal {
            // The one surface that draws over everything else, and the
            // one nobody sees while they are looking for layout bugs
            // because reaching it means picking a process and pressing
            // Delete.
            if let Some(process) = app
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.processes.iter().find(|p| p.pid == 7784))
            {
                app.pending = Some(rustaman::gui::app::Pending::EndTree(
                    process.key(),
                    process.display_name().to_string(),
                    2,
                ));
            }
        }

        // Every branch open. A tree screenshotted collapsed is a
        // screenshot of one row, and the indent, the disclosure arrows
        // and the way a long name behaves three levels deep are the
        // things worth looking at.
        if let Some(snapshot) = &app.snapshot {
            app.processes.expanded = snapshot
                .processes
                .iter()
                .map(rustaman::model::ProcessRow::key)
                .collect();
        }

        // Both lists are read on the view's own schedule rather than by
        // the sampler, and a view that has never refreshed starts a
        // background read on its first frame — which would race the
        // fabricated list this scene just installed and win.
        app.services.refreshed = Some(std::time::Instant::now());
        app.startup.refreshed = app.services.refreshed;

        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(width, height))
            .with_os(egui::os::OperatingSystem::Windows)
            .wgpu()
            .build_ui(move |ui| {
                rustaman::gui::ui::draw(&mut app, ui);
            });
        // The same face the window itself asks for, so a scene shows
        // what the app shows. Note this makes a scene's *metrics*
        // machine-dependent: a machine without the family renders in
        // egui's bundled face and every string is a different width.
        // That is the right trade for a harness whose output a person
        // looks at — there is no golden image here to diverge from, and
        // a screenshot in a face the app never uses is the one thing it
        // must not produce.
        rustaman::gui::font::install(&harness.ctx, rustaman::gui::DEFAULT_FAMILY);

        if let Some((x, y)) = scene.hover {
            harness
                .input_mut()
                .events
                .push(egui::Event::PointerMoved(egui::pos2(x, y)));
        }
        // Several frames, not one: every animation in this app starts at
        // zero — a hover fill, a meter's level, the view's own entry
        // transition — so a single frame renders the app mid-fade, at an
        // opacity that is not what anyone will ever see. A hover needs
        // them for a second reason: `egui_extras` records which row is
        // under the pointer and applies it on the *following* frame.
        harness.run();
        harness.run();
        harness.run();

        let image = harness
            .render()
            .map_err(|error| anyhow::anyhow!("could not render the scene: {error}"))?;

        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)
                .with_context(|| format!("could not create {}", directory.display()))?;
        }
        image
            .save(path)
            .with_context(|| format!("could not write {}", path.display()))?;
        Ok(())
    }

    /// Drives the real sampler until there is something to draw.
    ///
    /// The counterpart to [`fabricate`], and the reason both exist: a
    /// fabricated machine proves the layout copes with a configuration
    /// nobody has, and this proves the layout copes with the one in
    /// front of you — including whatever the Windows layer decided to
    /// report about it, which is the half a fabricated snapshot cannot
    /// test at all.
    ///
    /// It polls rather than sleeping once, because the first snapshot
    /// carries no rates: every rate in this app is a delta between two
    /// samples, so an adapter's throughput is zero until the second one
    /// lands.
    fn sample_this_machine(app: &mut App) -> Result<()> {
        /// How many snapshots to collect before drawing. Enough for the
        /// graphs to show a shape rather than a single point, and few
        /// enough that the command returns while someone is still
        /// watching it.
        const SAMPLES: usize = 6;
        /// How long to wait for one, before giving up on the sampler.
        const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

        let mut collected = 0;
        let deadline = std::time::Instant::now() + PATIENCE;
        while collected < SAMPLES {
            let before = app.snapshot.as_ref().map(|snapshot| snapshot.sequence);
            app.poll();
            let after = app.snapshot.as_ref().map(|snapshot| snapshot.sequence);
            if after != before {
                collected += 1;
                continue;
            }
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "the sampler produced {collected} snapshots in {PATIENCE:?} — \
                 it is stuck, or this build cannot read the machine"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        Ok(())
    }

    /// A machine that does not exist, chosen to be awkward.
    ///
    /// Every figure here is one the real UI has to cope with and that the
    /// machine you are sitting at probably will not produce: twenty-one
    /// network adapters of which three are hardware and one is unplugged,
    /// two disks of very different sizes, sixteen cores, and a GPU with
    /// four engines. A scene drawn against the local machine tests the
    /// local machine.
    fn fabricate() -> Snapshot {
        Snapshot {
            system: SystemSample {
                info: rustaman::model::SystemInfo {
                    computer_name: "RUSTAMAN-WORKSTATION".to_string(),
                    os_name: "Windows 11 Pro".to_string(),
                    os_version: "24H2".to_string(),
                    os_build: "26100".to_string(),
                    manufacturer: "Framework".to_string(),
                    model: "Desktop".to_string(),
                    bios_vendor: "INSYDE Corp.".to_string(),
                    bios_version: "03.03".to_string(),
                    build_revision: "4652".to_string(),
                },
                cpu: CpuSample {
                    total_percent: 23.5,
                    kernel_percent: 6.1,
                    per_core: (0..16).map(|core| f64::from(core) * 4.0 + 3.0).collect(),
                    name: "Intel Core Ultra 7 265K".to_string(),
                    physical_cores: 12,
                    logical_cores: 16,
                    megahertz: 3400,
                    // A hybrid machine, because the desktop this is run
                    // from is almost certainly not one and the P/E
                    // markers, the "8 performance + 8 efficiency cores"
                    // heading and the Topology row have nowhere else to
                    // be seen. Eight fast cores first, then eight small
                    // ones: the order Windows reports them in, and the
                    // order the grid has to survive.
                    core_kinds: (0..16)
                        .map(|core| {
                            if core < 8 {
                                CoreKind::Performance
                            } else {
                                CoreKind::Efficient
                            }
                        })
                        .collect(),
                },
                memory: MemorySample {
                    total: 68_719_476_736,
                    available: 41_231_686_042,
                    committed: 32_212_254_720,
                    commit_limit: 77_309_411_328,
                    cached: 12_884_901_888,
                    paged_pool: 1_073_741_824,
                    nonpaged_pool: 805_306_368,
                },
                disks: vec![
                    DiskSample {
                        index: 0,
                        name: "C:".to_string(),
                        read_rate: 4_194_304.0,
                        write_rate: 1_048_576.0,
                        active_percent: 41.0,
                    },
                    DiskSample {
                        index: 1,
                        name: "D: E:".to_string(),
                        read_rate: 0.0,
                        write_rate: 0.0,
                        active_percent: 0.0,
                    },
                ],
                volumes: vec![
                    rustaman::model::VolumeSample {
                        letter: "C:".to_string(),
                        capacity: 1_000_204_886_016,
                        free: 104_211_939_328,
                    },
                    rustaman::model::VolumeSample {
                        letter: "D:".to_string(),
                        capacity: 4_000_787_030_016,
                        free: 1_724_502_835_200,
                    },
                ],
                adapters: adapters(),
                gpus: vec![GpuSample {
                    luid: "0x0000d3f1".to_string(),
                    name: "NVIDIA GeForce RTX 4070".to_string(),
                    utilisation: 34.0,
                    memory_used: 3_221_225_472,
                    memory_total: 12_884_901_888,
                    engines: vec![
                        ("3D".to_string(), 34.0),
                        ("Copy".to_string(), 2.0),
                        ("Video Decode".to_string(), 11.0),
                        ("Video Encode".to_string(), 0.0),
                    ],
                }],
                uptime_seconds: 179_864,
                process_count: 421,
                thread_count: 6_112,
                handle_count: 214_998,
            },
            processes: processes(),
            interval: std::time::Duration::from_secs(1),
            sequence: 42,
        }
    }

    /// A process list with the shapes the table has to survive.
    ///
    /// A very long name, a deep parent chain, a process with no
    /// description, a suspended one, an elevated one, a 32-bit one, and
    /// one process saturating four cores — the rows that break a column
    /// width, a tree indent or a heat cell, rather than four hundred
    /// rows of `svchost.exe` at 0%.
    fn processes() -> Vec<rustaman::model::ProcessRow> {
        use rustaman::model::{Architecture, Priority, ProcessKind, ProcessStatus};

        /// One row of the table below, before it becomes a `ProcessRow`.
        struct Row {
            pid: u32,
            parent: u32,
            name: &'static str,
            description: &'static str,
            kind: ProcessKind,
            cpu: f64,
            memory: u64,
            threads: u32,
            handles: u32,
            window: Option<&'static str>,
        }

        // Written as a table and kept as one: every row is a machine
        // state chosen on purpose, and reading down a column is how you
        // check the set still covers them. `rustfmt` would otherwise
        // give each field its own line, which is 250 lines of vertical
        // scrolling for fifteen rows of data.
        #[rustfmt::skip]
        let table = [
            // `parent: 0` roots a row. Nothing here claims the idle
            // process as its parent: a tree descending from one node has
            // no shape to look at.
            Row { pid: 0, parent: 0, name: "System Idle Process", description: "", kind: ProcessKind::System, cpu: 76.5, memory: 8_192, threads: 16, handles: 0, window: None },
            Row { pid: 4, parent: 0, name: "System", description: "", kind: ProcessKind::System, cpu: 0.8, memory: 2_285_568, threads: 274, handles: 4_812, window: None },
            Row { pid: 1204, parent: 4, name: "svchost.exe", description: "Host Process for Windows Services", kind: ProcessKind::System, cpu: 0.2, memory: 41_943_040, threads: 24, handles: 892, window: None },
            Row { pid: 1288, parent: 4, name: "MsMpEng.exe", description: "Antimalware Service Executable", kind: ProcessKind::System, cpu: 12.4, memory: 412_090_368, threads: 41, handles: 1_204, window: None },
            Row { pid: 2044, parent: 4, name: "dwm.exe", description: "Desktop Window Manager", kind: ProcessKind::System, cpu: 3.6, memory: 132_120_576, threads: 16, handles: 2_204, window: None },
            // Parented to a `userinit.exe` that has already exited, the
            // way a real shell is — so the row re-roots itself, which is
            // the orphan-adoption path, on screen.
            Row { pid: 3312, parent: 1100, name: "explorer.exe", description: "Windows Explorer", kind: ProcessKind::App, cpu: 1.1, memory: 168_820_736, threads: 92, handles: 3_401, window: Some("Program Manager") },
            Row { pid: 7784, parent: 3312, name: "chrome.exe", description: "Google Chrome", kind: ProcessKind::App, cpu: 18.9, memory: 1_374_389_534, threads: 48, handles: 2_190, window: Some("Cards vs. Lists vs. Tables vs. Data Grids — Smart Interface Design Patterns — Google Chrome") },
            Row { pid: 7912, parent: 7784, name: "chrome.exe", description: "Google Chrome", kind: ProcessKind::Background, cpu: 4.2, memory: 289_406_976, threads: 18, handles: 604, window: None },
            Row { pid: 7998, parent: 7784, name: "chrome.exe", description: "Google Chrome", kind: ProcessKind::Background, cpu: 0.0, memory: 96_468_992, threads: 14, handles: 421, window: None },
            Row { pid: 8102, parent: 3312, name: "Code.exe", description: "Visual Studio Code", kind: ProcessKind::App, cpu: 9.7, memory: 812_055_040, threads: 61, handles: 1_802, window: Some("performance.rs — rustaman — Visual Studio Code") },
            Row { pid: 8340, parent: 8102, name: "rust-analyzer.exe", description: "", kind: ProcessKind::Background, cpu: 398.0, memory: 3_221_225_472, threads: 32, handles: 988, window: None },
            Row { pid: 9001, parent: 3312, name: "WindowsTerminal.exe", description: "Windows Terminal", kind: ProcessKind::App, cpu: 0.4, memory: 78_643_200, threads: 22, handles: 712, window: Some("rustaman — pwsh") },
            Row { pid: 9014, parent: 9001, name: "cargo.exe", description: "", kind: ProcessKind::Background, cpu: 62.1, memory: 204_472_320, threads: 12, handles: 388, window: None },
            Row { pid: 9020, parent: 9014, name: "rustc.exe", description: "", kind: ProcessKind::Background, cpu: 96.3, memory: 1_073_741_824, threads: 9, handles: 274, window: None },
            Row { pid: 5560, parent: 3312, name: "Teams.exe", description: "Microsoft Teams", kind: ProcessKind::App, cpu: 0.0, memory: 508_559_360, threads: 39, handles: 1_144, window: Some("Microsoft Teams") },
        ];

        table
            .into_iter()
            .enumerate()
            .map(|(index, row)| rustaman::model::ProcessRow {
                pid: row.pid,
                parent_pid: row.parent,
                // Ascending with the table's order, so a parent always
                // started before its child — the model rejects a parent
                // link that did not, which would otherwise re-root half
                // this tree and make the scene test the wrong thing.
                started_at: 133_000_000_000_000_000 + index as u64 * 1_000_000_000,
                name: row.name.to_string(),
                description: row.description.to_string(),
                path: Some(PathBuf::from(format!(
                    "C:\\Windows\\System32\\{}",
                    row.name
                ))),
                icon: None,
                user: if matches!(row.kind, ProcessKind::System) {
                    "NT AUTHORITY\\SYSTEM".to_string()
                } else {
                    "DESKTOP-7F2K1\\lawrence".to_string()
                },
                session_id: u32::from(!matches!(row.kind, ProcessKind::System)),
                kind: row.kind,
                elevated: row.pid == 1288,
                architecture: if row.pid == 5560 {
                    Architecture::X86
                } else {
                    Architecture::X64
                },
                window_title: row.window.map(str::to_string),
                status: if row.pid == 5560 {
                    ProcessStatus::Suspended
                } else {
                    ProcessStatus::Running
                },
                // Three states across the table, because all three
                // draw differently and only one of them draws a mark:
                // the throttled row gets the leaf, the unread row gets
                // nothing, and everything else gets nothing for a
                // different reason. A scene where every row is
                // `Standard` cannot tell the last two apart.
                efficiency: match row.pid {
                    5560 => Efficiency::Reduced,
                    9020 => Efficiency::Unknown,
                    _ => Efficiency::Standard,
                },
                cpu_percent: row.cpu / 16.0,
                cpu_time_ms: row.handles as u64 * 97,
                working_set: row.memory,
                private_bytes: row.memory / 2,
                virtual_bytes: row.memory * 3,
                // A shared slice that varies by row, so the Memory
                // view's private/shared split is a real split rather
                // than the same ratio everywhere.
                private_working_set: row.memory / 2 + row.memory / (4 + index as u64 % 5),
                peak_working_set: row.memory + row.memory / 8,
                peak_private_bytes: row.memory / 2 + row.memory / 16,
                paged_pool: u64::from(row.handles) * 512,
                nonpaged_pool: u64::from(row.handles) * 128,
                page_faults: u64::from(row.handles) * 97,
                hard_faults: u64::from(row.threads) * 3,
                hard_fault_rate: f64::from(row.threads) / 10.0,
                thread_count: row.threads,
                handle_count: row.handles,
                disk_read_rate: f64::from(row.threads) * 4_096.0,
                disk_write_rate: f64::from(row.handles) * 64.0,
                io_read_bytes: u64::from(row.handles) * 1_048_576,
                io_write_bytes: u64::from(row.threads) * 1_048_576,
                connections: if row.name == "chrome.exe" { 24 } else { 0 },
                gpu_percent: if row.pid == 2044 { 8.0 } else { 0.0 },
                gpu_memory: if row.pid == 2044 { 268_435_456 } else { 0 },
                priority: if row.pid == 0 {
                    Priority::Idle
                } else {
                    Priority::Normal
                },
            })
            .collect()
    }

    /// A services list, of the shapes the table has to survive: a very
    /// long display name, a stopped service with no PID, and several
    /// sharing one `svchost.exe`.
    fn services() -> Vec<rustaman::win::services::Service> {
        use rustaman::win::services::{Service, ServiceState};

        [
            (
                "Appinfo",
                "Application Information",
                ServiceState::Running,
                Some(1204),
            ),
            (
                "AudioSrv",
                "Windows Audio",
                ServiceState::Running,
                Some(1204),
            ),
            (
                "BITS",
                "Background Intelligent Transfer Service",
                ServiceState::Stopped,
                None,
            ),
            ("Dhcp", "DHCP Client", ServiceState::Running, Some(1204)),
            ("Dnscache", "DNS Client", ServiceState::Running, Some(1204)),
            (
                "EventLog",
                "Windows Event Log",
                ServiceState::Running,
                Some(1204),
            ),
            (
                "MSDTC",
                "Distributed Transaction Coordinator",
                ServiceState::Stopped,
                None,
            ),
            (
                "Spooler",
                "Print Spooler",
                ServiceState::Running,
                Some(3312),
            ),
            ("SysMain", "SysMain", ServiceState::Running, Some(1204)),
            (
                "WSearch",
                "Windows Search",
                ServiceState::Starting,
                Some(9014),
            ),
            (
                "WdNisSvc",
                "Microsoft Defender Antivirus Network Inspection Service",
                ServiceState::Running,
                Some(1288),
            ),
            (
                "WinDefend",
                "Microsoft Defender Antivirus Service",
                ServiceState::Running,
                Some(1288),
            ),
            (
                "wuauserv",
                "Windows Update",
                ServiceState::Stopping,
                Some(1204),
            ),
        ]
        .into_iter()
        .map(|(name, display_name, state, pid)| Service {
            name: name.to_string(),
            display_name: display_name.to_string(),
            state,
            pid,
        })
        .collect()
    }

    /// A startup list, including the two shapes that break the column: a
    /// quoted command line with arguments, and a disabled entry.
    fn startup() -> Vec<rustaman::win::startup::StartupEntry> {
        use rustaman::win::startup::StartupEntry;

        [
            ("OneDrive", "\"C:\\Program Files\\Microsoft OneDrive\\OneDrive.exe\" /background", "HKCU Run", false, true),
            ("SecurityHealth", "%windir%\\system32\\SecurityHealthSystray.exe", "HKLM Run", true, true),
            ("Steam", "\"C:\\Program Files (x86)\\Steam\\steam.exe\" -silent", "HKCU Run", false, false),
            ("Discord", "C:\\Users\\lawrence\\AppData\\Local\\Discord\\Update.exe --processStart Discord.exe", "Startup folder", false, true),
            ("Docker Desktop", "\"C:\\Program Files\\Docker\\Docker\\Docker Desktop.exe\" -Autostart", "HKCU Run", false, true),
            ("RtkAudUService", "\"C:\\WINDOWS\\System32\\DriverStore\\FileRepository\\realtekservice.inf_amd64_9c1\\RtkAudUService64.exe\" -background", "HKLM Run", true, true),
        ]
        .into_iter()
        .map(
            |(name, command, location, all_users, enabled)| StartupEntry {
                name: name.to_string(),
                command: command.to_string(),
                location,
                all_users,
                enabled,
            },
        )
        .collect()
    }

    /// The twenty-one adapters.
    ///
    /// Modelled on what a developer machine actually reports: a wired
    /// card carrying traffic, Wi-Fi associated but quiet, a second port
    /// with nothing plugged into it, and then the long tail of virtual
    /// switches, VPN interfaces and tunnels that made the old panel a
    /// wall of cards.
    fn adapters() -> Vec<AdapterSample> {
        let mut adapters = vec![
            adapter(
                1,
                "Ethernet 2",
                "Realtek Gaming 2.5GbE Family Controller",
                AdapterKind::Ethernet,
                AdapterState::Up,
                true,
                2_500_000_000,
                1_363_148.0,
                60_928.0,
            ),
            adapter(
                2,
                "Wi-Fi",
                "Intel(R) Wi-Fi 6E AX210 160MHz",
                AdapterKind::WiFi,
                AdapterState::Up,
                true,
                1_200_000_000,
                4_096.0,
                1_024.0,
            ),
            adapter(
                3,
                "Ethernet",
                "Intel(R) Ethernet Connection I219-V",
                AdapterKind::Ethernet,
                AdapterState::Disconnected,
                true,
                0,
                0.0,
                0.0,
            ),
            adapter(
                4,
                "Bluetooth Network Connection",
                "Bluetooth Device (Personal Area Network)",
                AdapterKind::Bluetooth,
                AdapterState::Disconnected,
                true,
                3_000_000,
                0.0,
                0.0,
            ),
            adapter(
                5,
                "vEthernet (Default Switch)",
                "Hyper-V Virtual Ethernet Adapter",
                AdapterKind::Virtual,
                AdapterState::Up,
                false,
                10_000_000_000,
                812_034.0,
                24_576.0,
            ),
            adapter(
                6,
                "vEthernet (WSL (Hyper-V firewall))",
                "Hyper-V Virtual Ethernet Adapter #2",
                AdapterKind::Virtual,
                AdapterState::Up,
                false,
                10_000_000_000,
                204_800.0,
                8_192.0,
            ),
            adapter(
                7,
                "WireGuard Tunnel",
                "WireGuard Tunnel",
                AdapterKind::Tunnel,
                AdapterState::Up,
                false,
                0,
                65_536.0,
                16_384.0,
            ),
            adapter(
                8,
                "Local Area Connection* 1",
                "Microsoft Wi-Fi Direct Virtual Adapter",
                AdapterKind::Virtual,
                AdapterState::Disconnected,
                false,
                0,
                0.0,
                0.0,
            ),
            adapter(
                9,
                "Local Area Connection* 2",
                "Microsoft Wi-Fi Direct Virtual Adapter #2",
                AdapterKind::Virtual,
                AdapterState::Disconnected,
                false,
                0,
                0.0,
                0.0,
            ),
            adapter(
                10,
                "Ethernet 3",
                "TAP-Windows Adapter V9",
                AdapterKind::Virtual,
                AdapterState::Disabled,
                false,
                0,
                0.0,
                0.0,
            ),
        ];
        // The long tail: eleven more tunnels and pseudo-adapters, which is
        // the part that made the panel unreadable and the part every
        // machine with a VPN client on it has.
        for index in 0..11u64 {
            adapters.push(adapter(
                20 + index,
                &format!("isatap.{{00000000-0000-0000-0000-{index:012}}}"),
                "Microsoft ISATAP Adapter",
                AdapterKind::Tunnel,
                AdapterState::NotPresent,
                false,
                0,
                0.0,
                0.0,
            ));
        }
        adapters
    }

    /// One fabricated adapter.
    #[expect(
        clippy::too_many_arguments,
        reason = "a table of adapters written as a table: every argument is \
                  one column of it, and grouping them into a struct would \
                  put the field names between the reader and the data"
    )]
    fn adapter(
        luid: u64,
        name: &str,
        description: &str,
        kind: AdapterKind,
        state: AdapterState,
        hardware: bool,
        link_speed: u64,
        receive_rate: f64,
        send_rate: f64,
    ) -> AdapterSample {
        AdapterSample {
            luid,
            name: name.to_string(),
            description: description.to_string(),
            kind,
            state,
            hardware,
            receive_rate,
            send_rate,
            // Plausible cumulative counters, derived from the rate so a
            // busy adapter also reads as having moved a lot of data.
            received_total: (receive_rate * 90_000.0) as u64,
            sent_total: (send_rate * 90_000.0) as u64,
            link_speed,
        }
    }

    /// Fills every history ring, so the graphs draw a shape rather than a
    /// flat line at the current value.
    ///
    /// A deterministic walk, not random: two runs of the same scene must
    /// produce the same picture, or a screenshot cannot be compared with
    /// the one taken before a change.
    fn fill_history(app: &mut App, snapshot: &Snapshot) {
        // Each ring is filled to *its own* capacity rather than to a
        // number chosen here: a ring holding sixty samples in a
        // three-hundred sample window draws its line across the right
        // fifth of the graph and leaves the rest blank, which looks like
        // an app that has just started rather than like the panel being
        // tested.
        let samples = app.performance.cpu.capacity();

        let performance = &mut app.performance;
        performance.cores = vec![Series::new(samples); snapshot.system.cpu.per_core.len()];
        performance.adapters.clear();

        for step in 0..samples {
            let wave = wobble(step, 0);
            performance
                .cpu
                .push((snapshot.system.cpu.total_percent * wave) as f32);
            performance
                .cpu_kernel
                .push((snapshot.system.cpu.kernel_percent * wave) as f32);
            for (index, series) in performance.cores.iter_mut().enumerate() {
                let base = snapshot
                    .system
                    .cpu
                    .per_core
                    .get(index)
                    .copied()
                    .unwrap_or_default();
                series.push((base * wobble(step, index + 1)) as f32);
            }
            performance
                .memory
                .push((snapshot.system.memory.used_percent() * wobble(step, 7)) as f32);
            // Read and write wobble on their own seeds. Scaling one
            // figure by one multiplier would give the Disk panel two
            // lines of identical shape at different heights, which
            // looks like a rendering fault rather than like a machine —
            // and would hide the one thing the split exists to show,
            // which is read and write diverging.
            let (read, write) = snapshot
                .system
                .disks
                .iter()
                .fold((0.0, 0.0), |(read, write), disk| {
                    (read + disk.read_rate, write + disk.write_rate)
                });
            let read = read * wobble(step, 3);
            let write = write * wobble(step, 17);
            performance.disk.push((read + write) as f32);
            performance.disk_read.push(read as f32);
            performance.disk_write.push(write as f32);

            // Receive is derived from the same two wobbled figures the
            // other rings are pushed from, rather than wobbled again:
            // an independently varying receive would exceed the total
            // it is part of on some samples, and the panel would show
            // a machine receiving more than it moved.
            let network = snapshot.system.network_rate() * wobble(step, 5);
            let sent = snapshot.system.network_send_rate() * wobble(step, 9);
            performance.network.push(network as f32);
            performance.network_send.push(sent as f32);
            performance
                .network_receive
                .push((network - sent).max(0.0) as f32);
            performance.gpu.push((34.0 * wobble(step, 11)) as f32);

            for (index, adapter) in snapshot.system.adapters.iter().enumerate() {
                performance
                    .adapters
                    .entry(adapter.luid)
                    .or_insert_with(|| Series::new(samples))
                    .push((adapter.total_rate() * wobble(step, index + 13)) as f32);
            }
        }
    }

    /// A repeatable multiplier around 1.0, varying with the sample and
    /// the series.
    ///
    /// Two sines at incommensurable frequencies, which never repeat
    /// within the window and cost nothing — the point is a line that
    /// looks like a measurement rather than a sawtooth, not statistical
    /// realism.
    fn wobble(step: usize, series: usize) -> f64 {
        let step = step as f64;
        let phase = series as f64 * 0.7;
        let value =
            0.62 + 0.26 * (step * 0.31 + phase).sin() + 0.12 * (step * 0.87 + phase * 1.7).sin();
        value.clamp(0.02, 1.4)
    }
}
