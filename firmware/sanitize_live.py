#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Sanitize an unpacked LOCAL system copy; never contacts or mounts a speaker.

The original live/rootfs image must remain outside the public repository.
Unknown findings are reported for human review and are never deleted.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys


SHADOW_PATHS = {"etc/shadow", "etc/shadow-", "etc/shadow.old", "etc/shadow.bak", "etc/shadow.orig"}
SSH_NAMES = {
    "authorized_keys", "authorized_keys2", "known_hosts", "known_hosts.old", "config",
    "id_rsa", "id_rsa.pub", "id_dsa", "id_dsa.pub", "id_ecdsa", "id_ecdsa.pub",
    "id_ed25519", "id_ed25519.pub", "id_dropbear", "id_dropbear.pub", "identity", "identity.pub",
}
HOST_KEY = re.compile(r"(?:dropbear_(?:rsa|dss|ecdsa|ed25519)_host_key(?:[.]pub)?|ssh_host_(?:rsa|dsa|ecdsa|ed25519)_key(?:[.]pub)?|ssh_host_key(?:[.]pub)?)\Z")
HASH = re.compile(r"(?:\$[A-Za-z0-9]+\$[^:\r\n]+|[A-Za-z0-9./]{13})\Z")
PRIVATE_PEM = re.compile(rb"(?m)^-----BEGIN (?:RSA |DSA |EC |OPENSSH |ENCRYPTED )?PRIVATE KEY-----\r?$")
PRIVATE_IP = re.compile(rb"(?<![0-9])(?:192\.168\.[0-9]{1,3}\.[0-9]{1,3}|10\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}|172\.(?:1[6-9]|2[0-9]|3[01])\.[0-9]{1,3}\.[0-9]{1,3})(?![0-9])")
CREDENTIAL = re.compile(rb"(?im)^\s*(?:export\s+)?(?:api[_-]?key|access[_-]?token|refresh[_-]?token|passToken|MI_PASS|MI_TOKEN|password)\s*[:=]\s*[^\s#]+")


class UnsafeCopyError(ValueError):
    pass


def digest_file(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def inventory(root):
    """lstat only for links; no symlink traversal, no file content in results."""
    result = {}
    for directory, dirs, files in os.walk(root, followlinks=False):
        for name in sorted(dirs + files):
            path = Path(directory) / name
            rel = path.relative_to(root).as_posix()
            s = path.lstat()
            kind = ("symlink" if stat.S_ISLNK(s.st_mode) else "file" if stat.S_ISREG(s.st_mode)
                    else "directory" if stat.S_ISDIR(s.st_mode) else "special")
            entry = {"type": kind, "mode": stat.S_IMODE(s.st_mode), "uid": s.st_uid,
                     "gid": s.st_gid, "inode": s.st_ino, "device": s.st_dev, "nlink": s.st_nlink}
            if kind == "file":
                entry.update(bytes=s.st_size, sha256=digest_file(path))
            elif kind == "symlink":
                entry["target_sha256"] = hashlib.sha256(os.fsencode(os.readlink(path))).hexdigest()
            elif kind == "special":
                entry["rdev"] = s.st_rdev
            result[rel] = entry
    return dict(sorted(result.items()))


def ssh_private_path(rel):
    path = Path(rel)
    parent = path.parent.as_posix()
    if parent in {"etc/dropbear", "etc/ssh", "data/etc/dropbear"}:
        return bool(HOST_KEY.fullmatch(path.name)) or path.name in {"authorized_keys", "authorized_keys2", "known_hosts", "known_hosts.old"}
    return parent == "root/.ssh" and (path.name in SSH_NAMES or bool(HOST_KEY.fullmatch(path.name)))


def public_password_hash():
    # This intentionally documented public password is not a user credential.
    process = subprocess.run(["openssl", "passwd", "-1", "-salt", "openxiao", "-stdin"],
                             input=b"open-xiaoai\n", capture_output=True, check=True)
    value = process.stdout.decode("ascii").strip()
    if not re.fullmatch(r"\$1\$openxiao\$[A-Za-z0-9./]{22}", value):
        raise UnsafeCopyError("Unexpected OpenSSL password-hash format")
    return value


def shadow_replacement(contents, public_hash):
    """Only password fields change; account names, dates, line endings survive."""
    result = []
    roots = 0
    for line in contents.decode("utf-8").splitlines(keepends=True):
        body = line.rstrip("\r\n")
        ending = line[len(body):]
        if not body or body.startswith("#"):
            result.append(line)
            continue
        fields = body.split(":")
        if len(fields) != 9:
            raise UnsafeCopyError("Unrecognized shadow record; review the local copy manually")
        if fields[0] == "root":
            roots += 1
            fields[1] = public_hash
        elif HASH.fullmatch(fields[1].lstrip("!")):
            fields[1] = "!"  # Remove non-root password hashes, retain the account.
        elif fields[1] not in {"", "!", "!!", "*", "!*"}:
            raise UnsafeCopyError("Unrecognized shadow password field; manual review required")
        result.append(":".join(fields) + ending)
    if roots != 1:
        raise UnsafeCopyError("Expected exactly one root shadow record")
    return "".join(result).encode("utf-8")


def findings(root, entries, planned_paths=()):
    """Reports contain path/category only: never a value, snippet or key body."""
    output = []
    planned = set(planned_paths)
    for rel, meta in entries.items():
        path = root / rel
        if meta["type"] == "symlink":
            target = os.readlink(path)
            if target.startswith("/") or ".." in Path(target).parts:
                output.append({"path": rel, "category": "symlink_target_requires_review"})
            continue
        if meta["type"] != "file":
            continue
        if rel in SHADOW_PATHS or rel == "etc/passwd":
            names = [line.split(":", 1)[0] for line in path.read_text().splitlines() if line and not line.startswith("#")]
            if any(name not in {"root", "daemon", "ftp", "network", "nobody"} for name in names):
                output.append({"path": rel, "category": "additional_account_requires_review"})
        if rel in planned:
            continue
        if rel in SHADOW_PATHS:
            continue
        categories = set()
        with path.open("rb") as stream:
            previous = b""
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                block = previous + chunk
                for expression, category in ((PRIVATE_PEM, "private_key_marker"), (PRIVATE_IP, "private_ipv4_marker"),
                                             (CREDENTIAL, "credential_assignment_marker")):
                    if expression.search(block):
                        categories.add(category)
                previous = block[-1024:]
        if path.name in {".env", ".ash_history", ".bash_history", "device.info", "known_hosts", "authorized_keys"} or path.suffix in {".log", ".pcap", ".pcm", ".wav"}:
            categories.add("possible_personal_state_file")
        if rel.startswith("root/.ssh/"):
            categories.add("unrecognized_ssh_state_requires_review")
        for category in sorted(categories):
            output.append({"path": rel, "category": category})
    return output


def write_preserving_metadata(path, contents):
    s = path.lstat()
    if not stat.S_ISREG(s.st_mode):
        raise UnsafeCopyError("Refusing to write through a link or non-regular shadow file")
    with path.open("r+b") as stream:
        stream.write(contents)
        stream.truncate()
    # Opening the existing inode preserves owner and hardlinks. Restore its mode
    # (some kernels clear special bits on writes) and timestamps explicitly.
    os.chmod(path, stat.S_IMODE(s.st_mode))
    if os.utime in os.supports_follow_symlinks:
        os.utime(path, ns=(s.st_atime_ns, s.st_mtime_ns), follow_symlinks=False)
    else:
        os.utime(path, ns=(s.st_atime_ns, s.st_mtime_ns))


def sanitize(root, report_dir, apply=False):
    root, report_dir = Path(root).resolve(), Path(report_dir).resolve()
    if not root.is_dir() or not (root / "etc").is_dir() or root == Path(root.anchor):
        raise UnsafeCopyError("Expected a dedicated unpacked local system copy")
    if report_dir == root or root in report_dir.parents:
        raise UnsafeCopyError("Report directory must be outside the image tree")
    if report_dir.exists() and any(report_dir.iterdir()):
        raise UnsafeCopyError("Use an empty report directory")
    before = inventory(root)
    if "etc/shadow" not in before:
        raise UnsafeCopyError("Local copy has no etc/shadow")
    planned_delete = [rel for rel, meta in before.items() if ssh_private_path(rel)]
    if any(before[rel]["type"] not in {"file", "symlink"} for rel in planned_delete):
        raise UnsafeCopyError("Unexpected object at a known SSH privacy path; manual review required")
    shadow_paths = [rel for rel in before if rel in SHADOW_PATHS]
    if any(before[rel]["type"] != "file" for rel in shadow_paths):
        raise UnsafeCopyError("Shadow is not a regular file; resolve its local-copy layout manually")
    # Never mutate an unknown hardlink alias by modifying shadow in place.
    for rel in shadow_paths:
        identity = (before[rel]["device"], before[rel]["inode"])
        aliases = [name for name, meta in before.items() if meta["type"] == "file" and (meta["device"], meta["inode"]) == identity]
        if any(name not in SHADOW_PATHS for name in aliases):
            raise UnsafeCopyError("Shadow has an unexpected hardlink alias; manual review required")
    for rel in planned_delete:
        if before[rel]["type"] != "file":
            continue
        identity = (before[rel]["device"], before[rel]["inode"])
        if any(meta["type"] == "file" and (meta["device"], meta["inode"]) == identity and name not in planned_delete
               for name, meta in before.items()):
            raise UnsafeCopyError("Known SSH file has an unexpected hardlink alias; manual review required")
    public_hash = public_password_hash()
    replacements = {rel: shadow_replacement((root / rel).read_bytes(), public_hash) for rel in shadow_paths}
    planned = set(planned_delete) | set(shadow_paths)
    scan_before = findings(root, before, planned)
    if apply:
        for rel, contents in replacements.items():
            write_preserving_metadata(root / rel, contents)
        for rel in planned_delete:
            (root / rel).unlink()  # Remove only this exact file/link, never its target.
    after = inventory(root)
    changes = []
    unchanged = []
    for rel, old in before.items():
        new = after.get(rel)
        if new is None:
            changes.append({"path": rel, "category": "removed_known_ssh_privacy_file", "before": old})
        elif old.get("sha256") != new.get("sha256"):
            changes.append({"path": rel, "category": "replaced_shadow_password_fields", "before": old, "after": new})
        elif old != new:
            changes.append({"path": rel, "category": "metadata_change", "before": old, "after": new})
        elif old["type"] == "file":
            unchanged.append(f"{old['sha256']}  {rel}\n")
    unexpected = [entry for entry in changes if entry["path"] not in planned]
    if unexpected:
        raise UnsafeCopyError("Unexpected change outside the explicit privacy paths")
    scan_after = findings(root, after, set(shadow_paths))
    report_dir.mkdir(parents=True, exist_ok=True)
    reports = {"before-manifest.json": before, "after-manifest.json": after,
               "changes.json": {"applied": apply, "changes": changes,
                                "planned_paths": sorted(planned), "root_password_policy": "public open-xiaoai; no forced rotation; rootfs read-only"},
               "scan.json": {"before": scan_before, "after": scan_after, "requires_review": bool(scan_after),
                             "policy": "Findings contain path/category only; unknown files are preserved."}}
    for name, data in reports.items():
        (report_dir / name).write_text(json.dumps(data, indent=2, ensure_ascii=True) + "\n", encoding="utf-8", newline="\n")
    (report_dir / "unchanged.SHA256SUMS").write_text("".join(unchanged), encoding="utf-8", newline="\n")
    return {"applied": apply, "changed_paths": len(changes), "unchanged_files": len(unchanged),
            "review_findings": len(scan_after), "requires_review": bool(scan_after)}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True, help="Dedicated unpacked LOCAL copy, never a live root filesystem")
    parser.add_argument("--report-dir", required=True, help="New/empty private report directory outside the image tree")
    parser.add_argument("--apply", action="store_true", help="Apply only the explicitly listed privacy edits; default is scan only")
    args = parser.parse_args(argv)
    try:
        result = sanitize(args.root, args.report_dir, args.apply)
    except Exception as error:
        # Never print exception values from parsers/subprocesses: they can carry
        # raw account text, credential data or an SSH-key body.
        print(f"Local-copy sanitization stopped ({type(error).__name__}); inspect privately.", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
