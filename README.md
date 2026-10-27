# 让 OH2P 小爱音箱用上小智

对音箱说 **“你好小智”**，就能开始聊天。支持连续问答、播报时插话，以及开始和结束提示音。音箱使用原有的回声消除，增强音频中断时会自动尝试恢复。

**直接下载现成安装包即可，不需要编译代码，也不需要安装 Python。** 每个人安装后绑定自己的小智账号。

## 这套版本主要改进了什么？

这套程序基于 [Open-XiaoAI 的小智示例](https://github.com/idootop/open-xiaoai/tree/main/examples/xiaozhi) 修改。接入小智、账号绑定、自定义唤醒词和连续对话都是上游已有的能力。我们主要针对 **OH2P 的采音、插话和日常使用**做了这些改进：

- **它在说，你也能直接插话。** 回答期间继续接收你的声音，检测到你开口后，停止当前播放并通知小智中止回答，不必专门再喊一次“小爱同学”。同时清掉音箱里还没播出的音频，避免打断后旧回答继续往外播。
- **把 OH2P 原有的回声消除接给小智。** 使用音箱原厂处理后的增强音频，让小智在音箱播放时也能接收你的讲话。AEC 是音箱原有的能力，本项目做的是接入和配套控制，不是重新发明一套回声消除算法。
- **补上了一种“程序还在，采音却停了”的恢复处理。** 原始音频仍在到达、但增强音频连续约 5 秒没有输出时，程序会尝试重新开启增强输出；未恢复时隔至少 30 秒再试。这针对的是已遇到的一类断流问题，不代表所有故障都能自动恢复。
- **开始和结束都有提示音。** 听到开始提示音就可以提问；结束提示音提醒你，这轮聊天已结束，下次需要重新唤醒。
- **减少久放后唤醒等待，避免重复启动。** 空闲时发现云端连接已断开，会尝试重新连接；连接失败隔 30 秒再试。正在唤醒或聊天时重复喊唤醒词，不会再启动一轮。连接尚未完成就结束，也不会稍后突然开始聊天。
- **准备好了能直接安装的包。** 音箱程序、NAS Docker 镜像和启动文件分别提供下载，照教程安装后绑定自己的小智账号，不需要从源码编译。

上游本来就支持打断；它的小智示例说明的是喊“小爱同学”打断。这里重点补的是播放期间持续采音、普通讲话触发打断，以及 OH2P 增强音频的接入。我们没有做同条件的音质、延迟和长期稳定性对比，不宣称全面优于上游。

**需要一台同一局域网内、持续开机的 NAS 或 Docker 主机。** 音箱负责采音和播放，NAS 负责唤醒检测、插话判断及连接小智云端。这不是脱离网络运行的本地大模型，也不是只装音箱就能使用的版本。

## 目录

- [下载](#下载)
- [安装和绑定](#安装和绑定)
- [升级](#升级)
- [需刷机才看](#需刷机才看)

## 下载

打开 [最新发行版下载页](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/latest)，下载：

1. [oh2p-xiaozhi-easy-install.zip](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/latest/download/oh2p-xiaozhi-easy-install.zip)：NAS 启动文件。
2. [oh2p-xiaozhi-nas-amd64.tar.gz](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/latest/download/oh2p-xiaozhi-nas-amd64.tar.gz)：Docker 镜像，**不用解压**。
3. [oh2p-xiaozhi-speaker.tar.gz](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/latest/download/oh2p-xiaozhi-speaker.tar.gz)：音箱程序，先留在电脑上，**不用解压**。

下载页里自动生成的 `Source code` 是源码，普通安装不用下载。`SHA256SUMS` 是下载完整性校验文件，不是安装程序；不需要把它导入 Docker 或上传音箱。



## 安装和绑定

适用于 **OH2P 音箱，固件 1.62.2，已经能通过 SSH 登录；NAS 为 Intel/AMD 64 位，已安装 Docker 和 Docker Compose。** 已符合条件的音箱不用刷机。还没有 SSH，或不清楚固件版本的，先看[需刷机才看](#需刷机才看)；脱敏固件尚未重新刷机验收，不是新手一键刷机包。

本教程使用现成安装包，全程不用编译代码，也不用安装 Python。NAS 和音箱放在同一局域网。安装时需要把两个地址换成自己的：

- NAS_HOST：NAS 的局域网地址，可以在 NAS 管理页面或路由器里看到。
- SPEAKER_HOST：音箱的局域网地址，可以在路由器的设备列表里看到。

不要把这两个词原样复制到安装命令里。

### 第一步：把镜像导入 NAS

**操作位置：NAS 文件管理器，然后是 NAS 终端。**

把 NAS 安装包解压到 NAS。解压后应看到一个 `oh2p-xiaozhi` 文件夹，里面有 `docker-compose.yml` 和 `data` 文件夹。把 Docker 镜像文件也上传到这个文件夹。

在 NAS 终端里进入这个 `oh2p-xiaozhi` 文件夹。不同 NAS 的文件夹路径不同，可从文件管理器复制实际路径，用 `cd 实际路径` 进入。

执行：

```sh
docker load -i oh2p-xiaozhi-nas-amd64.tar.gz
```

等它完成。正常会看到：

```text
Loaded image: oh2p-xiaozhi-aec:2026.10.27.1
```

看到这行后，镜像就已导入。先不要启动，继续安装音箱。后面写的“NAS 终端”命令，都在这个有 `docker-compose.yml` 的文件夹里执行。

如果提示没有 Docker 权限，请使用 NAS 的管理员终端，或在命令前加 `sudo`。如果提示没有 `compose` 命令，先在 NAS 中安装 Docker Compose。

### 第二步：把程序装到音箱

**操作位置：先在电脑终端上传，再登录音箱。**

在电脑上打开保存音箱安装包的文件夹，再在该文件夹打开终端。Windows 可以在文件夹空白处右键选择“在终端中打开”。执行下面两条命令，把 `SPEAKER_HOST` 换成音箱地址：

```sh
scp -O oh2p-xiaozhi-speaker.tar.gz root@SPEAKER_HOST:/data/
ssh root@SPEAKER_HOST
```

按提示输入音箱的 SSH 密码。输入密码时屏幕不显示字符是正常的。上传命令里的 `-O` 要保留，音箱通常没有新版 `scp` 默认需要的 SFTP 服务。

如果提示 `no matching host key type found`，换用下面两条命令，允许连接音箱的旧版 SSH：

```sh
scp -O -o HostKeyAlgorithms=+ssh-rsa oh2p-xiaozhi-speaker.tar.gz root@SPEAKER_HOST:/data/
ssh -o HostKeyAlgorithms=+ssh-rsa root@SPEAKER_HOST
```

登录后，**接下来三条命令都在音箱里执行**：

```sh
mkdir -p /data/oh2p-xiaozhi-package
tar -xzf /data/oh2p-xiaozhi-speaker.tar.gz -C /data/oh2p-xiaozhi-package
sh /data/oh2p-xiaozhi-package/edge/install.sh 'ws://NAS_HOST:4399' --configure-alsa
```

最后一条里的 `NAS_HOST` 换成 NAS 地址。端口保持 `4399`，与安装包一致。

正常会看到 `Installed; no processes started`，并显示备份位置。意思是“已安装，还没有启动”。看到 `ALSA routing changed` 也正常，下一步启动 NAS 后再重启音箱即可。

如果出现 `VPM differs`、`unrecognized layout` 或 `Existing project processes are active`，表示固件、声音配置或已有程序不符合安装条件，先看[排错说明](docs/TROUBLESHOOTING.md)。不要跳过检查，也不要手动杀掉音箱原生语音进程。

安装器会备份原来的声音配置和启动文件。它会接管 `/data/init.sh`；如果你以前把其他程序也放在这个文件里启动，应先保留并处理那些设置。

### 第三步：启动 NAS 服务，再重启音箱

**先回到 NAS 终端**，在有 `docker-compose.yml` 的文件夹里执行：

```sh
docker compose up -d
docker compose ps
```

正常应看到 `server` 服务处于 `Up` 或 `running` 状态。它会使用刚才导入的镜像，不会现场编译代码。

**再回到音箱终端**，执行：

```sh
reboot
```

SSH 连接断开是正常现象。等待音箱重新开机、连上网络，程序会自动启动。这样也能让声音配置在开机时完整生效。

### 第四步：绑定自己的小智账号

**操作位置：电脑或手机浏览器，并在音箱旁操作。**

打开 [小智](https://xiaozhi.me/)，登录自己的账号，创建或选择一个智能体，找到“设备绑定”或“添加设备”。

对音箱说 **“你好小智”**，按音箱播报的验证码绑定。验证码是你这次连接生成的，每个人都不同，过期后需要重新获取。

如果没有播报验证码，在 NAS 终端执行：

```sh
docker compose logs --tail=100 server
```

也可以在 NAS 文件管理器中打开 `data/config.py`，查看 `VERIFICATION_CODE`。不要把整份配置或日志发到公开页面。

绑定完成后，如果仍然提示未绑定，在 NAS 终端执行：

```sh
docker compose restart server
```

`data/config.py` 会保存你的设备身份和绑定信息。以后升级或重启时保留这个文件，不要反复用新安装包中的空白配置覆盖它。

### 第五步：试着聊几句

1. 说“你好小智”，听到开始提示音后提问。
2. 等它回答后，再问下一句，确认不用重新唤醒也能继续聊天。
3. 在它说话时插一句话，确认能够停止原来的回答并接收新问题。
4. 安静约 20 秒，确认结束提示音响起，再试一次唤醒。

能完成这些步骤，就已完成基本安装。更完整的检查见[验收清单](docs/VERIFICATION.md)。

### 日常操作

**在 NAS 终端执行**，仍然先进入有 `docker-compose.yml` 的文件夹：

| 想做什么 | 命令 |
| --- | --- |
| 查看是否运行 | `docker compose ps` |
| 重启服务 | `docker compose restart server` |
| 停止服务 | `docker compose stop server` |
| 再次启动 | `docker compose up -d` |
| 查看最近错误 | `docker compose logs --tail=100 server` |

要改唤醒词，在 NAS 上打开 `data/config.py`，找到 `"keywords": ["你好小智"]`，只修改引号里的唤醒词，保存后重启服务即可。其他设置先保持默认。



## 升级

升级只需要更新 NAS 上的 Docker 镜像，并把原有 `docker-compose.yml` 中 `server` 的 `image` 版本改为 `2026.10.27.1`。保留原来的 `data/config.py`、`.env`、端口和所有自定义设置；不要解压新安装包覆盖配置，也不用重装音箱程序或刷固件。升级会重建 NAS 服务，建议等当前对话结束后再操作。

**操作位置：NAS 文件管理器和 NAS 终端。** 在电脑下载 [v2026.10.27.1 的 NAS Docker 镜像](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/latest/download/oh2p-xiaozhi-nas-amd64.tar.gz)，上传到原来放 `docker-compose.yml` 的文件夹。编辑原有 compose 文件时，只改 `server` 下 `image` 行的版本，例如：

```yaml
image: oh2p-xiaozhi-aec:2026.10.27.1
```

保留原来的 compose 文件及其他内容。进入该文件夹，在 NAS 终端执行：

```sh
docker load -i oh2p-xiaozhi-nas-amd64.tar.gz
docker compose up -d --no-deps server
docker compose ps
docker compose images server
```

确认 `server` 状态为 `Up` 或 `running`，并且镜像版本显示为 `2026.10.27.1`。保留升级前的旧镜像以便回退。若需回退，把原 compose 文件中 image 版本改回升级前的版本，再执行 docker compose up -d --no-deps server。

如果要撤销音箱安装，重新 SSH 登录音箱，执行：

```sh
sh /data/open-xiaoai/rollback.sh
```

它会停止这套程序并恢复安装前的备份，不会自动启动旧程序。如果提示文件后来被改过，应先按提示检查，避免覆盖其他修改。


## 需刷机才看

如果音箱已经是 1.62.2 且能 SSH 登录，**跳过本节，直接按[安装和绑定](#安装和绑定)操作**。公开固件来自正在使用的音箱，文件做过检查，但脱敏后的公开包尚未重新刷到另一台音箱上验收。它不是新手一键刷机包；不熟悉分区刷写时先不要刷。

<details>
<summary>展开 OH2P 1.62.2 的 Windows 和 Mac 刷机说明</summary>


### 下载固件

**[点击下载 OH2P-1.62.2-live-sanitized.zip](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/latest/download/OH2P-1.62.2-live-sanitized.zip)**

解压后主要用到这三个文件：

| 文件 | 用途 |
| --- | --- |
| `root-live-sanitized.squashfs` | 开启 SSH 的系统，下面刷的就是它 |
| `boot.img.encrypt` | 官方 1.62.2 启动文件 |
| `dtb.img.encrypt` | 官方 1.62.2 设备树文件 |

这个 ZIP 也不能直接导入 USB Burning Tool 当整机镜像刷。

### 确认音箱版本

在米家里查看音箱的固件版本。

- **已经是完整的 1.62.2 系统**：按下面的同版本刷写步骤操作，保留配套的 boot 和设备树。
- **还是 1.60.3、1.58.6 等其他版本**：先升级到完整的官方 1.62.2，确认能正常开机联网，再回来刷。不能只把下面的系统文件写进去；旧 boot 或旧设备树与它混用，会导致无法启动。详见[跨版本说明](docs/FIRMWARE.md)。

下面这套步骤适用于 **boot0 已是 1.62.2** 的设备。不确定 boot0 版本，或以前改过启动分区的，先核对清楚，不要直接切换。

### 选择你的电脑系统

- [Windows：下载工具、装驱动、连接和刷入](#windows-刷机步骤)
- [Mac：连接和刷入](#mac-刷机步骤)
- [刷完后登录音箱、安装小智](#5-登录音箱安装小智)

两条路线选一条就行，**不要两套都执行**。刷之前备好原系统的恢复文件；写入过程中不要拔电源或数据线。

### Windows 刷机步骤

以下命令都在 **Windows 电脑的 Git Bash** 里运行，不是在 NAS 或音箱上运行。步骤按[上游 Windows 教程](https://github.com/stevenjoezhang/xiaoai-agent/blob/9fc56179fbdde610e95755a843615669dece0d08/deploy/README.md)整理，本项目尚未用 Windows 重新刷写这个公开包。

### A. 下载三个东西

| 要下载什么 | 下载地址 | 做什么用 |
| --- | --- | --- |
| 本页的 OH2P 固件 ZIP | [固件下载](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/latest/download/OH2P-1.62.2-live-sanitized.zip) | 解压出 `root-live-sanitized.squashfs` |
| Amlogic Flash Tool **v6.0.0** | [上游指定的工具下载页](https://androidmtk.com/download-amlogic-flash-tool) | 提供 `update.exe` 和 USB 驱动；选 `Amlogic_Flash_Tool_v6.0.0.zip` |
| Git for Windows | [Git 官方下载页](https://git-scm.com/install/windows) | 安装后提供 Git Bash 命令窗口，普通 Intel/AMD 电脑选 x64 安装版 |

先安装 Git for Windows，保留 Git Bash 选项。再把 Amlogic 工具 **全部解压**到例如 `C:\OH2P\Amlogic_Flash_Tool_v6.0.0`，别在压缩包预览窗口里直接运行。工具路径尽量不用中文和空格。

在解压后的工具文件夹中找到 **`AMLLogic driver installer.exe`**，右键“以管理员身份运行”，按提示完成 USB 驱动安装。驱动安装成功后再继续。

打开工具的 `bin` 文件夹，把固件中的 **`root-live-sanitized.squashfs`** 复制进去。此时同一个文件夹里应同时看到：

```text
update.exe
root-live-sanitized.squashfs
```

其余工具文件保留，不要只拷走 `update.exe`。包里的 `boot.img.encrypt` 和 `dtb.img.encrypt` 在这次同版本刷写中不用。

### B. 用数据线连接音箱

OH2P 使用**底部的 Type-C 接口**连接电脑，不需要按旧款音箱的教程拆机。数据线必须能传数据，不能只是充电线。

```text
OH2P 底部 Type-C → USB 数据线 → Windows 电脑
```

音箱仍用自己的电源适配器供电。直连认不到时，可以换 USB 口、数据线，或加 USB 2.0 Hub 再试。

在刚才的 `bin` 文件夹空白处右键，选择 **Open Git Bash here / Git Bash Here**。Windows 11 如果没看到，先点“显示更多选项”。

在打开的窗口输入：

```sh
ls update.exe root-live-sanitized.squashfs
```

能列出两个文件名，说明目录对了。如果提示找不到文件，先回到正确的 `bin` 文件夹，不要继续刷。

### C. 让工具认到音箱

先在 Git Bash 输入下面这行，暂时不要按回车：

```sh
./update.exe identify
```

拔掉音箱电源，保持 USB 数据线连接，再接回电源，立即按回车。正常会出现类似：

```text
This firmware version is 0-7-0-16-0-0-0-0
```

数字尾部可能不同；它是刷机连接信息，**不是音箱的 1.62.2 固件版本**。出现 `can not find device` 时，可以按方向键 ↑ 调出刚才的命令再回车，或重新断电上电后重试。

识别成功后，先查看当前启动分区：

```sh
./update.exe bulkcmd "printenv boot_part"
```

确认输出中有 **`boot_part=boot0`**，并且前面在米家查到当前系统为 **1.62.2**，再往下做。如果是 `boot1`、没有结果，或输出 `ERR`，先停下核对版本和分区，不要直接切换。

### D. 设置启动项并刷入系统

下面每次只执行一行，看到 `Read status return success`、没有报错，再执行下一行：

```sh
./update.exe bulkcmd "setenv bootdelay 15"
./update.exe bulkcmd "setenv boot_part boot0"
./update.exe bulkcmd "saveenv"
```

然后刷入系统：

```sh
./update.exe partition system0 root-live-sanitized.squashfs
```

等它完成。正常结束会显示 **`mwrite success`**。如果出现 `ERR`、`failed` 或 `Usb status not success`，这次就不算成功，保留报错信息并停止后续操作，不要改刷其他分区来碰运气。

这一步只写 `system0`，不擦除 `data`，不写 bootloader。`bootdelay 15` 是开机等待时间，不是小智的语音响应时间。

成功后拔掉 USB 数据线和音箱电源，再接回电源，等待音箱开机联网，然后继续[登录和安装小智](#5-登录音箱安装小智)。

### Windows 常见卡点

| 看到什么 | 怎么处理 |
| --- | --- |
| `update.exe: No such file` | Git Bash 没有开在工具的 `bin` 文件夹，回到该文件夹重新打开 |
| 固件文件找不到 | 检查是否已解压，并把 `root-live-sanitized.squashfs` 放到 `update.exe` 旁边；不要把 ZIP 当固件写入 |
| 一直找不到音箱 | 确认驱动已安装，换数据线或 USB 口，再按“先准备命令、后接电”的顺序识别 |
| 设备管理器有黄色感叹号 | 先解决 USB 驱动安装问题；电脑响了连接提示音，不代表驱动已可用 |
| 只有 `0-0-0-0` 或后续命令失败 | 不要当成已经连接成功，也不要继续刷；先排查驱动、线材和连接状态 |

仍连不上时可看[上游连接排查讨论](https://github.com/idootop/open-xiaoai/issues/6#issuecomment-2815632879)。

### Mac 刷机步骤

### A. 准备电脑和数据线

下面用 **Mac** 演示，沿用我们之前使用的刷机工具。命令都在 Mac 的“终端”里输入，不是在 NAS 或音箱里输入。

准备一根能传数据的 USB 线。我们使用的连接方式是：

```text
OH2P → Type-C 对 Type-A 数据线 → USB 2.0 Hub → Mac
```

音箱仍使用自己的电源。接不上时先换数据线或 Hub 端口，[这里有连接记录](https://github.com/ChouMax-1989/oh2p-macos-worldcup)。Windows 用户不要直接照抄下面的 Mac 命令。

Mac 先装好 [Homebrew](https://brew.sh/zh-cn/)，然后打开终端：

```sh
brew install libusb-compat
git clone https://github.com/stevenjoezhang/xiaoai-agent.git
cd xiaoai-agent
git checkout 9fc56179fbdde610e95755a843615669dece0d08
cd deploy/flash-tool
chmod +x flash macos/update
```

用 Finder 把解压出的 **`root-live-sanitized.squashfs`** 复制到这个 `flash-tool` 文件夹。可以在当前终端输入 `open .` 打开它。

### B. 连接并刷入

刷之前备好原系统的恢复文件；刷写过程中不要拔电源或数据线。

先运行：

```sh
./flash connect
```

随后拔掉音箱电源，再插上。等工具真正识别到设备，输出 `identify: success`，再继续。一直连不上或出现 `ERR` 时先停下，不要继续执行写入命令。

确认 **boot0 已配套 1.62.2** 后，依次运行，上一条成功再执行下一条：

```sh
./flash delay 15
./flash switch boot0
./flash system system0 root-live-sanitized.squashfs
```

这里仅写 `system0`，**不用写包里的 boot、dtb，也不要擦除 data 或改写 bootloader**。`15` 是刷机连接用的开机等待时间，不是语音响应时间。

看到写入成功、没有错误后，拔掉数据线和电源，再接回电源，让音箱正常启动。

### 5. 登录音箱，安装小智

音箱连上家庭网络后，在路由器的设备列表里找到它的 IP。在 **Windows 的 Git Bash 或 Mac 的终端**执行，把 `SPEAKER_HOST` 换成这个 IP：

```sh
ssh -o HostKeyAlgorithms=+ssh-rsa root@SPEAKER_HOST
```

登录信息：

- 用户名：`root`
- 密码：`open-xiaoai`

输入密码时不显示字符是正常的。这是公开密码，只在自己的局域网使用；系统只读，不能保证运行 `passwd` 就能永久改密。

登录成功后，按 **[小智安装教程](#安装和绑定)** 安装音箱程序和 NAS Docker，再到 [xiaozhi.me](https://xiaozhi.me/) 绑定自己的账号。刷完固件不会自动装好小智；装好后默认唤醒词是 **“你好小智”**。

工具用法参考[上游刷机说明](https://github.com/stevenjoezhang/xiaoai-agent/blob/9fc56179fbdde610e95755a843615669dece0d08/deploy/flash-tool/README.md)。需要查文件来源、修改内容和校验值，可看[固件制作记录](docs/FIRMWARE-VERIFICATION.md)或 ZIP 内的 `SHA256SUMS`。


</details>

## 使用情况

同源 AEC 路线曾由用户验证过连续对话和打断，增强音频修复后也确认可以唤醒。当前版本的长期稳定性、全新设备安装、首次绑定和整机重启等仍需按验收清单确认。

<details>
<summary>其他技术资料</summary>

- [更新说明](docs/UPDATE-20261027.md)
- [排错说明](docs/TROUBLESHOOTING.md)
- [验收清单](docs/VERIFICATION.md)
- [运行原理](docs/ARCHITECTURE.md)
- [固件资料](docs/FIRMWARE.md)
- [固件制作记录](docs/FIRMWARE-VERIFICATION.md)
- [服务端源码说明](server/README.md)
- [音箱端构建说明](edge/BUILD.md)
- [许可证和上游来源](NOTICE.md)

</details>

项目保留了实际使用的 AEC、连续采音、实时对话、插话、提示音，以及 5 秒增强断流恢复修改。基于 [open-xiaoai](https://github.com/idootop/open-xiaoai) 和 [xiaoai-agent](https://github.com/stevenjoezhang/xiaoai-agent) 的相关实现；设备固件中的专有库不随附加安装包分发。