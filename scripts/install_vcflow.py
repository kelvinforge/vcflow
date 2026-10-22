#!/usr/bin/env python3
"""Install the latest vcflow release from GitHub.

Usage: python3 install_vcflow.py

Standard library only. Downloads the installer for this OS from the latest
GitHub Release, refuses to continue unless its SHA256 matches the digest
GitHub publishes for that asset, then installs it.

macOS: copies vcflow.app into /Applications. Downloads made here carry no
quarantine flag (and the flag is cleared anyway), so Gatekeeper's
"Not Opened" dialog does not appear. That is only safe because the file
is verified against the release digest first.
Windows: runs the NSIS setup .exe.
"""

import hashlib
import json
import platform
import shutil
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path

REPO = "kelvinforge/vcflow"
API = f"https://api.github.com/repos/{REPO}/releases/latest"


def latest_release():
    req = urllib.request.Request(API, headers={"Accept": "application/vnd.github+json"})
    with urllib.request.urlopen(req) as resp:
        return json.load(resp)


def pick_asset(release, suffix):
    for asset in release["assets"]:
        if asset["name"].endswith(suffix):
            return asset
    sys.exit(f"No *{suffix} asset in release {release['tag_name']}")


def download_verified(asset, dest_dir):
    digest = asset.get("digest") or ""
    if not digest.startswith("sha256:"):
        sys.exit(f"No SHA256 digest published for {asset['name']}; refusing to install")
    expected = digest.split(":", 1)[1]

    path = Path(dest_dir) / asset["name"]
    print(f"Downloading {asset['name']} ...")
    sha = hashlib.sha256()
    with urllib.request.urlopen(asset["browser_download_url"]) as resp, open(path, "wb") as out:
        while chunk := resp.read(1 << 20):
            sha.update(chunk)
            out.write(chunk)

    if sha.hexdigest() != expected:
        sys.exit(f"Checksum mismatch for {asset['name']}\n  expected {expected}\n  got      {sha.hexdigest()}")
    print("Checksum OK")
    return path


def install_mac(dmg):
    mount = Path(tempfile.mkdtemp(prefix="vcflow-mnt-"))
    subprocess.run(["hdiutil", "attach", str(dmg), "-nobrowse", "-readonly", "-mountpoint", str(mount)], check=True)
    try:
        apps = list(mount.glob("*.app"))
        if not apps:
            sys.exit("No .app found inside the dmg")
        target = Path("/Applications") / apps[0].name
        if target.exists():
            shutil.rmtree(target)
        subprocess.run(["ditto", str(apps[0]), str(target)], check=True)
    finally:
        subprocess.run(["hdiutil", "detach", str(mount), "-quiet"], check=False)
    subprocess.run(["xattr", "-dr", "com.apple.quarantine", str(target)], check=False)
    print(f"Installed {target}")


def main():
    system = platform.system()
    if system == "Darwin":
        suffix = ".dmg"
    elif system == "Windows":
        suffix = "-setup.exe"
    else:
        sys.exit(f"Unsupported OS: {system}")

    release = latest_release()
    print(f"Latest release: {release['tag_name']}")
    asset = pick_asset(release, suffix)

    with tempfile.TemporaryDirectory() as tmp:
        installer = download_verified(asset, tmp)
        if system == "Darwin":
            install_mac(installer)
        else:
            subprocess.run([str(installer)], check=True)
    print("Done.")


if __name__ == "__main__":
    main()
