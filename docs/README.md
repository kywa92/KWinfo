# Documentation

| Document | What it covers |
|---|---|
| [install.md](install.md) | Installing the .deb, the AppImage or the command line tool, the helper, `sensors-detect` and the kernel modules for motherboard sensors (`nct6775`, `nct6683 force=1`, `it87` through DKMS and Secure Boot, `jc42`, `spd5118`, `drivetemp`), troubleshooting |
| [helper.md](helper.md) | The privileged helper: what it reads, how KWinfo reaches it, what an install writes, testing it, security notes |
| [diagnostics.md](diagnostics.md) | Log files, diagnostic reports and the environment variables that control them |
| [man/](man/) | The kwinfo(1), kwinfo-cli(1) and kwinfo-helper(8) manual pages, written by `cargo xtask man` from the command lines; `man docs/man/kwinfo.1` shows one |
| [screenshots/](screenshots/) | The pictures in the README and the AppStream metadata, rendered by `cargo test -p kwinfo-app --test preview -- --ignored screenshots` |

How the packages are built and how a release is made: [packaging/README.md](../packaging/README.md).
