# NAS 服务端源码与重建

这里保留当前实时服务的 `realtime.py`、`overlay.py`、设备提示音与 RPC 修改。Docker 从干净公开源码构建，不依赖作者 NAS 镜像、私有 builder、已部署容器层或个人配置。目标是 Linux amd64；默认环境与完整实时方案一致，三个门禁均为 `1`，须先安装配套边端 AEC/reader 并确认播放器 stop/start 可用。

## 部署契约

在仓库根目录执行：

```sh
python3 server/init_config.py
cp .env.example .env
docker compose build
docker compose up -d
docker compose logs -f server
```

Windows 的初始化命令为 `py -3 server/init_config.py`。也可用 `sh server/build.sh` 单独构建。需要 Docker Engine、Compose v2，以及可访问 GitHub、容器仓库、Debian、PyPI 和 crates.io 的构建网络。首次构建包含 Rust、Python 原生扩展和约 108 MB 模型下载；运行 NAS 只需 Docker，无需本机 Rust/Python。

`server/init_config.py` 支持 `--help`、`--path`；默认创建 `data/config.py`，已存在文件不会覆盖。Compose 绑定该文件到 `/app/config.py`，保持可写，使上游首次运行生成的 `DEVICE_ID` 和首次对话得到的 `VERIFICATION_CODE` 持久化。模板默认“你好小智”，设备身份、验证码和访问密钥均为空。用户按上游正常首次配对流程自行绑定自己的小智账号；没有跳过激活或携带作者绑定。首次新设备的真实云端配对需要在自己的账号中完成。

实时路径使用设备自带 `/usr/share/common_sound/multirounds_tone.opus` 和 `voip_off.opus`，会话就绪与结束各响一次，普通 TTS 切换不重复提示；提示音失败不阻止会话。模板 KWS 回调不额外播放旧的欢迎词，其他上游回调保留。实时 overlay 使用自己的 ready/end 提示音。模型和提示音不是作者录音。

`.env` 中端口默认为 `4399`；在边端设置实际 NAS 地址。修改环境变量后执行 `docker compose up -d --force-recreate`。配置文件包含后续生成的绑定信息，勿提交；不要重置已配对的 `data/config.py`。三门禁缺少任意一项而仍启用实时模式时，程序明确报错；将实时总开关设为 `0` 才显式回到上游路径。默认部署不会退回上游单轮行为。

## 构建输入与改动

- 上游固定为 [open-xiaoai bc3396c64e2a435f354eb5cb12a203981f1fe422](https://github.com/idootop/open-xiaoai/tree/bc3396c64e2a435f354eb5cb12a203981f1fe422)，源码 ZIP SHA256 为 `1d3d38eb3e099faef31bf8ae13929d89c8cdda64e9fa2eade649ca36c667c88c`。
- 上游 [VAD/KWS models.zip](https://github.com/idootop/open-xiaoai/releases/tag/vad-kws-models) SHA256 为 `97272c44184a0f8222726a164f35e0a504177240181753ae13230790c29eb566`。构建时验证后解压，不在仓库重复分发模型二进制。
- 公共工具链为 Python `3.12.12-slim-bookworm`、Rust `1.93.0-bookworm`、uv `0.7.22`。保留上游 `Cargo.lock`、`uv.lock`，分别使用 `--locked`、`--frozen`。Debian 软件包与镜像 tag 仍依赖上游仓库，故这是固定应用源码的可重建配方，不保证逐字节相同镜像。
- `prepare_source.py CHECKOUT` 对四个待改文件先核对原始 SHA256，再接线；只接受干净 checkout，再次执行会拒绝已修改源码。`patches/realtime.patch` 是完整变更的可读差异，构建实际运行 `patches/patch_upstream.py` 加准备脚本；无需再手动应用 patch。
- Rust 增加 `playback_control(stop|start)`，使用上游设备 stop_play/start_play RPC，检查错误；音频发送失败向上传播。RPC 先登记 pending 再发送，共用一次发送/收包 deadline，发送失败或超时清理 pending；附带 3 项回归测试。静默连接开关仅跳过“已连接”播报，保留原输入/播放器初始化。
- Python 增加连续实时上行、有界上下行队列、session 检查、设备播放取消、断线重连/EOF 清理、连续语音插话及空闲退出；复用已有 Silero/AEC 输入，语音阈值 `0.5`、连续约 `250 ms` 才打断，不增加新音频 owner 或落盘原始语音。正常 TTS stop 允许设备队列排空。

与运行源码的对应：`app/realtime.py` SHA256 为 `2e5a7d53537e91da32017e2ae1bdc6b0309a7ed7db184462803947f9cc576219`，`app/overlay.py` 为 `ee9dcd95e3ec0746e0daf8f170accc05950a8d4edb21e599f946f9ae0a5d7dd3`。接线后的 `xiaozhi.py` 为 `542cbe03f877c8dd1e08b7b59057c877c320ab1d362033055dd7bedd22859801`，已与运行文件只读校验一致。完整准备结果列在 `tests/prepared-source-sha256.json`；新环境构建的 `.so` 不承诺与既有构建二进制 hash 相同。

## 测试和已知限制

本地离线检查：12 项 `test_realtime` 和 3 项 `test_release` 通过；对下载的干净固定源码执行准备脚本成功，得到上述与运行文件一致的接线 hash。模板为空身份/默认唤醒词、初始化不覆盖、发布 Python 语法均通过检查。

```powershell
$env:PYTHONPATH = (Resolve-Path server/app).Path
py -3 -B -m unittest discover -s server/tests -p test_realtime.py -v
py -3 -B -m unittest discover -s server/tests -p test_release.py -v
```

完整容器测试入口：

```sh
docker build --target test -t oh2p-xiaozhi-tests ./server
docker run --rm --network none oh2p-xiaozhi-tests
```

构建会执行 Rust 的 3 项 pending-order 回归；测试容器含原有状态、真实上游协议与 VAD/app 接线、真实编译 Rust 扩展的合成 loopback 测试，以及本次发布边界测试。其 IO 使用合成数据，不连接小智云端或真实音箱，不下载运行模型，不进行真人发声测试。

2026-10-24 已在 GitHub 的干净 Ubuntu 环境完成公开 Dockerfile 构建、依赖导入和全部 30 项离线集成测试，Rust RPC 回归 3 项通过。[查看构建与测试记录](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/actions/runs/40101484884)。测试没有连接真实音箱或小智账号，首次云端配对仍需使用者完成。协议没有回复 turn id，同 session 内迟到旧响应无法宣称完整隔离；实际 AEC 判别、自然插话和长期稳定性也不能由合成测试替代。
