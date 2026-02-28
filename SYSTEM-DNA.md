# System DNA — Nimbini (Arch Linux)

Reconstruction specification for William's Linux desktop. This file, combined with the dotfiles repo, package lists, and Syncthing/GitHub data, is sufficient to rebuild the full system from a bare Arch ISO.

**Philosophy**: This is a compressed, lossless representation of system state. A few KB of text that replaces a 500GB disk image. The actual data (Forge, Clinical, code) lives off-machine in Syncthing, Dropbox, and GitHub. This file captures *everything else*.

**Last verified**: 2026-02-28

---

## Hardware

- **Machine**: Intel NUC13ANHi5
- **CPU**: 13th Gen Intel, 16 threads (4P+8E cores)
- **GPU**: Intel Iris Xe (Raptor Lake-P, integrated)
- **NVMe**: Single drive, ~2TB
- **Keyboard**: Piantor (split, ZMK firmware handles Colemak-DH — host XKB must stay `us`)

---

## Disk Layout

Single NVMe with two partitions:

```
nvme0n1
├─ nvme0n1p1  EFI System Partition (FAT32, ~1GB)  →  /boot
└─ nvme0n1p2  LUKS2 encrypted                     →  btrfs inside
   └─ root (mapper name)
      ├─ @            →  /
      ├─ @home        →  /home
      ├─ @log         →  /var/log
      ├─ @pkg         →  /var/cache/pacman/pkg
      └─ @.snapshots  →  /.snapshots
```

### Critical UUIDs (change on reinstall — record new ones)
- **EFI partition**: `F987-1C6C`
- **LUKS partition**: `0897eafe-aa66-4e3e-8d19-03c67d35ec46`
- **LUKS PARTUUID**: `6a95d2a4-94f3-40d2-9f8a-a2f2945e2fc0`
- **Btrfs filesystem**: `c581261f-a412-49b9-9cff-df4888be44fc`

### Btrfs mount options (all subvolumes)
```
rw,relatime,compress=zstd:3,ssd,space_cache=v2
```

### Swap
- zram (no swap partition) — configured via `zram-generator`

---

## Phase 1: Base Install (from Arch ISO)

```bash
# 1. Partition (gdisk or fdisk)
#    p1: 1GB EFI (type EF00)
#    p2: remainder (type 8309 = Linux LUKS)

# 2. Encrypt
cryptsetup luksFormat --type luks2 /dev/nvme0n1p2
cryptsetup open /dev/nvme0n1p2 root

# 3. Btrfs with subvolumes
mkfs.btrfs /dev/mapper/root
mount /dev/mapper/root /mnt
btrfs subvolume create /mnt/@
btrfs subvolume create /mnt/@home
btrfs subvolume create /mnt/@log
btrfs subvolume create /mnt/@pkg
btrfs subvolume create /mnt/@.snapshots
umount /mnt

# 4. Mount subvolumes
mount -o compress=zstd:3,subvol=@ /dev/mapper/root /mnt
mkdir -p /mnt/{boot,home,var/log,var/cache/pacman/pkg,.snapshots}
mount -o compress=zstd:3,subvol=@home /dev/mapper/root /mnt/home
mount -o compress=zstd:3,subvol=@log /dev/mapper/root /mnt/var/log
mount -o compress=zstd:3,subvol=@pkg /dev/mapper/root /mnt/var/cache/pacman/pkg
mount -o compress=zstd:3,subvol=@.snapshots /dev/mapper/root /mnt/.snapshots
mount /dev/nvme0n1p1 /mnt/boot

# 5. Pacstrap base
pacstrap /mnt base base-devel linux linux-firmware intel-ucode btrfs-progs git vim

# 6. Generate fstab
genfstab -U /mnt >> /mnt/etc/fstab

# 7. Chroot
arch-chroot /mnt
```

---

## Phase 2: System Configuration (in chroot)

### Locale and timezone
```bash
ln -sf /usr/share/zoneinfo/GB /etc/localtime
hwclock --systohc
echo "en_GB.UTF-8 UTF-8" >> /etc/locale.gen
locale-gen
echo "LANG=en_GB.UTF-8" > /etc/locale.conf
echo "KEYMAP=us" > /etc/vconsole.conf
echo "nimbini" > /etc/hostname
```

### Initramfs (LUKS + btrfs)
Edit `/etc/mkinitcpio.conf`:
```
MODULES=(btrfs)
BINARIES=(/usr/bin/btrfs)
HOOKS=(base udev autodetect microcode modconf kms keyboard keymap consolefont block encrypt filesystems fsck)
```
```bash
mkinitcpio -P
```

### Bootloader (systemd-boot)
```bash
bootctl install

# /boot/loader/loader.conf
cat > /boot/loader/loader.conf << 'EOF'
timeout 3
EOF

# /boot/loader/entries/arch.conf — use PARTUUID of the LUKS partition
cat > /boot/loader/entries/arch.conf << 'EOF'
title   Arch Linux (linux)
linux   /vmlinuz-linux
initrd  /initramfs-linux.img
options cryptdevice=PARTUUID=<LUKS-PARTUUID>:root root=/dev/mapper/root zswap.enabled=0 rootflags=subvol=@ rw rootfstype=btrfs
EOF

# Also create fallback entry with initramfs-linux-fallback.img
```

### User
```bash
useradd -m -G wheel,video,input,seat,ollama -s /bin/bash will
passwd will
# Add to sudoers: uncomment %wheel ALL=(ALL:ALL) ALL in visudo
```

---

## Phase 3: Packages

### Official repos
```bash
# Install from the tracked package list (dotfiles/arch-packages.txt)
pacman -S --needed - < arch-packages.txt
```

### AUR packages
```bash
# Install paru first (or yay)
git clone https://aur.archlinux.org/paru.git /tmp/paru
cd /tmp/paru && makepkg -si

# Then install AUR packages (dotfiles/arch-packages-aur.txt)
paru -S --needed - < arch-packages-aur.txt
```

### Rust toolchain + crates from crates.io
```bash
# Rustup should already be present via 'rust' package, or:
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Crates (pinned versions — update as needed)
cargo install bat fd-find ripgrep sd skim starship zoxide
cargo install dotter dprint jaq himalaya trashy
cargo install yazi-fm yazi-cli zellij
cargo install nu  # nushell
```

### Rust projects from dotfiles
```bash
cd ~/dotfiles/rust-projects
for project in */; do
    cd "$project"
    cargo install --path .
    cp ~/.cargo/bin/$(basename "$project") ~/.local/bin/ 2>/dev/null
    cd ..
done
```

### npm globals
```bash
npm install -g @google/gemini-cli @openai/codex
```

### pipx
```bash
pipx install west  # Zephyr/ZMK build tool
```

---

## Phase 4: System Services

### keyd (keyboard remapping)
```
# /etc/keyd/default.conf
[main]
print = f20
sysrq = f20
```
```bash
systemctl enable --now keyd
```

### System-level services to enable
```bash
systemctl enable bluetooth
systemctl enable keyd
systemctl enable NetworkManager
systemctl enable NetworkManager-dispatcher
systemctl enable NetworkManager-wait-online
systemctl enable ollama
systemctl enable seatd
systemctl enable sshd
systemctl enable systemd-resolved
systemctl enable systemd-timesyncd
systemctl enable tailscaled
systemctl enable snapper-timeline.timer
systemctl enable snapper-cleanup.timer
```

---

## Phase 5: User Environment

### Shell
```bash
# Set nushell as default shell
chsh -s $(which nu) will
```

### Dotfiles
```bash
# SSH key first (generate or restore from backup)
ssh-keygen -t ed25519 -C "will@nimbini"
# Add public key to GitHub

# Clone and deploy
git clone git@github.com:willnapier/dotfiles.git ~/dotfiles

# Create local.toml for platform detection
mkdir -p ~/.dotter
cat > ~/.dotter/local.toml << 'EOF'
packages = ["linux"]
EOF

cd ~/dotfiles && dotter deploy
```

### User services to enable
```bash
systemctl --user enable ai-export-watcher.service
systemctl --user enable assistants-docs-watcher.service
systemctl --user enable collect-projects-watcher.service
systemctl --user enable dropbox.service
systemctl --user enable forge-link-manager.service
systemctl --user enable git-auto-pull-watcher.service
systemctl --user enable git-auto-push-watcher.service
systemctl --user enable link-service.service
systemctl --user enable nushell-env.service
systemctl --user enable ssh-import-env.service
systemctl --user enable syncthing.service
systemctl --user enable web-clip-watcher.service
systemctl --user enable claude-code-nightly-cleanup.timer
systemctl --user enable collect-projects.timer
systemctl --user enable continuum-auto-import.timer
systemctl --user enable continuum-sync-claude.timer
systemctl --user enable dotter-drift-monitor.timer
systemctl --user enable firefox-nightly-restart.timer
systemctl --user enable frecency-daemon.timer
systemctl --user enable helix-undo-cleanup.timer
systemctl --user enable mail-sync.timer
systemctl --user enable package-list-backup.timer
systemctl --user enable readwise-sync.timer
systemctl --user enable system-health-check.timer
```

---

## Phase 6: Snapper (btrfs snapshots)

```bash
# Create configs
snapper -c root create-config /
snapper -c home create-config /home

# Allow user access
snapper -c root set-config "ALLOW_USERS=will"
snapper -c home set-config "ALLOW_USERS=will"

# Timeline settings (both configs)
for cfg in root home; do
    snapper -c $cfg set-config "TIMELINE_CREATE=yes"
    snapper -c $cfg set-config "TIMELINE_CLEANUP=yes"
    snapper -c $cfg set-config "TIMELINE_LIMIT_HOURLY=5"
    snapper -c $cfg set-config "TIMELINE_LIMIT_DAILY=7"
    snapper -c $cfg set-config "TIMELINE_LIMIT_WEEKLY=4"
    snapper -c $cfg set-config "TIMELINE_LIMIT_MONTHLY=6"
    snapper -c $cfg set-config "TIMELINE_LIMIT_YEARLY=2"
    snapper -c $cfg set-config "NUMBER_LIMIT=50"
done

# Enable timers (already done in Phase 4 system services)
# snap-pac is installed via packages — auto-snapshots on pacman operations
```

---

## Phase 7: Secrets (Manual Steps)

These cannot be automated — they require interactive authentication:

1. **SSH key**: Generate (Phase 5) and add to GitHub, Tailscale, etc.
2. **Tailscale**: `sudo tailscale up` — authenticate via browser
3. **Syncthing**: Access web UI at `localhost:8384` — pair with Mac
4. **Dropbox**: `dropbox start` — authenticate via browser
5. **gnome-keyring**: Entries are created on first use:
   - `gemini-api-key` / `forgepodium` — Gemini CLI API key
   - Email credentials (himalaya) — see email setup docs
6. **Git**: `git config --global user.name "William Napier"` + `git config --global user.email "..."`
7. **Claude Code**: `claude login` — authenticate via browser
8. **Gemini CLI**: API key stored in gnome-keyring (see handoff doc 2026-02-24)
9. **Surfshark VPN**: Log in via client

---

## Phase 8: Data Restoration

Once Syncthing and Dropbox are connected, data flows back automatically:

| Source | Data | Mechanism |
|--------|------|-----------|
| Syncthing | `~/Forge/`, `~/Assistants/shared/`, `~/Clinical/` | Auto-sync from Mac |
| Dropbox | `~/Dropbox/` | Dropbox client |
| GitHub | Code repos, `~/dotfiles/` | `git clone` |
| GitHub | `~/piantor-zmk/` | `git clone` |

---

## Maintenance: Keeping DNA Current

The package lists (`arch-packages.txt`, `arch-packages-aur.txt`) are auto-updated daily by `package-list-backup.timer`.

**What needs manual updates to this file:**
- New system services added/removed
- User group changes
- Disk layout changes
- New secret/credential requirements
- Boot configuration changes
- New cargo crate installs from crates.io (not from dotfiles/rust-projects)
- New npm/pipx globals

**Trigger**: When any of the above changes, update this file and commit. The `system-health-check` script could be extended to detect drift.

---

## Recovery Reference

For partial recovery (system boots but is broken), see:
`~/Assistants/shared/NIMBINI-RECOVERY-2026-02-22.md`

That document covers the exact mount sequence for accessing the system from an Arch live USB with LUKS + btrfs subvolumes.
