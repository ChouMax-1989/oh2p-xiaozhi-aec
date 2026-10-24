# OH2P 1.62.2 固件下载与刷机

这是给**中国版小米小爱音箱 Pro（OH2P）**用的 1.62.2 固件，已开启 SSH，保留音箱原有的回声消除（AEC），可配合本项目接入小智 AI。

固件来自我们正在使用的音箱，公开前去掉了个人登录信息。公开包做过文件检查，但没有重新刷到另一台音箱上测试。

**你的音箱已经是 1.62.2，而且能 SSH 登录？不用刷，直接看[小智安装教程](DEPLOYMENT.md)。**

## 1. 下载固件

**[点击下载 OH2P-1.62.2-live-sanitized.zip](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/download/firmware-1.62.2-live-sanitized-20261024/OH2P-1.62.2-live-sanitized.zip)**

解压后主要用到这三个文件：

| 文件 | 用途 |
| --- | --- |
| `root-live-sanitized.squashfs` | 开启 SSH 的系统，下面刷的就是它 |
| `boot.img.encrypt` | 官方 1.62.2 启动文件 |
| `dtb.img.encrypt` | 官方 1.62.2 设备树文件 |

不要下载带 `candidate` 字样的旧候选。这个 ZIP 也不能直接导入 USB Burning Tool 当整机镜像刷。

## 2. 确认音箱版本

在米家里查看音箱的固件版本。

- **已经是完整的 1.62.2 系统**：按下面的同版本刷写步骤操作，保留配套的 boot 和设备树。
- **还是 1.60.3、1.58.6 等其他版本**：先升级到完整的官方 1.62.2，确认能正常开机联网，再回来刷。不能只把下面的系统文件写进去；旧 boot 或旧设备树与它混用，会导致无法启动。详见[跨版本说明](FIRMWARE.md)。

下面这套步骤适用于 **boot0 已是 1.62.2** 的设备。不确定 boot0 版本，或以前改过启动分区的，先核对清楚，不要直接切换。

## 3. 准备电脑和数据线

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

## 4. 连接并刷入

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

## 5. 登录音箱，安装小智

音箱连上家庭网络后，在路由器的设备列表里找到它的 IP。在 Mac 终端执行，把 `SPEAKER_HOST` 换成这个 IP：

```sh
ssh -o HostKeyAlgorithms=+ssh-rsa root@SPEAKER_HOST
```

登录信息：

- 用户名：`root`
- 密码：`open-xiaoai`

输入密码时不显示字符是正常的。这是公开密码，只在自己的局域网使用；系统只读，不能保证运行 `passwd` 就能永久改密。

登录成功后，按 **[小智安装教程](DEPLOYMENT.md)** 安装音箱程序和 NAS Docker，再到 [xiaozhi.me](https://xiaozhi.me/) 绑定自己的账号。刷完固件不会自动装好小智；装好后默认唤醒词是 **“你好小智”**。

工具用法参考[上游刷机说明](https://github.com/stevenjoezhang/xiaoai-agent/blob/9fc56179fbdde610e95755a843615669dece0d08/deploy/flash-tool/README.md)。需要查文件来源、修改内容和校验值，可看[固件制作记录](FIRMWARE-VERIFICATION.md)或 ZIP 内的 `SHA256SUMS`。
