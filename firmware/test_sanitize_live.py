#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import stat
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("sanitize_live", Path(__file__).with_name("sanitize_live.py"))
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
PUBLIC_HASH = "$1$openxiao$aaaaaaaaaaaaaaaaaaaaaa"
PRIVATE_HASH = "$1$private$ThisIsOnlyAFixtureHash"


class SanitizeLiveTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.base = Path(self.temp.name)
        self.root = self.base / "image"
        self.report = self.base / "report"
        for name in ("etc/dropbear", "root/.ssh", "usr/bin", "etc/config"):
            (self.root / name).mkdir(parents=True)
        self.shadow = self.root / "etc/shadow"
        self.shadow.write_text(f"root:{PRIVATE_HASH}:18128:0:99999:7:::\ndaemon:*:0:0:99999:7:::\nuser:$6$private$notreal:1:0:99999:7:::\n")
        self.shadow.chmod(0o640)
        (self.root / "usr/bin/system-file").write_bytes(b"unchanged system bytes\x00\xff")
        (self.root / "etc/dropbear/dropbear_rsa_host_key").write_bytes(b"fixture private key, never real")
        (self.root / "root/.ssh/authorized_keys").write_bytes(b"ssh-rsa fixture user@machine\n")

    def tearDown(self):
        self.temp.cleanup()

    def run_sanitize(self, apply=True):
        with patch.object(MODULE, "public_password_hash", return_value=PUBLIC_HASH):
            return MODULE.sanitize(self.root, self.report, apply)

    def test_only_privacy_items_change_and_manifest_proves_unchanged_files(self):
        before = self.shadow.stat()
        expected = f"root:{PUBLIC_HASH}:18128:0:99999:7:::\ndaemon:*:0:0:99999:7:::\nuser:!:1:0:99999:7:::\n"
        self.run_sanitize()
        self.assertEqual(self.shadow.read_text(), expected)
        after = self.shadow.stat()
        self.assertEqual((before.st_uid, before.st_gid, stat.S_IMODE(before.st_mode), before.st_ino),
                         (after.st_uid, after.st_gid, stat.S_IMODE(after.st_mode), after.st_ino))
        self.assertFalse((self.root / "etc/dropbear/dropbear_rsa_host_key").exists())
        self.assertFalse((self.root / "root/.ssh/authorized_keys").exists())
        checksum = hashlib.sha256(b"unchanged system bytes\x00\xff").hexdigest()
        self.assertIn(checksum + "  usr/bin/system-file", (self.report / "unchanged.SHA256SUMS").read_text())
        changes = json.loads((self.report / "changes.json").read_text())["changes"]
        self.assertEqual({entry["path"] for entry in changes}, {"etc/shadow", "etc/dropbear/dropbear_rsa_host_key", "root/.ssh/authorized_keys"})

    def test_default_dry_run_does_not_change_contents(self):
        before = MODULE.inventory(self.root)
        result = self.run_sanitize(apply=False)
        self.assertFalse(result["applied"])
        self.assertEqual(MODULE.inventory(self.root), before)
        self.assertEqual(json.loads((self.report / "changes.json").read_text())["changes"], [])

    def test_unknown_findings_preserved_and_reports_never_include_values(self):
        secret = "fixture-secret-value-that-must-never-be-reported"
        # Synthetic test address, not a deployment address.
        address = ".".join(map(str, (192, 168, 55, 99)))
        (self.root / "etc/config/unknown").write_text("API_KEY=" + secret + "\nserver=" + address + "\n")
        result = self.run_sanitize()
        self.assertTrue(result["requires_review"])
        self.assertIn(secret, (self.root / "etc/config/unknown").read_text())
        report_text = "".join(p.read_text() for p in self.report.iterdir())
        self.assertNotIn(secret, report_text)
        self.assertNotIn(PRIVATE_HASH, report_text)
        self.assertNotIn(address, report_text)
        self.assertNotIn("user@machine", report_text)
        for finding in json.loads((self.report / "scan.json").read_text())["after"]:
            self.assertEqual(set(finding), {"path", "category"})

    def test_additional_accounts_and_unknown_ssh_state_require_review(self):
        (self.root / "root/.ssh/unrecognized-settings").write_bytes(b"preserve unknown bytes")
        self.run_sanitize()
        findings = json.loads((self.report / "scan.json").read_text())["after"]
        self.assertIn({"path": "etc/shadow", "category": "additional_account_requires_review"}, findings)
        self.assertIn({"path": "root/.ssh/unrecognized-settings", "category": "unrecognized_ssh_state_requires_review"}, findings)
        self.assertEqual((self.root / "root/.ssh/unrecognized-settings").read_bytes(), b"preserve unknown bytes")

    def test_shadow_unknown_format_is_refused_before_any_deletion(self):
        self.shadow.write_text("unexpected-secret-record\n")
        with self.assertRaises(MODULE.UnsafeCopyError):
            self.run_sanitize()
        self.assertTrue((self.root / "etc/dropbear/dropbear_rsa_host_key").exists())

    def test_private_report_must_be_outside_image(self):
        self.report = self.root / "report"
        with self.assertRaises(MODULE.UnsafeCopyError):
            self.run_sanitize()

    def test_ordinary_hardlinks_are_preserved(self):
        a = self.root / "usr/bin/system-file"
        b = self.root / "usr/bin/system-hardlink"
        try:
            os.link(a, b)
        except OSError:
            self.skipTest("hardlinks unavailable")
        self.run_sanitize()
        self.assertEqual(a.stat().st_ino, b.stat().st_ino)
        self.assertEqual(a.read_bytes(), b.read_bytes())

    def test_shadow_alias_to_unknown_file_is_refused(self):
        try:
            os.link(self.shadow, self.root / "usr/bin/unknown-alias")
        except OSError:
            self.skipTest("hardlinks unavailable")
        with self.assertRaises(MODULE.UnsafeCopyError):
            self.run_sanitize()

    def test_ssh_alias_to_unknown_file_is_refused_before_changes(self):
        try:
            os.link(self.root / "etc/dropbear/dropbear_rsa_host_key", self.root / "usr/bin/unknown-alias")
        except OSError:
            self.skipTest("hardlinks unavailable")
        before = self.shadow.read_bytes()
        with self.assertRaises(MODULE.UnsafeCopyError):
            self.run_sanitize()
        self.assertEqual(self.shadow.read_bytes(), before)

    def test_known_key_symlink_removed_without_following_target(self):
        key = self.root / "root/.ssh/id_rsa"
        outside = self.base / "do-not-touch"
        outside.write_bytes(b"outside fixture")
        try:
            key.symlink_to(outside)
        except OSError:
            self.skipTest("symlinks unavailable")
        self.run_sanitize()
        self.assertFalse(key.is_symlink())
        self.assertEqual(outside.read_bytes(), b"outside fixture")

    def test_unknown_symlink_is_preserved(self):
        link = self.root / "usr/bin/system-link"
        try:
            link.symlink_to("system-file")
        except OSError:
            self.skipTest("symlinks unavailable")
        self.run_sanitize()
        self.assertTrue(link.is_symlink())
        self.assertEqual(os.readlink(link), "system-file")

    def test_openssl_receives_password_only_on_stdin(self):
        with patch.object(MODULE.subprocess, "run") as run:
            run.return_value.stdout = PUBLIC_HASH.encode() + b"\n"
            self.assertEqual(MODULE.public_password_hash(), PUBLIC_HASH)
            args, kwargs = run.call_args
            self.assertNotIn("open-xiaoai", args[0])
            self.assertEqual(kwargs["input"], b"open-xiaoai\n")

    def test_cli_failure_does_not_print_exception_content(self):
        output = io.StringIO()
        with patch.object(MODULE, "sanitize", side_effect=ValueError("secret-should-not-print")), contextlib.redirect_stderr(output):
            result = MODULE.main(["--root", str(self.root), "--report-dir", str(self.report)])
        self.assertEqual(result, 1)
        self.assertNotIn("secret-should-not-print", output.getvalue())


if __name__ == "__main__":
    unittest.main()
