//! The user's settings, kept in `config.toml` in the XDG configuration directory:
//! `~/.config/kwinfo/config.toml` on Linux (and
//! `~/Library/Application Support/io.github.kylanwalters.kwinfo/config.toml` when developing
//! on macOS).
//!
//! Per-sensor settings are keyed by [`ChannelId`] and per-device settings by [`DeviceId`].
//! Both stay the same across reboots, so the settings follow the hardware. Every field
//! has a default, unknown keys are ignored, and entries that only hold defaults are left
//! out of the file, which keeps it short and readable by older and newer KWinfo versions:
//!
//! ```toml
//! [general]
//! temperature_unit = "Celsius"
//! show_hidden = false
//!
//! [theme]
//! mode = "auto"
//!
//! [polling]
//! interval_ms = 1000
//!
//! [columns.average]
//! visible = false
//!
//! [window]
//! width = 780.0
//! height = 720.0
//!
//! [devices."cpu:0"]
//! collapsed_groups = ["Thread Usage"]
//!
//! [channels."cpu:0/clock.core0"]
//! label = "P-core 0"
//! color = "#ff8c00"
//! hidden = true
//! ```

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::Context as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::format::TemperatureUnit;
use crate::model::{Channel, ChannelId, DeviceId};

/// The polling period when the user has not chosen one.
pub const DEFAULT_INTERVAL_MS: u64 = 1000;

/// The file name inside the configuration directory.
pub const FILE_NAME: &str = "config.toml";

/// Everything KWinfo remembers between runs. See the module documentation for the file
/// layout.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub theme: ThemeSettings,
    pub polling: Polling,
    #[serde(skip_serializing_if = "Columns::is_default")]
    pub columns: Columns,
    #[serde(skip_serializing_if = "WindowSettings::is_default")]
    pub window: WindowSettings,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub devices: BTreeMap<DeviceId, DeviceSettings>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub channels: BTreeMap<ChannelId, ChannelSettings>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub temperature_unit: TemperatureUnit,
    /// Lists hidden sensors anyway, greyed out, so that they can be un-hidden.
    pub show_hidden: bool,
}

/// The theme setting. `Auto` follows the desktop's dark/light preference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Auto,
    Dark,
    Light,
}

impl ThemeMode {
    pub const ALL: [ThemeMode; 3] = [ThemeMode::Auto, ThemeMode::Dark, ThemeMode::Light];

    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::Auto => "Follow system",
            ThemeMode::Dark => "Dark",
            ThemeMode::Light => "Light",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemeSettings {
    pub mode: ThemeMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Polling {
    /// Time between two sampling ticks.
    pub interval_ms: u64,
}

impl Default for Polling {
    fn default() -> Self {
        Self {
            interval_ms: DEFAULT_INTERVAL_MS,
        }
    }
}

/// The columns of the Sensor Status table, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Column {
    Sensor,
    Current,
    Minimum,
    Maximum,
    Average,
}

impl Column {
    pub const ALL: [Column; 5] = [
        Column::Sensor,
        Column::Current,
        Column::Minimum,
        Column::Maximum,
        Column::Average,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Column::Sensor => "Sensor",
            Column::Current => "Current",
            Column::Minimum => "Minimum",
            Column::Maximum => "Maximum",
            Column::Average => "Average",
        }
    }

    /// The value columns can be hidden; the Sensor column is always shown.
    pub fn can_hide(self) -> bool {
        self != Column::Sensor
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColumnSettings {
    #[serde(skip_serializing_if = "is_true")]
    pub visible: bool,
    /// Width in points; `None` means the built-in default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f32>,
}

impl Default for ColumnSettings {
    fn default() -> Self {
        Self {
            visible: true,
            width: None,
        }
    }
}

impl ColumnSettings {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Columns {
    #[serde(skip_serializing_if = "ColumnSettings::is_default")]
    pub sensor: ColumnSettings,
    #[serde(skip_serializing_if = "ColumnSettings::is_default")]
    pub current: ColumnSettings,
    #[serde(skip_serializing_if = "ColumnSettings::is_default")]
    pub minimum: ColumnSettings,
    #[serde(skip_serializing_if = "ColumnSettings::is_default")]
    pub maximum: ColumnSettings,
    #[serde(skip_serializing_if = "ColumnSettings::is_default")]
    pub average: ColumnSettings,
}

impl Columns {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn get(&self, column: Column) -> &ColumnSettings {
        match column {
            Column::Sensor => &self.sensor,
            Column::Current => &self.current,
            Column::Minimum => &self.minimum,
            Column::Maximum => &self.maximum,
            Column::Average => &self.average,
        }
    }

    pub fn get_mut(&mut self, column: Column) -> &mut ColumnSettings {
        match column {
            Column::Sensor => &mut self.sensor,
            Column::Current => &mut self.current,
            Column::Minimum => &mut self.minimum,
            Column::Maximum => &mut self.maximum,
            Column::Average => &mut self.average,
        }
    }

    pub fn is_visible(&self, column: Column) -> bool {
        !column.can_hide() || self.get(column).visible
    }

    /// The columns to show, in display order. Always starts with [`Column::Sensor`].
    pub fn visible(&self) -> impl Iterator<Item = Column> + '_ {
        Column::ALL.into_iter().filter(|&c| self.is_visible(c))
    }

    /// Forgets every width the user dragged, keeping visibility.
    pub fn reset_widths(&mut self) {
        for column in Column::ALL {
            self.get_mut(column).width = None;
        }
    }
}

/// The main window's last size, in points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f32>,
}

impl WindowSettings {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn size(&self) -> Option<[f32; 2]> {
        Some([self.width?, self.height?])
    }
}

/// Settings of one device (a group row of the table).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceSettings {
    #[serde(skip_serializing_if = "is_false")]
    pub collapsed: bool,
    /// Sub-groups ([`Channel::group`]) the user collapsed, by name.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub collapsed_groups: BTreeSet<String>,
    /// "Disable monitoring" for the whole device: none of its channels is polled.
    #[serde(skip_serializing_if = "is_false")]
    pub disabled: bool,
}

impl DeviceSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Settings of one sensor (a row of the table).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelSettings {
    /// The user's name for the row, replacing the backend's label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The user's colour for the row (and later its graph), stored once for both themes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Rgb>,
    /// Not listed, and counted in the window title's "[N value(s) hidden]".
    #[serde(skip_serializing_if = "is_false")]
    pub hidden: bool,
    /// "Disable monitoring": the channel is not polled.
    #[serde(skip_serializing_if = "is_false")]
    pub disabled: bool,
}

impl ChannelSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl Config {
    /// Parses a configuration file's contents.
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// The file contents for this configuration, without default-only entries.
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        let mut pruned = self.clone();
        pruned.prune();
        toml::to_string(&pruned)
    }

    pub fn channel(&self, id: &ChannelId) -> Option<&ChannelSettings> {
        self.channels.get(id)
    }

    pub fn device(&self, id: &DeviceId) -> Option<&DeviceSettings> {
        self.devices.get(id)
    }

    /// Changes a channel's settings through `f`. An entry left with only defaults is
    /// removed, so the configuration never grows from toggling a setting back and forth.
    pub fn update_channel<R>(
        &mut self,
        id: &ChannelId,
        f: impl FnOnce(&mut ChannelSettings) -> R,
    ) -> R {
        let entry = self.channels.entry(id.clone()).or_default();
        let result = f(entry);
        if entry.is_default() {
            self.channels.remove(id);
        }
        result
    }

    /// Changes a device's settings through `f`; see [`Config::update_channel`].
    pub fn update_device<R>(
        &mut self,
        id: &DeviceId,
        f: impl FnOnce(&mut DeviceSettings) -> R,
    ) -> R {
        let entry = self.devices.entry(id.clone()).or_default();
        let result = f(entry);
        if entry.is_default() {
            self.devices.remove(id);
        }
        result
    }

    /// The row label: the user's name if they gave one, else the backend's.
    pub fn label_of<'a>(&'a self, channel: &'a Channel) -> &'a str {
        self.channel(&channel.id)
            .and_then(|c| c.label.as_deref())
            .unwrap_or(&channel.label)
    }

    pub fn color_of(&self, id: &ChannelId) -> Option<Rgb> {
        self.channel(id).and_then(|c| c.color)
    }

    pub fn is_hidden(&self, id: &ChannelId) -> bool {
        self.channel(id).is_some_and(|c| c.hidden)
    }

    /// Whether the channel is switched off itself.
    pub fn is_channel_disabled(&self, id: &ChannelId) -> bool {
        self.channel(id).is_some_and(|c| c.disabled)
    }

    pub fn is_device_disabled(&self, id: &DeviceId) -> bool {
        self.device(id).is_some_and(|d| d.disabled)
    }

    /// Whether the channel is not polled, because it or its device is switched off.
    pub fn is_disabled(&self, channel: &Channel) -> bool {
        self.is_channel_disabled(&channel.id) || self.is_device_disabled(&channel.device)
    }

    pub fn is_collapsed(&self, id: &DeviceId) -> bool {
        self.device(id).is_some_and(|d| d.collapsed)
    }

    pub fn is_group_collapsed(&self, id: &DeviceId, group: &str) -> bool {
        self.device(id)
            .is_some_and(|d| d.collapsed_groups.contains(group))
    }

    pub fn set_collapsed(&mut self, id: &DeviceId, collapsed: bool) {
        self.update_device(id, |d| d.collapsed = collapsed);
    }

    pub fn set_group_collapsed(&mut self, id: &DeviceId, group: &str, collapsed: bool) {
        self.update_device(id, |d| {
            if collapsed {
                d.collapsed_groups.insert(group.to_string());
            } else {
                d.collapsed_groups.remove(group);
            }
        });
    }

    /// Channels switched off one by one (not counting those of disabled devices).
    pub fn disabled_channels(&self) -> HashSet<ChannelId> {
        self.channels
            .iter()
            .filter(|(_, c)| c.disabled)
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn disabled_devices(&self) -> HashSet<DeviceId> {
        self.devices
            .iter()
            .filter(|(_, d)| d.disabled)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Removes entries that only hold defaults.
    pub fn prune(&mut self) {
        self.channels.retain(|_, c| !c.is_default());
        self.devices.retain(|_, d| !d.is_default());
    }
}

/// A colour as the user picked it, written as `"#rrggbb"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

/// Why a string is not an `#rrggbb` colour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseRgbError(String);

impl fmt::Display for ParseRgbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} is not a colour of the form #rrggbb", self.0)
    }
}

impl std::error::Error for ParseRgbError {}

impl FromStr for Rgb {
    type Err = ParseRgbError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let error = || ParseRgbError(s.to_string());
        let hex = s.trim().strip_prefix('#').unwrap_or(s.trim());
        if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(error());
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| error());
        Ok(Self::new(byte(0)?, byte(2)?, byte(4)?))
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// The location of `config.toml`, and reading and writing it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigFile {
    path: PathBuf,
}

impl ConfigFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `config.toml` in KWinfo's directory under the platform's configuration directory
    /// (`$XDG_CONFIG_HOME/kwinfo`, by default `~/.config/kwinfo`, on Linux). `None` when
    /// the home directory is unknown.
    pub fn default_location() -> Option<Self> {
        directories::ProjectDirs::from("io.github", "kylanwalters", "kwinfo")
            .map(|dirs| Self::new(dirs.config_dir().join(FILE_NAME)))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the file. A file that does not exist yet is the default configuration.
    pub fn load(&self) -> anyhow::Result<Config> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(err) => {
                return Err(err).with_context(|| format!("reading {}", self.path.display()));
            }
        };
        Config::from_toml(&text).with_context(|| format!("parsing {}", self.path.display()))
    }

    /// Like [`ConfigFile::load`], but never fails. A file that cannot be read or parsed
    /// is renamed to `config.toml.bad`, so that the next save does not destroy what the
    /// user wrote, and the defaults are used. The second value explains what happened.
    pub fn load_or_default(&self) -> (Config, Option<String>) {
        match self.load() {
            Ok(config) => (config, None),
            Err(err) => {
                let aside = self.path.with_extension("toml.bad");
                let note = match fs::rename(&self.path, &aside) {
                    Ok(()) => format!(
                        "The settings file could not be used ({err:#}); it was kept as {} \
                         and the defaults are in effect.",
                        aside.display()
                    ),
                    Err(rename) => format!(
                        "The settings file could not be used ({err:#}) nor moved aside \
                         ({rename}); the defaults are in effect."
                    ),
                };
                tracing::warn!("{note}");
                (Config::default(), Some(note))
            }
        }
    }

    /// Writes `config` to the file, creating its directory. The new contents go to a
    /// temporary file that then replaces the old one, so a crash never leaves half a file.
    pub fn save(&self, config: &Config) -> anyhow::Result<()> {
        let text = config.to_toml().context("serialising the settings")?;
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let temporary = self.path.with_extension("toml.tmp");
        let write = || -> io::Result<()> {
            let mut file = fs::File::create(&temporary)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)
        };
        write().with_context(|| format!("writing {}", self.path.display()))
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_true(value: &bool) -> bool {
    *value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Kind;

    fn channel(key: &str) -> Channel {
        Channel::new(&DeviceId::new("cpu:0"), key, "Core #0 Clock", Kind::Clock)
    }

    #[test]
    fn the_default_configuration_writes_only_the_main_sections() {
        let text = Config::default().to_toml().unwrap();
        assert!(text.contains("[general]"), "{text}");
        assert!(text.contains("interval_ms = 1000"), "{text}");
        assert!(text.contains("mode = \"auto\""), "{text}");
        for absent in ["[columns", "[window", "[devices", "[channels"] {
            assert!(!text.contains(absent), "{absent} in {text}");
        }
        assert_eq!(Config::from_toml(&text).unwrap(), Config::default());
        assert_eq!(Config::from_toml("").unwrap(), Config::default());
    }

    #[test]
    fn per_sensor_settings_round_trip() {
        let mut config = Config::default();
        let clock = channel("clock.core0");
        config.update_channel(&clock.id, |c| {
            c.label = Some("P-core 0".into());
            c.color = Some(Rgb::new(0xff, 0x8c, 0x00));
            c.hidden = true;
        });
        config.update_channel(&ChannelId::from_raw("cpu:0/usage.cpu3"), |c| {
            c.disabled = true;
        });
        config.update_device(&DeviceId::new("hwmon:nct6793:nct6775.656"), |d| {
            d.collapsed = true;
            d.disabled = true;
        });
        config.set_group_collapsed(&DeviceId::new("cpu:0"), "Thread Usage", true);
        config.general.temperature_unit = TemperatureUnit::Fahrenheit;
        config.general.show_hidden = true;
        config.theme.mode = ThemeMode::Light;
        config.polling.interval_ms = 2000;
        config.columns.average.visible = false;
        config.columns.sensor.width = Some(280.0);
        config.window = WindowSettings {
            width: Some(900.0),
            height: Some(640.0),
        };

        let text = config.to_toml().unwrap();
        assert!(
            text.contains("[channels.\"cpu:0/clock.core0\"]"),
            "ids are quoted keys: {text}"
        );
        assert!(text.contains("color = \"#ff8c00\""), "{text}");
        assert!(text.contains("[columns.average]"), "{text}");
        assert!(!text.contains("[columns.current]"), "{text}");
        let back = Config::from_toml(&text).unwrap();
        assert_eq!(back, config);
        assert_eq!(back.label_of(&clock), "P-core 0");
        assert!(back.is_hidden(&clock.id));
        assert!(back.is_group_collapsed(&DeviceId::new("cpu:0"), "Thread Usage"));
        assert_eq!(
            back.disabled_channels(),
            HashSet::from([ChannelId::from_raw("cpu:0/usage.cpu3")])
        );
        assert_eq!(
            back.disabled_devices(),
            HashSet::from([DeviceId::new("hwmon:nct6793:nct6775.656")])
        );
        assert_eq!(back.window.size(), Some([900.0, 640.0]));
    }

    #[test]
    fn unknown_keys_are_ignored_and_missing_ones_default() {
        let text = r#"
            future_section = { anything = 1 }

            [general]
            show_hidden = true
            start_minimised = true

            [channels."cpu:0/clock.core0"]
            hidden = true
            tray = true
        "#;
        let config = Config::from_toml(text).unwrap();
        assert!(config.general.show_hidden);
        assert_eq!(config.general.temperature_unit, TemperatureUnit::Celsius);
        assert_eq!(config.polling.interval_ms, DEFAULT_INTERVAL_MS);
        assert!(config.is_hidden(&ChannelId::from_raw("cpu:0/clock.core0")));
    }

    #[test]
    fn entries_back_at_their_defaults_disappear() {
        let mut config = Config::default();
        let id = ChannelId::from_raw("cpu:0/clock.core0");
        config.update_channel(&id, |c| c.hidden = true);
        assert!(config.channel(&id).is_some());
        config.update_channel(&id, |c| c.hidden = false);
        assert!(config.channel(&id).is_none());

        let device = DeviceId::new("cpu:0");
        config.set_group_collapsed(&device, "Core Clocks", true);
        config.set_collapsed(&device, true);
        config.set_group_collapsed(&device, "Core Clocks", false);
        assert!(config.is_collapsed(&device));
        config.set_collapsed(&device, false);
        assert!(config.devices.is_empty());

        config
            .channels
            .insert(id.clone(), ChannelSettings::default());
        config.prune();
        assert!(config.channels.is_empty());
    }

    #[test]
    fn device_switches_disable_their_channels() {
        let mut config = Config::default();
        let clock = channel("clock.core0");
        assert!(!config.is_disabled(&clock));
        config.update_device(&clock.device, |d| d.disabled = true);
        assert!(config.is_disabled(&clock));
        assert!(!config.is_channel_disabled(&clock.id));
    }

    #[test]
    fn colours_parse_and_print_as_hex() {
        assert_eq!("#FF8c00".parse(), Ok(Rgb::new(255, 140, 0)));
        assert_eq!(" 0a64c2 ".parse(), Ok(Rgb::new(10, 100, 194)));
        assert_eq!(Rgb::new(1, 2, 3).to_string(), "#010203");
        for bad in ["", "#fff", "#gg0000", "#ff00001", "#ff 000"] {
            assert!(bad.parse::<Rgb>().is_err(), "{bad:?}");
        }
        let error = Config::from_toml("[channels.\"a/b\"]\ncolor = \"red\"\n").unwrap_err();
        assert!(error.to_string().contains("#rrggbb"), "{error}");
    }

    #[test]
    fn columns_keep_the_sensor_column_and_the_display_order() {
        let mut columns = Columns::default();
        columns.sensor.visible = false;
        columns.minimum.visible = false;
        columns.average.width = Some(120.0);
        let visible: Vec<Column> = columns.visible().collect();
        assert_eq!(
            visible,
            vec![
                Column::Sensor,
                Column::Current,
                Column::Maximum,
                Column::Average
            ]
        );
        columns.reset_widths();
        assert_eq!(columns.average.width, None);
        assert!(
            !columns.minimum.visible,
            "visibility survives a width reset"
        );
    }

    #[test]
    fn files_save_atomically_and_load_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = ConfigFile::new(dir.path().join("nested/kwinfo/config.toml"));
        assert_eq!(
            file.load().unwrap(),
            Config::default(),
            "missing is default"
        );

        let mut config = Config::default();
        config.polling.interval_ms = 500;
        config.update_channel(&ChannelId::from_raw("cpu:0/temp"), |c| c.hidden = true);
        file.save(&config).unwrap();
        assert_eq!(file.load().unwrap(), config);
        assert!(!file.path().with_extension("toml.tmp").exists());
    }

    #[test]
    fn broken_files_are_kept_aside() {
        let dir = tempfile::tempdir().unwrap();
        let file = ConfigFile::new(dir.path().join("config.toml"));
        fs::write(file.path(), "[general\nshow_hidden = maybe").unwrap();
        let (config, note) = file.load_or_default();
        assert_eq!(config, Config::default());
        let note = note.expect("the user is told");
        assert!(note.contains("config.toml.bad"), "{note}");
        assert!(dir.path().join("config.toml.bad").exists());
        assert!(!file.path().exists());

        let (_, note) = file.load_or_default();
        assert!(note.is_none(), "a missing file is not an error");
    }
}
