# OH2P 1.62.2 固件下载与刷机

这是给**国行小米小爱音箱 Pro（OH2P）**用的 1.62.2 固件。刷它是为了让音箱能安装这套小智程序：电脑可以登录音箱，程序能使用麦克风和扬声器，开机后也能自动运行。

原厂系统不方便直接安装和启动这套程序，这版主要改了四处：

- **打开 SSH 登录**：可以从电脑登录音箱、上传和安装程序。
- **加入开机启动入口**：开机时执行 `/data/init.sh`，安装好小智程序后就能自动启动。
- **调整麦克风输入分配**：给自定义语音程序使用，减少与原生小爱抢麦；原有的回声消除（AEC）库保留，不是另外写了一套 AEC。
- **关闭原有的自动升级流程**：避免系统更新把这些修改覆盖掉。以后不能再指望米家自动更新这套系统。

公开版还去掉了个人登录信息，SSH 默认密码改成了 `open-xiaoai`。**刷完只是把音箱准备好，连续对话、插话打断和提示音还需要安装[音箱程序和 NAS 服务](DEPLOYMENT.md)。**

固件来自我正在使用的音箱。固件包已做过文件检查，但没有重新刷到另一台音箱上测试。

**如果你的音箱已经是 1.62.2，而且能 SSH 登录，就不用刷，直接看[小智安装教程](DEPLOYMENT.md)。**

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

## 3. 选你的电脑系统

- [Windows：下载工具、装驱动、连接和刷入](#windows-刷机步骤)
- [Mac：连接和刷入](#mac-刷机步骤)
- [刷完后登录音箱、安装小智](#5-登录音箱安装小智)

两条路线选一条就行，**不要两套都执行**。刷之前备好原系统的恢复文件；写入过程中不要拔电源或数据线。

## Windows 刷机步骤

以下命令都在 **Windows 电脑的 Git Bash** 里运行，不是在 NAS 或音箱上运行。步骤按[上游 Windows 教程](https://github.com/stevenjoezhang/xiaoai-agent/blob/9fc56179fbdde610e95755a843615669dece0d08/deploy/README.md)整理，本项目尚未用 Windows 重新刷写这个公开包。

### A. 下载三个东西

| 要下载什么 | 下载地址 | 做什么用 |
| --- | --- | --- |
| 本页的 OH2P 固件 ZIP | [固件下载](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/download/firmware-1.62.2-live-sanitized-20261024/OH2P-1.62.2-live-sanitized.zip) | 解压出 `root-live-sanitized.squashfs` |
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

## Mac 刷机步骤

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

## 5. 登录音箱，安装小智

音箱连上家庭网络后，在路由器的设备列表里找到它的 IP。在 **Windows 的 Git Bash 或 Mac 的终端**执行，把 `SPEAKER_HOST` 换成这个 IP：

```sh
ssh -o HostKeyAlgorithms=+ssh-rsa root@SPEAKER_HOST
```

登录信息：

- 用户名：`root`
- 密码：`open-xiaoai`

输入密码时不显示字符是正常的。这是公开密码，只在自己的局域网使用；系统只读，不能保证运行 `passwd` 就能永久改密。

登录成功后，按 **[小智安装教程](DEPLOYMENT.md)** 安装音箱程序和 NAS Docker，再到 [xiaozhi.me](https://xiaozhi.me/) 绑定自己的账号。刷完固件不会自动装好小智；装好后默认唤醒词是 **“你好小智”**。

工具用法参考[上游刷机说明](https://github.com/stevenjoezhang/xiaoai-agent/blob/9fc56179fbdde610e95755a843615669dece0d08/deploy/flash-tool/README.md)。需要查文件来源、修改内容和校验值，可看[固件制作记录](FIRMWARE-VERIFICATION.md)或 ZIP 内的 `SHA256SUMS`。
