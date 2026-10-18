//! Turns raw channel values into the strings shown in the table and by the CLI.

use serde::{Deserialize, Serialize};

use crate::model::Unit;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TemperatureUnit {
    #[default]
    Celsius,
    Fahrenheit,
}

impl TemperatureUnit {
    pub fn symbol(self) -> &'static str {
        match self {
            TemperatureUnit::Celsius => "°C",
            TemperatureUnit::Fahrenheit => "°F",
        }
    }

    pub fn convert(self, celsius: f64) -> f64 {
        match self {
            TemperatureUnit::Celsius => celsius,
            TemperatureUnit::Fahrenheit => celsius * 9.0 / 5.0 + 32.0,
        }
    }
}

/// Shown for a channel that has no reading.
pub const NO_VALUE: &str = "–";

/// Formats a value the way the Sensor Status table shows it: fixed decimals, thousands
/// separators, the unit symbol, byte scaling, Yes/No for booleans.
pub fn format_value(
    value: Option<f64>,
    unit: Unit,
    decimals: u8,
    temperature: TemperatureUnit,
) -> String {
    let Some(v) = value else {
        return NO_VALUE.to_string();
    };
    if !v.is_finite() {
        return NO_VALUE.to_string();
    }
    match unit {
        Unit::Celsius => {
            format!(
                "{} {}",
                format_number(temperature.convert(v), decimals),
                temperature.symbol()
            )
        }
        Unit::Bool => (if v != 0.0 { "Yes" } else { "No" }).to_string(),
        Unit::Bytes => format_bytes(v, decimals),
        Unit::BytesPerSecond => format!("{}/s", format_bytes(v, decimals)),
        Unit::Count | Unit::None => format_number(v, decimals),
        other => format!("{} {}", format_number(v, decimals), other.symbol()),
    }
}

/// Fixed decimals with thousands separators: `format_number(2550.0, 1) == "2,550.0"`.
pub fn format_number(v: f64, decimals: u8) -> String {
    let plain = format!("{:.*}", usize::from(decimals), v);
    let (sign, rest) = plain
        .strip_prefix('-')
        .map_or(("", plain.as_str()), |r| ("-", r));
    let (int, frac) = rest
        .split_once('.')
        .map_or((rest, None), |(i, f)| (i, Some(f)));
    let mut grouped = String::with_capacity(int.len() + int.len() / 3);
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

fn format_bytes(v: f64, decimals: u8) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut magnitude = v.abs();
    let mut index = 0;
    while magnitude >= 1024.0 && index < UNITS.len() - 1 {
        magnitude /= 1024.0;
        index += 1;
    }
    let scaled = if v < 0.0 { -magnitude } else { magnitude };
    let decimals = if index == 0 { 0 } else { decimals };
    format!("{} {}", format_number(scaled, decimals), UNITS[index])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_get_thousands_separators() {
        assert_eq!(format_number(2550.0, 1), "2,550.0");
        assert_eq!(format_number(-1_234_567.891, 2), "-1,234,567.89");
        assert_eq!(format_number(999.0, 0), "999");
        assert_eq!(format_number(0.9449, 3), "0.945");
    }

    #[test]
    fn units_and_placeholders() {
        assert_eq!(
            format_value(None, Unit::Volt, 3, TemperatureUnit::Celsius),
            NO_VALUE
        );
        assert_eq!(
            format_value(Some(f64::NAN), Unit::Volt, 3, TemperatureUnit::Celsius),
            NO_VALUE
        );
        assert_eq!(
            format_value(Some(1.2345), Unit::Volt, 3, TemperatureUnit::Celsius),
            "1.234 V"
        );
        assert_eq!(
            format_value(Some(1.0), Unit::Bool, 0, TemperatureUnit::Celsius),
            "Yes"
        );
        assert_eq!(
            format_value(Some(0.0), Unit::Bool, 0, TemperatureUnit::Celsius),
            "No"
        );
        assert_eq!(
            format_value(Some(4120.0), Unit::Count, 0, TemperatureUnit::Celsius),
            "4,120"
        );
    }

    #[test]
    fn temperatures_follow_the_chosen_unit() {
        assert_eq!(
            format_value(Some(100.0), Unit::Celsius, 1, TemperatureUnit::Celsius),
            "100.0 °C"
        );
        assert_eq!(
            format_value(Some(100.0), Unit::Celsius, 1, TemperatureUnit::Fahrenheit),
            "212.0 °F"
        );
    }

    #[test]
    fn bytes_scale_to_a_readable_unit() {
        assert_eq!(
            format_value(Some(512.0), Unit::Bytes, 1, TemperatureUnit::Celsius),
            "512 B"
        );
        assert_eq!(
            format_value(Some(1536.0), Unit::Bytes, 1, TemperatureUnit::Celsius),
            "1.5 KB"
        );
        let gib = 30333.0 * 1024.0 * 1024.0;
        assert_eq!(
            format_value(Some(gib), Unit::Bytes, 1, TemperatureUnit::Celsius),
            "29.6 GB"
        );
        assert_eq!(
            format_value(
                Some(2.5 * 1024.0 * 1024.0),
                Unit::BytesPerSecond,
                1,
                TemperatureUnit::Celsius
            ),
            "2.5 MB/s"
        );
    }
}
