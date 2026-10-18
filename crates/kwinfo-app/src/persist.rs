//! Saving the configuration: a short while after the last change, and at once on quit.

use std::path::Path;
use std::time::{Duration, Instant};

use kwinfo_core::{Config, ConfigFile};

/// Quiet time after the last change before the file is written, so that dragging a
/// column or a colour does not write the file on every frame.
pub const SAVE_DELAY: Duration = Duration::from_millis(800);

pub struct Persistence {
    /// `None` keeps the settings in memory only (tests and previews).
    file: Option<ConfigFile>,
    /// What the file holds.
    saved: Config,
    /// The configuration at the last [`Persistence::update`], and when it last changed.
    seen: Config,
    changed_at: Option<Instant>,
    error: Option<String>,
}

impl Persistence {
    /// `loaded` is what the file held at start.
    pub fn new(file: Option<ConfigFile>, loaded: &Config) -> Self {
        Self {
            file,
            saved: loaded.clone(),
            seen: loaded.clone(),
            changed_at: None,
            error: None,
        }
    }

    /// Call once per frame. Writes the file once `config` has stayed the same for
    /// [`SAVE_DELAY`] after a change, and returns how long until that is due so the
    /// caller can ask for a repaint then.
    pub fn update(&mut self, config: &Config, now: Instant) -> Option<Duration> {
        if *config != self.seen {
            self.seen.clone_from(config);
            self.changed_at = Some(now);
        }
        let due = self.changed_at? + SAVE_DELAY;
        if now >= due {
            self.flush(config);
            None
        } else {
            Some(due - now)
        }
    }

    /// Writes `config` now if it differs from what the file holds.
    pub fn flush(&mut self, config: &Config) {
        self.changed_at = None;
        self.seen.clone_from(config);
        if *config == self.saved {
            return;
        }
        let Some(file) = &self.file else {
            self.saved.clone_from(config);
            return;
        };
        match file.save(config) {
            Ok(()) => {
                tracing::debug!("settings saved to {}", file.path().display());
                self.saved.clone_from(config);
                self.error = None;
            }
            Err(err) => {
                tracing::warn!("could not save the settings: {err:#}");
                self.error = Some(format!("{err:#}"));
            }
        }
    }

    /// Whether `config` has changes the file does not have yet.
    pub fn is_dirty(&self, config: &Config) -> bool {
        *config != self.saved
    }

    pub fn path(&self) -> Option<&Path> {
        self.file.as_ref().map(ConfigFile::path)
    }

    /// Why the last save failed, if it did.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_after_a_quiet_period_and_on_flush() {
        let dir = tempfile::tempdir().unwrap();
        let file = ConfigFile::new(dir.path().join("config.toml"));
        let mut config = Config::default();
        let mut persistence = Persistence::new(Some(file.clone()), &config);
        let t0 = Instant::now();
        assert_eq!(persistence.update(&config, t0), None, "nothing to do");

        config.polling.interval_ms = 2000;
        assert_eq!(persistence.update(&config, t0), Some(SAVE_DELAY));
        // Another change restarts the wait.
        config.polling.interval_ms = 500;
        let t1 = t0 + SAVE_DELAY / 2;
        assert_eq!(persistence.update(&config, t1), Some(SAVE_DELAY));
        assert!(!file.path().exists());
        assert!(persistence.is_dirty(&config));

        assert_eq!(persistence.update(&config, t1 + SAVE_DELAY), None);
        assert_eq!(file.load().unwrap().polling.interval_ms, 500);
        assert!(!persistence.is_dirty(&config));

        config.general.show_hidden = true;
        persistence.update(&config, t1 + SAVE_DELAY * 2);
        persistence.flush(&config);
        assert!(file.load().unwrap().general.show_hidden);
        assert_eq!(persistence.path(), Some(file.path()));
    }

    #[test]
    fn write_errors_are_reported_and_retried_on_flush() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where the file should be makes every write fail.
        let path = dir.path().join("config.toml");
        std::fs::create_dir(&path).unwrap();
        let mut config = Config::default();
        let mut persistence = Persistence::new(Some(ConfigFile::new(&path)), &config);
        config.polling.interval_ms = 250;
        persistence.flush(&config);
        assert!(persistence.error().is_some());
        assert!(persistence.is_dirty(&config));
    }

    #[test]
    fn memory_only_settings_never_touch_the_disk() {
        let mut config = Config::default();
        let mut persistence = Persistence::new(None, &config);
        config.polling.interval_ms = 250;
        persistence.flush(&config);
        assert!(!persistence.is_dirty(&config));
        assert_eq!(persistence.path(), None);
    }
}
