//! The motherboard's name from DMI (`/sys/class/dmi/id`), which is readable without
//! root for everything except the serial numbers and UUID.

use crate::sysroot::{SysRoot, read_string};

use super::util::squash_spaces;

/// "ASUS PRIME Z370-A", "MSI MPG B850 EDGE TI WIFI (MS-7E62)", or the system name on
/// machines whose board fields are firmware placeholders.
pub(crate) fn board_name(root: &SysRoot) -> Option<String> {
    let field = |name: &str| {
        read_string(&root.sys("class/dmi/id").join(name)).filter(|v| !is_placeholder(v))
    };
    let (vendor, name) = match field("board_name") {
        Some(board) => (field("board_vendor"), board),
        None => (field("sys_vendor"), field("product_name")?),
    };
    let label = match vendor.as_deref().map(short_vendor) {
        Some(vendor) if !name.to_lowercase().starts_with(&vendor.to_lowercase()) => {
            format!("{vendor} {name}")
        }
        _ => name,
    };
    Some(squash_spaces(&label))
}

/// Firmware vendors leave these in DMI fields they did not fill in.
fn is_placeholder(value: &str) -> bool {
    const PLACEHOLDERS: [&str; 14] = [
        "to be filled by o.e.m.",
        "default string",
        "system manufacturer",
        "system product name",
        "system version",
        "base board product name",
        "not applicable",
        "not specified",
        "n/a",
        "none",
        "oem",
        "o.e.m.",
        "empty",
        "unknown",
    ];
    let value = value.trim().to_lowercase();
    value.is_empty() || PLACEHOLDERS.contains(&value.as_str())
}

/// "ASUSTeK COMPUTER INC." → "ASUS", "Micro-Star International Co., Ltd." → "MSI".
fn short_vendor(vendor: &str) -> String {
    let lower = vendor.to_lowercase();
    let known = [
        ("asustek", "ASUS"),
        ("micro-star", "MSI"),
        ("gigabyte", "Gigabyte"),
        ("asrock", "ASRock"),
        ("dell", "Dell"),
        ("lenovo", "Lenovo"),
        ("hewlett-packard", "HP"),
        ("super micro", "Supermicro"),
        ("supermicro", "Supermicro"),
        ("intel corporation", "Intel"),
        ("biostar", "Biostar"),
        ("evga", "EVGA"),
    ];
    if let Some((_, short)) = known.iter().find(|(prefix, _)| lower.starts_with(prefix)) {
        return (*short).to_string();
    }
    let mut name = vendor.trim().to_string();
    for suffix in [
        ", Inc.",
        " Inc.",
        " INC.",
        " Co., Ltd.",
        " Co.,Ltd.",
        " Co., Ltd",
        " Corporation",
        " Corp.",
        " Ltd.",
        " GmbH",
    ] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            name = stripped.trim_end_matches(',').trim().to_string();
        }
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::test_support::{fixture_copy, fixture_root, write};

    #[test]
    fn board_name_combines_short_vendor_and_board() {
        assert_eq!(
            board_name(&fixture_root()).as_deref(),
            Some("ASUS PRIME Z370-A")
        );
    }

    #[test]
    fn placeholders_fall_back_to_the_system_name() {
        let (_dir, root) = fixture_copy();
        write(&root, "sys/class/dmi/id/board_name", "Default string\n");
        write(&root, "sys/class/dmi/id/sys_vendor", "LENOVO\n");
        write(&root, "sys/class/dmi/id/product_name", "20XWCTO1WW\n");
        assert_eq!(board_name(&root).as_deref(), Some("Lenovo 20XWCTO1WW"));

        write(
            &root,
            "sys/class/dmi/id/product_name",
            "To be filled by O.E.M.\n",
        );
        assert_eq!(board_name(&root), None);
    }

    #[test]
    fn vendor_names_are_shortened() {
        assert_eq!(short_vendor("ASUSTeK COMPUTER INC."), "ASUS");
        assert_eq!(short_vendor("Micro-Star International Co., Ltd."), "MSI");
        assert_eq!(short_vendor("Gigabyte Technology Co., Ltd."), "Gigabyte");
        assert_eq!(short_vendor("Framework"), "Framework");
        assert_eq!(short_vendor("Acme Widgets Co., Ltd."), "Acme Widgets");
    }
}
