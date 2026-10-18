//! The `hwmon` backend: every chip under `/sys/class/hwmon` — Super I/O chips, `coretemp`,
//! `k10temp`, `nvme`, `drivetemp`, `amdgpu`, `acpitz`, PSU and cooler drivers, ...
//!
//! For each chip it reads `name`, the `device` link, and the attributes
//! `{temp,in,curr,power,energy,fan,freq}N_input` (or `powerN_average`), `pwmN` and
//! `intrusionN_alarm`, each with its `_label`, `_max`, `_crit` and `_alarm` where present.
//! Values are scaled from the sysfs units (m°C, mV, mA, µW, µJ, Hz, 0–255) to °C, V, A,
//! W, J, MHz and percent.
//!
//! Device ids are stable across reboots: `hwmon:<chip name>:<device>` where `<device>`
//! comes from the chip's `device` link (`nct6775.656`, `0000:03:00.0`, `thermal_zone0`),
//! never from the `hwmonN` number. Chips that describe a CPU, drive or GPU join that
//! component's group instead (`cpu:0`, `block:...`, `drm:...`), with their channel keys
//! prefixed by the chip name (`coretemp.temp1_input`).
//!
//! Labels come, in order of preference, from a [`ChipOverlay`] (libsensors
//! configuration), the driver's `_label` files, KWinfo's knowledge of a few chips, and
//! finally generic names ("Fan #2").

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::backend::Backend;
use crate::model::{Category, Channel, ChannelId, Device, DeviceId, Kind, Limits, Sample};
use crate::sysroot::{SysRoot, list_dir, read_i64, read_int, read_string};

use super::pci::PciNames;
use super::util::key_part;
use super::{block, cpu, dmi, drm};

/// Labels, ignore statements and computed values for hwmon chips from a source that
/// knows more than sysfs: libsensors and its configuration files in production, a fixed
/// table in tests.
pub trait ChipOverlay: Send {
    /// Called at the start of every enumeration.
    fn refresh(&mut self) {}

    /// What the overlay knows about the chip whose attributes live in `attr_dir` (a
    /// resolved path), or `None` if it does not know the chip.
    fn chip(&mut self, attr_dir: &Path) -> Option<OverlayChip>;

    /// The current value of `feature` (`in4`, `temp1`, `fan2`, ...) of the chip with
    /// handle `chip`, in base units (V, °C, RPM, W, J, A) with compute rules applied.
    fn read(&mut self, chip: usize, feature: &str) -> Option<f64>;
}

/// An overlay's view of one chip.
#[derive(Clone, Debug, Default)]
pub struct OverlayChip {
    /// Passed back to [`ChipOverlay::read`].
    pub handle: usize,
    /// Label per feature name. Features of a kind the overlay understands (everything
    /// except `pwm` and `freq`) that are missing from this map are ignored.
    pub labels: HashMap<String, String>,
}

/// The sensor types KWinfo shows, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum SensorType {
    Temp,
    In,
    Curr,
    Power,
    Energy,
    Fan,
    Pwm,
    Freq,
    Intrusion,
}

impl SensorType {
    fn from_prefix(prefix: &str) -> Option<Self> {
        Some(match prefix {
            "temp" => Self::Temp,
            "in" => Self::In,
            "curr" => Self::Curr,
            "power" => Self::Power,
            "energy" => Self::Energy,
            "fan" => Self::Fan,
            "pwm" => Self::Pwm,
            "freq" => Self::Freq,
            "intrusion" => Self::Intrusion,
            _ => return None,
        })
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::Temp => "temp",
            Self::In => "in",
            Self::Curr => "curr",
            Self::Power => "power",
            Self::Energy => "energy",
            Self::Fan => "fan",
            Self::Pwm => "pwm",
            Self::Freq => "freq",
            Self::Intrusion => "intrusion",
        }
    }

    fn kind(self) -> Kind {
        match self {
            Self::Temp => Kind::Temperature,
            Self::In => Kind::Voltage,
            Self::Curr => Kind::Current,
            Self::Power => Kind::Power,
            Self::Energy => Kind::Energy,
            Self::Fan => Kind::Fan,
            Self::Pwm => Kind::Pwm,
            Self::Freq => Kind::Clock,
            Self::Intrusion => Kind::Boolean,
        }
    }

    /// Converts a raw sysfs value to the channel's unit. Divides rather than multiplies
    /// by 1e-3 so that 84850 m°C becomes exactly 84.85 °C.
    fn convert(self, raw: i64) -> f64 {
        let raw = raw as f64;
        match self {
            Self::Temp | Self::In | Self::Curr => raw / 1e3,
            Self::Power | Self::Energy | Self::Freq => raw / 1e6,
            Self::Fan | Self::Intrusion => raw,
            Self::Pwm => raw * 100.0 / 255.0,
        }
    }

    /// Whether libsensors has a feature type for this sensor type.
    fn overlay_knows(self) -> bool {
        !matches!(self, Self::Pwm | Self::Freq)
    }

    /// Range of believable thresholds; drivers report "unset" as 0, -273 °C or huge values.
    fn plausible_limit(self, value: f64) -> bool {
        match self {
            Self::Temp => value > 0.0 && value < 250.0,
            _ => value > 0.0 && value < 1e6,
        }
    }
}

/// The attribute files of one sensor (`temp2_*`).
#[derive(Debug, Default)]
struct Feature {
    /// The value file: `temp2_input`, `power1_average`, `pwm1`, `intrusion0_alarm`.
    input: Option<String>,
    label: Option<String>,
    min: Option<f64>,
    max: Option<f64>,
    crit: Option<f64>,
    alarm: bool,
}

/// Splits `temp2_crit_alarm` into (`Temp`, 2, `crit_alarm`) and `pwm1` into (`Pwm`, 1, ``).
fn parse_attribute(name: &str) -> Option<(SensorType, u32, &str)> {
    let (head, item) = name.split_once('_').unwrap_or((name, ""));
    let digits = head.find(|c: char| c.is_ascii_digit())?;
    let (prefix, number) = head.split_at(digits);
    let kind = SensorType::from_prefix(prefix)?;
    Some((kind, number.parse().ok()?, item))
}

/// Reads the features of a chip, ordered by type then number.
fn read_features(attr_dir: &Path) -> BTreeMap<(SensorType, u32), Feature> {
    let mut features: BTreeMap<(SensorType, u32), Feature> = BTreeMap::new();
    for name in list_dir(attr_dir) {
        let Some((kind, number, item)) = parse_attribute(&name) else {
            continue;
        };
        let path = attr_dir.join(&name);
        let scaled = |path: &Path| read_i64(path).map(|v| kind.convert(v));
        let is_input = match kind {
            SensorType::Pwm => item.is_empty(),
            SensorType::Intrusion => item == "alarm",
            SensorType::Power => item == "input" || item == "average",
            _ => item == "input",
        };
        let feature = features.entry((kind, number)).or_default();
        if is_input {
            // Prefer `power1_input` over `power1_average` when a driver has both.
            if feature.input.is_none() || item == "input" {
                feature.input = Some(name.clone());
            }
            continue;
        }
        match item {
            "label" => feature.label = read_string(&path),
            "min" => feature.min = scaled(&path),
            "max" => feature.max = scaled(&path),
            "crit" => feature.crit = scaled(&path),
            "alarm" => feature.alarm = true,
            _ => {}
        }
    }
    features.retain(|_, f| f.input.is_some());
    features
}

/// One hwmon chip as found in sysfs.
#[derive(Debug)]
struct Chip {
    /// The `name` attribute, e.g. `nct6793`, `coretemp`, `nvme`.
    name: String,
    /// Where the attributes live: the hwmon directory, or its `device` directory for
    /// drivers that still use the pre-3.x layout.
    attr_dir: PathBuf,
    /// The resolved parent device, if the chip has one.
    device_path: Option<PathBuf>,
    /// Stable identification of the parent device: `nct6775.656`, `0000:03:00.0`, ...
    bus_id: String,
}

/// `coretemp.0`, `0000:00:18.3`, `thermal_zone0`; for USB HID devices the instance
/// counter that changes on every plug-in is replaced by the USB port path.
fn bus_id(device_path: Option<&Path>) -> String {
    let Some(path) = device_path else {
        return "virtual".to_string();
    };
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // `0003:1B1C:1C08.0005`: bus:vendor:product.instance
    let is_hid = base.len() == 19 && base.as_bytes()[4] == b':' && base.as_bytes()[14] == b'.';
    if is_hid && let Some(port) = path.parent().and_then(|p| p.file_name()) {
        return format!("{}:{}", port.to_string_lossy(), &base[..14]);
    }
    key_part(&base)
}

fn find_chips(root: &SysRoot) -> Vec<Chip> {
    let class = root.sys("class/hwmon");
    let mut chips = Vec::new();
    for entry in list_dir(&class) {
        let Some(dir) = root.resolve(&class.join(&entry)) else {
            continue;
        };
        let device_path = root.resolve(&dir.join("device"));
        let (attr_dir, name) = match read_string(&dir.join("name")) {
            Some(name) => (dir, name),
            None => {
                let Some(device) = device_path.clone() else {
                    continue;
                };
                let Some(name) = read_string(&device.join("name")) else {
                    continue;
                };
                (device, name)
            }
        };
        // Batteries and chargers register hwmon chips too; the power_supply backend
        // covers them.
        if device_path
            .as_ref()
            .is_some_and(|p| p.to_string_lossy().contains("/power_supply/"))
        {
            continue;
        }
        chips.push(Chip {
            name,
            bus_id: bus_id(device_path.as_deref()),
            attr_dir,
            device_path,
        });
    }
    chips
}

/// Which group a chip's rows belong to, from the driver name.
pub(crate) fn chip_category(name: &str) -> Category {
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| name.starts_with(p));
    match name {
        "coretemp" | "k10temp" | "k8temp" | "zenpower" | "fam15h_power" | "via_cputemp" => {
            Category::Cpu
        }
        "amdgpu" | "radeon" | "nouveau" | "i915" | "xe" => Category::Gpu,
        "nvme" | "drivetemp" => Category::Drive,
        "jc42" | "spd5118" => Category::Memory,
        "corsairpsu" => Category::Psu,
        "asusec" | "asus_wmi_sensors" | "gigabyte_wmi" | "dell_smm" | "thinkpad" | "applesmc"
        | "hp_wmi_sensors" => Category::Motherboard,
        _ if is_superio(name) => Category::Motherboard,
        _ if starts(&[
            "kraken",
            "nzxtsmart",
            "smart2",
            "corsaircpro",
            "d5next",
            "quadro",
            "octo",
            "farbwerk",
            "highflow",
            "aquaero",
            "aquastream",
            "leakshield",
        ]) =>
        {
            Category::Cooler
        }
        _ => Category::Other,
    }
}

/// Super I/O chips: Nuvoton, ITE, Winbond, Fintek and SMSC hardware monitors.
fn is_superio(name: &str) -> bool {
    let numbered = |prefix: &str| {
        name.strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
    };
    numbered("nct6")
        || numbered("nct7")
        || numbered("it8")
        || numbered("w83")
        || numbered("f71")
        || numbered("f81")
        || numbered("sch5")
}

/// The chip's marketing name: `nct6793` → `Nuvoton NCT6793D`, `it8686` → `ITE IT8686E`.
fn superio_name(name: &str) -> String {
    let upper = name.to_uppercase();
    if let Some(model) = name.strip_prefix("nct") {
        let suffix = match model {
            "6775" | "6776" => "F",
            m if m.len() == 4 && m.bytes().all(|b| b.is_ascii_digit()) => "D",
            _ => "",
        };
        format!("Nuvoton {upper}{suffix}")
    } else if name.starts_with("it8") {
        format!("ITE {upper}E")
    } else if name.starts_with("w83") {
        format!("Winbond {upper}")
    } else if name.starts_with("f71") || name.starts_with("f81") {
        format!("Fintek {upper}")
    } else if name.starts_with("sch5") {
        format!("SMSC {upper}")
    } else {
        upper
    }
}

/// Group labels for chips that stand alone.
fn standalone_label(name: &str, category: Category, board: Option<&str>) -> String {
    let with_board = |chip: &str| match board {
        Some(board) => format!("{board} ({chip})"),
        None => chip.to_string(),
    };
    match name {
        _ if is_superio(name) => with_board(&superio_name(name)),
        "acpitz" => "ACPI Thermal Zone".to_string(),
        "asusec" => with_board("ASUS EC"),
        "asus_wmi_sensors" => with_board("ASUS WMI"),
        "gigabyte_wmi" => with_board("Gigabyte WMI"),
        "dell_smm" => with_board("Dell SMM"),
        "thinkpad" => with_board("ThinkPad EC"),
        "applesmc" => with_board("Apple SMC"),
        "hp_wmi_sensors" => with_board("HP WMI"),
        "jc42" | "spd5118" => "DIMM Temperature Sensor".to_string(),
        "corsairpsu" => "Corsair PSU".to_string(),
        _ if name.starts_with("pch_") => {
            let mut family = name["pch_".len()..].replace('_', " ");
            if let Some(first) = family.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            format!("Intel PCH ({family})")
        }
        _ if name.starts_with("iwlwifi") => "Intel Wi-Fi".to_string(),
        _ if category == Category::Motherboard => with_board(name),
        _ => name.to_string(),
    }
}

/// Labels the nct6775 driver family leaves out, for the inputs the chips wire up
/// internally. They match lm-sensors' stock configuration; board-specific inputs
/// (+12V, +5V through dividers) need a board configuration in `/etc/sensors.d`.
fn builtin_label(chip: &str, kind: SensorType, number: u32) -> Option<&'static str> {
    let nct6775_family = matches!(
        chip,
        "nct6775"
            | "nct6776"
            | "nct6779"
            | "nct6791"
            | "nct6792"
            | "nct6793"
            | "nct6795"
            | "nct6796"
            | "nct6797"
            | "nct6798"
            | "nct6799"
    );
    if !nct6775_family || kind != SensorType::In {
        return None;
    }
    match number {
        0 => Some("Vcore"),
        2 => Some("AVCC"),
        3 => Some("+3.3V"),
        7 => Some("3VSB"),
        8 => Some("VBAT"),
        _ => None,
    }
}

/// What a chip's rows are called inside a component group, from the driver's label.
fn component_label(chip: &str, kind: SensorType, label: &str) -> Option<String> {
    match (chip, kind) {
        ("coretemp", SensorType::Temp) => {
            if label.starts_with("Package id") || label.starts_with("Physical id") {
                Some("CPU Package".to_string())
            } else {
                let core = label.strip_prefix("Core ")?;
                Some(format!("Core #{core}"))
            }
        }
        ("nvme", SensorType::Temp) => {
            if label == "Composite" {
                Some("Drive Temperature".to_string())
            } else {
                let n: u32 = label.strip_prefix("Sensor ")?.parse().ok()?;
                Some(format!("Drive Temperature {}", n + 1))
            }
        }
        _ => None,
    }
}

/// "Fan #2", "Voltage #4", "Temperature" for a chip's only temperature, ...
fn generic_label(kind: SensorType, number: u32, only_one: bool) -> String {
    let noun = match kind {
        SensorType::Temp => "Temperature",
        SensorType::In => "Voltage",
        SensorType::Curr => "Current",
        SensorType::Power => "Power",
        SensorType::Energy => "Energy",
        SensorType::Fan => "Fan",
        SensorType::Pwm => "PWM",
        SensorType::Freq => "Clock",
        SensorType::Intrusion => "Chassis Intrusion",
    };
    if only_one || (kind == SensorType::Intrusion && number == 0) {
        noun.to_string()
    } else {
        format!("{noun} #{number}")
    }
}

/// How one channel gets its value each tick.
#[derive(Debug)]
enum ReadFrom {
    File { path: PathBuf, kind: SensorType },
    Overlay { chip: usize, feature: String },
}

#[derive(Debug)]
struct Reader {
    channel: ChannelId,
    from: ReadFrom,
}

/// Channels computed from other channels of the same tick.
#[derive(Debug)]
enum Derived {
    /// The highest of the inputs: coretemp's "Core Max".
    Max {
        channel: ChannelId,
        inputs: Vec<usize>,
    },
    /// `limit - input`: "Core #N Distance to TjMAX".
    Below {
        channel: ChannelId,
        input: usize,
        limit: f64,
    },
}

/// See the module documentation.
pub struct HwmonBackend {
    root: SysRoot,
    overlay: Option<Box<dyn ChipOverlay>>,
    names: PciNames,
    readers: Vec<Reader>,
    derived: Vec<Derived>,
    values: Vec<Option<f64>>,
}

impl HwmonBackend {
    pub fn new(root: SysRoot) -> Self {
        Self {
            root,
            overlay: None,
            names: PciNames::default(),
            readers: Vec::new(),
            derived: Vec::new(),
            values: Vec::new(),
        }
    }

    /// Adds a source of labels and computed values, such as libsensors. An overlay
    /// describes the live system, so only pass one when the root is `/`.
    pub fn with_overlay(mut self, overlay: Option<Box<dyn ChipOverlay>>) -> Self {
        self.overlay = overlay;
        self
    }

    /// Adds `channel`, read from `from`, and returns its reader index.
    fn add(&mut self, channels: &mut Vec<Channel>, channel: Channel, from: ReadFrom) -> usize {
        self.readers.push(Reader {
            channel: channel.id.clone(),
            from,
        });
        channels.push(channel);
        self.readers.len() - 1
    }
}

/// Where a chip's rows go.
struct Placement {
    device: Device,
    /// Prefix for channel keys when the device is shared with other sources.
    key_prefix: String,
}

impl Backend for HwmonBackend {
    fn name(&self) -> &'static str {
        "hwmon"
    }

    fn enumerate(&mut self) -> anyhow::Result<Vec<(Device, Vec<Channel>)>> {
        self.readers.clear();
        self.derived.clear();
        if let Some(overlay) = &mut self.overlay {
            overlay.refresh();
        }
        let chips = find_chips(&self.root);
        if chips.is_empty() {
            return Ok(Vec::new());
        }
        let board = dmi::board_name(&self.root);
        let drives = if chips
            .iter()
            .any(|c| chip_category(&c.name) == Category::Drive)
        {
            block::drives(&self.root)
        } else {
            Vec::new()
        };
        let gpus = if chips
            .iter()
            .any(|c| chip_category(&c.name) == Category::Gpu)
        {
            drm::gpus(&self.root, &mut self.names)
        } else {
            Vec::new()
        };

        // CPU chips other than coretemp are numbered in bus order.
        let mut cpu_chip_index: HashMap<&str, Vec<&str>> = HashMap::new();
        for chip in &chips {
            if chip_category(&chip.name) == Category::Cpu && chip.name != "coretemp" {
                cpu_chip_index
                    .entry(chip.name.as_str())
                    .or_default()
                    .push(chip.bus_id.as_str());
            }
        }
        for buses in cpu_chip_index.values_mut() {
            buses.sort_unstable();
        }
        let mut name_counts: HashMap<&str, usize> = HashMap::new();
        for chip in &chips {
            *name_counts.entry(chip.name.as_str()).or_default() += 1;
        }

        let mut placements = Vec::with_capacity(chips.len());
        let mut seen_ids: HashMap<String, usize> = HashMap::new();
        let mut name_seen: HashMap<&str, usize> = HashMap::new();
        for chip in &chips {
            let category = chip_category(&chip.name);
            let prefix = format!("{}.", key_part(&chip.name));
            let component = match category {
                Category::Cpu => {
                    let package = if chip.name == "coretemp" {
                        chip.bus_id
                            .strip_prefix("coretemp.")
                            .and_then(|n| n.parse().ok())
                    } else {
                        cpu_chip_index.get(chip.name.as_str()).and_then(|buses| {
                            buses
                                .iter()
                                .position(|b| *b == chip.bus_id)
                                .and_then(|i| u32::try_from(i).ok())
                        })
                    };
                    package.map(|p| cpu::package_device(&self.root, p))
                }
                Category::Drive => drives
                    .iter()
                    .find(|d| d.hw_path.is_some() && d.hw_path == chip.device_path)
                    .map(|d| d.device.clone()),
                Category::Gpu => gpus
                    .iter()
                    .find(|g| chip.device_path.as_deref() == Some(g.pci_dir.as_path()))
                    .map(|g| g.device.clone()),
                _ => None,
            };
            let placement = match component {
                Some(device) => Placement {
                    device,
                    key_prefix: prefix,
                },
                None => {
                    let mut id = format!("hwmon:{}:{}", key_part(&chip.name), chip.bus_id);
                    let n = seen_ids.entry(id.clone()).or_default();
                    *n += 1;
                    if *n > 1 {
                        id = format!("{id}#{n}");
                    }
                    let mut label = standalone_label(&chip.name, category, board.as_deref());
                    if name_counts[chip.name.as_str()] > 1 {
                        let index = name_seen.entry(chip.name.as_str()).or_default();
                        label = format!("{label} [#{index}]");
                        *index += 1;
                    }
                    // A CPU, drive or GPU chip that matched no component keeps its
                    // category in a group of its own.
                    Placement {
                        device: Device::new(id, label, category),
                        key_prefix: String::new(),
                    }
                }
            };
            placements.push(placement);
        }

        let mut result: Vec<(Device, Vec<Channel>)> = Vec::new();
        for (chip, placement) in chips.iter().zip(placements) {
            let slot = match result.iter().position(|(d, _)| d.id == placement.device.id) {
                Some(slot) => slot,
                None => {
                    result.push((placement.device.clone(), Vec::new()));
                    result.len() - 1
                }
            };
            let mut channels = std::mem::take(&mut result[slot].1);
            self.add_chip(
                chip,
                &placement.device.id,
                &placement.key_prefix,
                &mut channels,
            );
            result[slot].1 = channels;
        }
        result.retain(|(_, channels)| !channels.is_empty());
        self.values = vec![None; self.readers.len()];
        Ok(result)
    }

    fn sample(&mut self, out: &mut Vec<Sample>) -> anyhow::Result<()> {
        for (i, reader) in self.readers.iter().enumerate() {
            let value = match &reader.from {
                ReadFrom::File { path, kind } => read_int(path).ok().map(|v| kind.convert(v)),
                ReadFrom::Overlay { chip, feature } => self
                    .overlay
                    .as_mut()
                    .and_then(|overlay| overlay.read(*chip, feature)),
            };
            self.values[i] = value;
            out.push(Sample::new(reader.channel.clone(), value));
        }
        for derived in &self.derived {
            match derived {
                Derived::Max { channel, inputs } => {
                    let max = inputs
                        .iter()
                        .filter_map(|&i| self.values[i])
                        .reduce(f64::max);
                    out.push(Sample::new(channel.clone(), max));
                }
                Derived::Below {
                    channel,
                    input,
                    limit,
                } => {
                    let value = self.values[*input].map(|v| limit - v);
                    out.push(Sample::new(channel.clone(), value));
                }
            }
        }
        Ok(())
    }
}

impl HwmonBackend {
    /// Adds the channels of `chip` to `channels` (the rows of `device`).
    fn add_chip(
        &mut self,
        chip: &Chip,
        device: &DeviceId,
        key_prefix: &str,
        channels: &mut Vec<Channel>,
    ) {
        let features = read_features(&chip.attr_dir);
        let overlay = self
            .overlay
            .as_mut()
            .and_then(|overlay| overlay.chip(&chip.attr_dir));
        let mut per_type: HashMap<SensorType, usize> = HashMap::new();
        for (kind, _) in features.keys() {
            *per_type.entry(*kind).or_default() += 1;
        }

        let mut alarms = Vec::new();
        let mut core_inputs: Vec<(usize, &str, Option<f64>)> = Vec::new();
        for (&(kind, number), feature) in &features {
            let Some(input) = &feature.input else {
                continue;
            };
            let feature_name = format!("{}{number}", kind.prefix());
            let overlay_label = match &overlay {
                Some(chip_overlay) if kind.overlay_knows() => {
                    match chip_overlay.labels.get(&feature_name) {
                        Some(label) => Some(label.as_str()),
                        // libsensors was told to ignore this feature.
                        None => continue,
                    }
                }
                _ => None,
            };
            // A label from the overlay counts only when it says more than sysfs does.
            let configured = overlay_label
                .filter(|l| *l != feature_name && Some(*l) != feature.label.as_deref());
            let label = match configured {
                Some(label) => label.to_string(),
                None => feature
                    .label
                    .as_deref()
                    .and_then(|l| component_label(&chip.name, kind, l))
                    .or_else(|| feature.label.clone())
                    .or_else(|| builtin_label(&chip.name, kind, number).map(str::to_string))
                    .unwrap_or_else(|| match (chip.name.as_str(), kind) {
                        ("nvme" | "drivetemp", SensorType::Temp) if number == 1 => {
                            "Drive Temperature".to_string()
                        }
                        _ => generic_label(kind, number, per_type[&kind] == 1),
                    }),
            };

            let plausible = |v: Option<f64>| v.filter(|&v| kind.plausible_limit(v));
            let limits = Limits {
                max: plausible(feature.max),
                crit: plausible(feature.crit),
            };
            let key = format!("{key_prefix}{input}");
            let channel =
                Channel::new(device, &key, label.clone(), kind.kind()).with_limits(limits);
            let from = match (&overlay, overlay_label) {
                (Some(chip_overlay), Some(_)) => ReadFrom::Overlay {
                    chip: chip_overlay.handle,
                    feature: feature_name.clone(),
                },
                _ => ReadFrom::File {
                    path: chip.attr_dir.join(input),
                    kind,
                },
            };
            let index = self.add(channels, channel, from);

            // Decided on the driver's label, so a renamed core still gets its rows.
            let is_core = feature
                .label
                .as_deref()
                .is_some_and(|l| l.starts_with("Core "));
            if chip.name == "coretemp" && kind == SensorType::Temp && is_core {
                core_inputs.push((index, input.as_str(), limits.crit));
            }
            let limited = [feature.min, feature.max, feature.crit]
                .into_iter()
                .any(|v| plausible(v).is_some());
            if feature.alarm && kind != SensorType::Intrusion && limited {
                alarms.push((kind, number, label));
            }
        }

        // coretemp: distance of each core to TjMAX, and the hottest core.
        if !core_inputs.is_empty() {
            for &(index, input, tjmax) in &core_inputs {
                let Some(limit) = tjmax else {
                    continue;
                };
                let base = &self.readers[index];
                let label = channels
                    .iter()
                    .find(|c| c.id == base.channel)
                    .map(|c| c.label.clone())
                    .unwrap_or_default();
                let number = input.trim_end_matches("_input");
                let channel = Channel::new(
                    device,
                    &format!("{key_prefix}{number}_distance"),
                    format!("{label} Distance to TjMAX"),
                    Kind::Temperature,
                );
                self.derived.push(Derived::Below {
                    channel: channel.id.clone(),
                    input: index,
                    limit,
                });
                channels.push(channel);
            }
            let channel = Channel::new(
                device,
                &format!("{key_prefix}core_max"),
                "Core Max",
                Kind::Temperature,
            );
            self.derived.push(Derived::Max {
                channel: channel.id.clone(),
                inputs: core_inputs.iter().map(|&(i, _, _)| i).collect(),
            });
            channels.push(channel);
        }

        // Alarm flags of inputs whose limits are set, after the readings themselves.
        for (kind, number, label) in alarms {
            let file = format!("{}{number}_alarm", kind.prefix());
            let channel = Channel::new(
                device,
                &format!("{key_prefix}{file}"),
                format!("{label} Alarm"),
                Kind::Boolean,
            );
            // Alarm flags are 0/1 like intrusion flags, so they convert the same way.
            let from = ReadFrom::File {
                path: chip.attr_dir.join(&file),
                kind: SensorType::Intrusion,
            };
            self.add(channels, channel, from);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::backends::test_support::*;
    use crate::model::Unit;

    fn board_id() -> &'static str {
        "hwmon:nct6793:nct6775.656"
    }

    #[test]
    fn attributes_split_into_type_number_and_item() {
        assert_eq!(
            parse_attribute("temp2_crit_alarm"),
            Some((SensorType::Temp, 2, "crit_alarm"))
        );
        assert_eq!(parse_attribute("pwm1"), Some((SensorType::Pwm, 1, "")));
        assert_eq!(
            parse_attribute("pwm1_enable"),
            Some((SensorType::Pwm, 1, "enable"))
        );
        assert_eq!(
            parse_attribute("intrusion0_alarm"),
            Some((SensorType::Intrusion, 0, "alarm"))
        );
        assert_eq!(parse_attribute("name"), None);
        assert_eq!(parse_attribute("cpu0_vid"), None);
        assert_eq!(parse_attribute("beep_enable"), None);
    }

    #[test]
    fn categories_follow_the_driver_name() {
        assert_eq!(chip_category("coretemp"), Category::Cpu);
        assert_eq!(chip_category("k10temp"), Category::Cpu);
        assert_eq!(chip_category("nct6793"), Category::Motherboard);
        assert_eq!(chip_category("nct6687"), Category::Motherboard);
        assert_eq!(chip_category("it8686"), Category::Motherboard);
        assert_eq!(chip_category("asusec"), Category::Motherboard);
        assert_eq!(chip_category("nvme"), Category::Drive);
        assert_eq!(chip_category("drivetemp"), Category::Drive);
        assert_eq!(chip_category("amdgpu"), Category::Gpu);
        assert_eq!(chip_category("jc42"), Category::Memory);
        assert_eq!(chip_category("corsairpsu"), Category::Psu);
        assert_eq!(chip_category("kraken3"), Category::Cooler);
        assert_eq!(chip_category("acpitz"), Category::Other);
        assert_eq!(chip_category("iwlwifi_1"), Category::Other);
        assert_eq!(superio_name("nct6793"), "Nuvoton NCT6793D");
        assert_eq!(superio_name("nct6775"), "Nuvoton NCT6775F");
        assert_eq!(superio_name("nct6687"), "Nuvoton NCT6687D");
        assert_eq!(superio_name("it8686"), "ITE IT8686E");
    }

    #[test]
    fn devices_have_stable_ids_and_groups() {
        let mut backend = HwmonBackend::new(fixture_root());
        let devices = backend.enumerate().unwrap();
        let summary: Vec<(&str, &str, Category)> = devices
            .iter()
            .map(|(d, _)| (d.id.as_str(), d.label.as_str(), d.category))
            .collect();
        let nvme = format!(
            "block:samsung-ssd-970-evo-plus-1tb:{}",
            crate::backends::util::short_hash("eui.0025385b71b02c5f")
        );
        assert_eq!(
            summary,
            [
                (
                    "hwmon:acpitz:thermal_zone0",
                    "ACPI Thermal Zone",
                    Category::Other
                ),
                ("cpu:0", "CPU [#0]: Intel Core i7-8700K", Category::Cpu),
                (
                    board_id(),
                    "ASUS PRIME Z370-A (Nuvoton NCT6793D)",
                    Category::Motherboard
                ),
                (
                    nvme.as_str(),
                    "Drive: Samsung SSD 970 EVO Plus 1TB",
                    Category::Drive
                ),
            ]
        );
    }

    #[test]
    fn coretemp_rows_join_the_cpu_group() {
        let mut backend = HwmonBackend::new(fixture_root());
        let (devices, samples) = enumerate_and_sample(&mut backend);
        let (_, channels) = device(&devices, "cpu:0");
        let mut expected = vec![(
            "coretemp.temp1_input".to_string(),
            "CPU Package".to_string(),
        )];
        for core in 0..6 {
            expected.push((
                format!("coretemp.temp{}_input", core + 2),
                format!("Core #{core}"),
            ));
        }
        for core in 0..6 {
            expected.push((
                format!("coretemp.temp{}_distance", core + 2),
                format!("Core #{core} Distance to TjMAX"),
            ));
        }
        expected.push(("coretemp.core_max".to_string(), "Core Max".to_string()));
        let actual: Vec<(String, String)> = channels
            .iter()
            .map(|c| (c.id.key().to_string(), c.label.clone()))
            .collect();
        assert_eq!(actual, expected);

        let package = channel(channels, "coretemp.temp1_input");
        assert_eq!(package.id.as_str(), "cpu:0/coretemp.temp1_input");
        assert_eq!(
            (package.kind, package.unit, package.decimals),
            (Kind::Temperature, Unit::Celsius, 1)
        );
        assert_eq!(
            package.limits,
            Limits {
                max: Some(80.0),
                crit: Some(100.0)
            }
        );

        let values = sample_map(&samples);
        let get = |key: &str| values[&ChannelId::from_raw(format!("cpu:0/{key}"))];
        assert_close(get("coretemp.temp1_input"), 38.0);
        assert_close(get("coretemp.temp2_input"), 35.0);
        assert_close(get("coretemp.temp5_input"), 37.0);
        assert_close(get("coretemp.temp2_distance"), 65.0);
        assert_close(get("coretemp.core_max"), 37.0);
    }

    #[test]
    fn super_io_rows_scale_and_label() {
        let mut backend = HwmonBackend::new(fixture_root());
        let (devices, samples) = enumerate_and_sample(&mut backend);
        let (_, channels) = device(&devices, board_id());
        assert_eq!(
            keys_and_labels(channels),
            [
                ("temp1_input", "SYSTIN"),
                ("temp2_input", "CPUTIN"),
                ("temp3_input", "AUXTIN0"),
                ("temp4_input", "AUXTIN1"),
                ("temp5_input", "AUXTIN2"),
                ("temp6_input", "AUXTIN3"),
                ("temp7_input", "PECI Agent 0"),
                ("temp8_input", "PCH_CHIP_CPU_MAX_TEMP"),
                ("temp9_input", "PCH_CHIP_TEMP"),
                ("temp10_input", "PCH_CPU_TEMP"),
                ("in0_input", "Vcore"),
                ("in1_input", "Voltage #1"),
                ("in2_input", "AVCC"),
                ("in3_input", "+3.3V"),
                ("in4_input", "Voltage #4"),
                ("in5_input", "Voltage #5"),
                ("in6_input", "Voltage #6"),
                ("in7_input", "3VSB"),
                ("in8_input", "VBAT"),
                ("in9_input", "Voltage #9"),
                ("in10_input", "Voltage #10"),
                ("in11_input", "Voltage #11"),
                ("in12_input", "Voltage #12"),
                ("in13_input", "Voltage #13"),
                ("in14_input", "Voltage #14"),
                ("fan1_input", "Fan #1"),
                ("fan2_input", "Fan #2"),
                ("fan3_input", "Fan #3"),
                ("fan4_input", "Fan #4"),
                ("fan5_input", "Fan #5"),
                ("fan6_input", "Fan #6"),
                ("pwm1", "PWM #1"),
                ("pwm2", "PWM #2"),
                ("pwm3", "PWM #3"),
                ("pwm4", "PWM #4"),
                ("pwm5", "PWM #5"),
                ("pwm6", "PWM #6"),
                ("intrusion0_alarm", "Chassis Intrusion"),
                ("intrusion1_alarm", "Chassis Intrusion #1"),
                // Alarm rows only for inputs whose limits the firmware set.
                ("in0_alarm", "Vcore Alarm"),
                ("in2_alarm", "AVCC Alarm"),
                ("in3_alarm", "+3.3V Alarm"),
                ("in7_alarm", "3VSB Alarm"),
                ("in8_alarm", "VBAT Alarm"),
            ]
        );
        let kinds = |key: &str| {
            let c = channel(channels, key);
            (c.kind, c.unit, c.decimals)
        };
        assert_eq!(kinds("in0_input"), (Kind::Voltage, Unit::Volt, 3));
        assert_eq!(kinds("fan2_input"), (Kind::Fan, Unit::Rpm, 0));
        assert_eq!(kinds("pwm1"), (Kind::Pwm, Unit::Percent, 1));
        assert_eq!(kinds("intrusion0_alarm"), (Kind::Boolean, Unit::Bool, 0));
        assert_eq!(kinds("in2_alarm"), (Kind::Boolean, Unit::Bool, 0));
        assert_eq!(
            channel(channels, "in2_input").limits,
            Limits {
                max: Some(3.632),
                crit: None
            }
        );
        assert!(
            channel(channels, "temp1_input").limits.is_empty(),
            "0 means unset"
        );

        let values = sample_map(&samples);
        let get = |key: &str| values[&ChannelId::from_raw(format!("{}/{key}", board_id()))];
        assert_close(get("in0_input"), 1.04);
        assert_close(get("in2_input"), 3.392);
        assert_close(get("temp2_input"), 36.5);
        assert_close(get("fan2_input"), 1132.0);
        assert_close(get("pwm1"), 40.0);
        assert_close(get("intrusion0_alarm"), 0.0);
        assert_close(get("intrusion1_alarm"), 1.0);
        assert_close(get("in2_alarm"), 0.0);
    }

    /// A stand-in for libsensors with a board configuration that names and scales the
    /// divided inputs, renames fans and ignores two unconnected inputs.
    struct BoardConfig {
        board_dir: PathBuf,
    }

    impl ChipOverlay for BoardConfig {
        fn chip(&mut self, attr_dir: &Path) -> Option<OverlayChip> {
            if attr_dir != self.board_dir {
                return None;
            }
            // Without a `label` statement libsensors reports the feature name.
            let mut labels: HashMap<String, String> = HashMap::new();
            for number in 1..=10 {
                labels.insert(format!("temp{number}"), format!("temp{number}"));
            }
            for number in 0..=14 {
                labels.insert(format!("in{number}"), format!("in{number}"));
            }
            for number in 1..=6 {
                labels.insert(format!("fan{number}"), format!("fan{number}"));
            }
            labels.insert("intrusion0".into(), "intrusion0".into());
            labels.insert("intrusion1".into(), "intrusion1".into());
            // `label` statements.
            labels.insert("in1".into(), "+5V".into());
            labels.insert("in4".into(), "+12V".into());
            labels.insert("temp1".into(), "Motherboard".into());
            labels.insert("temp2".into(), "CPU".into());
            labels.insert("fan1".into(), "Chassis Fan #1".into());
            labels.insert("fan2".into(), "CPU Fan".into());
            labels.insert("fan4".into(), "Pump".into());
            // `ignore` statements.
            labels.remove("in5");
            labels.remove("fan6");
            Some(OverlayChip { handle: 7, labels })
        }

        fn read(&mut self, chip: usize, feature: &str) -> Option<f64> {
            assert_eq!(chip, 7);
            // `compute in1 @*5`, `compute in4 @*12`; everything else as sysfs has it.
            let (raw, factor) = match feature {
                "in1" => (1.016, 5.0),
                "in4" => (1.008, 12.0),
                _ => (0.5, 1.0),
            };
            Some(raw * factor)
        }
    }

    #[test]
    fn overlay_labels_ignores_and_computes() {
        let root = fixture_root();
        let board_dir = root
            .resolve(&root.sys("class/hwmon/hwmon2"))
            .expect("the board chip is hwmon2");
        let overlay = BoardConfig { board_dir };
        let mut backend = HwmonBackend::new(root).with_overlay(Some(Box::new(overlay)));
        let (devices, samples) = enumerate_and_sample(&mut backend);
        let (device, channels) = device(&devices, board_id());
        assert_eq!(device.category, Category::Motherboard);
        let label = |key: &str| channel(channels, key).label.as_str();
        assert_eq!(label("in1_input"), "+5V");
        assert_eq!(label("in4_input"), "+12V");
        assert_eq!(label("temp1_input"), "Motherboard");
        assert_eq!(
            label("temp3_input"),
            "AUXTIN0",
            "unlabelled features keep sysfs names"
        );
        assert_eq!(label("in0_input"), "Vcore");
        assert_eq!(label("fan2_input"), "CPU Fan");
        assert_eq!(label("fan3_input"), "Fan #3");
        assert_eq!(label("pwm2"), "PWM #2", "libsensors has no pwm features");
        assert!(
            channels.iter().all(|c| c.id.key() != "in5_input"),
            "ignored"
        );
        assert!(
            channels.iter().all(|c| c.id.key() != "fan6_input"),
            "ignored"
        );

        let values = sample_map(&samples);
        let get = |key: &str| values[&ChannelId::from_raw(format!("{}/{key}", board_id()))];
        assert_close(get("in1_input"), 5.08);
        assert_close(get("in4_input"), 12.096);
        // PWM still comes from sysfs.
        assert_close(get("pwm1"), 40.0);
    }

    /// libsensors renames the first coretemp core and reads everything through sysfs.
    struct RenameCore;

    impl ChipOverlay for RenameCore {
        fn chip(&mut self, attr_dir: &Path) -> Option<OverlayChip> {
            if !attr_dir.ends_with("coretemp.0/hwmon/hwmon1") {
                return None;
            }
            let mut labels: HashMap<String, String> = HashMap::new();
            labels.insert("temp1".into(), "Package id 0".into());
            labels.insert("temp2".into(), "P-Core 0".into());
            for n in 3..=7 {
                labels.insert(format!("temp{n}"), format!("Core {}", n - 2));
            }
            Some(OverlayChip { handle: 0, labels })
        }

        fn read(&mut self, _chip: usize, feature: &str) -> Option<f64> {
            Some(if feature == "temp2" { 40.0 } else { 30.0 })
        }
    }

    #[test]
    fn renamed_cores_keep_their_derived_rows() {
        let mut backend =
            HwmonBackend::new(fixture_root()).with_overlay(Some(Box::new(RenameCore)));
        let (devices, samples) = enumerate_and_sample(&mut backend);
        let (_, channels) = device(&devices, "cpu:0");
        assert_eq!(channel(channels, "coretemp.temp2_input").label, "P-Core 0");
        assert_eq!(
            channel(channels, "coretemp.temp2_distance").label,
            "P-Core 0 Distance to TjMAX"
        );
        // The libsensors label equals the driver's, so KWinfo's own name stays.
        assert_eq!(channel(channels, "coretemp.temp3_input").label, "Core #1");
        let values = sample_map(&samples);
        let get = |key: &str| values[&ChannelId::from_raw(format!("cpu:0/{key}"))];
        assert_close(get("coretemp.temp2_distance"), 60.0);
        assert_close(get("coretemp.core_max"), 40.0);
    }
}
