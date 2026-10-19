//! Renders the whole application window headlessly and writes PNGs to the workspace's
//! `target/preview/`, for looking at the UI without a display. Not a regression test:
//! the mock values move, so the pictures are never identical between runs. kittest's
//! accessibility check runs before every picture, so each one also proves that every
//! control on screen has an accessible name.
//!
//! ```bash
//! cargo test -p kwinfo-app --test preview -- --ignored
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use egui::{Key, Modifiers, Role};
use egui_kittest::kittest::Queryable as _;
use egui_kittest::{Harness, SnapshotOptions};
use kwinfo_app::app::{KwinfoApp, Startup};
use kwinfo_app::settings::SettingsTab;
use kwinfo_app::theme::ThemeMode;
use kwinfo_core::config::GraphEntry;
use kwinfo_core::{BackendChoice, ChannelId, Config, DeviceId, Rgb};

fn out_dir() -> PathBuf {
    // The workspace `target/` directory, whatever the test's working directory is.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/preview")
}

fn app(choice: BackendChoice, mode: ThemeMode, config: Config) -> Harness<'static, KwinfoApp> {
    app_sized(
        choice,
        mode,
        config,
        egui::vec2(780.0, 720.0),
        Duration::from_millis(400),
    )
}

/// `app`, in a window of `size`, after letting the sampler run for `run`.
fn app_sized(
    choice: BackendChoice,
    mode: ThemeMode,
    config: Config,
    size: egui::Vec2,
    run: Duration,
) -> Harness<'static, KwinfoApp> {
    let mut harness = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(move |cc| {
            KwinfoApp::new(
                cc,
                Startup {
                    config,
                    interval_ms: Some(100),
                    theme: Some(mode),
                    ..Startup::in_memory(choice)
                },
            )
        });
    // Let the sampler publish a few ticks so the table has values and statistics.
    std::thread::sleep(run);
    harness.run_steps(6);
    harness
}

fn write(harness: &mut Harness<'_, KwinfoApp>, name: &str) {
    let out = out_dir();
    for suffix in ["png", "new.png"] {
        let _ = std::fs::remove_file(out.join(format!("{name}.{suffix}")));
    }
    // With no baseline, kittest writes `{name}.new.png` and reports a missing snapshot;
    // that file is the picture we want. The accessibility check runs first.
    let _ = harness.try_snapshot_options(name, &SnapshotOptions::default().output_path(&out));
    let written = ["png", "new.png"]
        .iter()
        .any(|suffix| out.join(format!("{name}.{suffix}")).exists());
    assert!(
        written,
        "preview image was not written to {}",
        out.display()
    );
}

/// Some of everything: hidden rows shown greyed, renamed and recoloured sensors,
/// disabled monitoring for a sensor and a device, collapsed groups.
fn customised() -> Config {
    let mut config = Config::default();
    let id = |raw: &str| ChannelId::from_raw(raw);
    config.general.show_hidden = true;
    for hidden in [
        "cpu:mock/temp.core1",
        "cpu:mock/temp.core3",
        "cpu:mock/usage.max",
    ] {
        config.update_channel(&id(hidden), |c| c.hidden = true);
    }
    config.update_channel(&id("cpu:mock/temp.package"), |c| {
        c.label = Some("CPU Die (renamed)".into());
    });
    config.update_channel(&id("cpu:mock/temp.coremax"), |c| {
        c.color = Some(Rgb::new(0xff, 0x8c, 0x00));
    });
    config.update_channel(&id("cpu:mock/power.gt"), |c| c.disabled = true);
    let cpu = DeviceId::new("cpu:mock");
    config.set_group_collapsed(&cpu, "Core Usage", true);
    config.update_device(&DeviceId::new("hwmon:mock:nct6793d"), |d| {
        d.disabled = true;
        d.collapsed = true;
    });
    config
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn app_window_dark() {
    let mut harness = app(BackendChoice::Mock, ThemeMode::Dark, Config::default());
    write(&mut harness, "app_dark");
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn app_window_light() {
    let mut harness = app(BackendChoice::Mock, ThemeMode::Light, Config::default());
    write(&mut harness, "app_light");
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn app_window_customised() {
    for (mode, name) in [
        (ThemeMode::Dark, "app_customised_dark"),
        (ThemeMode::Light, "app_customised_light"),
    ] {
        let mut harness = app(BackendChoice::Mock, mode, customised());
        assert!(
            harness
                .state()
                .window_title()
                .contains("[3 value(s) hidden]"),
            "{}",
            harness.state().window_title()
        );
        write(&mut harness, name);
    }
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn app_window_settings() {
    let tabs = [
        (SettingsTab::General, "general"),
        (SettingsTab::Columns, "columns"),
        (SettingsTab::Hidden, "hidden"),
    ];
    for (mode, theme) in [(ThemeMode::Dark, "dark"), (ThemeMode::Light, "light")] {
        for (tab, tab_name) in tabs {
            let mut harness = app(BackendChoice::Mock, mode, customised());
            harness.state_mut().set_settings_open(true);
            harness.state_mut().set_settings_tab(tab);
            harness.run_steps(4);
            write(&mut harness, &format!("app_settings_{tab_name}_{theme}"));
        }
    }
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn app_window_context_menu() {
    for (mode, name) in [
        (ThemeMode::Dark, "app_menu_dark"),
        (ThemeMode::Light, "app_menu_light"),
    ] {
        let mut harness = app(BackendChoice::Mock, mode, Config::default());
        harness.get_by_label("Core #2").click_secondary();
        harness.run_steps(4);
        harness.get_by_label("Custom colour").hover();
        harness.run_steps(4);
        write(&mut harness, name);
    }
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn app_window_filter() {
    let mut harness = app(BackendChoice::Mock, ThemeMode::Dark, Config::default());
    harness.key_press_modifiers(Modifiers::COMMAND, Key::F);
    harness.run_steps(3);
    harness
        .get_by_role_and_label(Role::TextInput, "Filter")
        .type_text("clock");
    harness.run_steps(4);
    write(&mut harness, "app_filter_dark");
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn app_window_replay() {
    // The real Linux backends reading the synthetic i7-8700K fixture.
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/synthetic-8700k");
    let mut harness = app(
        BackendChoice::Replay(fixture),
        ThemeMode::Dark,
        Config::default(),
    );
    write(&mut harness, "app_replay_dark");
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn app_window_dialogs() {
    for (mode, theme) in [(ThemeMode::Dark, "dark"), (ThemeMode::Light, "light")] {
        let mut harness = app(BackendChoice::Mock, mode, Config::default());
        harness.get_by_label("Core #2").click_secondary();
        harness.run_steps(3);
        harness.get_by_label("Custom colour").hover();
        harness.run_steps(3);
        harness.get_by_label("Other colour…").click();
        harness.run_steps(4);
        write(&mut harness, &format!("app_colour_dialog_{theme}"));

        let mut harness = app(BackendChoice::Mock, mode, Config::default());
        harness.get_by_label("Core #2").click_secondary();
        harness.run_steps(3);
        harness.get_by_label("Rename…").click();
        harness.run_steps(4);
        write(&mut harness, &format!("app_rename_dialog_{theme}"));
    }
}

/// Three graphs in the panel: a temperature on its fixed range, a power reading under
/// Auto Fit, and the total load; all showing the last minute.
fn with_graphs() -> Config {
    let mut config = Config::default();
    for (raw, autofit) in [
        ("cpu:mock/temp.package", false),
        ("cpu:mock/power.package", true),
        ("cpu:mock/usage.total", false),
    ] {
        let mut entry = GraphEntry::new(ChannelId::from_raw(raw));
        entry.autofit = autofit;
        entry.span_s = 60;
        config.graphs.open.push(entry);
    }
    config.graphs.panel_width = Some(560.0);
    config
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn app_window_graphs() {
    for (mode, name) in [
        (ThemeMode::Dark, "app_graphs_dark"),
        (ThemeMode::Light, "app_graphs_light"),
    ] {
        let mut harness = app_sized(
            BackendChoice::Mock,
            mode,
            with_graphs(),
            egui::vec2(1240.0, 720.0),
            Duration::from_secs(4),
        );
        assert!(
            harness
                .query_by_label("Close the graph of CPU Package")
                .is_some()
        );
        write(&mut harness, name);
    }
}
