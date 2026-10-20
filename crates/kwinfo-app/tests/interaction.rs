//! Drives the whole application headlessly (no GPU needed) through its context menus,
//! dialogs, filter and settings, and checks the configuration, the window title, the
//! sampler and the settings file. kittest's accessibility check runs whenever the UI
//! settles, so every menu and dialog opened here is also checked for named controls.

use std::time::{Duration, Instant};

use egui::{Key, Modifiers, Role};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use kwinfo_app::app::{KwinfoApp, Startup};
use kwinfo_app::graphs::Placement;
use kwinfo_app::settings::SettingsTab;
use kwinfo_core::config::GraphEntry;
use kwinfo_core::{BackendChoice, ChannelId, Config, ConfigFile, DeviceId, GraphMode, Rgb};

/// The mock backend with a long polling period: one tick of values at start, then
/// quiet, so that the UI settles between interactions.
fn app(config: Config, config_file: Option<ConfigFile>) -> Harness<'static, KwinfoApp> {
    start(Startup {
        config,
        config_file,
        ..Startup::in_memory(BackendChoice::Mock)
    })
}

/// Frames `Harness::run` may take to settle. kittest's default of 4 is too tight for this
/// app: every widget that appears for the first time costs egui an extra frame to style,
/// and a background thread (the sampler, the report writer) finishing mid-run adds more,
/// which on a slow CI runner overflowed the default.
const MAX_STEPS: u64 = 16;

fn start(startup: Startup) -> Harness<'static, KwinfoApp> {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(780.0, 720.0))
        .with_max_steps(MAX_STEPS)
        .build_eframe(move |cc| {
            KwinfoApp::new(
                cc,
                Startup {
                    interval_ms: Some(60_000),
                    ..startup
                },
            )
        });
    // The device's name is also on the card of any of its graphs.
    wait_for(&mut harness, |h| {
        h.query_all_by_label("CPU [#0]: Mock Hexa-Core 6000X")
            .next()
            .is_some()
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
    // The report is written on a background thread: step until it reports back rather
    // than expecting the UI to settle within a fixed number of frames.
    harness.step();
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

/// The first graph's settings, as they would be saved.
fn first_graph(harness: &Harness<'_, KwinfoApp>) -> GraphEntry {
    harness.state().config().graphs.open[0].clone()
}

#[test]
fn graphs_open_once_and_close_from_their_card() {
    let mut harness = app(Config::default(), None);
    let core2 = id("cpu:mock/temp.core2");
    menu(&mut harness, "Core #2", "Show Graph");
    // The harness cannot open windows, so the graph is in the panel.
    assert_eq!(
        harness.state().graph_placement(&harness.ctx),
        Placement::Panel
    );
    assert_eq!(harness.state().open_graphs(), vec![core2.clone()]);
    assert!(
        harness
            .query_by_label("Close the graph of Core #2")
            .is_some()
    );
    assert!(harness.query_by_label("Graph of Core #2").is_some());
    let entry = first_graph(&harness);
    assert_eq!(entry.channel, core2);
    assert!(
        !entry.autofit,
        "temperatures start on their 0 to 100 °C range"
    );

    // Asking again brings the same graph forward instead of opening another.
    let ctx = harness.ctx.clone();
    harness.state_mut().open_graph(&ctx, &core2);
    harness.run();
    assert_eq!(harness.state().open_graphs().len(), 1);

    harness.get_by_label("Close the graph of Core #2").click();
    harness.run();
    assert!(harness.state().open_graphs().is_empty());
    assert!(harness.state().config().graphs.open.is_empty());
    assert!(harness.query_by_label("Graph of Core #2").is_none());
}

#[test]
fn auto_fit_and_typed_bounds_set_the_range() {
    let mut harness = app(Config::default(), None);
    menu(&mut harness, "Core #2", "Show Graph");
    harness.get_by_label("Auto Fit Core #2").click();
    harness.run();
    assert!(first_graph(&harness).autofit);

    let field = harness.get_by_role_and_label(Role::TextInput, "Maximum of Core #2");
    field.focus();
    harness.run();
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness
        .get_by_role_and_label(Role::TextInput, "Maximum of Core #2")
        .type_text("90");
    harness.key_press(Key::Enter);
    harness.run();
    let entry = first_graph(&harness);
    assert!(!entry.autofit, "typing a bound fixes the range");
    assert_eq!(entry.y_max, Some(90.0));
    assert!(entry.y_min.is_some_and(|min| min < 90.0));

    // A minimum above the maximum is refused.
    let field = harness.get_by_role_and_label(Role::TextInput, "Minimum of Core #2");
    field.focus();
    harness.run();
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness
        .get_by_role_and_label(Role::TextInput, "Minimum of Core #2")
        .type_text("95");
    harness.key_press(Key::Enter);
    harness.run();
    assert_eq!(first_graph(&harness).y_max, Some(90.0));
    assert!(first_graph(&harness).y_min.is_some_and(|min| min < 90.0));
}

#[test]
fn reset_clears_the_sensors_statistics() {
    let mut harness = app(Config::default(), None);
    let core2 = id("cpu:mock/temp.core2");
    menu(&mut harness, "Core #2", "Show Graph");
    let count = |h: &Harness<'_, KwinfoApp>| {
        h.state()
            .latest_snapshot()
            .find(&core2)
            .map_or(0, |c| c.stats.count())
    };
    assert!(count(&harness) > 0);
    harness.get_by_label("Reset Core #2").click();
    harness.run();
    wait_for(&mut harness, |h| count(h) == 0);
}

#[test]
fn open_graphs_come_back_after_a_restart() {
    let mut config = Config::default();
    let mut entry = GraphEntry::new(id("cpu:mock/power.package"));
    entry.span_s = 60;
    config.graphs.open.push(entry);
    // A sensor this machine does not have: kept in the settings, not shown.
    config
        .graphs
        .open
        .push(GraphEntry::new(id("gone:device/temp1")));
    let mut harness = app(config, None);
    harness.run();
    assert!(
        harness
            .query_by_label("Close the graph of CPU Package Power")
            .is_some()
    );
    assert_eq!(harness.state().open_graphs().len(), 2);
    assert_eq!(harness.state().config().graphs.open.len(), 2);
    assert_eq!(first_graph(&harness).span_s, 60);

    harness.get_by_label("Close all graphs").click();
    harness.run();
    assert!(harness.state().config().graphs.open.is_empty());
}

#[test]
fn the_graphs_settings_choose_placement_and_history() {
    let mut harness = app(Config::default(), None);
    harness.state_mut().set_settings_tab(SettingsTab::Graphs);
    harness.state_mut().set_settings_open(true);
    harness.run();
    harness
        .get_by_role_and_label(Role::RadioButton, "Separate windows")
        .click();
    harness.run();
    assert_eq!(harness.state().config().graphs.mode, GraphMode::Windows);
    // Still the panel: the harness cannot open windows.
    assert_eq!(
        harness.state().graph_placement(&harness.ctx),
        Placement::Panel
    );

    harness
        .get_by_role_and_label(Role::ComboBox, "History")
        .click();
    harness.run();
    harness.get_by_label("10 min").click();
    harness.run();
    assert_eq!(harness.state().config().graphs.history_minutes, 10);
}

#[test]
fn the_toolbar_menu_closes_all_graphs() {
    let mut harness = app(Config::default(), None);
    menu(&mut harness, "Core #2", "Show Graph");
    menu(&mut harness, "Core #4", "Show Graph");
    assert_eq!(harness.state().open_graphs().len(), 2);
    harness
        .get_by_role_and_label(Role::Button, "Graphs")
        .click();
    harness.run();
    assert!(
        harness.query_by_label("Tile graph windows").is_some(),
        "listed, though disabled with the panel"
    );
    // The panel's header has the same button; the menu adds the second one.
    let buttons: Vec<_> = harness
        .get_all_by_role_and_label(Role::Button, "Close all graphs")
        .collect();
    assert_eq!(buttons.len(), 2);
    buttons.last().unwrap().click();
    harness.run();
    assert!(harness.state().open_graphs().is_empty());
}

#[test]
fn the_helper_tab_offers_install_and_reports_a_failed_pkexec() {
    use kwinfo_ipc::client::{self, HelperState};
    let mut harness = start(Startup {
        helper: Some((
            client::fixed(HelperState::NotInstalled),
            Some(std::path::PathBuf::from("/nonexistent/kwinfo-helper")),
        )),
        ..Startup::in_memory(BackendChoice::Mock)
    });
    harness.state_mut().set_settings_tab(SettingsTab::Helper);
    harness.state_mut().set_settings_open(true);
    harness.run();
    assert!(harness.query_by_label("Run for this session").is_some());
    assert!(harness.query_by_label("Try again").is_some());
    assert!(harness.query_by_label("Uninstall helper…").is_none());
    assert_eq!(
        harness.query_by_label("Helper: not installed").map(|_| ()),
        Some(()),
        "the toolbar names the state"
    );

    // There is no pkexec on the test machine (or no polkit agent to answer it): the
    // job ends with an error the tab shows, and nothing is left running.
    harness.get_by_label("Install helper…").click();
    harness.step();
    wait_for(&mut harness, |h| !h.state().helper_busy());
    harness.run();
    assert!(matches!(harness.state().helper_result(), Some(Err(_))));
}

#[test]
fn logging_from_the_toolbar_writes_a_csv_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.logging.folder = Some(dir.path().to_path_buf());
    config.logging.interval_ms = 0;
    // A quick sampler for this one: rows come with ticks.
    let mut harness = Harness::builder()
        .with_size(egui::vec2(780.0, 720.0))
        .with_max_steps(MAX_STEPS)
        .build_eframe(move |cc| {
            KwinfoApp::new(
                cc,
                Startup {
                    config,
                    interval_ms: Some(150),
                    ..Startup::in_memory(BackendChoice::Mock)
                },
            )
        });
    wait_for(&mut harness, |h| h.state().latest_snapshot().tick >= 1);
    harness.get_by_label("Start logging").click();
    harness.step();
    wait_for(&mut harness, |h| h.state().logging_status().rows >= 2);
    assert!(harness.state().logging_on());
    harness.get_by_label("Stop logging").click();
    harness.step();
    wait_for(&mut harness, |h| !h.state().logging_status().active);

    let files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let text = std::fs::read_to_string(&files[0]).unwrap();
    assert!(
        text.starts_with("time_utc,elapsed_s,CPU [#0]: Mock Hexa-Core 6000X / CPU Package [°C],"),
        "{text}"
    );
    assert!(text.lines().count() >= 3);
}

#[test]
fn an_alert_set_from_the_menu_starts_and_can_be_removed() {
    let mut harness = app(Config::default(), None);
    let core2 = id("cpu:mock/temp.core2");
    menu(&mut harness, "Core #2", "Set alert…");
    let field = harness.get_by_role_and_label(Role::TextInput, "Alert above");
    field.focus();
    harness.run();
    harness
        .get_by_role_and_label(Role::TextInput, "Alert above")
        .type_text("0");
    harness.get_by_label("OK").click();
    harness.run();
    assert_eq!(
        harness
            .state()
            .config()
            .alert_of(&core2)
            .and_then(|a| a.above),
        Some(0.0)
    );
    // Checked at once, not at the next tick a minute away.
    wait_for(&mut harness, |h| h.state().in_alert(&core2));

    harness.state_mut().set_settings_tab(SettingsTab::Alerts);
    harness.state_mut().set_settings_open(true);
    harness.run();
    assert!(harness.query_by_label("Notify for Core #2").is_some());
    harness.get_by_label("Remove").click();
    harness.run();
    assert!(harness.state().config().alert_of(&core2).is_none());
    wait_for(&mut harness, |h| !h.state().in_alert(&core2));
}

#[test]
fn a_lower_limit_above_the_upper_one_is_refused() {
    let mut harness = app(Config::default(), None);
    menu(&mut harness, "Core #2", "Set alert…");
    for (field, text) in [("Alert above", "50"), ("Alert below", "60")] {
        harness
            .get_by_role_and_label(Role::TextInput, field)
            .focus();
        harness.run();
        harness
            .get_by_role_and_label(Role::TextInput, field)
            .type_text(text);
    }
    harness.get_by_label("OK").click();
    harness.run();
    assert!(
        harness
            .query_by_label("The lower limit must be below the upper one.")
            .is_some()
    );
    assert!(
        harness
            .state()
            .config()
            .alert_of(&id("cpu:mock/temp.core2"))
            .is_none()
    );
}

#[test]
fn sensors_are_marked_for_the_log_and_offered_to_the_tray() {
    let mut harness = app(Config::default(), None);
    let core2 = id("cpu:mock/temp.core2");
    menu(&mut harness, "Core #2", "Log this sensor");
    assert!(
        harness
            .state()
            .config()
            .channel(&core2)
            .is_some_and(|c| c.log)
    );
    menu(&mut harness, "Core #2", "Don't log this sensor");
    assert!(harness.state().config().channel(&core2).is_none());
    harness.get_by_label("Core #2").click_secondary();
    harness.run();
    assert!(harness.query_by_label("Add to tray").is_some());
}

#[test]
fn the_logging_tab_sets_interval_and_sensors() {
    let mut harness = app(Config::default(), None);
    harness.state_mut().set_settings_tab(SettingsTab::Logging);
    harness.state_mut().set_settings_open(true);
    harness.run();
    harness
        .get_by_role_and_label(Role::ComboBox, "Interval")
        .click();
    harness.run();
    harness.get_by_label("5 s").click();
    harness.run();
    assert_eq!(harness.state().config().logging.interval_ms, 5000);
    harness
        .get_by_role_and_label(Role::RadioButton, "Marked sensors only (0 marked)")
        .click();
    harness.run();
    assert_eq!(
        harness.state().config().logging.sensors,
        kwinfo_core::config::LogSensors::Marked
    );
}
