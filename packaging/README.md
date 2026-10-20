# Packaging

| Path | What it is |
|---|---|
| `systemd/kwinfo-helper.service.in` | the helper's systemd unit, with sandboxing; D-Bus activation starts it |
| `dbus/io.github.kywa92.KWinfo.Helper.service.in` | D-Bus activation file for the system bus |
| `dbus/io.github.kywa92.KWinfo.Helper.conf` | D-Bus policy: only root owns the name, anyone may call |
| `polkit/io.github.kywa92.KWinfo.policy` | polkit action `io.github.kywa92.KWinfo.read-sensors` (active local sessions only) |
| `modules-load/kwinfo.conf` | loads `msr` at boot |
| `desktop/io.github.kywa92.KWinfo.desktop` | the desktop entry |
| `desktop/io.github.kywa92.KWinfo.metainfo.xml` | AppStream metadata, with the screenshots in `docs/screenshots/` |
| `desktop/icons/` | the icon: `hicolor/scalable` and its PNG renders, `src/` the simpler source of the 16 to 32 px renders; `desktop/render-icons.sh` redraws the PNGs |
| `build-release.sh` | the release build every package is made from |
| `deb/` | `build.sh` for the Debian package (cargo-deb; its metadata is in `crates/kwinfo-app/Cargo.toml`), the maintainer scripts, the copyright file and the lintian overrides |
| `appimage/` | `build.sh` for the AppImage (linuxdeploy at pinned versions) and its `AppRun`, which also runs `kwinfo-cli` and `kwinfo-helper` |
| `cli-tarball.sh` | the `kwinfo-cli` tarball |
| `smoke-test.sh` | installs and starts a package on a fresh system (the release workflow runs it in containers) |
| `check-release.sh`, `release-notes.sh` | the release workflow's version check and release notes |

`@HELPER@` in the `.in` files is the helper's absolute path. `kwinfo-helper install` renders
and installs all of them for a local install (helper in `/usr/local/libexec/kwinfo/`); the
Debian package has the helper in `/usr/libexec/kwinfo/` and renders them with
`kwinfo-helper install --root <staging dir> --helper-path /usr/libexec/kwinfo/kwinfo-helper --no-copy`.
See PLAN.md sections 3.5 and 7, and [docs/install.md](../docs/install.md) for users.

## Building the packages

The packages are built on Ubuntu 22.04, so that they run on systems with glibc 2.35 or
later. With the build dependencies from the README plus `lintian appstream zsync` and
`cargo install cargo-deb --version 3.8.0 --locked --features static-lzma`:

```bash
packaging/build-release.sh
```

```bash
packaging/deb/build.sh --no-build && packaging/appimage/build.sh --no-build && packaging/cli-tarball.sh --no-build
```

The packages land in `dist/`: `kwinfo_<version>-1_<arch>.deb`, `KWinfo-<arch>.AppImage`
with its `.zsync`, and `kwinfo-cli-<version>-<arch>-linux-gnu.tar.gz`. The scripts work on
aarch64 too. `packaging/smoke-test.sh deb dist/kwinfo_*.deb` (as root, in a throwaway
container) installs and starts one.

From the Mac, GitHub Actions builds them: Actions → Release → *Run workflow* on `main`.
The run's `packages` artifact holds the files, the `screenshot-*` artifacts show the window
started on each test system, and the summary page shows the release notes. Pushes to `main`
that change the packaging start the same run.

## Making a release

1. Run the Release workflow by hand and try its packages on real hardware (the .deb and
   the AppImage, the helper through Settings → Helper).
2. Date the version's section in `CHANGELOG.md` (`## [0.1.0] - 2026-11-02`) and start an
   empty `## [Unreleased]` section above it. In
   `desktop/io.github.kywa92.KWinfo.metainfo.xml`, give the `<release>` entry the same
   date and remove `type="development"`. Commit.
3. Tag and push: `git tag -a v0.1.0 -m "KWinfo 0.1.0"`, then
   `git push origin main v0.1.0`.
4. The workflow checks that the tag, `Cargo.toml`, `CHANGELOG.md` and the metadata agree,
   builds, lints and smoke-tests the packages, and creates a **draft** release with them,
   `SHA256SUMS` and the notes. Read it on GitHub and publish it.

For the next version, raise `version` in the workspace `Cargo.toml`, run `cargo xtask man`
(the man pages carry the version), and add a `<release>` entry to the metadata.
