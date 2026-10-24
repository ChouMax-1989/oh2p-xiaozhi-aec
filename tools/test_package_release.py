"""Offline archive validation; does not load Docker or contact a speaker."""
import ast
import hashlib
import json
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
import package_release as release

ROOT = Path(__file__).resolve().parents[1]


class PackageRelease(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temporary = tempfile.TemporaryDirectory()
        cls.output = Path(cls.temporary.name)
        release.package(ROOT, cls.output, release.DEFAULT_VERSION)
        with zipfile.ZipFile(cls.output / release.EASY_NAME) as archive:
            cls.easy = {info.filename: archive.read(info) for info in archive.infolist()}
        with tarfile.open(cls.output / release.SPEAKER_NAME, "r:gz") as archive:
            cls.tar_modes = {info.name: info.mode for info in archive.getmembers()}
            cls.speaker = {}
            for info in archive.getmembers():
                if not info.isfile() or info.issym() or info.islnk():
                    raise AssertionError("Only regular, allowlisted files belong in the speaker package")
                cls.speaker[info.name] = archive.extractfile(info).read()

    @classmethod
    def tearDownClass(cls):
        cls.temporary.cleanup()

    def test_exact_archive_allowlists(self):
        expected_easy = {"oh2p-xiaozhi/" + name for name in (
            "docker-compose.yml", ".env", "data/config.py", "开始使用.txt", "manifest.json", *release.LICENSE_FILES)}
        expected_speaker = set(release.SPEAKER_FILES) | {"edge/" + name for name in release.LICENSE_FILES}
        expected_speaker |= {"edge/SOURCE.txt", "edge/manifest.json"}
        self.assertEqual(set(self.easy), expected_easy)
        self.assertEqual(set(self.speaker), expected_speaker)
        for name in [*self.easy, *self.speaker]:
            self.assertNotIn("..", Path(name).parts)
            self.assertFalse(name.startswith("/"))
            self.assertFalse(any(part in {".git", "source", "sources", "logs", "private", "privates"} for part in Path(name).parts))

    def test_compose_uses_loaded_version_and_preserves_runtime_contract(self):
        text = self.easy["oh2p-xiaozhi/docker-compose.yml"].decode()
        self.assertIn(f"image: oh2p-xiaozhi-aec:{release.DEFAULT_VERSION}", text)
        self.assertIn("pull_policy: never", text)
        self.assertNotIn("build:", text)
        for field in ("OH2P_REALTIME", "OH2P_AEC_READY", "OH2P_CANCEL_READY", "OH2P_QUIET_CONNECT"):
            self.assertIn(f"{field}: ${{{field}:-1}}", text)
        for required in ("source: ./data/config.py", "target: /app/config.py", "create_host_path: false", "cpus: 2", "mem_limit: 1g", 'max-size: "5m"', 'max-file: "2"'):
            self.assertIn(required, text)
        self.assertIn("docker load -i ../oh2p-xiaozhi-nas-amd64.tar.gz", self.easy["oh2p-xiaozhi/开始使用.txt"].decode())

    def test_config_has_blank_identity_without_executing_it(self):
        config = self.easy["oh2p-xiaozhi/data/config.py"]
        release.validate_config(config)
        with self.assertRaises(ValueError):
            release.validate_config(config.replace(b'"DEVICE_ID": ""', b'"DEVICE_ID": "synthetic-existing-device"'))
        ast.parse(config.decode())

    def test_every_asset_manifest_and_prebuilt_checksum_matches(self):
        for entries, name, prefix in (
            (self.easy, "oh2p-xiaozhi/manifest.json", "oh2p-xiaozhi/"),
            (self.speaker, "edge/manifest.json", ""),
        ):
            metadata = json.loads(entries[name])
            self.assertEqual(metadata["version"], release.DEFAULT_VERSION)
            self.assertEqual(set(metadata["files"]), {entry[len(prefix):] for entry in entries if entry != name})
            for relative, expected in metadata["files"].items():
                self.assertEqual(hashlib.sha256(entries[prefix + relative]).hexdigest(), expected)
        for line in self.speaker["edge/prebuilt/SHA256SUMS"].decode().splitlines():
            digest, name = line.split(None, 1)
            self.assertEqual(hashlib.sha256(self.speaker["edge/prebuilt/" + name]).hexdigest(), digest)
        for line in (self.output / "SHA256SUMS").read_text().splitlines():
            digest, name = line.split(None, 1)
            self.assertEqual(hashlib.sha256((self.output / name).read_bytes()).hexdigest(), digest)

    def test_no_private_addresses_or_credential_patterns(self):
        for name, content in {**self.easy, **self.speaker}.items():
            self.assertIsNone(release.PRIVATE_ADDRESS.search(content), name)
            self.assertIsNone(release.CREDENTIAL.search(content), name)

    def test_speaker_scripts_are_lf_and_executable(self):
        for name, content in self.speaker.items():
            if name.endswith((".sh", ".awk")):
                self.assertNotIn(b"\r\n", content)
            if name.endswith(".sh") or name in {"edge/prebuilt/" + item for item in release.PREBUILT}:
                self.assertEqual(self.tar_modes[name], 0o755)

    def test_deterministic_archives_and_optional_docker_image_checksum(self):
        before = {name: (self.output / name).read_bytes() for name in (release.EASY_NAME, release.SPEAKER_NAME)}
        (self.output / release.IMAGE_NAME).write_bytes(b"synthetic-image-artifact")
        release.package(ROOT, self.output, release.DEFAULT_VERSION)
        for name, content in before.items():
            self.assertEqual((self.output / name).read_bytes(), content)
        sums = (self.output / "SHA256SUMS").read_text()
        self.assertIn(hashlib.sha256(b"synthetic-image-artifact").hexdigest() + "  " + release.IMAGE_NAME, sums)
        (self.output / release.IMAGE_NAME).unlink()
        release.package(ROOT, self.output, release.DEFAULT_VERSION)

    def test_rejects_unsafe_version(self):
        with self.assertRaises(ValueError):
            release.package(ROOT, self.output, "../../unsafe")


if __name__ == "__main__":
    unittest.main()
