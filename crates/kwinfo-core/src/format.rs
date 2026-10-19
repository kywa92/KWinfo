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

    /// The inverse of [`TemperatureUnit::convert`].
    pub fn to_celsius(self, value: f64) -> f64 {
        match self {
            TemperatureUnit::Celsius => value,
            TemperatureUnit::Fahrenheit => (value - 32.0) * 5.0 / 9.0,
        }
    }
}

/// `value`, in `unit`'s base unit, in the unit it is shown in: temperatures in the chosen
/// temperature unit, everything else unchanged.
pub fn to_display(value: f64, unit: Unit, temperature: TemperatureUnit) -> f64 {
    if unit == Unit::Celsius {
        temperature.convert(value)
    } else {
        value
    }
}

/// The inverse of [`to_display`].
pub fn from_display(value: f64, unit: Unit, temperature: TemperatureUnit) -> f64 {
    if unit == Unit::Celsius {
        temperature.to_celsius(value)
    } else {
        value
    }
}

/// A value for an edit field, such as a graph's range: the number as the table shows it,
/// without the unit symbol, except that byte sizes keep their scaled unit (`1.5 GB`).
pub fn format_input(value: f64, unit: Unit, decimals: u8, temperature: TemperatureUnit) -> String {
    if !value.is_finite() {
        return String::new();
    }
    match unit {
        Unit::Bytes => format_bytes(value, decimals),
        Unit::BytesPerSecond => format!("{}/s", format_bytes(value, decimals)),
        _ => format_number(to_display(value, unit, temperature), decimals),
    }
}

/// Reads what the user typed into a value field, returning the value in `unit`'s base
/// unit. Accepts surrounding spaces, thousands separators and the unit symbols the table
/// shows (`48.5 °C`, `1,200 MHz`); byte sizes accept `K`, `M`, `G`, `T` and `P` (powers
/// of 1024, as the table uses), so `1.5 GB` and `250M` both work.
pub fn parse_input(text: &str, unit: Unit, temperature: TemperatureUnit) -> Option<f64> {
    let text: String = text.trim().chars().filter(|&c| c != ',').collect();
    let mut end = 0;
    let mut seen_digit = false;
    let mut seen_dot = false;
    for (i, c) in text.char_indices() {
        match c {
            '+' | '-' if i == 0 => {}
            '0'..='9' => seen_digit = true,
            '.' if !seen_dot => seen_dot = true,
            _ => break,
        }
        end = i + c.len_utf8();
    }
    if !seen_digit {
        return None;
    }
    let number: f64 = text[..end].parse().ok()?;
    let rest = text[end..].trim_start();
    let factor = match unit {
        Unit::Bytes | Unit::BytesPerSecond => {
            match rest.chars().next().map(|c| c.to_ascii_lowercase()) {
                Some('k') => 1024f64,
                Some('m') => 1024f64.powi(2),
                Some('g') => 1024f64.powi(3),
                Some('t') => 1024f64.powi(4),
                Some('p') => 1024f64.powi(5),
                _ => 1.0,
            }
        }
        _ => 1.0,
    };
    let value = from_display(number * factor, unit, temperature);
    value.is_finite().then_some(value)
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

    #[test]
    fn display_conversion_only_touches_temperatures() {
        let f = TemperatureUnit::Fahrenheit;
        assert_eq!(to_display(100.0, Unit::Celsius, f), 212.0);
        assert_eq!(from_display(212.0, Unit::Celsius, f), 100.0);
        assert_eq!(to_display(1.2, Unit::Volt, f), 1.2);
        assert_eq!(from_display(1.2, Unit::Volt, f), 1.2);
        assert!((f.to_celsius(f.convert(37.4)) - 37.4).abs() < 1e-9);
    }

    #[test]
    fn inputs_format_without_the_unit_except_for_bytes() {
        let c = TemperatureUnit::Celsius;
        assert_eq!(format_input(100.0, Unit::Celsius, 1, c), "100.0");
        assert_eq!(
            format_input(100.0, Unit::Celsius, 1, TemperatureUnit::Fahrenheit),
            "212.0"
        );
        assert_eq!(format_input(4300.0, Unit::MegaHertz, 1, c), "4,300.0");
        assert_eq!(
            format_input(1536.0 * 1024.0 * 1024.0, Unit::Bytes, 1, c),
            "1.5 GB"
        );
        assert_eq!(
            format_input(2.5 * 1024.0 * 1024.0, Unit::BytesPerSecond, 1, c),
            "2.5 MB/s"
        );
        assert_eq!(format_input(f64::NAN, Unit::Volt, 3, c), "");
    }

    #[test]
    fn inputs_parse_back_with_or_without_units() {
        let c = TemperatureUnit::Celsius;
        assert_eq!(parse_input(" 48.5 °C ", Unit::Celsius, c), Some(48.5));
        assert_eq!(
            parse_input("212", Unit::Celsius, TemperatureUnit::Fahrenheit),
            Some(100.0)
        );
        assert_eq!(parse_input("1,200 MHz", Unit::MegaHertz, c), Some(1200.0));
        assert_eq!(parse_input("-0.5", Unit::Volt, c), Some(-0.5));
        assert_eq!(parse_input("+3", Unit::Watt, c), Some(3.0));
        assert_eq!(
            parse_input("1.5 GB", Unit::Bytes, c),
            Some(1.5 * 1024f64.powi(3))
        );
        assert_eq!(
            parse_input("250M", Unit::BytesPerSecond, c),
            Some(250.0 * 1024f64.powi(2))
        );
        assert_eq!(parse_input("512 B", Unit::Bytes, c), Some(512.0));
        assert_eq!(
            parse_input("12 MB", Unit::Watt, c),
            Some(12.0),
            "prefixes only scale bytes"
        );
        assert_eq!(parse_input("", Unit::Volt, c), None);
        assert_eq!(parse_input("abc", Unit::Volt, c), None);
        assert_eq!(parse_input("-", Unit::Volt, c), None);
        assert_eq!(parse_input(".", Unit::Volt, c), None);
    }
}
