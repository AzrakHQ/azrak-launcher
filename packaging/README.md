# Packaging

Everything that turns a release binary into something users install.

| Folder | What it builds |
|---|---|
| [linux/](linux/README.md) | deb, rpm and Arch packages (nfpm), the AppImage, and the standalone login session. |
| [macos/](macos) | `PS5 Launcher.app` (`bundle.sh`): a universal, ad-hoc-signed app bundle with its icon. |

`install.sh` in the repository root installs the launcher for one user, from a release download
or a source checkout. The release workflow (`.github/workflows/package.yml`) runs these builds,
and `ci.yml` runs them on every change.
