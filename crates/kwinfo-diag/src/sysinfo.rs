//! A machine summary for log headers and reports. Every item tolerates missing files,
//! so it works on macOS during development and on any Linux box.

use std::path::Path;

const SENSOR_MODULES: &[&str] = &[
    "coretemp",
    "k10temp",
    "zenpower",
    "ryzen_smu",
    "nct6775",
    "nct6683",
    "it87",
    "w83627ehf",
    "asus_ec_sensors",
    "asus_wmi_sensors",
    "drivetemp",
    "jc42",
    "spd5118",
    "ee1004",
    "i2c_i801",
    "i2c_piix4",
    "i915",
    "xe",
    "amdgpu",
    "nouveau",
    "nvidia",
    "msr",
    "intel_rapl_msr",
    "intel_rapl_common",
    "acpi_power_meter",
    "corsair_psu",
    "nzxt_kraken2",
    "nzxt_kraken3",
    "aquacomputer_d5next",
    "hid_sensor_hub",
    "thinkpad_acpi",
    "dell_smm_hwmon",
    "nvme",
];

const TOOLS: &[&str] = &[
    "sensors",
    "sensors-detect",
    "smartctl",
    "pkexec",
    "dmidecode",
];

/// Ordered `(key, value)` pairs describing this machine and session.
pub fn collect() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut push = |k: &str, v: String| out.push((k.to_string(), v));

    push(
        "os",
        format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
    );
    push(
        "kernel",
        read("/proc/sys/kernel/osrelease").unwrap_or_else(na),
    );
    push("distro", distro().unwrap_or_else(na));
    push("hostname", hostname());
    push("cpu", cpu_model().unwrap_or_else(na));
    push(
        "cpu_threads",
        std::thread::available_parallelism()
            .map(|n| n.get().to_string())
            .unwrap_or_else(|_| na()),
    );
    push("memory_total", meminfo_total().unwrap_or_else(na));
    push(
        "system",
        dmi(&["sys_vendor", "product_name", "product_version"]),
    );
    push(
        "board",
        dmi(&["board_vendor", "board_name", "board_version"]),
    );
    push("bios", dmi(&["bios_vendor", "bios_version", "bios_date"]));
    push("desktop", env_or_na("XDG_CURRENT_DESKTOP"));
    push("desktop_session", env_or_na("DESKTOP_SESSION"));
    push("session_type", env_or_na("XDG_SESSION_TYPE"));
    push(
        "wayland_display",
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            "set"
        } else {
            "unset"
        }
        .into(),
    );
    push(
        "x11_display",
        if std::env::var_os("DISPLAY").is_some() {
            "set"
        } else {
            "unset"
        }
        .into(),
    );
    push("appimage", env_or_na("APPIMAGE"));
    push("locale", env_or_na("LANG"));
    push("uid", uid().unwrap_or_else(na));
    push(
        "exe",
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| na()),
    );
    push(
        "cwd",
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| na()),
    );
    push("hwmon_chips", hwmon_chips());
    push("sensor_modules", loaded_modules());
    push("tools", tools());
    push("kwinfo_env", kwinfo_env());
    out
}

/// `key: value` lines.
pub fn render(pairs: &[(String, String)]) -> String {
    let mut text = String::new();
    for (key, value) in pairs {
        text.push_str(key);
        text.push_str(": ");
        text.push_str(value);
        text.push('\n');
    }
    text
}

fn na() -> String {
    "n/a".to_string()
}

fn read(path: impl AsRef<Path>) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn env_or_na(name: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(na)
}

fn distro() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    let field = |key: &str| {
        text.lines()
            .find_map(|line| {
                line.strip_prefix(key)
                    .and_then(|rest| rest.strip_prefix('='))
            })
            .map(|v| v.trim().trim_matches('"').to_string())
    };
    let pretty = field("PRETTY_NAME")?;
    Some(match field("VERSION_ID") {
        Some(version) if !pretty.contains(&version) => format!("{pretty} ({version})"),
        _ => pretty,
    })
}

fn hostname() -> String {
    read("/proc/sys/kernel/hostname")
        .or_else(|| read("/etc/hostname"))
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|h| !h.is_empty()))
        .unwrap_or_else(|| "unknown-host".to_string())
}

fn cpu_model() -> Option<String> {
    let text = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    text.lines()
        .find(|line| line.starts_with("model name"))
        .and_then(|line| line.split_once(':'))
        .map(|(_, model)| model.trim().to_string())
}

fn meminfo_total() -> Option<String> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kib: u64 = text
        .lines()
        .find(|line| line.starts_with("MemTotal:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some(format!("{:.1} GiB", kib as f64 / (1024.0 * 1024.0)))
}

fn dmi(fields: &[&str]) -> String {
    let parts: Vec<String> = fields
        .iter()
        .filter_map(|f| read(format!("/sys/class/dmi/id/{f}")))
        .collect();
    if parts.is_empty() {
        na()
    } else {
        parts.join(" / ")
    }
}

fn uid() -> Option<String> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let uid = status
        .lines()
        .find(|l| l.starts_with("Uid:"))?
        .split_whitespace()
        .nth(1)?;
    Some(if uid == "0" {
        "0 (root)".to_string()
    } else {
        uid.to_string()
    })
}

fn hwmon_chips() -> String {
    let Ok(entries) = std::fs::read_dir("/sys/class/hwmon") else {
        return na();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = read(e.path().join("name"))?;
            Some(format!("{}={name}", e.file_name().to_string_lossy()))
        })
        .collect();
    names.sort();
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(", ")
    }
}

fn loaded_modules() -> String {
    let Ok(text) = std::fs::read_to_string("/proc/modules") else {
        return na();
    };
    let mut found: Vec<&str> = text
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| SENSOR_MODULES.contains(name))
        .collect();
    found.sort_unstable();
    if found.is_empty() {
        "none of the known sensor modules".to_string()
    } else {
        found.join(", ")
    }
}

fn tools() -> String {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs: Vec<_> = std::env::split_paths(&path).collect();
    TOOLS
        .iter()
        .map(|tool| {
            let present = dirs.iter().any(|d| d.join(tool).is_file());
            format!("{tool}={}", if present { "yes" } else { "no" })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn kwinfo_env() -> String {
    let mut vars: Vec<String> = std::env::vars()
        .filter(|(k, _)| k.starts_with("KWINFO_"))
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    vars.sort();
    if vars.is_empty() {
        "none".to_string()
    } else {
        vars.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_a_summary_on_any_platform() {
        let pairs = collect();
        let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"os") && keys.contains(&"hwmon_chips") && keys.contains(&"tools"));
        assert!(pairs.iter().all(|(_, v)| !v.is_empty()));
        let text = render(&pairs);
        assert!(text.starts_with("os: "));
    }
}
