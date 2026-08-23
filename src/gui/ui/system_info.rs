// ============================================================================
// Module:       gui::ui::system_info
// Description:  System Information — Windows, hardware, firmware, storage,
//               graphics, networking, and live machine totals.
//
// Dependencies: egui; super::{chrome, theme, widgets}; crate::{format, model}
// ============================================================================

//! A readable inventory of the machine behind the live counters.
//!
//! Performance answers "what is it doing?"; this view answers "what is
//! it?". Static identity is read once by the sampler and carried in the
//! snapshot, while the four headline values remain live.

use super::theme::{self, SPACE_MD, SPACE_SM, SPACE_XS};
use super::{chrome, widgets};
use crate::gui::app::App;
use crate::model::{AdapterSample, SystemSample, VolumeSample};
use crate::theme::Palette;
use egui::{CornerRadius, Ui};

/// Narrowest useful information card.
const CARD_MINIMUM: f32 = 290.0;

/// Draws the System Information view.
pub fn draw(app: &mut App, ui: &mut Ui) {
    let theme = app.theme.clone();
    let Some(snapshot) = app.snapshot.clone() else {
        widgets::empty_state(ui, &theme, "Waiting for the first sample…");
        return;
    };
    let system = &snapshot.system;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            heading(ui, &theme, system);
            ui.add_space(chrome::SECTION_GAP);
            overview(ui, &theme, system);
            ui.add_space(chrome::SECTION_GAP);

            widgets::section(ui, &theme, "System identity");
            identity_cards(ui, &theme, system);

            ui.add_space(chrome::SECTION_GAP);
            widgets::section(ui, &theme, "Storage");
            volumes(ui, &theme, &system.volumes);

            ui.add_space(chrome::SECTION_GAP);
            widgets::section(ui, &theme, "Graphics");
            graphics(ui, &theme, system);

            ui.add_space(chrome::SECTION_GAP);
            widgets::section(ui, &theme, "Network hardware");
            network(ui, &theme, system);
        });
}

fn heading(ui: &mut Ui, theme: &Palette, system: &SystemSample) {
    let name = present(&system.info.computer_name, "This PC");
    ui.label(
        egui::RichText::new(name)
            .size(25.0)
            .strong()
            .color(theme::rgb(theme.text)),
    );
    let machine = join_nonempty(&system.info.manufacturer, &system.info.model);
    if !machine.is_empty() {
        ui.label(
            egui::RichText::new(machine)
                .color(theme::rgb(theme.text_muted))
                .text_style(egui::TextStyle::Body),
        );
    }
    ui.add_space(SPACE_SM);
    ui.horizontal_wrapped(|ui| {
        for label in [
            system.info.os_name.as_str(),
            system.info.os_version.as_str(),
            build_label(&system.info.build_display()).as_str(),
        ] {
            if !label.is_empty() {
                widgets::chip(ui, label, theme.raised, theme.text_muted);
                ui.add_space(SPACE_XS);
            }
        }
    });
}

fn overview(ui: &mut Ui, theme: &Palette, system: &SystemSample) {
    widgets::card_grid(ui, 185.0, 4, |ui, index| {
        panel(ui, theme, |ui| match index {
            0 => {
                widgets::stat(
                    ui,
                    theme,
                    "CPU now",
                    &crate::format::percent(system.cpu.total_percent),
                    theme.accent,
                );
                widgets::meter(
                    ui,
                    theme,
                    (system.cpu.total_percent / 100.0) as f32,
                    theme.accent,
                );
            }
            1 => {
                widgets::stat(
                    ui,
                    theme,
                    "Memory in use",
                    &crate::format::bytes(system.memory.used()),
                    theme.warning,
                );
                widgets::meter(
                    ui,
                    theme,
                    (system.memory.used_percent() / 100.0) as f32,
                    theme.warning,
                );
            }
            2 => {
                widgets::stat(
                    ui,
                    theme,
                    "Processes",
                    &crate::format::count(system.process_count as u64),
                    theme.success,
                );
                widgets::detail_row(
                    ui,
                    theme,
                    "Threads",
                    &crate::format::count(system.thread_count),
                );
            }
            _ => {
                widgets::stat(
                    ui,
                    theme,
                    "Uptime",
                    &crate::format::duration(system.uptime_seconds),
                    theme.info,
                );
                widgets::detail_row(
                    ui,
                    theme,
                    "Handles",
                    &crate::format::count(system.handle_count),
                );
            }
        });
    });
}

fn identity_cards(ui: &mut Ui, theme: &Palette, system: &SystemSample) {
    widgets::card_grid(ui, CARD_MINIMUM, 4, |ui, index| {
        panel(ui, theme, |ui| match index {
            0 => {
                card_title(ui, theme, "Windows");
                widgets::detail_row(
                    ui,
                    theme,
                    "Edition",
                    present(&system.info.os_name, "Unknown"),
                );
                widgets::detail_row(
                    ui,
                    theme,
                    "Version",
                    present(&system.info.os_version, "Unknown"),
                );
                // Build *and* revision. Two machines both reading
                // "26100" can be a year of patches apart, and the
                // revision is the half that says which is this one.
                let build = system.info.build_display();
                widgets::detail_row(ui, theme, "Build", present(&build, "Unknown"));
                widgets::detail_row(
                    ui,
                    theme,
                    "Computer",
                    present(&system.info.computer_name, "Unknown"),
                );
            }
            1 => {
                card_title(ui, theme, "Computer");
                widgets::detail_row(
                    ui,
                    theme,
                    "Manufacturer",
                    present(&system.info.manufacturer, "Unknown"),
                );
                widgets::detail_row(ui, theme, "Model", present(&system.info.model, "Unknown"));
                widgets::detail_row(
                    ui,
                    theme,
                    "Installed RAM",
                    &crate::format::bytes(system.memory.total),
                );
                widgets::detail_row(ui, theme, "Physical disks", &system.disks.len().to_string());
            }
            2 => {
                card_title(ui, theme, "Processor");
                widgets::detail_row(ui, theme, "Name", present(&system.cpu.name, "Unknown"));
                widgets::detail_row(ui, theme, "Cores", &system.cpu.physical_cores.to_string());
                widgets::detail_row(ui, theme, "Logical", &system.cpu.logical_cores.to_string());
                // Only on a machine that has a split to report. A
                // "Topology: uniform" row on a desktop Ryzen is a line
                // of type spent saying nothing.
                if let Some((performance, efficient)) = system.cpu.hybrid_counts() {
                    widgets::detail_row(
                        ui,
                        theme,
                        "Topology",
                        &format!("{performance}P + {efficient}E"),
                    );
                }
                let clock = if system.cpu.megahertz == 0 {
                    "Unknown".to_string()
                } else {
                    format!("{} MHz", system.cpu.megahertz)
                };
                widgets::detail_row(ui, theme, "Nominal clock", &clock);
            }
            _ => {
                card_title(ui, theme, "Firmware");
                widgets::detail_row(
                    ui,
                    theme,
                    "Vendor",
                    present(&system.info.bios_vendor, "Unknown"),
                );
                widgets::detail_row(
                    ui,
                    theme,
                    "Version",
                    present(&system.info.bios_version, "Unknown"),
                );
                widgets::detail_row(ui, theme, "GPU adapters", &system.gpus.len().to_string());
                widgets::detail_row(
                    ui,
                    theme,
                    "Network adapters",
                    &system
                        .adapters
                        .iter()
                        .filter(|adapter| adapter.hardware)
                        .count()
                        .to_string(),
                );
            }
        });
    });
}

fn volumes(ui: &mut Ui, theme: &Palette, volumes: &[VolumeSample]) {
    if volumes.is_empty() {
        widgets::empty_note(ui, theme, "No mounted local volumes were reported");
        return;
    }
    widgets::card_grid(ui, 220.0, volumes.len(), |ui, index| {
        let Some(volume) = volumes.get(index) else {
            return;
        };
        panel(ui, theme, |ui| {
            card_title(ui, theme, &volume.letter);
            let used = volume.capacity.saturating_sub(volume.free);
            widgets::detail_row(ui, theme, "Used", &crate::format::bytes(used));
            widgets::detail_row(ui, theme, "Free", &crate::format::bytes(volume.free));
            widgets::detail_row(
                ui,
                theme,
                "Capacity",
                &crate::format::bytes(volume.capacity),
            );
            let fraction = if volume.capacity == 0 {
                0.0
            } else {
                used as f32 / volume.capacity as f32
            };
            widgets::meter(ui, theme, fraction, theme.accent);
        });
    });
}

fn graphics(ui: &mut Ui, theme: &Palette, system: &SystemSample) {
    if system.gpus.is_empty() {
        widgets::empty_note(ui, theme, "No GPU performance adapters were reported");
        return;
    }
    widgets::card_grid(ui, CARD_MINIMUM, system.gpus.len(), |ui, index| {
        let Some(gpu) = system.gpus.get(index) else {
            return;
        };
        panel(ui, theme, |ui| {
            card_title(ui, theme, &gpu.name);
            widgets::detail_row(ui, theme, "Adapter", &gpu.luid);
            widgets::detail_row(
                ui,
                theme,
                "In use",
                &crate::format::percent(gpu.utilisation),
            );
            widgets::detail_row(
                ui,
                theme,
                "Dedicated",
                &crate::format::bytes(gpu.memory_used),
            );
        });
    });
}

fn network(ui: &mut Ui, theme: &Palette, system: &SystemSample) {
    let adapters: Vec<&AdapterSample> = system
        .adapters
        .iter()
        .filter(|adapter| adapter.hardware)
        .collect();
    if adapters.is_empty() {
        widgets::empty_note(ui, theme, "No physical network adapters were reported");
        return;
    }
    widgets::card_grid(ui, CARD_MINIMUM, adapters.len(), |ui, index| {
        let Some(adapter) = adapters.get(index) else {
            return;
        };
        panel(ui, theme, |ui| {
            card_title(ui, theme, &adapter.name);
            widgets::detail_row(ui, theme, "Hardware", &adapter.description);
            widgets::detail_row(ui, theme, "Type", adapter.kind.label());
            widgets::detail_row(ui, theme, "State", adapter.state.label());
            widgets::detail_row(
                ui,
                theme,
                "Link speed",
                &crate::format::link_speed(adapter.link_speed),
            );
        });
    });
}

fn panel(ui: &mut Ui, theme: &Palette, contents: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(theme::rgb(theme.panel))
        .stroke(egui::Stroke::new(1.0, theme::rgb(theme.border)))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(theme::margin(SPACE_MD))
        .show(ui, contents);
}

fn card_title(ui: &mut Ui, theme: &Palette, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .strong()
            .color(theme::rgb(theme.text)),
    );
    ui.add_space(SPACE_SM);
}

fn present<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}

fn join_nonempty(left: &str, right: &str) -> String {
    [left.trim(), right.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

fn build_label(build: &str) -> String {
    if build.trim().is_empty() {
        String::new()
    } else {
        format!("Build {}", build.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_facts_have_an_explicit_fallback() {
        assert_eq!(present("", "Unknown"), "Unknown");
        assert_eq!(present("Framework", "Unknown"), "Framework");
    }

    #[test]
    fn machine_labels_do_not_leave_dangling_separators() {
        assert_eq!(join_nonempty("Vendor", "Model"), "Vendor · Model");
        assert_eq!(join_nonempty("Vendor", ""), "Vendor");
        assert_eq!(join_nonempty("", "Model"), "Model");
    }
}
