# Linux packages

The deb, rpm and Arch packages come from `nfpm.yaml`. The AppImage comes from `appimage/AppRun` and
`build-appimage.sh` (it needs the system's libarchive, libmpv, xdotool and xrandr, like the
other packages). An AppImage updates itself from the `ps5-launcher-linux-x86_64.AppImage` file of
the latest release; the updater replaces the file named by the `APPIMAGE` variable.

`nfpm.yaml` describes the deb, rpm and Arch packages. `package.yml` builds them for each release
from the release binary, and `ci.yml` builds them on every change to check the files and the
dependency lists.

| File | Installed as |
|---|---|
| `ps5-launcher` (the binary) | `/usr/bin/ps5-launcher` |
| `ps5-launcher.desktop` | `/usr/share/applications/ps5-launcher.desktop`: the app menu entry of KDE, GNOME and other desktops. `install.sh` writes the same file for a per-user install. |
| `ps5-launcher-session` | `/usr/bin/ps5-launcher-session`: runs the launcher as the whole session. |
| `ps5-launcher-session.desktop` | `/usr/share/wayland-sessions/ps5-launcher.desktop`: lists "PS5 Launcher" at the login screen. |

## Standalone session

Choose **PS5 Launcher** at the login screen (SDDM, GDM, LightDM and others list it), or run
`ps5-launcher-session` from a text console. It starts [gamescope](https://github.com/ValveSoftware/gamescope),
which shows the launcher full screen and keeps the games it starts in the same session. Install
gamescope first (`gamescope` in Fedora, Arch and recent Debian and Ubuntu). Settings has
**Restart** and **Shut down** in this session, since there is no desktop to go back to. Output goes to
`~/.cache/ps5-launcher-session.log`.

To log in to it by itself, set the display manager's automatic login to the `ps5-launcher` session:

```ini
# /etc/sddm.conf.d/ps5-launcher.conf
[Autologin]
User=yourname
Session=ps5-launcher.desktop
```

Not tested yet: gamescope's focus and input between the launcher and a game, and the flags
gamescope needs on every graphics driver.

## Updates

A packaged launcher is updated by the package manager. The launcher's own updater needs a folder
it can write to, so it reports "not writable" for `/usr/bin`.
