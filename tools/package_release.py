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

DEFAULT_VERSION = "2026.10.24.1"
EASY_NAME = "oh2p-xiaozhi-easy-install.zip"
SPEAKER_NAME = "oh2p-xiaozhi-speaker.tar.gz"
IMAGE_NAME = "oh2p-xiaozhi-nas-amd64.tar.gz"
SOURCE_REPOSITORY = "https://github.com/ChouMax-1989/oh2p-xiaozhi-aec"
PREBUILT = ("arecord", "client", "vpm-owner")
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

适用：已有 SSH 的 OH2P 1.62.2 音箱，以及支持 Linux amd64 镜像的 NAS Docker。
这是现有固件的附加软件包，不是整机固件，不刷写固件。使用者无需安装 Python 或编译源码。

1. 从同一 Release 下载本 ZIP、oh2p-xiaozhi-nas-amd64.tar.gz、
   oh2p-xiaozhi-speaker.tar.gz 和 SHA256SUMS，核对下载文件 SHA256。
2. 解压本 ZIP。把 NAS 镜像放在解压得到的 oh2p-xiaozhi 文件夹旁边。
   进入 oh2p-xiaozhi 目录，在 NAS 终端执行：
     docker load -i ../oh2p-xiaozhi-nas-amd64.tar.gz
3. 音箱端把 oh2p-xiaozhi-speaker.tar.gz 上传到音箱可写目录，解压后进入 edge：
     tar -xzf oh2p-xiaozhi-speaker.tar.gz
     cd edge
     sh install.sh ws://NAS_HOST:4399 --configure-alsa
   把 NAS_HOST 换成你自己的 NAS 局域网地址。安装需要 root，校验固件库和现有启动入口。
   安装脚本会备份现有配置、保留回退脚本，且不会启动进程。
   如果安装提示 ALSA 已改变，先重启音箱；否则按安装输出的启动命令操作。
4. NAS 在 oh2p-xiaozhi 目录执行：
     docker compose up -d
     docker compose logs -f server
5. 默认唤醒词“你好小智”。首次唤醒按上游提示，用你自己的小智账号完成验证码绑定。
   data/config.py 中设备标识、验证码和密钥初始为空，启动后由上游生成并持久化。
   已绑定后保留 data/config.py，不要用空模板覆盖它，也不要公开该文件。

环境参数位于 .env，默认启用完整实时连续对话和 AEC/播放取消路径。
改 .env 后执行 docker compose up -d --force-recreate。
音箱回退命令：sh /data/open-xiaoai/rollback.sh；脚本恢复备份，不自动启动旧进程。
完整安装、兼容条件、已验证范围与源码：
{SOURCE_REPOSITORY}/tree/v{version}
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
