#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Preparation/inspection of a clean public OTA only. Never reads a speaker."""
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys


def check_key(path):
    fields = Path(path).read_text().strip().split()
    if len(fields) < 2 or fields[0] != "ssh-rsa":
        raise ValueError("Use a single RSA public key (id_rsa.pub), never a private key")
    if len(Path(path).read_text().strip().splitlines()) != 1:
        raise ValueError("Only one public-key line is accepted")
    data = base64.b64decode(fields[1], validate=True)
    length = int.from_bytes(data[:4], "big")
    if data[4:4+length] != b"ssh-rsa":
        raise ValueError("Public-key blob does not match ssh-rsa")
    return f"ssh-rsa {fields[1]}\n"  # Discard account/machine-name comments.


def replace_exact(path, before, after):
    text = path.read_text()
    if text.count(before) != 1:
        raise ValueError(f"Unexpected source layout: {path.name}")
    path.write_text(text.replace(before, after), newline="\n")


def patch(root, patches, mode, key):
    root, patches = Path(root), Path(patches)
    # Keep the original UART login; the upstream login patch additionally enables
    # a passwordless serial shell, which this generator does not need.
    inittab = (root / "etc/inittab").read_bytes()
    password = "open-xiaoai" if mode == "public" else os.urandom(48).hex()
    hashed = subprocess.run(["openssl", "passwd", "-1", "-salt", "openxiao", "-stdin"], input=password+"\n", text=True, check=True, capture_output=True).stdout.strip()
    for name in ("01-ssh.patch", "02-login.patch", "03-ota.patch", "04-start.patch"):
        contents = (patches / name).read_text().replace("{SSH_PASSWORD}", hashed)
        subprocess.run(["patch", "--batch", "--fuzz=0", "-p1"], cwd=root, input=contents, text=True, check=True)
    (root / "etc/inittab").write_bytes(inittab)
    # Official 1.62.2 contains factory SSH keys. Do not copy them into a candidate.
    for directory in (root / "etc/dropbear", root / "root/.ssh"):
        if directory.is_symlink():
            raise ValueError("Unexpected SSH directory symlink")
        if directory.exists():
            for entry in directory.iterdir():
                if entry.is_dir() and not entry.is_symlink():
                    shutil.rmtree(entry)
                else:
                    entry.unlink()
        else:
            directory.mkdir(parents=True)
        directory.chmod(0o700)
    if mode == "key":
        public = check_key(key)
        for dest in (root / "etc/dropbear/authorized_keys", root / "root/.ssh/authorized_keys"):
            dest.write_text(public, newline="\n")
            dest.chmod(0o600)
        replace_exact(root / "etc/config/dropbear", "option PasswordAuth '1'", "option PasswordAuth '0'")
        replace_exact(root / "etc/config/dropbear", "option RootPasswordAuth '1'", "option RootPasswordAuth '0'")
    # Isolate native capture; preserve the official playback/noop/Capture setup.
    replace_exact(root / "etc/asound.conf", "pcm.mico_record {\n    type plug\n    slave.pcm Capture\n}", "pcm.mico_record {\n    type plug\n    slave.pcm {\n        type hw\n        card 0\n        device 0\n    }\n}")
    verify(root, mode)


def verify(root, mode):
    root = Path(root)
    if "1.62.2" not in (root / "usr/share/mico/version").read_text():
        raise ValueError("Wrong firmware version")
    vpm = root / "usr/lib/libvpm.so"
    if hashlib.sha256(vpm.read_bytes()).hexdigest() != "f5d76a34dc40eb1b7c9045614da636fe5558344c3334a44e152d22fbfaed56d9":
        raise ValueError("Wrong VPM ABI")
    if "/data/init.sh" not in (root / "etc/rc.local").read_text():
        raise ValueError("Missing startup hook")
    dropbear = (root / "etc/init.d/dropbear").read_text()
    if "/data/etc/dropbear/dropbear_rsa_host_key" not in dropbear or "|| keygen" not in dropbear:
        raise ValueError("Per-device SSH host-key generation is missing")
    for p in root.rglob("*"):
        if p.is_file() and not p.is_symlink():
            if re.search(r"(dropbear_.*_host_key|ssh_host_.*_key|id_rsa|id_ed25519)$", p.name):
                raise ValueError(f"SSH private key remained in image: {p.relative_to(root)}")
    config = (root / "etc/config/dropbear").read_text()
    for name in ("etc/dropbear/authorized_keys", "root/.ssh/authorized_keys"):
        p = root / name
        if mode == "public" and p.exists():
            raise ValueError("Public-default image contains an authorized key")
        if mode == "key" and (not p.is_file() or not p.read_text().startswith("ssh-rsa ")):
            raise ValueError("Personal public key is missing")
    if mode == "key" and ("option PasswordAuth '0'" not in config or "option RootPasswordAuth '0'" not in config):
        raise ValueError("Password login was not disabled")


def manifest(directory, mode):
    directory = Path(directory)
    data = {"model": "OH2P", "official_version": "1.62.2", "candidate_device_tested": False,
            "format": "partition inputs, not a signed full-device OTA",
            "official_ota_url": "https://cdn.cnbj1.fds.api.mi-img.com/xiaoqiang/rom/oh2p/mico_all_616cd9d93_1.62.2.bin",
            "official_ota_sha256": "08c6c0c5e7dfd3de59a7e37b4709ad90ab84e8025c799a642730e97ef53c0eba",
            "patch_source_commit": "9fc56179fbdde610e95755a843615669dece0d08",
            "squashfs_tools": "4.7.5", "ssh_auth_mode": mode,
            "public_default_password": "open-xiaoai" if mode == "public" else None,
            "candidate_ssh_private_keys_included": False, "device_dump_used": False,
            "files": {p.name: {"bytes": p.stat().st_size, "sha256": hashlib.sha256(p.read_bytes()).hexdigest()}
                      for p in sorted(directory.iterdir()) if p.is_file()}}
    (directory / "firmware-manifest.json").write_text(json.dumps(data, indent=2)+"\n")


if __name__ == "__main__":
    commands = {"check-key": check_key, "patch": patch, "verify": verify, "manifest": manifest}
    commands[sys.argv[1]](*sys.argv[2:])
