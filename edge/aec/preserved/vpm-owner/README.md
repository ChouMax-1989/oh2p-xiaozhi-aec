# OH2P VPM Owner

这里保留当前运行版本的完整 Rust 源码，包含连续增强音频、FVSH1、恢复保护以及默认关闭的 render/native trial/acoustic probe 模块。默认运行参数为 `--wake-mode sherpa`，它在这里选择持续 VPM status2 音频流；不会运行 Sherpa 模型。

构建使用仓库 `edge/build.sh`，目标 `armv7-unknown-linux-gnueabihf.2.25`，默认 feature，不依赖第三方 Rust crate。不要使用历史项目的联合构建说明。Rust 1.93.0、Zig 0.16.0 是已有构建记录中的版本，公开 Docker 入口仍需在用户构建环境中验证。

owner 通过绝对 `/usr/bin/arecord` 读取 `noop`（48kHz、4ch、S32_LE，8ms帧），调用音箱已有 `/usr/lib/libvpm.so`；回调 type2 的 16kHz mono S16_LE 经私有 socket 交给 reader。库文件不随本仓库分发。

`stream_health.rs` 仅在 Sherpa 模式、原始采集仍有输入、无显式 capture、有效增强回调计数连续5秒不增加时请求 status2；仍未恢复则最多每30秒重试。它不靠音量判断静音、不转用 raw，也不证明闭源 VPM 断流内因已经解决。

同一设备仅运行一个定制采集 owner。保留固件原生进程，通过 ALSA 隔离其输入到 hw:0,0；本 owner 的 noop/Capture 指向 hw:0,3。音频仅使用内存与匿名管道。

ABI 派生自 [xiaoai-agent 固定提交](https://github.com/stevenjoezhang/xiaoai-agent/tree/9fc56179fbdde610e95755a843615669dece0d08)。许可证为 LGPL-3.0-or-later，见仓库 `LICENSES/`。源码的保留起点为 [既有公开快照](https://github.com/ChouMax-1989/oh2p-family-voice-firmware-source/tree/2ab6b4e2c3226389877fc3d0f50ebb0756ea3fda)，本目录还包含之后的 stream_health 更新；以本仓库文件为准。
