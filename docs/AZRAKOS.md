# Azrak Launcher on AzrakOS

On [AzrakOS](https://github.com/AzrakHQ/azrak-os) the launcher is the whole screen: there's no
desktop behind it. So the screens the OS needs (internet, controllers, sound, updates, installing
from the USB stick) are part of the launcher, in [src/os/](../src/os/), and appear only on AzrakOS.
Everywhere else (Ubuntu, Fedora, SteamOS, …) none of it shows.

This page is the contract between the two repos: what the launcher expects from the OS, and what
the OS can expect from the launcher. Change both sides together, and keep old versions working
(below).

## How the launcher knows where it is

| | Detected by | Shows | Pretend for screenshots |
|---|---|---|---|
| AzrakOS | `/usr/lib/azrak-os/release` exists (the OS writes the bundled launcher's `--version` into it). Also the older PS5 Launcher OS marker. | Settings → SYSTEM | `AZRAK_OS=1` |
| The AzrakOS USB stick | AzrakOS, plus `rd.live.image` on the kernel command line and the stick's installer (`/usr/libexec/azrak-os/install`) | "Welcome to AzrakOS" (Try / Install) first, and Settings → SYSTEM → Install AzrakOS on this PC | `AZRAK_USB=1`, `AZRAK_INSTALLER=stand-in` |

## What the OS provides

| Feature (launcher) | The launcher runs | The OS provides (azrak-os) |
|---|---|---|
| Internet, Join Wi-Fi | `nmcli` | NetworkManager, and a polkit rule (`minimal/files/usr/share/polkit-1/rules.d/50-azrak-os.rules`) letting the logged-in `player` manage the network: joining a new network saves a system-wide connection, which polkit otherwise reserves for an administrator. |
| Controllers and headsets | `bluetoothctl` | BlueZ |
| Sound output | `pactl` | PipeWire with its PulseAudio server |
| System updates | `rpm-ostree status --json` (a staged deployment means "Restart to update") | `azrak-os-update.timer` runs `/usr/libexec/azrak-os/update` daily: it stages the next version (and switches an NVIDIA PC to `:minimal-nvidia`), never restarts by itself |
| Restart, shut down | `systemctl reboot` / `poweroff` | logind (the active local session may) |
| This PC | `lspci` | pciutils |
| The session | Logs `window shown` (on stdout, which the session appends to `~/.cache/azrak-os.log`) as soon as its window is up | greetd logs `player` in and runs `/usr/libexec/azrak-os/session`: it starts `~/.local/bin/azrak` (copied from `/usr/share/azrak-os/release` the first time, and again if the copy doesn't answer `--version`) in Gamescope, or in cage when Gamescope can't run. If no `window shown` (or, from older launchers, `welcome screen ready` / `home screen shown`) comes within 90 s, it starts the launcher again. |

## The installer (USB stick only)

`/usr/libexec/azrak-os/install` ([azrak-os: live/](https://github.com/AzrakHQ/azrak-os/tree/main/live)) does the work; the
launcher only shows it ([src/os/install.rs](../src/os/install.rs), parsing in [src/os/system.rs](../src/os/system.rs)).

- `install --disks` (as the user): one line per disk it can install on (not the stick, nothing
  under 40 GB):
  `disk PATH SIZE_BYTES WINDOWS(0/1) USB(0/1) MODEL…`
- `pkexec install DISK` (a polkit rule on the stick lets `player` run it, no password): erases
  DISK and installs AzrakOS from the image stored on the stick. It prints, one per line:
  - `progress PERCENT TEXT` as it goes,
  - then `done TAG [mok] [nvidia-later]`: TAG is the edition installed (`minimal` or
    `minimal-nvidia`); `mok`: Secure Boot's key screen comes at the restart; `nvidia-later`: the
    PC has an NVIDIA card and gets its driver on the installed system,
  - or `error TEXT`.

  Anything else it prints is ignored.

## What the OS can ask of the launcher

`azrak --supports FEATURE` exits 0 when the launcher has it, 1 otherwise (releases from before the
option reject it, which also counts as no):

| FEATURE | Means | Checked by |
|---|---|---|
| `gamescope` | It tags its window, so Gamescope shows it | the session (otherwise cage) and the image build (`:minimal` is published only with such a release) |
| `system-settings` | Settings → SYSTEM | — |
| `os-install` | The USB stick's install screens | `build-live-iso.sh` refuses to build a stick whose launcher can't install |

## Changing it

- A new capability gets a new `--supports` name, and the OS checks it before relying on it.
- The installer may add words to its lines (the launcher ignores what it doesn't know); it doesn't
  change or remove the ones above.
- The launcher keeps working on an older OS: a missing tool or rule shows as unavailable in
  Settings, never as a crash.
