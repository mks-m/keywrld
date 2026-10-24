#!/usr/bin/env python3
"""Prepare a release locally or publish the current clean commit on GitHub."""

import argparse
import hashlib
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
ARCH = ROOT / "packaging/arch"
REPO = "mks-m/keywrld"
ENV = {**os.environ, "RUSTUP_TOOLCHAIN": "stable", "CARGO_TARGET_DIR": str(ROOT / "target")}


def run(*args, cwd=None, capture=False):
    return subprocess.run(args, cwd=cwd or ROOT, env=ENV, check=True,
                          text=True, stdout=subprocess.PIPE if capture else None).stdout


def replace(path, pattern, replacement):
    text, count = re.subn(pattern, replacement, path.read_text(), count=1, flags=re.M)
    if count != 1:
        raise ValueError(f"Expected one match for {pattern!r} in {path}")
    path.write_text(text)


def version():
    return tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]


def srcinfo():
    text = run("makepkg", "--printsrcinfo", cwd=ARCH, capture=True)
    (ARCH / ".SRCINFO").write_text(text)


def test():
    run("cargo", "test", "--locked", "--release")


def prepare(new):
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", new):
        raise ValueError("Use a stable version such as 0.1.7 (without v)")
    if tuple(map(int, new.split('.'))) < tuple(map(int, version().split('.'))):
        raise ValueError("Release version cannot go backwards")
    tags = run("git", "tag", "--list", f"v{new}", capture=True)
    if tags.strip():
        raise ValueError(f"Local tag v{new} already exists")
    replace(ROOT / "Cargo.toml", r'^version = "[^"]+"$', f'version = "{new}"')
    # Refresh only the workspace package entry; preserve all resolved dependencies.
    run("cargo", "update", "--offline", "--workspace")
    replace(ARCH / "PKGBUILD", r"^pkgver=.*$", f"pkgver={new}")
    replace(ARCH / "PKGBUILD", r"^pkgrel=.*$", "pkgrel=1")
    replace(ARCH / "PKGBUILD", r"^sha256sums=.*$", "sha256sums=('TODO_RELEASE_SHA256')")
    srcinfo()
    test()
    print(f"Prepared {new}. Review and commit the changes, then run scripts/release.py publish.")


def publish():
    if run("git", "status", "--porcelain", capture=True).strip():
        raise ValueError("Commit or remove pending changes before publishing")
    current = version()
    if not re.search(rf"^pkgver={re.escape(current)}$", (ARCH / "PKGBUILD").read_text(), re.M):
        raise ValueError("Cargo and PKGBUILD versions differ; run prepare first")
    commit = run("git", "rev-parse", "HEAD", capture=True).strip()
    tag = f"v{current}"
    run("gh", "auth", "status")
    test()
    # Push the exact release commit under a new tag; never force an existing tag.
    run("git", "push", f"https://github.com/{REPO}.git", f"{commit}:refs/tags/{tag}")
    run("gh", "release", "create", tag, "--repo", REPO, "--verify-tag",
        "--title", tag, "--generate-notes")
    package_metadata()


def package_metadata():
    """May be rerun independently after a download or metadata failure."""
    current = version()
    # Verify that the release exists before using its tag archive.
    run("gh", "release", "view", f"v{current}", "--repo", REPO, capture=True)
    archive = ARCH / f"keywrld-{current}.tar.gz"
    url = f"https://github.com/{REPO}/archive/refs/tags/v{current}.tar.gz"
    run("curl", "--fail", "--location", "--retry", "3", "--output", str(archive), url)
    with archive.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    replace(ARCH / "PKGBUILD", r"^sha256sums=.*$", f"sha256sums=('{digest}')")
    srcinfo()
    run("makepkg", "--verifysource", cwd=ARCH)
    print("Release checksum and .SRCINFO updated. Review and commit these packaging changes.")
    print("Run the clean-chroot and namcap checks in packaging/arch/README.md before AUR submission.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("prepare", help="Update versions and run tests").add_argument("version")
    commands.add_parser("publish", help="Push the clean commit as a tag and publish on GitHub")
    commands.add_parser("package", help="Refresh checksum/.SRCINFO for the published version")
    args = parser.parse_args()
    try:
        if args.command == "prepare":
            prepare(args.version)
        elif args.command == "publish":
            publish()
        else:
            package_metadata()
    except (ValueError, subprocess.CalledProcessError, OSError) as error:
        sys.exit(f"Release failed: {error}")


if __name__ == "__main__":
    main()
