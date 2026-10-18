//! Chooses which backends to run on this machine.

use crate::backend::Backend;
use crate::mock::MockBackend;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendChoice {
    /// Real hardware where available.
    Auto,
    /// The synthetic backend, for development and tests.
    Mock,
}

pub struct BackendSet {
    pub backends: Vec<Box<dyn Backend>>,
    /// Human-readable notes about what was (not) selected, for the UI status line.
    pub notes: Vec<String>,
}

impl BackendSet {
    pub fn build(choice: BackendChoice) -> Self {
        let mut notes = Vec::new();
        let backends: Vec<Box<dyn Backend>> = match choice {
            BackendChoice::Mock => vec![Box::new(MockBackend::new(0x5eed))],
            BackendChoice::Auto => {
                #[cfg(target_os = "linux")]
                {
                    notes.push(
                        "Real hardware backends are not implemented yet (milestone M1); run \
                         with --mock to see synthetic sensors."
                            .to_string(),
                    );
                    Vec::new()
                }
                #[cfg(not(target_os = "linux"))]
                {
                    notes.push(
                        "Hardware backends only exist on Linux; showing the mock backend."
                            .to_string(),
                    );
                    vec![Box::new(MockBackend::new(0x5eed))]
                }
            }
        };
        Self { backends, notes }
    }
}
