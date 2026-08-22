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
        AdapterKind, AdapterSample, AdapterState, CpuSample, DiskSample, GpuSample, MemorySample,
        Snapshot, SystemSample,
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
    /// A normal window on a 1080p screen, which is the size at which the
    /// layout has to work — a screenshot taken at 2560 points wide makes
    /// every crowded panel look spacious.
    const DEFAULT_SIZE: (f32, f32) = (1280.0, 860.0);

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
    }

    /// Every scene, in the order `--list` prints them.
    const SCENES: [Scene; 10] = [
        Scene {
            name: "live-network",
            about: "Performance › Network on THIS machine, really sampled",
            view: View::Performance,
            focus: PerformanceFocus::Network,
            expanded: true,
            size: Some((1280.0, 1500.0)),
            live: true,
        },
        Scene {
            name: "live-processes",
            about: "The process tree on THIS machine, really sampled",
            view: View::Processes,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: true,
        },
        Scene {
            name: "network",
            about: "Performance › Network on a machine with 21 adapters",
            view: View::Performance,
            focus: PerformanceFocus::Network,
            expanded: false,
            size: None,
            live: false,
        },
        Scene {
            name: "network-open",
            about: "The same, with the virtual-adapter group expanded",
            view: View::Performance,
            focus: PerformanceFocus::Network,
            expanded: true,
            size: None,
            live: false,
        },
        Scene {
            name: "network-narrow",
            about: "The Network panel at the width where rows drop their sparkline",
            view: View::Performance,
            focus: PerformanceFocus::Network,
            expanded: false,
            size: Some((900.0, 720.0)),
            live: false,
        },
        Scene {
            name: "cpu",
            about: "Performance › CPU, sixteen cores",
            view: View::Performance,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
        },
        Scene {
            name: "memory",
            about: "Performance › Memory",
            view: View::Performance,
            focus: PerformanceFocus::Memory,
            expanded: false,
            size: None,
            live: false,
        },
        Scene {
            name: "disk",
            about: "Performance › Disk, two drives",
            view: View::Performance,
            focus: PerformanceFocus::Disk,
            expanded: false,
            size: None,
            live: false,
        },
        Scene {
            name: "gpu",
            about: "Performance › GPU",
            view: View::Performance,
            focus: PerformanceFocus::Gpu,
            expanded: false,
            size: None,
            live: false,
        },
        Scene {
            name: "processes",
            about: "The process tree",
            view: View::Processes,
            focus: PerformanceFocus::Cpu,
            expanded: false,
            size: None,
            live: false,
        },
    ];

    pub(crate) fn main() -> Result<()> {
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
        } else {
            let snapshot = fabricate();
            fill_history(&mut app, &snapshot);
            app.snapshot = Some(snapshot);
        }

        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(width, height))
            .with_os(egui::os::OperatingSystem::Windows)
            .wgpu()
            .build_ui(move |ui| {
                rustaman::gui::ui::draw(&mut app, ui);
            });
        // Several frames, not one: every animation in this app starts at
        // zero — a hover fill, a meter's level, the view's own entry
        // transition — so a single frame renders the app mid-fade, at an
        // opacity that is not what anyone will ever see.
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
                cpu: CpuSample {
                    total_percent: 23.5,
                    kernel_percent: 6.1,
                    per_core: (0..16).map(|core| f64::from(core) * 4.0 + 3.0).collect(),
                    name: "AMD Ryzen 9 5950X 16-Core Processor".to_string(),
                    physical_cores: 16,
                    logical_cores: 16,
                    megahertz: 3400,
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
                        capacity: 1_000_204_886_016,
                        free: 104_211_939_328,
                    },
                    DiskSample {
                        index: 1,
                        name: "D: E:".to_string(),
                        read_rate: 0.0,
                        write_rate: 0.0,
                        active_percent: 0.0,
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
            ..Snapshot::default()
        }
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
            performance.disk.push(
                (snapshot
                    .system
                    .disks
                    .iter()
                    .map(DiskSample::total_rate)
                    .sum::<f64>()
                    * wobble(step, 3)) as f32,
            );
            performance
                .network
                .push((snapshot.system.network_rate() * wobble(step, 5)) as f32);
            performance
                .network_send
                .push((snapshot.system.network_send_rate() * wobble(step, 9)) as f32);
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
