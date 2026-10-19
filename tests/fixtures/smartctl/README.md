# smartctl fixtures

Captured `smartctl` JSON output (smartmontools 7.4) used by the helper's S.M.A.R.T.
provider tests and by `kwinfo-helper stdio --smartctl-dir`: `scan.json` from
`smartctl --scan -j`, and one file per drive from
`smartctl -j -i -H -A -n standby -d <type> <device>`, named after the device
(`nvme0.json` for `/dev/nvme0`). `sdb.json` is a sleeping disk. Serial numbers, WWNs and
EUI-64s are scrubbed.
