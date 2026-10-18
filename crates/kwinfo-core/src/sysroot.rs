//! Where the Linux backends read from: the live `/`, or a captured tree such as a test
//! fixture or a snapshot attached to a bug report.
//!
//! Every backend takes a [`SysRoot`] and only reads `<root>/sys/...`, `<root>/proc/...`
//! and the PCI name database under `<root>/usr/share`, so the same parsers run on Linux,
//! in unit tests on any OS, and in `--replay` mode. Nothing here writes or runs commands.

use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

/// The root of a Linux file system tree, `/` on the live system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SysRoot {
    root: PathBuf,
}

impl SysRoot {
    /// The running system.
    pub fn host() -> Self {
        Self {
            root: PathBuf::from("/"),
        }
    }

    /// A captured tree, e.g. `tests/fixtures/synthetic-8700k`. The path is made absolute
    /// and symlink-free when it exists, so that paths resolved inside it share its prefix.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let root = fs::canonicalize(&root).unwrap_or(root);
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether this is the live system rather than a captured tree.
    pub fn is_host(&self) -> bool {
        self.root == Path::new("/")
    }

    /// `<root>/<rel>`, where `rel` is relative, e.g. `usr/share/hwdata/pci.ids`.
    pub fn path(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.root.join(rel)
    }

    /// `<root>/sys/<rel>`.
    pub fn sys(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.root.join("sys").join(rel)
    }

    /// `<root>/proc/<rel>`.
    pub fn proc(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.root.join("proc").join(rel)
    }

    /// Resolves every symlink in `path`, which lies inside this root. When the final link
    /// dangles (a fixture captured the link but not its target) the target is resolved
    /// lexically instead. `None` if the path does not exist or leads outside the root.
    pub fn resolve(&self, path: &Path) -> Option<PathBuf> {
        let resolved = match fs::canonicalize(path) {
            Ok(resolved) => resolved,
            Err(_) => {
                let target = fs::read_link(path).ok()?;
                let parent = fs::canonicalize(path.parent()?).ok()?;
                match target.strip_prefix("/") {
                    // Absolute links in a captured tree point into the captured system.
                    Ok(rel) => lexical_join(&self.root, rel),
                    Err(_) => lexical_join(&parent, &target),
                }
            }
        };
        resolved.starts_with(&self.root).then_some(resolved)
    }

    /// `path` relative to the root, e.g. `sys/devices/platform/coretemp.0`.
    pub fn relative<'a>(&self, path: &'a Path) -> Option<&'a Path> {
        path.strip_prefix(&self.root).ok()
    }
}

/// Joins `rel` onto `base`, resolving `.` and `..` without touching the file system.
/// Correct as long as `base` itself contains no symlinks.
pub fn lexical_join(base: &Path, rel: &Path) -> PathBuf {
    let mut out = base.to_path_buf();
    for component in rel.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            Component::Normal(part) => out.push(part),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    out
}

/// Trims whitespace and the NUL padding some firmware strings carry.
pub fn trim_attr(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || c == '\0')
}

/// Reads a small text attribute, trimmed. `None` if it is missing, unreadable, not UTF-8
/// or empty.
pub fn read_string(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let value = trim_attr(&raw);
    (!value.is_empty()).then(|| value.to_string())
}

/// Reads a decimal integer attribute such as `temp1_input`.
pub fn read_i64(path: &Path) -> Option<i64> {
    read_int(path).ok()
}

/// Reads a non-negative decimal integer attribute such as a byte counter.
pub fn read_u64(path: &Path) -> Option<u64> {
    read_string(path)?.parse().ok()
}

/// Reads a hexadecimal attribute such as a PCI `vendor` (`0x8086`).
pub fn read_hex(path: &Path) -> Option<u32> {
    parse_hex(&read_string(path)?)
}

pub fn parse_hex(s: &str) -> Option<u32> {
    let s = s.trim();
    let digits = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    u32::from_str_radix(digits, 16).ok()
}

/// Reads a decimal integer attribute without allocating. This is the per-tick path, so
/// it reuses nothing but a stack buffer and reports why a read failed.
pub fn read_int(path: &Path) -> io::Result<i64> {
    let mut buf = [0u8; 64];
    let mut file = File::open(path)?;
    let mut len = 0;
    while len < buf.len() {
        match file.read(&mut buf[len..]) {
            Ok(0) => break,
            Ok(n) => len += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    parse_int(&buf[..len])
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not an integer"))
}

pub fn parse_int(bytes: &[u8]) -> Option<i64> {
    trim_attr(std::str::from_utf8(bytes).ok()?).parse().ok()
}

/// Reads a whole file into `buf`, replacing its contents. For `/proc` files that are read
/// every tick, so the buffer's allocation is reused.
pub fn read_into(path: &Path, buf: &mut Vec<u8>) -> io::Result<()> {
    buf.clear();
    File::open(path)?.read_to_end(buf)?;
    Ok(())
}

/// The final component of a symlink's target, e.g. `i915` for a `driver` link. Works on
/// dangling links, which is what captured trees usually contain for `driver`.
pub fn link_name(path: &Path) -> Option<String> {
    let target = fs::read_link(path).ok()?;
    Some(target.file_name()?.to_str()?.to_string())
}

/// Names of the entries of `dir` in natural order (`hwmon2` before `hwmon10`). Empty if
/// the directory is missing or unreadable.
pub fn list_dir(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    names.sort_by(|a, b| natural_cmp(a, b));
    names
}

/// Compares names the way people number things: runs of digits compare by value.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    loop {
        match (a.first(), b.first()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let da = a.iter().take_while(|c| c.is_ascii_digit()).count();
                let db = b.iter().take_while(|c| c.is_ascii_digit()).count();
                let (na, nb) = (strip_zeros(&a[..da]), strip_zeros(&b[..db]));
                let order = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
                if order != Ordering::Equal {
                    return order;
                }
                a = &a[da..];
                b = &b[db..];
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(y);
                }
                a = &a[1..];
                b = &b[1..];
            }
        }
    }
}

fn strip_zeros(digits: &[u8]) -> &[u8] {
    let start = digits
        .iter()
        .position(|&d| d != b'0')
        .unwrap_or(digits.len());
    &digits[start..]
}

/// Parses a kernel CPU list such as `0-3,8,10-11`.
pub fn parse_cpu_list(s: &str) -> Vec<u32> {
    let mut cpus = Vec::new();
    for part in trim_attr(s)
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        match part.split_once('-') {
            Some((lo, hi)) => {
                if let (Ok(lo), Ok(hi)) = (lo.parse::<u32>(), hi.parse::<u32>()) {
                    cpus.extend(lo..=hi);
                }
            }
            None => {
                if let Ok(cpu) = part.parse() {
                    cpus.push(cpu);
                }
            }
        }
    }
    cpus
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order_compares_numbers_by_value() {
        let mut names = vec![
            "hwmon10",
            "hwmon2",
            "hwmon1",
            "card0-DP-1",
            "card0",
            "cpu11",
        ];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            [
                "card0",
                "card0-DP-1",
                "cpu11",
                "hwmon1",
                "hwmon2",
                "hwmon10"
            ]
        );
        assert_eq!(natural_cmp("in007", "in7"), Ordering::Equal);
        assert_eq!(natural_cmp("nvme0n1", "nvme0n10"), Ordering::Less);
    }

    #[test]
    fn cpu_lists_expand_ranges() {
        assert_eq!(parse_cpu_list("0-3,8,10-11\n"), vec![0, 1, 2, 3, 8, 10, 11]);
        assert_eq!(parse_cpu_list("0,6"), vec![0, 6]);
        assert!(parse_cpu_list("").is_empty());
    }

    #[test]
    fn attributes_parse_tolerantly() {
        assert_eq!(parse_int(b"48000\n"), Some(48000));
        assert_eq!(parse_int(b"-273150\n"), Some(-273_150));
        assert_eq!(parse_int(b"N/A\n"), None);
        assert_eq!(parse_hex("0x8086\n"), Some(0x8086));
        assert_eq!(parse_hex("3e92"), Some(0x3e92));
        assert_eq!(trim_attr("Samsung SSD 860 \0\0"), "Samsung SSD 860");
    }

    #[test]
    fn lexical_join_resolves_parent_components() {
        let joined = lexical_join(
            Path::new("/r/sys/class/hwmon"),
            Path::new("../../devices/platform/coretemp.0/hwmon/hwmon1"),
        );
        assert_eq!(
            joined,
            Path::new("/r/sys/devices/platform/coretemp.0/hwmon/hwmon1")
        );
    }

    #[test]
    fn resolve_follows_links_and_stays_inside_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::new(dir.path());
        let real = root.sys("devices/platform/coretemp.0/hwmon/hwmon1");
        fs::create_dir_all(&real).unwrap();
        fs::create_dir_all(root.sys("class/hwmon")).unwrap();
        let link = root.sys("class/hwmon/hwmon1");
        std::os::unix::fs::symlink("../../devices/platform/coretemp.0/hwmon/hwmon1", &link)
            .unwrap();
        assert_eq!(root.resolve(&link).as_deref(), Some(real.as_path()));
        assert_eq!(
            root.relative(&real),
            Some(Path::new("sys/devices/platform/coretemp.0/hwmon/hwmon1"))
        );

        // A dangling link resolves lexically.
        let dangling = root.sys("class/hwmon/hwmon9");
        std::os::unix::fs::symlink("../../devices/platform/gone.0/hwmon/hwmon9", &dangling)
            .unwrap();
        assert_eq!(
            root.resolve(&dangling),
            Some(root.sys("devices/platform/gone.0/hwmon/hwmon9"))
        );

        // A link that climbs out of the captured tree is refused.
        let escape = root.sys("class/hwmon/hwmon5");
        std::os::unix::fs::symlink("../../../../../../../../../../etc", &escape).unwrap();
        assert_eq!(root.resolve(&escape), None);
    }
}
