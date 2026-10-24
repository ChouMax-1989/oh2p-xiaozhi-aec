# 安装与设置

完整部署分两部分：音箱上的 AEC/client，以及 NAS 上的 Docker 服务。两边要配套，默认保留 AEC、实时对话、插话、提示音和 5 秒增强断流恢复。

以下以 Linux 命令为例。`NAS_HOST` 和 `SPEAKER_HOST` 要换成自己的主机地址；它们不是固定值。

## 1. 准备环境

音箱应为 OH2P，固件 1.62.2，已能以 root 通过 SSH 登录，并已有 `/data/init.sh` 自启入口。安装程序会核对设备的 `libvpm.so`，不刷写固件，不提供该专有库。

Docker 主机需要 Docker Compose 和 Python 3。默认服务镜像目标为 `linux/amd64`；其他主机架构需要额外验证，不能把能运行 Docker 等同于本包兼容。音箱需要能连到 Docker 主机的服务端口。

先下载本仓库，在仓库根目录执行后续宿主机命令：

```sh
git clone https://github.com/ChouMax-1989/oh2p-xiaozhi-aec.git
cd oh2p-xiaozhi-aec
python3 server/init_config.py
cp .env.example .env
```

Windows 可用 `py -3 server/init_config.py` 初始化，再复制 `.env.example` 为 `.env`。初始化脚本不会覆盖已经存在的 `data/config.py`。

检查 `.env`：

```dotenv
SERVER_BIND_ADDRESS=0.0.0.0
SERVER_PORT=4399
OH2P_REALTIME=1
OH2P_AEC_READY=1
OH2P_CANCEL_READY=1
OH2P_QUIET_CONNECT=1
```

`SERVER_PORT` 是给音箱连接的端口；音箱安装时填写同一端口。`SERVER_BIND_ADDRESS` 可设为 Docker 主机实际的局域网地址，默认监听各接口。该音箱连接应只用于可信局域网，不直接暴露到公网。

三个实时开关默认都是 `1`，代表使用配套 AEC 和支持播放取消的客户端。它们是功能门禁，不是自动检测报告；设备实际状态仍要按验收清单确认。`OH2P_QUIET_CONNECT=1` 取消客户端连接时的额外提示音，保留会话开始与结束音。

先完成镜像构建，暂不启动服务：

```sh
docker compose build
```

构建会获取锁定的上游版本并加入本仓库的实时适配和 RPC 修改，不依赖个人部署镜像。首次构建需要网络及编译时间。

## 2. 安装音箱端

仓库的 `edge/prebuilt/` 提供配套 client、owner 和 reader。安装器会验证随附校验和。安装器发现已有本项目进程会退出，应先按原项目的方式停止；启动时还会检查 PCM3 是否空闲，避免第二个自定义采音 owner。不要手动杀掉音箱原生语音进程。

把整个 `edge` 目录上传到音箱：

```sh
ssh root@SPEAKER_HOST 'mkdir -p /data/oh2p-xiaozhi-package'
scp -r edge root@SPEAKER_HOST:/data/oh2p-xiaozhi-package/
ssh root@SPEAKER_HOST
```

以下命令在音箱 SSH 中执行。先用默认检查模式安装：

```sh
sh /data/oh2p-xiaozhi-package/edge/install.sh 'ws://NAS_HOST:4399'
```

安装器默认检查 `/etc/asound.conf` 中已知的 OH2P 1.62.2 布局：`mico_record` 使用 `hw:0,0`，`noop` 经 `Capture`/dsnoop 读取 `hw:0,3`，原始采音为 48 kHz、4 声道、S32_LE。

若结构符合已知布局，仅路由尚未调整，可显式允许安装器修改这两个路由字段：

```sh
sh /data/oh2p-xiaozhi-package/edge/install.sh 'ws://NAS_HOST:4399' --configure-alsa
```

若设备实际使用其他配置文件，可加 `--alsa-config /实际配置路径`。未知布局会被拒绝；不要为了通过检查覆盖整份 ALSA 配置。

安装完成后会显示备份目录 `/data/oh2p-xiaozhi-backup-时间-进程号`。其中保留旧 `open-xiaoai` 目录、旧 `/data/init.sh` 和原 ALSA 配置。安装会替换该自启入口，若它还承载其他任务，应先核对并整合，不能直接假设可以丢弃旧内容。

安装器不立即启动进程。若 `--configure-alsa` 实际改变了路由，后面需要重启音箱，让原生语音进程按新配置打开声卡；不能直接启动新增 owner 与旧采音争用。安装前路由已正确时，可在服务启动后手动启动边端。

## 3. 启动 Docker 和音箱端

回到 Docker 主机的仓库根目录：

```sh
docker compose up -d
docker compose logs --tail=100 server
```

Compose 默认限制 2 CPU、1 GiB 内存，设置重启策略和日志轮转。持久化文件为：

```text
data/config.py  →  容器 /app/config.py
```

该挂载必须允许写入，上游会把生成的设备身份和配对状态写回文件。不要用样例反复覆盖它，也不要把已有个人配置提交到仓库。

若安装时改变了 ALSA 路由，在音箱 SSH 中执行 `reboot`，等待原有启动钩子自动启动边端。若安装前路由已正确，可直接启动：

```sh
sh /data/init.sh </dev/null >/dev/null 2>&1 &
```

正常启动会依次启动 owner 和 client。监督脚本避免重复实例；任一关键进程退出时，会收拢另一侧，不留下无人消费的采音 owner。

## 4. 绑定自己的小智账号

打开 [小智](https://xiaozhi.me/)，登录自己的账号，创建或选择智能体，进入设备绑定/添加设备入口。

对音箱说“你好小智”，按当次播报的验证码完成绑定；若没有播报，在自己的机器查看服务日志及 `data/config.py` 的 `VERIFICATION_CODE`。验证码由小智返回，会过期；仓库不带任何人的验证码、账号或设备身份。

绑定后如需重新连接，可在 Docker 主机执行：

```sh
docker compose restart server
```

首次安装后请按[验收清单](VERIFICATION.md)确认绑定和实际对话结果。

## 5. 改唤醒词和其他设置

修改自己机器的 `data/config.py`：

```python
"wakeup": {
    "keywords": ["你好小智"],
    # 保留其余配置
}
```

这里只展示要修改的字段，不是完整可替换配置。保存后执行 `docker compose restart server`。公开版默认“你好小智”，每位使用者可自行调整，不包含维护者的个人唤醒词配置。

小智的 OTA、WebSocket 地址和可选访问令牌也在同一文件中。使用默认小智服务时先保留样例设置；只有连接自己的服务时才按该服务要求调整。`DEVICE_ID` 和绑定状态由正常首次连接流程生成，日常重启不要清空。

当前实时适配的静默退出时间为 20 秒。它不是只改 `wakeup.timeout` 就能保证生效的普通配置项；具体边界见[运行原理](ARCHITECTURE.md)。

更改 `.env` 后要重新创建容器以应用环境变量：

```sh
docker compose up -d --force-recreate
```

## 6. 自行构建边端

默认安装使用随包预编译件。若要检查或修改 owner/reader，在有 Docker 的 Linux 主机上运行：

```sh
sh edge/build.sh
```

产物位于 `edge/build/out/`。构建目录不会自动替换 `edge/prebuilt/`；自行替换时需同步维护校验和，并重新做兼容性和真人验收。

可选的 `sh edge/build.sh --client` 会从固定上游源码构建一个带最小 S16 适配的 client 候选。它与原发行二进制的构建产物不同，使用前需另外完成设备兼容性和真人验收。

## 7. 停止与回退

临时停止 Docker 服务：

```sh
docker compose stop server
```

如果要撤销音箱端安装，在音箱上执行：

```sh
sh /data/open-xiaoai/rollback.sh
```

回退脚本会核对安装后文件是否被其他操作改动，停止本项目监督进程并确认子进程退出，然后恢复备份的自启入口、ALSA 配置和旧项目目录。发现文件变化会要求人工处理，避免覆盖后续修改。回退不会自动启动旧服务，也不是刷回原厂固件。

保留 `data/config.py`，便于之后恢复自己的绑定。不要使用其他旧项目的顶层回退脚本管理这一套 AEC 进程。

最后按[验收清单](VERIFICATION.md)逐项确认；遇到异常参考[排错说明](TROUBLESHOOTING.md)。
