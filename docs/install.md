# Installing KWinfo

Every [release](https://github.com/kywa92/KWinfo/releases) has three downloads for x86_64:

| File | For |
|---|---|
| `kwinfo_<version>-1_amd64.deb` | Ubuntu 22.04 and later, Debian 12 and later, and distributions based on them |
| `KWinfo-x86_64.AppImage` | any other distribution with glibc 2.35 or later (Fedora 36+, openSUSE Tumbleweed, Arch, …) |
| `kwinfo-cli-<version>-x86_64-linux-gnu.tar.gz` | the command line tool and the helper alone, for machines without a desktop |

`SHA256SUMS` lists their checksums. In the folder with the downloads:

```bash
sha256sum --check --ignore-missing SHA256SUMS
```

After installing, most machines need one more step for motherboard sensors: see
[Motherboard sensors](#motherboard-sensors-kernel-modules) below.

## Debian and Ubuntu: the .deb

```bash
sudo apt install ./kwinfo_0.1.0-1_amd64.deb
```

This installs `kwinfo` (in the applications menu as KWinfo), `kwinfo-cli`, their man pages,
and the privileged helper as a system service: `/usr/libexec/kwinfo/kwinfo-helper`, its
systemd unit, D-Bus files and polkit action, and `msr` in `/usr/lib/modules-load.d`. There
is nothing to set up: D-Bus starts the helper on KWinfo's first request, and polkit lets
the active local session use it without a password (see [helper.md](helper.md)).

apt also installs the recommended `lm-sensors` (for `sensors-detect` and the sensor
labels) and `smartmontools` (S.M.A.R.T. data needs `smartctl`). To remove KWinfo:

```bash
sudo apt remove kwinfo
```

If the helper was installed from the AppImage before (Settings → Helper → *Install
helper…*), that copy in `/usr/local` takes precedence over the package's; the package says
so when it is installed. Remove it with:

```bash
sudo /usr/local/libexec/kwinfo/kwinfo-helper uninstall
```

## Other distributions: the AppImage

```bash
chmod +x KWinfo-x86_64.AppImage
./KWinfo-x86_64.AppImage
```

The AppImage needs FUSE to mount itself, which desktops have (`fusermount3` from fuse3).
Without FUSE, add `--appimage-extract-and-run`. Tools such as
[Gear Lever](https://flathub.org/apps/it.mijorus.gearlever) or AppImageLauncher add it to
the applications menu. The image carries update information pointing at its `.zsync` file
on GitHub, so AppImageUpdate and the tools built on it download only what changed. The
latest version is always at
`https://github.com/kywa92/KWinfo/releases/latest/download/KWinfo-x86_64.AppImage`.

The AppImage also contains the command line tool and the helper; give their name as the
first argument, or start the AppImage through a link with that name:

```bash
./KWinfo-x86_64.AppImage kwinfo-cli list --ids
```

**Root-only sensors.** KWinfo shows the Helper button in its toolbar grey until the helper
runs. Settings → Helper offers:

- *Install helper…* runs `pkexec kwinfo-helper install` once (an administrator password):
  it copies the helper to `/usr/local/libexec/kwinfo/` and sets up the same system service
  the .deb has, so from then on the AppImage behaves like the package. *Uninstall helper…*
  removes exactly what it installed.
- *Run for this session* starts the helper with `pkexec` until KWinfo quits, asking for the
  password every time.

From a terminal, `sudo ./KWinfo-x86_64.AppImage kwinfo-helper install` does the same as
*Install helper…*. Both need polkit's `pkexec` and, for the password dialog, the polkit
agent every GNOME and KDE session runs.

## The command line tool alone

```bash
tar -xzf kwinfo-cli-0.1.0-x86_64-linux-gnu.tar.gz
cd kwinfo-cli-0.1.0-x86_64-linux-gnu
./kwinfo-cli list --ids
```

It needs glibc 2.35 or later and libsensors (`libsensors5` on Debian and Ubuntu,
`lm_sensors-libs` on Fedora). `kwinfo-helper` must stay next to `kwinfo-cli`:
`./kwinfo-cli helper --session` starts it through pkexec for one command, and
`sudo ./kwinfo-helper install` sets it up as a system service that `kwinfo-cli list` then
uses. See `man ./kwinfo-cli.1` and `man ./kwinfo-helper.8`.

## Motherboard sensors: kernel modules

KWinfo reads the sensors that the kernel's drivers publish in `/sys/class/hwmon`. CPU
temperatures (`coretemp`, `k10temp`), NVMe drives and graphics cards work out of the box.
Motherboard voltages, fans and temperatures come from the board's monitoring chip, usually
a Super I/O chip by Nuvoton or ITE, and appear only once its driver is loaded. Until then
KWinfo's motherboard group is missing or shows only `acpitz`.

### Find the chip with sensors-detect

`sensors-detect` comes with lm-sensors (`sudo apt install lm-sensors`, `sudo dnf install
lm_sensors`). It probes the Super I/O chip and the SMBus and names the drivers to load;
pressing Enter at every question takes the safe default answers:

```bash
sudo sensors-detect
```

At the end it lists what it found, for example:

```text
Driver `nct6775':
  * ISA bus, address 0x290
    Chip `Nuvoton NCT6793D Super IO Sensors' (confidence: 9)
```

and offers to add the modules to `/etc/modules`. Or load the driver now and at every boot
yourself:

```bash
sudo modprobe nct6775
```

```bash
echo nct6775 | sudo tee /etc/modules-load.d/sensors.conf
```

`sensors` or `kwinfo-cli list` then show the new readings; KWinfo picks them up at its next
rescan (within a minute) or restart. `kwinfo-cli report` records which sensor modules are
loaded, which helps when asking for help.

### Which driver for which chip

| Chip | Common on | Driver |
|---|---|---|
| Nuvoton NCT6775 to NCT6799 (NCT67xxD) | ASUS, ASRock, many Gigabyte and MSI boards | `nct6775`, in the kernel |
| Nuvoton NCT6683D, NCT6686D, NCT6687D | MSI boards, Intel NUCs | `nct6683`, in the kernel; MSI boards need `force=1` |
| ITE IT86xx and IT87xx | Gigabyte, some ASRock and Biostar boards | `it87`, in the kernel for older chips; newer ones through DKMS |
| ASUS embedded controller | many ASUS ROG, TUF and ProArt boards | `asus_ec_sensors` (kernel 5.18+), `asus_wmi_sensors` (5.17+) |
| DDR4 DIMM temperature sensors | modules with a JEDEC JC-42.4 sensor | `jc42`, with `i2c-i801` (Intel) or `i2c-piix4` (AMD) |
| DDR5 SPD hub | every DDR5 module | `spd5118` (kernel 6.11+) |
| SATA hard disks and SSDs | | `drivetemp` (NVMe needs nothing) |

**nct6775.** Loads without options on most boards. If `sudo dmesg | grep -i conflict` shows
an ACPI resource conflict for its I/O ports, the firmware uses the chip too. Recent kernels
read many ASUS boards through the firmware's WMI interface instead, with no conflict. The
kernel parameter `acpi_enforce_resources=lax` lets the driver load anyway, at the risk that
the driver and the firmware disturb each other; use it only if you accept that.

**nct6683 on MSI boards.** The driver only recognises the boards it was tested on and
refuses others with "No such device". Read-only access is safe, so tell it to load:

```bash
echo "options nct6683 force=1" | sudo tee /etc/modprobe.d/nct6683.conf
```

```bash
sudo modprobe nct6683
```

Its readings carry generic names (`in0`, `fan2`, `temp3`). A file in `/etc/sensors.d/` can
name them, and KWinfo uses those names (see [lm-sensors labels](#lm-sensors-labels)). The
out-of-tree [`nct6687`](https://github.com/Fred78290/nct6687d) driver knows the MSI layouts
better; it installs through DKMS like `it87` below.

**it87 and DKMS.** Try the kernel's driver first: `sudo modprobe it87`. On Gigabyte
boards recent kernels often need `ignore_resource_conflict=1`, set the same way as
`force=1` above (`options it87 ignore_resource_conflict=1`). Chips the kernel's driver does not know
need the maintained out-of-tree driver,
[frankcrawford/it87](https://github.com/frankcrawford/it87), built by DKMS for every
kernel update:

```bash
sudo apt install dkms build-essential linux-headers-$(uname -r) git
```

```bash
git clone https://github.com/frankcrawford/it87 && cd it87 && sudo ./dkms-install.sh
```

With **Secure Boot** on, the kernel refuses unsigned modules ("Key was rejected by
service"). Ubuntu and Debian sign DKMS modules with a machine owner key (MOK) and ask, when
DKMS first builds one, for a password to enrol it; at the next boot the blue MOK manager
asks for that password ("Enroll MOK"). If that step was missed:

```bash
sudo mokutil --import /var/lib/dkms/mok.pub
```

then reboot and enrol the key. The alternative is turning Secure Boot off in the firmware
settings, which lowers the protection against boot-time malware.

**DIMM temperatures.** sensors-detect finds DDR4 modules' JEDEC JC-42.4 sensors on the
SMBus and suggests `jc42`, which needs the SMBus driver too:

```bash
sudo modprobe i2c-i801 jc42
```

(`i2c-piix4` instead of `i2c-i801` on AMD). DDR5 modules report through `spd5118`, which
kernels from 6.11 on set up by themselves once the SMBus driver is loaded.

**SATA drive temperatures** need `drivetemp`, which is not loaded by default:

```bash
sudo modprobe drivetemp
```

```bash
echo drivetemp | sudo tee /etc/modules-load.d/drivetemp.conf
```

### lm-sensors labels

KWinfo applies the `label`, `ignore` and `compute` lines of `/etc/sensors3.conf` and
`/etc/sensors.d/*.conf`, so a board configuration written for `sensors` carries over. For
example, `/etc/sensors.d/board.conf`:

```text
chip "nct6687-*"
    label in0 "+12V"
    compute in0 @*12, @/12
    label fan1 "CPU Fan"
    ignore temp7
```

Configurations for many boards are collected in the
[lm-sensors repository](https://github.com/lm-sensors/lm-sensors/tree/master/configs).

## Troubleshooting

| Problem | What to do |
|---|---|
| No motherboard group, or only `acpitz` | Load the Super I/O driver, see above. |
| No CPU package power, core voltages, S.M.A.R.T. or GPU load | The helper is not running: the toolbar's Helper button and Settings → Helper say why; [helper.md](helper.md) has the details. |
| S.M.A.R.T. rows missing | Install smartmontools; the helper needs `smartctl`. |
| Tray icons do not appear on GNOME | GNOME shows them only with the AppIndicator extension (`gnome-shell-extension-appindicator`). |
| Graph windows open in odd places on Wayland | Wayland lets the compositor place windows; the default *Automatic* setting shows the graphs in a panel of the main window instead. |
| The AppImage does not start and mentions FUSE or fusermount | Install fuse3 (`sudo apt install fuse3`, `sudo dnf install fuse3`), or start it with `--appimage-extract-and-run`. |

To report a problem, attach the file that `kwinfo-cli report` (or Settings → General →
*Save diagnostic report…*) writes; see [diagnostics.md](diagnostics.md).
