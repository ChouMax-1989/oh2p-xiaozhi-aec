# 小白安装步骤

这份教程使用现成安装包，全程不用编译代码，也不用安装 Python。先准备好 NAS 和音箱，再绑定自己的小智账号。

**适用设备：OH2P 音箱，固件 1.62.2，已经能通过 SSH 登录；NAS 为 Intel/AMD 64 位，已安装 Docker 和 Docker Compose。** 已符合条件的音箱不用刷机。还没有 SSH 的，先看[固件下载与 SSH 准备](FIRMWARE.md)，完成后再回来。

教程里只有两个地址要换成自己的：

- `NAS_HOST`：NAS 的局域网地址，可以在 NAS 管理页面或路由器里看到。
- `SPEAKER_HOST`：音箱的局域网地址，可以在路由器的设备列表里看到。

不要把这两个词原样复制到安装命令里。

## 第一步：在电脑上下载三个文件

打开 [v2026.10.24.1 下载页](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/tag/v2026.10.24.1)，下载：

1. [oh2p-xiaozhi-easy-install.zip](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/download/v2026.10.24.1/oh2p-xiaozhi-easy-install.zip)：NAS 启动文件。
2. [oh2p-xiaozhi-nas-amd64.tar.gz](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/download/v2026.10.24.1/oh2p-xiaozhi-nas-amd64.tar.gz)：Docker 镜像，**不用解压**。
3. [oh2p-xiaozhi-speaker.tar.gz](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/download/v2026.10.24.1/oh2p-xiaozhi-speaker.tar.gz)：音箱程序，先留在电脑上，**不用解压**。

下载页里自动生成的 `Source code` 是源码，普通安装不用下载。

## 第二步：把镜像导入 NAS

**操作位置：NAS 文件管理器，然后是 NAS 终端。**

把 NAS 安装包解压到 NAS。解压后应看到一个 `oh2p-xiaozhi` 文件夹，里面有 `docker-compose.yml` 和 `data` 文件夹。把 Docker 镜像文件也上传到这个文件夹。

在 NAS 终端里进入这个 `oh2p-xiaozhi` 文件夹。不同 NAS 的文件夹路径不同，可从文件管理器复制实际路径，用 `cd 实际路径` 进入。

执行：

```sh
docker load -i oh2p-xiaozhi-nas-amd64.tar.gz
```

等它完成。正常会看到：

```text
Loaded image: oh2p-xiaozhi-aec:2026.10.24.1
```

看到这行后，镜像就已导入。先不要启动，继续安装音箱。后面写的“NAS 终端”命令，都在这个有 `docker-compose.yml` 的文件夹里执行。

如果提示没有 Docker 权限，请使用 NAS 的管理员终端，或在命令前加 `sudo`。如果提示没有 `compose` 命令，先在 NAS 中安装 Docker Compose。

## 第三步：把程序装到音箱

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

如果出现 `VPM differs`、`unrecognized layout` 或 `Existing project processes are active`，表示固件、声音配置或已有程序不符合安装条件，先看[排错说明](TROUBLESHOOTING.md)。不要跳过检查，也不要手动杀掉音箱原生语音进程。

安装器会备份原来的声音配置和启动文件。它会接管 `/data/init.sh`；如果你以前把其他程序也放在这个文件里启动，应先保留并处理那些设置。

## 第四步：启动 NAS 服务，再重启音箱

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

## 第五步：绑定自己的小智账号

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

## 第六步：试着聊几句

1. 说“你好小智”，听到开始提示音后提问。
2. 等它回答后，再问下一句，确认不用重新唤醒也能继续聊天。
3. 在它说话时插一句话，确认能够停止原来的回答并接收新问题。
4. 安静约 20 秒，确认结束提示音响起，再试一次唤醒。

能完成这些步骤，就已完成基本安装。更完整的检查见[验收清单](VERIFICATION.md)。

## 常用操作

**在 NAS 终端执行**，仍然先进入有 `docker-compose.yml` 的文件夹：

| 想做什么 | 命令 |
| --- | --- |
| 查看是否运行 | `docker compose ps` |
| 重启服务 | `docker compose restart server` |
| 停止服务 | `docker compose stop server` |
| 再次启动 | `docker compose up -d` |
| 查看最近错误 | `docker compose logs --tail=100 server` |

要改唤醒词，在 NAS 上打开 `data/config.py`，找到 `"keywords": ["你好小智"]`，只修改引号里的唤醒词，保存后重启服务即可。其他设置先保持默认。

如果要撤销音箱安装，重新 SSH 登录音箱，执行：

```sh
sh /data/open-xiaoai/rollback.sh
```

它会停止这套程序并恢复安装前的备份，不会自动启动旧程序。如果提示文件后来被改过，应先按提示检查，避免覆盖其他修改。
