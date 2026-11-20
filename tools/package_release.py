"""Build end-user download archives from an explicit public-file allowlist.

The Docker image is produced separately. If it is already present in the output
directory, SHA256SUMS includes it. No network, device access or application import.
"""
import argparse
import ast
from datetime import datetime, timezone
import gzip
import hashlib
import io
import json
from pathlib import Path
import re
import tarfile
import zipfile

DEFAULT_VERSION = "2026.11.20.1"
EASY_NAME = "oh2p-xiaozhi-easy-install.zip"
SPEAKER_NAME = "oh2p-xiaozhi-speaker.tar.gz"
IMAGE_NAME = "oh2p-xiaozhi-nas-amd64.tar.gz"
SOURCE_REPOSITORY = "https://github.com/ChouMax-1989/oh2p-xiaozhi-aec"
PREBUILT = ("aplay", "arecord", "client", "vpm-owner")
SPEAKER_FILES = (
    "edge/boot-aec.sh", "edge/install.sh", "edge/rollback.sh",
    "edge/tools/alsa-routes.awk", "edge/prebuilt/SHA256SUMS",
    *("edge/prebuilt/" + name for name in PREBUILT),
)
LICENSE_FILES = (
    "LICENSE", "NOTICE.md", "LICENSES/GPL-3.0.txt",
    "LICENSES/LGPL-3.0.txt", "LICENSES/open-xiaoai-MIT.txt",
)
PRIVATE_ADDRESS = re.compile(rb"(?<![\d.])(?:10(?:\.\d{1,3}){3}|192\.168(?:\.\d{1,3}){2}|172\.(?:1[6-9]|2\d|3[01])(?:\.\d{1,3}){2})(?![\d.])")
CREDENTIAL = re.compile(rb"-----BEGIN (?:OPENSSH |RSA |EC )?PRIVATE KEY-----|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9]{24,}")


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def file_sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def read_public(root, relative):
    path = root / relative
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"Release input is missing or symlinked: {relative}")
    data = path.read_bytes()
    if relative not in {"edge/prebuilt/" + name for name in PREBUILT}:
        data = data.decode("utf-8").replace("\r\n", "\n").encode("utf-8")
    if PRIVATE_ADDRESS.search(data) or CREDENTIAL.search(data):
        raise ValueError(f"Private address or credential pattern in release input: {relative}")
    return data


def validate_config(data):
    module = ast.parse(data.decode("utf-8"))
    assignment = next(node for node in module.body if isinstance(node, ast.Assign)
        and any(isinstance(target, ast.Name) and target.id == "APP_CONFIG" for target in node.targets))
    sections = {ast.literal_eval(key): value for key, value in zip(assignment.value.keys, assignment.value.values)}
    identity = {ast.literal_eval(key): value for key, value in zip(sections["xiaozhi"].keys, sections["xiaozhi"].values)}
    for field in ("DEVICE_ID", "VERIFICATION_CODE", "WEBSOCKET_ACCESS_TOKEN"):
        if ast.literal_eval(identity[field]) != "":
            raise ValueError(f"Release pairing identity must be empty: {field}")
    wakeup = {ast.literal_eval(key): value for key, value in zip(sections["wakeup"].keys, sections["wakeup"].values)}
    if ast.literal_eval(wakeup["keywords"]) != ["你好小智"]:
        raise ValueError("Release default wakeup must be 你好小智")


def validate_prebuilt(root):
    listed = {}
    for line in read_public(root, "edge/prebuilt/SHA256SUMS").decode().splitlines():
        digest, name = line.split(None, 1)
        name = name.lstrip("*")
        if name in listed or name not in PREBUILT or not re.fullmatch(r"[a-f0-9]{64}", digest):
            raise ValueError("Unexpected prebuilt checksum entry")
        listed[name] = digest
    if set(listed) != set(PREBUILT):
        raise ValueError("Incomplete prebuilt checksum manifest")
    for name, expected in listed.items():
        if sha256(read_public(root, "edge/prebuilt/" + name)) != expected:
            raise ValueError(f"Prebuilt checksum mismatch: {name}")


def compose_text(version):
    return f'''services:
  server:
    image: oh2p-xiaozhi-aec:{version}
    pull_policy: never
    platform: linux/amd64
    restart: unless-stopped
    ports:
      - "${{SERVER_BIND_ADDRESS:-0.0.0.0}}:${{SERVER_PORT:-4399}}:4399"
    volumes:
      - type: bind
        source: ./data/config.py
        target: /app/config.py
        bind:
          create_host_path: false
    environment:
      OH2P_REALTIME: ${{OH2P_REALTIME:-1}}
      OH2P_AEC_READY: ${{OH2P_AEC_READY:-1}}
      OH2P_CANCEL_READY: ${{OH2P_CANCEL_READY:-1}}
      OH2P_QUIET_CONNECT: ${{OH2P_QUIET_CONNECT:-1}}
    cpus: 2
    mem_limit: 1g
    logging:
      driver: json-file
      options:
        max-size: "5m"
        max-file: "2"
'''


def instructions(version):
    return f'''OH2P 小智 AEC 下载包 {version}

适用于已经能通过 SSH 登录的 OH2P 1.62.2 音箱，以及装有 Docker 和 Docker Compose 的 Intel/AMD 64 位 NAS。按教程安装和升级都无需编译、安装 Python 或刷机。

安装、绑定、升级和刷机都以仓库首页为准：
{SOURCE_REPOSITORY}/tree/v{version}#readme

第一次安装

从同一个 GitHub 发行页下载本 ZIP、NAS 镜像、音箱安装包和 SHA256SUMS。按首页核对下载文件。
把 NAS 镜像放进解压得到的 oh2p-xiaozhi 文件夹，在 NAS 上进入该文件夹并导入镜像：
  docker load -i oh2p-xiaozhi-nas-amd64.tar.gz

按首页把音箱包上传并解压到音箱，进入 edge 文件夹，用 root 安装。把 NAS_HOST 换成自己的 NAS 局域网地址：
  sh install.sh ws://NAS_HOST:4399 --configure-alsa

回到 NAS，在 oh2p-xiaozhi 文件夹执行 docker compose up -d，再重启音箱。
默认唤醒词是“你好小智”。按音箱提示，用自己的小智账号完成验证码绑定。
首次启动时 data/config.py 里的设备身份、验证码和令牌为空。绑定后配置会写回该文件，请妥善保存。

从旧版升级

本版要同时更新 NAS 镜像和音箱程序。按仓库首页导入新版镜像，在原 docker-compose.yml 中把 server 的 image 改为
oh2p-xiaozhi-aec:{version}，在原目录执行 docker compose up -d --no-deps server。
继续使用原来的 data/config.py、.env、端口和其他设置。

在音箱将新版压缩包解压到新目录，进入其中的 edge 文件夹并执行：
  sh install.sh --upgrade
成功后重启音箱。安装器保留原 NAS 地址和 ALSA 声音配置。
需要恢复升级前的音箱文件时，运行 sh /data/open-xiaoai/rollback.sh，再按首页指引重启。
'''.encode("utf-8")


def manifest(version, package, entries):
    return (json.dumps({
        "version": version, "package": package,
        "source": f"{SOURCE_REPOSITORY}/tree/v{version}",
        "image": f"oh2p-xiaozhi-aec:{version}",
        "files": {name: sha256(content) for name, content in sorted(entries.items())},
    }, ensure_ascii=False, indent=2) + "\n").encode("utf-8")


def write_zip(path, entries, release_date):
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, content in sorted(entries.items()):
            info = zipfile.ZipInfo(name, release_date.timetuple()[:6])
            info.create_system = 3
            info.external_attr = 0o100644 << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, content, compresslevel=9)


def write_tar(path, entries):
    with path.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0, compresslevel=9) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
                for name, content in sorted(entries.items()):
                    info = tarfile.TarInfo(name)
                    info.size = len(content)
                    info.mode = 0o755 if name.endswith(".sh") or name in {"edge/prebuilt/" + item for item in PREBUILT} else 0o644
                    info.mtime = 0
                    archive.addfile(info, io.BytesIO(content))


def package(root, output, version):
    if not re.fullmatch(r"\d{4}\.\d{2}\.\d{2}\.\d+", version):
        raise ValueError("Version must use YYYY.MM.DD.N")
    release_date = datetime.strptime(version.rsplit(".", 1)[0], "%Y.%m.%d").replace(tzinfo=timezone.utc)
    root, output = Path(root).resolve(), Path(output).resolve()
    validate_prebuilt(root)
    config = read_public(root, "server/config.example.py")
    validate_config(config)
    licenses = {name: read_public(root, name) for name in LICENSE_FILES}
    easy = {
        "docker-compose.yml": compose_text(version).encode(),
        ".env": read_public(root, ".env.example"),
        "data/config.py": config,
        "开始使用.txt": instructions(version),
        **licenses,
    }
    easy["manifest.json"] = manifest(version, "nas-easy-install", easy)
    easy = {"oh2p-xiaozhi/" + name: content for name, content in easy.items()}
    speaker = {name: read_public(root, name) for name in SPEAKER_FILES}
    speaker.update({"edge/" + name: content for name, content in licenses.items()})
    speaker["edge/SOURCE.txt"] = (
        f"Corresponding source and build instructions for this binary release:\n"
        f"{SOURCE_REPOSITORY}/tree/v{version}\n"
        "See edge/BUILD.md in that source tree and the included NOTICE.md and LICENSES.\n"
    ).encode()
    speaker["edge/manifest.json"] = manifest(version, "speaker-addon-oh2p-1.62.2", speaker)
    output.mkdir(parents=True, exist_ok=True)
    write_zip(output / EASY_NAME, easy, release_date)
    write_tar(output / SPEAKER_NAME, speaker)
    assets = [EASY_NAME, SPEAKER_NAME]
    if (output / IMAGE_NAME).is_file():
        assets.append(IMAGE_NAME)
    checksums = "".join(f"{file_sha256(output / name)}  {name}\n" for name in sorted(assets))
    (output / "SHA256SUMS").write_text(checksums, encoding="utf-8", newline="\n")
    return {name: output / name for name in [*assets, "SHA256SUMS"]}


def main():
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", default=DEFAULT_VERSION)
    parser.add_argument("--output", type=Path, default=root / "dist")
    args = parser.parse_args()
    for name, path in package(root, args.output, args.version).items():
        print(f"Created {name}: {path.stat().st_size} bytes")


if __name__ == "__main__":
    main()
