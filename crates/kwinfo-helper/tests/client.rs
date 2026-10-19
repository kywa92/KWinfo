//! The client against the real helper binary in `stdio` mode, reading the 8700K and
//! smartctl fixtures: what KWinfo does with `pkexec kwinfo-helper stdio`, minus pkexec.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kwinfo_core::{Backend, ChannelId, Sampler, SamplerConfig, Source};
use kwinfo_ipc::client::{self, ClientOptions, HelperState, SessionCommand, Via};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

fn session() -> SessionCommand {
    let root = fixtures().join("synthetic-8700k");
    let smart = fixtures().join("smartctl");
    SessionCommand::direct(
        env!("CARGO_BIN_EXE_kwinfo-helper"),
        [
            "stdio".to_string(),
            "--root".to_string(),
            root.display().to_string(),
            "--smartctl-dir".to_string(),
            smart.display().to_string(),
        ],
    )
}

#[test]
fn a_session_helper_feeds_the_sampler() {
    let (mut backend, handle) = client::start(ClientOptions {
        service: false,
        session: Some(session()),
    });
    let state = handle.wait_settled(Duration::from_secs(20));
    let HelperState::Running {
        via, capabilities, ..
    } = &state
    else {
        panic!("{state:?}");
    };
    assert_eq!(*via, Via::Session);
    assert!(capabilities.rapl && capabilities.smart);
    assert!(!capabilities.msr, "the fixture has no /dev/cpu");
    assert!(
        capabilities
            .notes
            .iter()
            .any(|n| n.contains("modprobe msr"))
    );

    let devices = backend.enumerate().unwrap();
    let ids: Vec<&str> = devices.iter().map(|(d, _)| d.id.as_str()).collect();
    assert_eq!(ids[0], "cpu:0");
    assert!(
        ids.iter()
            .any(|id| id.starts_with("smart:samsung-ssd-970-evo-plus-1tb:"))
    );
    assert!(
        devices
            .iter()
            .flat_map(|(_, c)| c)
            .all(|c| c.source == Source::Helper)
    );

    // In a sampler, as the application runs it: helper rows arrive with values.
    let sampler = Sampler::start(
        vec![Box::new(backend)],
        SamplerConfig {
            interval: Duration::from_millis(300),
            history_capacity: 8,
        },
    );
    let temperature = ChannelId::from_raw(format!(
        "{}/smart.temperature",
        ids.iter().find(|id| id.contains("970-evo")).unwrap()
    ));
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let snapshot = sampler.snapshot();
        let reading = snapshot.find(&temperature).and_then(|c| c.stats.current());
        if reading == Some(38.0) {
            assert!(
                snapshot
                    .find(&ChannelId::from_raw("cpu:0/rapl.package"))
                    .is_some()
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no helper readings in the sampler"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    handle.stop_session();
    let deadline = Instant::now() + Duration::from_secs(10);
    while handle.state().is_running() {
        assert!(Instant::now() < deadline, "the session did not stop");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(sampler);
}

#[test]
fn the_backend_waits_briefly_for_the_first_contact() {
    let (mut backend, _handle) = client::start(ClientOptions {
        service: false,
        session: Some(session()),
    });
    // No explicit wait: the sampler's first enumeration gives the helper a moment.
    let devices = backend.enumerate().unwrap();
    assert!(
        !devices.is_empty(),
        "the helper's rows are there from the first tick"
    );
    let mut out = Vec::new();
    backend.sample(&mut out).unwrap();
    assert!(
        out.iter()
            .any(|s| s.channel.key() == "smart.temperature" && s.value == Some(38.0))
    );
}
