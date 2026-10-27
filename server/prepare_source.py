"""Patch only an exact, disposable upstream checkout; never launch the app."""
import argparse
import hashlib
from pathlib import Path
import shutil
import subprocess
import sys

REVISION = "bc3396c64e2a435f354eb5cb12a203981f1fe422"
EXPECTED = {
    "examples/xiaozhi/xiaozhi/xiaozhi.py": "60cf9de227d280d255faa43c7afbd8a7ad496d2382eb3e0683355dfd7d4f720f",
    "examples/xiaozhi/src/lib.rs": "475e3fc9ca791fdd4c9a2a542c9018e216cea3c3f662aca648c3a23206351844",
    "examples/xiaozhi/src/server.rs": "0bb6d718023eedb36b6efc099bd8fc498bbfe917bbd2b94ab0edb6274dd64afe",
    "packages/client-rust/src/services/connect/rpc.rs": "588f3767167a9558ab446350fb2ed14a40d732d22d769b60c6d3522e1d79c5cf",
}


def prepare(checkout):
    base = Path(__file__).resolve().parent
    checkout = Path(checkout).resolve()
    for relative, expected in EXPECTED.items():
        if hashlib.sha256((checkout / relative).read_bytes()).hexdigest() != expected:
            raise RuntimeError(f"source mismatch (requires clean {REVISION}): {relative}")
    subprocess.run([sys.executable, str(base / "patches/patch_upstream.py"), str(checkout)], check=True)
    app = checkout / "examples/xiaozhi/xiaozhi"
    path = app / "xiaozhi.py"
    text = path.read_text(encoding="utf-8")
    anchor = "        self._initialize_audio()\n\n        # 设置协议回调"
    if text.count(anchor) != 1:
        raise RuntimeError("application hook mismatch")
    text = text.replace(anchor,
        "        self._initialize_audio()\n        from xiaozhi.realtime_overlay import install\n"
        "        self.realtime_session = install(self)\n\n        # 设置协议回调", 1)
    path.write_text(text, encoding="utf-8", newline="\n")
    shutil.copyfile(base / "app/realtime.py", app / "realtime.py")
    shutil.copyfile(base / "app/overlay.py", app / "realtime_overlay.py")
    protocol = app / "services/protocols/websocket_protocol.py"
    text = protocol.read_text(encoding="utf-8")
    stages = [
        ('import json', 'import json\nimport time'),
        ('            await self._close_websocket()',
         '            stamp = time.monotonic()\n            await self._close_websocket()\n'
         '            print(f"OH2P connect close_ms={(time.monotonic()-stamp)*1000:.1f}", flush=True)'),
        ('            self.websocket = await websockets.connect(',
         '            stamp = time.monotonic()\n            self.websocket = await websockets.connect('),
        ('            # 启动消息处理循环',
         '            print(f"OH2P connect websocket_ms={(time.monotonic()-stamp)*1000:.1f}", flush=True)\n'
         '            stamp = time.monotonic()\n            # 启动消息处理循环'),
        ('                self.connected = True',
         '                print(f"OH2P connect hello_ms={(time.monotonic()-stamp)*1000:.1f}", flush=True)\n'
         '                self.connected = True'),
    ]
    for anchor, replacement in stages:
        if text.count(anchor) != 1:
            raise RuntimeError("connection timing hook mismatch")
        text = text.replace(anchor, replacement, 1)
    compile(text, str(protocol), "exec")
    protocol.write_text(text, encoding="utf-8", newline="\n")
    rpc = checkout / "packages/client-rust/src/services/connect/rpc.rs"
    with rpc.open("a", encoding="utf-8", newline="\n") as stream:
        stream.write((base / "patches/rpc_tests.rs").read_text(encoding="utf-8"))
    server = checkout / "examples/xiaozhi/src/server.rs"
    text = server.read_text(encoding="utf-8")
    anchor = '    SpeakerManager::play_text("已连接").await?;'
    if text.count(anchor) != 1:
        raise RuntimeError("quiet-connect hook mismatch")
    server.write_text(text.replace(anchor,
        '    if std::env::var("OH2P_QUIET_CONNECT").as_deref() != Ok("1") {\n'
        + anchor + '\n    }', 1), encoding="utf-8", newline="\n")
    print(f"Prepared {REVISION}: realtime, cues, playback RPC, pending ordering and deadline")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("checkout", type=Path)
    prepare(parser.parse_args().checkout)
