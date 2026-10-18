//! Drives the whole application headlessly (no GPU needed) through its context menus,
//! dialogs, filter and settings, and checks the configuration, the window title, the
//! sampler and the settings file. kittest's accessibility check runs whenever the UI
//! settles, so every menu and dialog opened here is also checked for named controls.

use std::time::{Duration, Instant};

use egui::{Key, Modifiers, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use kwinfo_app::app::{KwinfoApp, Startup};
use kwinfo_core::{BackendChoice, ChannelId, Config, ConfigFile, DeviceId, Rgb};

/// The mock backend with a long polling period: one tick of values at start, then
/// quiet, so that the UI settles between interactions.
fn app(config: Config, config_file: Option<ConfigFile>) -> Harness<'static, KwinfoApp> {
    start(Startup {
        config,
        config_file,
        ..Startup::in_memory(BackendChoice::Mock)
    })
}

fn start(startup: Startup) -> Harness<'static, KwinfoApp> {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(780.0, 720.0))
        .build_eframe(move |cc| {
            KwinfoApp::new(
                cc,
                Startup {
                    interval_ms: Some(60_000),
                    ..startup
                },
            )
        });
    wait_for(&mut harness, |h| {
        h.query_by_label("CPU [#0]: Mock Hexa-Core 6000X").is_some()
    });
    harness.run();
    harness
}

/// Steps the UI until `done` holds, for things the sampler thread does.
fn wait_for(harness: &mut Harness<'_, KwinfoApp>, done: impl Fn(&Harness<'_, KwinfoApp>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(harness) {
        assert!(Instant::now() < deadline, "timed out waiting for the UI");
        std::thread::sleep(Duration::from_millis(10));
        harness.step();
    }
}

fn id(raw: &str) -> ChannelId {
    ChannelId::from_raw(raw)
}

/// Opens the context menu of the row labelled `row` and clicks `item`.
fn menu(harness: &mut Harness<'_, KwinfoApp>, row: &str, item: &str) {
    harness.get_by_label(row).click_secondary();
    harness.run();
    harness.get_by_label(item).click();
    harness.run();
}

#[test]
fn hiding_a_sensor_updates_the_title_and_show_hidden_reveals_it() {
    let mut harness = app(Config::default(), None);
    assert_eq!(harness.state().window_title(), "KWinfo – Sensor Status");

    menu(&mut harness, "Core #2", "Hide");
    assert!(
        harness
            .state()
            .config()
            .is_hidden(&id("cpu:mock/temp.core2"))
    );
    assert!(
        harness.query_by_label("Core #2").is_none(),
        "the row is gone"
    );
    assert_eq!(
        harness.state().window_title(),
        "KWinfo – Sensor Status [1 value(s) hidden]"
    );

    harness.state_mut().config_mut().general.show_hidden = true;
    harness.run();
    menu(&mut harness, "Core #2", "Unhide");
    assert!(
        !harness
            .state()
            .config()
            .is_hidden(&id("cpu:mock/temp.core2"))
    );
    assert_eq!(harness.state().window_title(), "KWinfo – Sensor Status");
    assert!(
        harness.state().config().channels.is_empty(),
        "nothing left over"
    );
}

#[test]
fn renaming_goes_through_the_dialog() {
    let mut harness = app(Config::default(), None);
    menu(&mut harness, "Core #2", "Rename…");
    let field = harness.get_by_role_and_label(Role::TextInput, "New name");
    field.focus();
    harness.run();
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness
        .get_by_role_and_label(Role::TextInput, "New name")
        .type_text("Hot core");
    harness.run();
    harness.get_by_label("OK").click();
    harness.run();
    assert_eq!(
        harness
            .state()
            .config()
            .channel(&id("cpu:mock/temp.core2"))
            .and_then(|c| c.label.as_deref()),
        Some("Hot core")
    );
    assert!(harness.query_by_label("Hot core").is_some());

    // Back to the original name.
    menu(&mut harness, "Hot core", "Rename…");
    harness.get_by_label("Original name").click();
    harness.run();
    assert!(harness.query_by_label("Core #2").is_some());
    assert!(harness.state().config().channels.is_empty());
}

#[test]
fn colours_come_from_the_swatches() {
    let mut harness = app(Config::default(), None);
    harness.get_by_label("Core #2").click_secondary();
    harness.run();
    harness.get_by_label("Custom colour").hover();
    harness.run();
    harness
        .get_by_role_and_label(Role::RadioButton, "Orange")
        .click();
    harness.run();
    assert_eq!(
        harness
            .state()
            .config()
            .color_of(&id("cpu:mock/temp.core2")),
        Some(Rgb::new(0xff, 0x8c, 0x00))
    );
}

#[test]
fn disabling_monitoring_reaches_the_sampler() {
    let mut harness = app(Config::default(), None);
    let core2 = id("cpu:mock/temp.core2");
    menu(&mut harness, "Core #2", "Disable monitoring");
    assert!(harness.state().config().is_channel_disabled(&core2));
    wait_for(&mut harness, |h| {
        h.state()
            .latest_snapshot()
            .find(&core2)
            .is_some_and(|c| c.disabled)
    });
    assert!(harness.query_by_label("Disabled").is_some());

    // Device level, from the device row's menu.
    menu(
        &mut harness,
        "CPU [#0]: Mock Hexa-Core 6000X",
        "Disable monitoring",
    );
    let cpu = DeviceId::new("cpu:mock");
    assert!(harness.state().config().is_device_disabled(&cpu));
    wait_for(&mut harness, |h| {
        let snapshot = h.state().latest_snapshot();
        snapshot
            .devices
            .iter()
            .filter(|d| d.device.id == cpu)
            .flat_map(|d| &d.channels)
            .all(|c| c.disabled)
    });
    menu(
        &mut harness,
        "CPU [#0]: Mock Hexa-Core 6000X",
        "Enable monitoring",
    );
    assert!(!harness.state().config().is_device_disabled(&cpu));
}

#[test]
fn ctrl_f_filters_and_escape_clears() {
    let mut harness = app(Config::default(), None);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::F);
    harness.run();
    harness
        .get_by_role_and_label(Role::TextInput, "Filter")
        .type_text("clock");
    harness.run();
    assert!(harness.query_by_label("Core #3 Clock").is_some());
    assert!(harness.query_by_label("CPU Package").is_none());
    assert!(harness.query_by_label("6 sensors").is_none());
    assert!(
        harness.query_by_label("8 sensors").is_some(),
        "six cores and two GPU clocks"
    );

    harness.key_press(Key::Escape);
    harness.run();
    assert!(!harness.state().table_state().filter_open);
    assert!(harness.query_by_label("CPU Package").is_some());
}

#[test]
fn the_header_menu_hides_columns() {
    let mut harness = app(Config::default(), None);
    harness.get_by_label("Average").click_secondary();
    harness.run();
    harness
        .get_by_role_and_label(Role::CheckBox, "Average")
        .click();
    harness.run();
    assert!(!harness.state().config().columns.average.visible);
    assert!(harness.query_by_label("Average").is_none());
}

#[test]
fn collapsing_is_remembered() {
    let mut harness = app(Config::default(), None);
    harness.get_by_label("Core Clocks").click();
    harness.run();
    let cpu = DeviceId::new("cpu:mock");
    assert!(
        harness
            .state()
            .config()
            .is_group_collapsed(&cpu, "Core Clocks")
    );
    assert!(harness.query_by_label("Core #3 Clock").is_none());

    harness.get_by_label("Collapse all").click();
    harness.run();
    assert!(harness.state().config().is_collapsed(&cpu));
    assert!(harness.query_by_label("Core #2").is_none());
    harness.get_by_label("Expand all").click();
    harness.run();
    assert!(harness.query_by_label("Core #2").is_some());
}

#[test]
fn settings_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let file = ConfigFile::new(dir.path().join("kwinfo/config.toml"));
    {
        let (config, note) = file.load_or_default();
        assert!(note.is_none());
        let mut harness = app(config, Some(file.clone()));
        menu(&mut harness, "Core #1", "Hide");
        menu(&mut harness, "Core #4", "Disable monitoring");
        harness.get_by_label("Core #2").click_secondary();
        harness.run();
        harness.get_by_label("Custom colour").hover();
        harness.run();
        harness
            .get_by_role_and_label(Role::RadioButton, "Teal")
            .click();
        harness.run();
        menu(&mut harness, "Core #2", "Rename…");
        harness
            .get_by_role_and_label(Role::TextInput, "New name")
            .focus();
        harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
        harness
            .get_by_role_and_label(Role::TextInput, "New name")
            .type_text("Hot core");
        harness.get_by_label("OK").click();
        harness.run();
        // Dropping the application writes anything not yet saved.
    }

    let saved = file.load().unwrap();
    assert!(saved.is_hidden(&id("cpu:mock/temp.core1")));
    assert!(saved.is_channel_disabled(&id("cpu:mock/temp.core4")));
    let core2 = saved.channel(&id("cpu:mock/temp.core2")).unwrap();
    assert_eq!(core2.label.as_deref(), Some("Hot core"));
    assert_eq!(core2.color, Some(Rgb::new(0x00, 0x89, 0x7b)));
    let text = std::fs::read_to_string(file.path()).unwrap();
    assert!(
        text.contains("[channels.\"cpu:mock/temp.core2\"]"),
        "{text}"
    );

    // A new run starts where the last one stopped.
    let mut harness = app(saved, Some(file.clone()));
    assert!(harness.query_by_label("Hot core").is_some());
    assert!(harness.query_by_label("Core #1").is_none());
    assert_eq!(
        harness.state().window_title(),
        "KWinfo – Sensor Status [1 value(s) hidden]"
    );
    wait_for(&mut harness, |h| {
        h.state()
            .latest_snapshot()
            .find(&id("cpu:mock/temp.core4"))
            .is_some_and(|c| c.disabled && c.stats.count() == 0)
    });
}

#[test]
fn dragged_column_widths_are_remembered() {
    let mut harness = app(Config::default(), None);
    // The divider after the Sensor column: 330 points plus half the item spacing.
    let divider = egui::pos2(333.0, 11.0);
    let moved = divider + egui::vec2(40.0, 0.0);
    harness.hover_at(divider);
    harness.step();
    harness.drag_at(divider);
    harness.step();
    for step in 1..=4 {
        harness.hover_at(divider + egui::vec2(10.0 * step as f32, 0.0));
        harness.step();
    }
    harness.drop_at(moved);
    harness.run();
    let width = harness.state().config().columns.sensor.width;
    assert!(
        width.is_some_and(|w| (w - 370.0).abs() <= 1.0),
        "sensor column width {width:?}"
    );
}

#[test]
fn the_window_size_is_remembered_unless_maximised() {
    let mut harness = app(Config::default(), None);
    let viewport = |harness: &mut Harness<'_, KwinfoApp>| {
        harness
            .input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .clone()
    };
    let mut info = viewport(&mut harness);
    info.inner_rect = Some(egui::Rect::from_min_size(
        egui::pos2(100.0, 80.0),
        egui::vec2(900.4, 650.0),
    ));
    harness
        .input_mut()
        .viewports
        .insert(egui::ViewportId::ROOT, info);
    harness.run();
    assert_eq!(harness.state().config().window.size(), Some([900.0, 650.0]));

    let mut info = viewport(&mut harness);
    info.maximized = Some(true);
    info.inner_rect = Some(egui::Rect::from_min_size(
        egui::pos2(0.0, 0.0),
        egui::vec2(1920.0, 1080.0),
    ));
    harness
        .input_mut()
        .viewports
        .insert(egui::ViewportId::ROOT, info);
    harness.run();
    assert_eq!(harness.state().config().window.size(), Some([900.0, 650.0]));
}

#[test]
fn the_colour_dialog_applies_and_resets() {
    let mut harness = app(Config::default(), None);
    harness.get_by_label("Core #2").click_secondary();
    harness.run();
    harness.get_by_label("Custom colour").hover();
    harness.run();
    harness.get_by_label("Other colour…").click();
    harness.run();
    assert!(harness.query_by_label("Sensor colour").is_some());
    harness.get_by_label("OK").click();
    harness.run();
    let core2 = id("cpu:mock/temp.core2");
    assert!(harness.state().config().color_of(&core2).is_some());

    harness.get_by_label("Core #2").click_secondary();
    harness.run();
    harness.get_by_label("Custom colour").hover();
    harness.run();
    harness.get_by_label("Default colour").click();
    harness.run();
    assert_eq!(harness.state().config().color_of(&core2), None);
}

#[test]
fn the_diagnostic_report_holds_what_the_window_shows() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("report.txt");
    let mut config = Config::default();
    config.update_channel(&id("cpu:mock/temp.core2"), |c| c.hidden = true);
    let mut harness = start(Startup {
        config,
        report_out: Some(out.clone()),
        ..Startup::in_memory(BackendChoice::Mock)
    });
    harness.state_mut().set_settings_open(true);
    harness.run();
    harness.get_by_label("Save diagnostic report…").click();
    harness.run();
    wait_for(&mut harness, |h| h.state().report_result().is_some());
    harness.run();
    assert_eq!(harness.state().report_result(), Some(&Ok(out.clone())));
    assert!(harness.query_by_label("Report saved to").is_some());

    let text = std::fs::read_to_string(&out).unwrap();
    assert!(
        text.contains("## Sensor inventory\nCPU [#0]: Mock Hexa-Core 6000X  [cpu:mock]"),
        "{text}"
    );
    assert!(text.contains("temp.core2  (hidden)"), "{text}");
}
