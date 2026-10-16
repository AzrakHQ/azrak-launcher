# Third-party distribution notices

This release contains third-party software. These notices do not set or change
PS5 Launcher's own license and are not comprehensive legal certification.

The adjacent `licenses/inventory.json` lists active default-feature Rust
dependencies for `x86_64-unknown-linux-gnu`, using locked Cargo metadata and the
normal/build dependency tree. Build-time dependencies are conservatively included;
dev-only and unused optional dependencies are excluded. Original Cargo license
fields and normalized SPDX expressions are recorded separately. Except for the
Slint choice below, expressions are declarations, not a legal determination or
selection among alternatives. An `AND` must not be read as `OR`.

## Preserved documents and provenance

- `licenses/crates/` retains upstream LICENSE, NOTICE, COPYING, COPYRIGHT,
  AUTHORS and license-directory documents discovered recursively in cached
  source packages, including vendored native libraries. Texts are not rewritten.
- Where a published crate omits its license documents, the helper retrieves
  applicable documents from its official GitHub repository **at the published
  crate's VCS commit**, not a moving default branch. The inventory records URLs
  and SHA-256 hashes. A complete repository tree is checked for applicable
  standalone NOTICE files at the root, package ancestors and package subtree;
  an empty `noticePaths` means none was found in that scope, not that one was
  invented or that all inline source notices were audited.
- `licenses/standard/Apache-2.0.txt` is a complete standard license copied from
  a selected local crate, with provenance. It supplements short Apache headers;
  it does **not** replace upstream copyright or NOTICE texts.
- `licenses/assets/fonts/` retains the bundled fonts' discovered licensing
  documents. Non-Cargo assets and generated/embedded code need separate review.
- For `option-ext` under MPL-2.0, `licenses/crates/option-ext@0.2.0/source/`
  contains its exact cached source package. That Covered Software source is
  available under MPL-2.0; this does not relicense the larger application.

Unknown missing license sources, incomplete repository listings, unreadable
documents, unsafe paths and failed upstream requests stop packaging. Upstream
retrieval requires explicit `--allow-upstream`; Cargo itself runs locked/offline
after the release build populates its cache. The helper never publishes anything.

## Slint 1.18.1 — royalty-free desktop application choice

The Slint framework is used under
**LicenseRef-Slint-Royalty-free-2.0**, not by selecting its GPL or commercial
alternative. The full license and upstream licensing overview are retained in
the Slint packages' upstream document directories.

**Archive notices alone do not satisfy section 2 attribution.** The exact
[official version 2.0 license, section 2](https://github.com/slint-ui/slint/blob/v1.18.1/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md#2-license-conditions---attribution)
requires either:

1. Display the **`AboutSlint` widget** in an About screen/dialog accessible from
   the application's top-level menu; if there is no such screen/dialog, display
   the widget in the splash screen; **or**
2. Display the **official Slint attribution badge** on a public webpage,
   preferably the binary download page, easily found by any visitor.

Thus visible **in-application** attribution is not mandatory if the public badge
alternative is actually fulfilled. For the application route, a plain Settings
row saying “Built with Slint” or a text hyperlink is **not the specified widget**.
There is no standalone text sentence prescribed as a substitute. The official
[AboutSlint documentation](https://docs.slint.dev/latest/docs/slint/reference/std-widgets/misc/aboutslint/)
describes the widget as displaying a **“Made with Slint”** badge. Use the widget
or the [official badge linked by the license](https://github.com/slint-ui/slint/blob/master/logo/MadeWithSlint-logo-whitebg.png),
not a reconstructed text-only credit. Verify one route before publishing.

PS5 Launcher follows both routes: the `AboutSlint` widget is shown in the About
section at the end of Settings, which opens from the top bar, and the official
badge is shown on the project's README page, which links to the downloads.

The desktop grant excludes embedded-system use and applications exposing Slint
APIs. These notices do not establish eligibility for a different distribution
model or a commercial agreement.

## librqbit 9.0.1

The librqbit workspace crates declare Apache-2.0. Their published source packages
omit license documents. The retained
[official upstream license](https://github.com/ikatson/rqbit/blob/v9.0.1/LICENSE)
states **Copyright 2021 Igor Katson** and the Apache-2.0 license notice. It is
distributed unchanged alongside the full standard Apache-2.0 text. The audited
v9.0.1 repository tree has no applicable standalone NOTICE file. This is not a
claim that no other source attribution exists. Apache-2.0 does not impose a
general in-application “Built with librqbit” branding requirement.

## shadPS4 code in this release

`src/emulator/shadps4/pkg.rs` (the PS4 package reader) and `src/emulator/shadps4/pkg_data.rs` (its RSA keys and entry names) are
ported from shadPS4 v0.7.0, Copyright 2024 shadPS4 Emulator Project, under GPL-2.0-or-later
(<https://github.com/shadps4-emu/shadPS4>). Their file system walk follows LibOrbisPkg (MIT,
<https://github.com/maxton/LibOrbisPkg>); no LibOrbisPkg code is copied. A binary that contains
GPL code is distributed under the GPL's terms, which include offering the corresponding source.

## Tools downloaded at run time

These are not part of this release. The launcher downloads them from their official
GitHub releases when needed, verifies a SHA-256 checksum and runs them as separate
programs, under their own licenses:

- **KytyPS5** (PS5 games) and **shadPS4** (PS4 games, GPL-2.0).

## Scope requiring separate review

This is a source-document retention mechanism, not a complete binary-content or
legal audit. It does not certify toolchain/runtime licensing, system libraries
(including externally loaded libarchive), non-Cargo assets, changes to upstream
code, patent/trademark rights, or application license selection. Installation or
repackaging must retain these documents; the existing installer is not modified
by this packaging change. Review obligations again when dependencies, features,
target platform or distribution contents change.