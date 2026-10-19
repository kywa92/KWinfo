# Packaging

| Path | What it is |
|---|---|
| `systemd/kwinfo-helper.service.in` | the helper's systemd unit, with sandboxing; D-Bus activation starts it |
| `dbus/io.github.kywa92.KWinfo.Helper.service.in` | D-Bus activation file for the system bus |
| `dbus/io.github.kywa92.KWinfo.Helper.conf` | D-Bus policy: only root owns the name, anyone may call |
| `polkit/io.github.kywa92.KWinfo.policy` | polkit action `io.github.kywa92.KWinfo.read-sensors` (active local sessions only) |
| `modules-load/kwinfo.conf` | loads `msr` at boot |

`@HELPER@` in the `.in` files is the helper's absolute path. `kwinfo-helper install`
renders and installs all of them for a local install (helper in
`/usr/local/libexec/kwinfo/`); packages (milestone M6) install them with the helper in
`/usr/libexec/kwinfo/`, for example with
`kwinfo-helper install --root <staging dir> --helper-path /usr/libexec/kwinfo/kwinfo-helper --no-copy`.
The desktop file, icons, AppStream metadata and the AppImage script arrive in M6; see
PLAN.md sections 3.5 and 7.
