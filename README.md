# 让 OH2P 小爱音箱用上小智

对音箱说 **“你好小智”**，就能开始聊天。支持连续问答、播报时插话，以及开始和结束提示音。音箱使用原有的回声消除，增强音频中断时会自动尝试恢复。

**直接下载现成安装包即可，不需要编译代码，也不需要安装 Python。** 每个人安装后绑定自己的小智账号。

## 下载

当前版本：[v2026.10.24.1 下载页](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/tag/v2026.10.24.1)。普通安装需要前三个文件：

| 文件 | 用在哪里 |
| --- | --- |
| [NAS 安装包：oh2p-xiaozhi-easy-install.zip](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/download/v2026.10.24.1/oh2p-xiaozhi-easy-install.zip) | 解压到 NAS，里面已经放好启动文件和空白账号配置 |
| [Docker 镜像：oh2p-xiaozhi-nas-amd64.tar.gz](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/download/v2026.10.24.1/oh2p-xiaozhi-nas-amd64.tar.gz) | 导入 NAS 的 Docker，适用于 Intel/AMD 64 位主机 |
| [音箱安装包：oh2p-xiaozhi-speaker.tar.gz](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/download/v2026.10.24.1/oh2p-xiaozhi-speaker.tar.gz) | 上传到已经开启 SSH 的 OH2P 音箱 |
| [OH2P 1.62.2 现机脱敏固件](docs/LIVE-FIRMWARE.md) | 需要含 SSH 的系统准备文件时优先查看；公开重打包尚未刷机验收 |

**已经是 OH2P 1.62.2、能用 SSH 登录的音箱，不用重新刷机。** 音箱安装包是附加程序；官方原厂固件本身不等于已经开启 SSH。

现机脱敏固件保留实际使用的声音和启动配置，排除账号、Wi-Fi 与设备专属数据；来源镜像使用正常，不代表公开包已在新机刷写通过。原厂包和旧候选见[固件资料](docs/FIRMWARE.md)。

## 安装前准备

- 一台小米小爱音箱 Pro，型号 **OH2P**，固件 **1.62.2**，可以用 SSH 登录。
- 一台 Intel/AMD 64 位 NAS 或 Linux 主机，已经安装 Docker 和 Docker Compose。
- NAS 和音箱在同一局域网，能正常上网。
- 自己的小智账号。

按[小白安装步骤](docs/DEPLOYMENT.md)操作：下载 → 导入 NAS 镜像 → 安装音箱 → 启动 → 绑定账号。每一步都写明了在哪个设备执行、正常会看到什么。

## 装好以后

1. 说“你好小智”，听到开始提示音后提问。
2. 可以接着问下一句，也可以在音箱说话时插话。
3. 保持安静约 20 秒，会话结束并播放结束提示音；下次再说唤醒词即可。

配置默认是“你好小智”，没有附带任何人的账号、设备绑定或验证码。第一次使用时，按音箱提示绑定自己的小智账号。

## 使用情况

同源 AEC 路线曾由用户验证过连续对话和打断，增强音频修复后也确认可以唤醒。当前版本的长期稳定性、全新设备安装、首次绑定和整机重启等仍需按[验收清单](docs/VERIFICATION.md)确认。

遇到问题看[排错说明](docs/TROUBLESHOOTING.md)。需要了解实现或自行修改代码，再看[运行原理](docs/ARCHITECTURE.md)、[服务端源码说明](server/README.md)和[音箱端构建说明](edge/BUILD.md)。

项目保留了实际使用的 AEC、连续采音、实时对话、插话、提示音，以及 5 秒增强断流恢复修改。基于 [open-xiaoai](https://github.com/idootop/open-xiaoai) 和 [xiaoai-agent](https://github.com/stevenjoezhang/xiaoai-agent) 的相关实现，许可证见 [NOTICE.md](NOTICE.md)；设备固件中的专有库不随附加安装包分发。
