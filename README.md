# hboot

A lightweight UEFI boot manager for x86_64 Linux and Windows. Finds your
installed operating systems, presents a boot menu, and loads the kernel.

## Why

I built hboot for two reasons. First, I wanted a project that'd make a solid addition to my GitHub portfolio. Second — and more practically — I got tired of writing systemd-boot entries from scratch every time I reinstalled. hboot detects all your operating systems automatically and handles the setup, so you don't have to think about it.

## Installation

```bash
curl -LO https://github.com/person134/hboot/releases/latest/download/hboot
chmod +x hboot
sudo ./hboot install
```

On first run, `hboot install` automatically installs `efibootmgr` (via
`pacman`, `apt`, `dnf`, `zypper`, `apk`, `yum`, or `emerge`) if missing.
Manual installation of `efibootmgr` is only required if `hboot install`
can't find the package manager. 

Reboot and pick **hboot** from your UEFI boot menu.

The installer copies itself to `/usr/bin/hboot`, so subsequent runs are
just `sudo hboot install`.

To build from source instead, see [Build from source](#build-from-source).

## How it works

hboot scans your ESP for installed operating systems, generates a config with entries
in `\EFI\hboot\entries\*.conf`, and at boot shows a menu of the detected
entries. Each entry loads the kernel `.efi` binary.

| Detected | How |
|----------|------|
| Windows | `\EFI\Microsoft\Boot\bootmgfw.efi` |
| Linux (kernel on ESP) | `\vmlinuz-*` — any kernel found on ESP root. matching `initramfs-*.img` or `initrd.img-*` autodetected |
| UKI | `\EFI\Linux\*.efi` — unified kernel images |
| BLS type-1 | `\loader\entries\*.conf` — Boot Loader Spec entries |

## Commands

| Command | What it does |
|---------|--------------|
| `hboot install` | Install efibootmgr, build, copy to ESP, register with firmware |
| `hboot remove` | Remove from ESP and boot entries |
| `hboot status` | Check if hboot is installed |
| `hboot config edit` | Open config in `$EDITOR` |
| `hboot config detect` | Print detected entries as config |
| `hboot config init` | Generate a sample config file |
| `hboot config set-default <name>` | Set the auto-boot entry |
| `hboot detect` | List detected OSes on the ESP |
| `hboot entry list` | List all boot entries with titles |
| `hboot entry add <name> --efi <path>` | Add a new boot entry |
| `hboot entry remove <name>` | Remove a boot entry |
| `hboot entry edit <name>` | Edit a boot entry in your editor |
| `hboot entry mark-good <name>` | Mark entry as good (remove boot counter) |
| `hboot entry set-tries <name> <N>` | Set boot tries for an entry |
| `hboot update` | Pull latest release and reinstall |

### Flags

| Flag | Commands | Purpose |
|------|----------|---------|
| `--esp /boot` | install, detect, edit, entry | Specify ESP mount |
| `--disk /dev/nvme0n1 --part 1` | install | Set disk/partition for efibootmgr |
| `--efi /path/to/hboot-efi.efi --no-build` | install | Skip rebuild, use prebuilt EFI |
| `--sign` | install | Sign for Secure Boot |
| `--no-efi` | remove | Skip efibootmgr cleanup |
| `--self-remove` | remove | Also delete `/usr/bin/hboot` |
| `--all` | remove | Also remove config files and entries |

## Configuration

Boot entries are stored as individual `.conf` files in `\EFI\hboot\entries\`.
The main configuration at `\EFI\hboot\hboot.conf` holds global settings only.
Generated automatically by `hboot install`. Edit with `hboot config edit`:

### Example Main config (`\EFI\hboot\hboot.conf`)

```ini
default = arch
timeout = 5
order = arch windows
# no_scan = true    # uncomment to skip auto-detection
```

### Example Linux Entry file (e.g. `\EFI\hboot\entries\arch.conf`)

```ini
title = Arch Linux
efi = \vmlinuz-linux
options = root=UUID=your-uuid rw quiet
initrd = \initramfs-linux.img
```

### Entry with boot counter (`\EFI\hboot\entries\arch+3.conf`)

A `+N` suffix in the filename sets the boot counter. The entry will be
auto-selected at most N times. After each boot the counter decrements;
when exhausted the entry is hidden. Mark it good via userspace once the
system comes up:

```bash
sudo hboot entry mark-good arch
```

An entry like `arch+3.conf` becomes `arch.conf`.

### Windows Entry file (`\EFI\hboot\entries\windows.conf`)

```ini
title = Windows
efi = \EFI\Microsoft\Boot\bootmgfw.efi
```

| Key | Where | Description |
|-----|-------|-------------|
| `default` | `hboot.conf` | Entry auto-selected when timeout expires |
| `timeout` | `hboot.conf` | Seconds before auto-boot (0 = wait forever) |
| `order` | `hboot.conf` | Space-separated display order |
| `no_scan` | `hboot.conf` | Use only config entries (skip auto-detect) |
| `title` | `entries/*.conf` | Display name in the menu |
| `efi` | `entries/*.conf` | Path to the `.efi` binary on the ESP |
| `options` | `entries/*.conf` | Kernel command-line arguments |
| `initrd` | `entries/*.conf` | Path to initramfs on the ESP |
| `+N` suffix | filename | Boot counter — decrements each boot, entry hidden at 0 |

## Boot counting

hboot supports automatic fallback with boot counters.
Name an entry file `name+N.conf` where N is the number
of allowed boot attempts:

- `arch+3.conf` — allows 3 boot attempts
- Each time hboot boots it, the counter decrements (`+3` → `+2` → `+1`)
- On the last try (`+1` → no suffix), the entry becomes a normal entry
- If booting fails repeatedly and the counter reaches 0, the entry is
  hidden from the menu and the next entry in `order` is tried
- After a successful boot, run `hboot entry mark-good arch` to remove
  the counter (renames `arch+3.conf` → `arch.conf`)

Set up boot counting for a kernel update:

```bash
sudo hboot entry set-tries linux-testing 3
```

This renames `linux-testing.conf` to `linux-testing+3.conf`.

## BLS parity

hboot reads [Boot Loader Spec](https://uapi-group.org/specifications/specs/boot_loader_specification/)
type-1 entries from `\loader\entries\*.conf` on the ESP (the same
directory systemd-boot uses). This means entries generated by
`kernel-install`, `grub-mkconfig`, or package managers work without
modification.

BLS format keys supported: `title`, `linux` (→ efi), `initrd`, `options`,
`efi` (direct.efi path). Paths with `/` are converted to `\` automatically.

Boot counters in BLS filenames (e.g., `arch+3.conf`) work the same way.

## Boot menu

| Key | Action |
|-----|--------|
| `↑`/`↓` | Select entry |
| `1`–`9` | Direct entry selection |
| `Enter` | Boot |
| `m` | Manual boot (type an `.efi` path) |
| `f` | Enter UEFI firmware setup |

Entries with a boot counter show `[N]` next to the name (remaining tries).

If the boot fails, a recovery menu offers reboot, restore backup entries,
manual boot, firmware setup, or shutdown.

## One-key recovery

Before every boot and before any `hboot entry` command, hboot backs up
`\EFI\hboot\entries\*.conf` to `\EFI\hboot\backup\entries\*.conf`.

At startup, hold **r** for 2 seconds to restore the last backup:

```
Hold r for recovery...
```

This overwrites the current entries with the backed-up versions. Use it
when a bad entry or boot counter change leaves you unable to boot.

You can also restore backups from the recovery menu (option **b**) after a
failed boot.

## One-time setup

If your kernel is not on the ESP (e.g., ESP at `/efi`, kernel at `/boot`),
copy it there once:

```bash
# Find your kernel version
KVER=$(uname -r)
sudo cp /boot/vmlinuz-$KVER /boot/initramfs-$KVER.img /EFI/hboot/ 2>/dev/null \
  || sudo cp /boot/vmlinuz-linux /boot/initramfs-linux.img /boot/EFI/hboot/
sudo hboot install
```

hboot auto-detects any `vmlinuz-*` file on the ESP, regardless of distribution.

## Build from source

```bash
cargo build --target x86_64-unknown-uefi --release -p hboot-efi
cargo build --release -p hboot
```

## License

MIT
