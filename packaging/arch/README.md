# Arch / AUR release

This directory contains the release package recipe. It is not ready to submit
until its release checksum is filled in.
The packaging files are 0BSD licensed, following Arch's package-source guidance;
the application is MIT licensed (../../LICENSE).

## Prepare and publish upstream

Requires Python 3.11+, Git, Cargo/Rust, makepkg, curl, and the GitHub CLI.
Authenticate using `gh auth login` and configure Git HTTPS authentication
(`gh auth setup-git` if needed). The repository must already exist on GitHub.

From the repository root, for example:

```sh
scripts/release.py prepare 0.1.7
git diff
git add Cargo.toml Cargo.lock packaging/arch/PKGBUILD packaging/arch/.SRCINFO
git commit -m 'Release 0.1.7'
scripts/release.py publish
```

For the first release, commit all project files before publishing. Preparing
the current version is allowed when it has not been released yet. Preparation
updates Cargo.toml/Cargo.lock and pkgver, resets pkgrel and the checksum, generates
.SRCINFO, and runs tests. It does not commit changes or contact GitHub.

Publishing requires a clean working tree, reruns tests, pushes the exact HEAD
commit as `vVERSION`, and creates a GitHub release with generated notes. This
command publishes immediately. It never force-pushes tags. It does not push your
branch; push it separately when appropriate.

After publishing, it downloads the tagged source archive, calculates SHA-256,
regenerates .SRCINFO, and verifies the package source. Review and commit these
packaging changes separately: the archive must exist before its checksum can be
recorded. Do not move the published tag to include that follow-up commit.

If publishing stops after pushing the tag, run `scripts/release.py resume`.
It resolves the existing remote tag to its commit and passes that target
explicitly to GitHub, including when the default branch has not been pushed.
It does not move the tag or publish the current local HEAD.
If the release already exists, resume reuses it (publishing it if it is a draft)
and continues with packaging metadata. API failures stop the operation.
If the release exists but downloading or packaging failed, rerun
`scripts/release.py package`. Never overwrite a released version to retry.

Keep Cargo.lock committed and verify that the source archive contains LICENSE,
Cargo.lock, and the icon assets. No binary packages are uploaded by this tooling.

## Validate

Install the packaging tools (`base-devel`, `devtools`, `namcap`, and
`pacman-contrib`) on the build machine. From this directory:

```sh
bash -n PKGBUILD
makepkg --verifysource
extra-x86_64-build
namcap PKGBUILD
namcap keywrld-*.pkg.tar.zst
makepkg --printsrcinfo > .SRCINFO
pkgctl license check
```

Review all diagnostics. The clean chroot must build and pass the tests before
submission. Test installing the resulting package and launching it in a Hyprland
Wayland session. Check `keywrld --version`, reopening to toggle, theme loading,
and keyboard/mouse discovery. Do not install or replace user keybindings from
the package. The README documents the optional binding.

## Submit to AUR

Check the official package database and AUR for an existing keywrld package.
Create an AUR account and configure its SSH key, then clone:

```sh
git clone ssh://aur@aur.archlinux.org/keywrld.git
```

Copy PKGBUILD, .SRCINFO, LICENSE, REUSE.toml, and this README.md into that
separate checkout. LICENSE here covers packaging; package() installs the
application's MIT license from the upstream archive. Do not copy the application source, archives, or built
packages. Commit the reviewed packaging files on `master` and push only when
ready to publish. Regenerate .SRCINFO after every metadata change.

For a new upstream release, update pkgver, reset pkgrel to 1, update checksums,
and repeat validation. Increment pkgrel for packaging changes requiring a
rebuild of the same upstream version.

References:
- https://wiki.archlinux.org/title/Rust_package_guidelines
- https://wiki.archlinux.org/title/Arch_package_guidelines
- https://wiki.archlinux.org/title/AUR_submission_guidelines
