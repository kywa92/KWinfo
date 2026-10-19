//! `kwinfo-helper install` and `uninstall`: a local install for the AppImage or a build
//! from source, which packages (milestone M6) replace with the same files.
//!
//! Installing copies the helper to [`LOCAL_HELPER_PATH`] and renders the templates in
//! `packaging/` with that path: the systemd unit (`/etc/systemd/system`), the D-Bus
//! activation file and the polkit action (`/usr/share/...`, the only places polkit and
//! every D-Bus implementation read), the D-Bus policy (`/etc/dbus-1/system.d`) and the
//! `msr` module list (`/etc/modules-load.d`). The list of written files goes to
//! [`LOCAL_MANIFEST_PATH`], and uninstalling removes exactly those.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail};
use kwinfo_ipc::{LOCAL_HELPER_PATH, LOCAL_MANIFEST_PATH, PACKAGED_HELPER_PATH};

const SYSTEMD_UNIT: &str = include_str!("../../../packaging/systemd/kwinfo-helper.service.in");
const DBUS_SERVICE: &str =
    include_str!("../../../packaging/dbus/io.github.kywa92.KWinfo.Helper.service.in");
const DBUS_POLICY: &str =
    include_str!("../../../packaging/dbus/io.github.kywa92.KWinfo.Helper.conf");
const POLKIT_ACTION: &str =
    include_str!("../../../packaging/polkit/io.github.kywa92.KWinfo.policy");
const MODULES: &str = include_str!("../../../packaging/modules-load/kwinfo.conf");

/// One file the install writes, as an absolute path on the target system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallFile {
    pub path: &'static str,
    pub contents: String,
}

/// Where an install goes: the system root (`/`, or a staging directory for packages and
/// tests) and the helper's path as the installed files name it.
#[derive(Clone, Debug)]
pub struct Layout {
    pub root: PathBuf,
    pub helper_path: String,
}

impl Layout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            helper_path: LOCAL_HELPER_PATH.to_string(),
        }
    }

    pub fn with_helper_path(mut self, path: impl Into<String>) -> Self {
        self.helper_path = path.into();
        self
    }

    /// `absolute` under the root.
    pub fn target(&self, absolute: &str) -> PathBuf {
        self.root.join(absolute.trim_start_matches('/'))
    }

    /// Every file but the helper itself, rendered.
    pub fn files(&self) -> Vec<InstallFile> {
        let render = |template: &str| template.replace("@HELPER@", &self.helper_path);
        vec![
            InstallFile {
                path: "/etc/systemd/system/kwinfo-helper.service",
                contents: render(SYSTEMD_UNIT),
            },
            InstallFile {
                path: "/usr/share/dbus-1/system-services/io.github.kywa92.KWinfo.Helper.service",
                contents: render(DBUS_SERVICE),
            },
            InstallFile {
                path: "/etc/dbus-1/system.d/io.github.kywa92.KWinfo.Helper.conf",
                contents: DBUS_POLICY.to_string(),
            },
            InstallFile {
                path: "/usr/share/polkit-1/actions/io.github.kywa92.KWinfo.policy",
                contents: POLKIT_ACTION.to_string(),
            },
            InstallFile {
                path: "/etc/modules-load.d/kwinfo.conf",
                contents: MODULES.to_string(),
            },
        ]
    }

    /// A package installed the helper: a local install would shadow its unit.
    pub fn packaged(&self) -> bool {
        self.target(PACKAGED_HELPER_PATH).exists()
    }

    /// A local install's manifest exists.
    pub fn installed_locally(&self) -> bool {
        self.target(LOCAL_MANIFEST_PATH).exists()
    }
}

/// Writes `contents` to `path` through a temporary file in the same directory, so that
/// readers never see half a file.
fn write_atomic(path: &Path, contents: &[u8], mode: u32) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("/"));
    fs::create_dir_all(dir)?;
    let temporary = dir.join(format!(
        ".{}.kwinfo-new",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file")
    ));
    {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    fs::set_permissions(&temporary, fs::Permissions::from_mode(mode))?;
    fs::rename(&temporary, path)
}

/// Installs into `layout`: the helper copied from `binary` (unless `None`, for packaging
/// where the helper is already in place), then the rendered files and the manifest.
/// Returns the absolute paths written.
pub fn install(layout: &Layout, binary: Option<&Path>) -> anyhow::Result<Vec<String>> {
    if layout.packaged() && layout.helper_path != PACKAGED_HELPER_PATH {
        bail!(
            "KWinfo's helper is already installed by a package ({PACKAGED_HELPER_PATH}); \
             a local install on top would shadow it"
        );
    }
    let mut written = Vec::new();
    if let Some(binary) = binary {
        let target = layout.target(&layout.helper_path);
        let same = fs::canonicalize(binary).ok() == fs::canonicalize(&target).ok();
        if !same {
            let bytes =
                fs::read(binary).with_context(|| format!("reading {}", binary.display()))?;
            write_atomic(&target, &bytes, 0o755)
                .with_context(|| format!("writing {}", target.display()))?;
        }
        written.push(layout.helper_path.clone());
    }
    for file in layout.files() {
        let target = layout.target(file.path);
        write_atomic(&target, file.contents.as_bytes(), 0o644)
            .with_context(|| format!("writing {}", target.display()))?;
        written.push(file.path.to_string());
    }
    if binary.is_some() {
        let manifest = layout.target(LOCAL_MANIFEST_PATH);
        let mut list = written.join("\n");
        list.push('\n');
        write_atomic(&manifest, list.as_bytes(), 0o644)
            .with_context(|| format!("writing {}", manifest.display()))?;
    }
    Ok(written)
}

/// Removes what a local install wrote (as its manifest lists it, limited to the files an
/// install writes). Returns the absolute paths removed.
pub fn uninstall(layout: &Layout) -> anyhow::Result<Vec<String>> {
    let manifest = layout.target(LOCAL_MANIFEST_PATH);
    let list = match fs::read_to_string(&manifest) {
        Ok(list) => list,
        Err(err) if err.kind() == io::ErrorKind::NotFound => bail!(
            "no local install of the helper was found ({LOCAL_MANIFEST_PATH} is missing); \
             a helper installed by a package is removed with the package"
        ),
        Err(err) => return Err(err).with_context(|| format!("reading {}", manifest.display())),
    };
    let mut allowed: HashSet<String> = layout
        .files()
        .into_iter()
        .map(|f| f.path.to_string())
        .collect();
    allowed.insert(layout.helper_path.clone());
    let mut removed = Vec::new();
    for path in list.lines().map(str::trim).filter(|p| !p.is_empty()) {
        if !allowed.contains(path) {
            tracing::warn!("the manifest lists {path}, which an install never writes; left alone");
            continue;
        }
        match fs::remove_file(layout.target(path)) {
            Ok(()) => removed.push(path.to_string()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("removing {path}")),
        }
    }
    fs::remove_file(&manifest).with_context(|| format!("removing {}", manifest.display()))?;
    if let Some(dir) = layout.target(&layout.helper_path).parent() {
        let _ = fs::remove_dir(dir);
    }
    Ok(removed)
}

/// Whether this process runs with effective user id 0.
pub fn is_root() -> bool {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            let line = status.lines().find(|l| l.starts_with("Uid:"))?;
            line.split_whitespace().nth(2).map(|euid| euid == "0")
        })
        .unwrap_or(false)
}

fn run(program: &str, args: &[&str]) -> Option<String> {
    match Command::new(program).args(args).output() {
        Ok(output) if output.status.success() => None,
        Ok(output) => Some(format!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(err) => Some(format!("{program} could not be run: {err}")),
    }
}

/// Asks the system bus to read its configuration again, so that the new policy and
/// activation file count at once.
fn reload_dbus() -> Option<String> {
    #[cfg(unix)]
    {
        let reload = || -> zbus::Result<()> {
            let bus = zbus::blocking::Connection::system()?;
            bus.call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "ReloadConfig",
                &(),
            )?;
            Ok(())
        };
        reload()
            .err()
            .map(|err| format!("reloading the D-Bus configuration failed: {err}"))
    }
    #[cfg(not(unix))]
    None
}

/// After installing on the live system: loads `msr`, reloads systemd and D-Bus, and
/// restarts a running helper so that it is the new one. Returns problems as notes.
pub fn activate() -> Vec<String> {
    [
        run("modprobe", &["msr"]),
        run("systemctl", &["daemon-reload"]),
        reload_dbus(),
        run("systemctl", &["try-restart", "kwinfo-helper.service"]),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Before uninstalling from the live system: stops the helper; afterwards the caller
/// reloads with [`reload`].
pub fn deactivate() -> Vec<String> {
    run("systemctl", &["stop", "kwinfo-helper.service"])
        .into_iter()
        .collect()
}

/// Reloads systemd and D-Bus after files went away.
pub fn reload() -> Vec<String> {
    [run("systemctl", &["daemon-reload"]), reload_dbus()]
        .into_iter()
        .flatten()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_name_the_helper_path() {
        let layout = Layout::new("/").with_helper_path("/opt/kw/kwinfo-helper");
        let files = layout.files();
        let unit = &files[0];
        assert!(
            unit.contents
                .contains("ExecStart=/opt/kw/kwinfo-helper serve")
        );
        assert!(
            unit.contents
                .contains("BusName=io.github.kywa92.KWinfo.Helper")
        );
        assert!(!files.iter().any(|f| f.contents.contains("@HELPER@")));
        assert!(
            files[1]
                .contents
                .contains("SystemdService=kwinfo-helper.service")
        );
        assert!(files[3].contents.contains(kwinfo_ipc::POLKIT_ACTION));
        assert!(files[2].contents.contains(kwinfo_ipc::DBUS_INTERFACE));
    }

    #[test]
    fn install_and_uninstall_round_trip_in_a_staging_root() {
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("build/kwinfo-helper");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"#!/bin/true\n").unwrap();
        let layout = Layout::new(root.path());
        assert!(!layout.installed_locally());

        let written = install(&layout, Some(&binary)).unwrap();
        assert_eq!(written.len(), 6);
        assert!(layout.installed_locally());
        let helper = layout.target(LOCAL_HELPER_PATH);
        assert_eq!(fs::read(&helper).unwrap(), b"#!/bin/true\n");
        assert_eq!(
            fs::metadata(&helper).unwrap().permissions().mode() & 0o777,
            0o755
        );
        let unit =
            fs::read_to_string(layout.target("/etc/systemd/system/kwinfo-helper.service")).unwrap();
        assert!(unit.contains(&format!("ExecStart={LOCAL_HELPER_PATH} serve")));
        let policy = layout.target("/usr/share/polkit-1/actions/io.github.kywa92.KWinfo.policy");
        assert_eq!(
            fs::metadata(&policy).unwrap().permissions().mode() & 0o777,
            0o644
        );

        // Installing again replaces the files in place.
        install(&layout, Some(&binary)).unwrap();

        // A tampered manifest cannot make uninstall delete other files.
        let manifest = layout.target(LOCAL_MANIFEST_PATH);
        let mut list = fs::read_to_string(&manifest).unwrap();
        list.push_str("/etc/passwd\n");
        fs::write(&manifest, list).unwrap();
        fs::create_dir_all(layout.target("/etc")).unwrap();
        fs::write(layout.target("/etc/passwd"), "root:x:0:0\n").unwrap();

        let removed = uninstall(&layout).unwrap();
        assert_eq!(removed.len(), 6);
        assert!(!helper.exists());
        assert!(!policy.exists());
        assert!(!layout.installed_locally());
        assert!(layout.target("/etc/passwd").exists());
        assert!(
            !layout.target("/usr/local/libexec/kwinfo").exists(),
            "the empty directory goes too"
        );
        let err = uninstall(&layout).unwrap_err().to_string();
        assert!(err.contains("no local install"), "{err}");
    }

    #[test]
    fn a_packaged_helper_is_not_shadowed() {
        let root = tempfile::tempdir().unwrap();
        let layout = Layout::new(root.path());
        let packaged = layout.target(PACKAGED_HELPER_PATH);
        fs::create_dir_all(packaged.parent().unwrap()).unwrap();
        fs::write(&packaged, b"").unwrap();
        let binary = root.path().join("kwinfo-helper");
        fs::write(&binary, b"").unwrap();
        let err = install(&layout, Some(&binary)).unwrap_err().to_string();
        assert!(err.contains("already installed by a package"), "{err}");
        // Packaging itself renders the files for the packaged path.
        let staging = Layout::new(root.path()).with_helper_path(PACKAGED_HELPER_PATH);
        assert_eq!(install(&staging, None).unwrap().len(), 5);
        assert!(!staging.installed_locally(), "packages need no manifest");
    }
}
