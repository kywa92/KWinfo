//! Renders the whole application window headlessly and writes PNGs to the workspace's
//! `target/preview/`, for looking at the UI without a display. Not a regression test:
//! the mock values move, so the pictures are never identical between runs.
//!
//! ```bash
//! cargo test -p kwinfo-app --test preview -- --ignored
//! ```

use std::path::Path;
use std::time::Duration;

use egui_kittest::{Harness, SnapshotOptions};
use kwinfo_app::app::KwinfoApp;
use kwinfo_app::theme::ThemeMode;
use kwinfo_core::BackendChoice;

fn preview(mode: ThemeMode, name: &str, settings_open: bool) {
    // The workspace `target/` directory, whatever the test's working directory is.
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/preview");
    for suffix in ["png", "new.png"] {
        let _ = std::fs::remove_file(out.join(format!("{name}.{suffix}")));
    }

    let mut harness = Harness::builder()
        .with_size(egui::vec2(780.0, 720.0))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(|cc| KwinfoApp::new(cc, BackendChoice::Mock, 100, mode));
    harness.state_mut().set_settings_open(settings_open);
    // Let the sampler publish a few ticks so the table has values and statistics.
    std::thread::sleep(Duration::from_millis(400));
    harness.run_steps(6);

    // With no baseline, kittest writes `{name}.new.png` and reports a missing snapshot;
    // that file is the picture we want.
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

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn app_window_dark() {
    preview(ThemeMode::Dark, "app_dark", false);
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn app_window_light() {
    preview(ThemeMode::Light, "app_light", false);
}

#[test]
#[ignore = "renders with wgpu; run with --ignored on a machine with a GPU"]
fn app_window_settings() {
    preview(ThemeMode::Dark, "app_settings_dark", true);
    preview(ThemeMode::Light, "app_settings_light", true);
}
