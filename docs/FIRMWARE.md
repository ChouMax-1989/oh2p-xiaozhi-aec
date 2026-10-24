# OH2P 1.62.2 固件下载与准备

已有 **中国版 OH2P 1.62.2 + SSH + `/data/init.sh` 启动钩子** 的音箱，直接安装本项目应用包即可，不需要重刷。

需要固件时，优先查看[实际使用系统的脱敏固件包](LIVE-FIRMWARE.md)。它从正在使用的音箱只读提取系统，只处理副本中的登录密码和 SSH 密钥，保留现有声音与启动配置。**来源镜像在原音箱使用正常，脱敏重打包尚未在新机刷写验收，也未刷回原音箱。**

本页保留官方原厂文件、兼容说明和旧候选构建资料。应用安装包仍按[小白安装步骤](DEPLOYMENT.md)单独安装。

## 先选你需要的文件

- **已经能 SSH 登录 1.62.2 音箱**：不用刷机，直接看[小白安装步骤](DEPLOYMENT.md)。
- **需要现有使用方案的系统文件**：看[现机提取脱敏版下载与说明](LIVE-FIRMWARE.md)。其中系统来自现机，配套 boot 和设备树来自官方同版 1.62.2；不含账号、Wi-Fi、校准和设备身份数据。
- **只需要小米原厂固件**：使用下方官方链接。原厂包不能直接让音箱接入小智。
- **需要研究旧的原厂重建方式**：[历史候选 OH2P-1.62.2-firmware-candidate.zip](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/releases/download/firmware-1.62.2-candidate-20261024/OH2P-1.62.2-firmware-candidate.zip)。它保留作历史备选，不作为主要固件下载入口；尚未刷机验收。

下面关于候选生成器和 `root-original.squashfs` 的内容，只适用于旧的原厂重建候选，不是现机脱敏包的文件清单。

历史候选的 SSH 登录用户名是 `root`，公开默认密码是 `open-xiaoai`，不属于你的个人密码。它没有强制改密功能，也不能保证直接执行 `passwd` 可持久改密。只用于可信内网；需要专属访问控制时使用下方公钥模式构建。旧候选 ZIP 中的 `root-original.squashfs` 是未改动的官方恢复材料；删除预置 SSH 密钥的检查仅针对 `root-ssh-candidate.squashfs`。

## 官方原厂包：可直接下载，不含本项目 SSH 补丁

[下载小米官方 OH2P 1.62.2 OTA：mico_all_616cd9d93_1.62.2.bin](https://cdn.cnbj1.fds.api.mi-img.com/xiaoqiang/rom/oh2p/mico_all_616cd9d93_1.62.2.bin)

2026-10-24 实际下载核对结果：

| 项目 | 值 |
|---|---|
| 型号/版本 | 中国版 Xiaomi 智能音箱 Pro，OH2P 1.62.2 |
| 大小 | `37,822,040` bytes |
| MD5 | `bc81f5b40f3db5e9a9d5943616cd9d93` |
| SHA256 | `08c6c0c5e7dfd3de59a7e37b4709ad90ab84e8025c799a642730e97ef53c0eba` |

下载后应先检查：

```sh
sha256sum mico_all_616cd9d93_1.62.2.bin
```

来源为小米官方 CDN；同日通过上游 OTA 查询算法、`OH2P`/`1.62.2` 参数与空设备序列号查询官方接口，返回 `code=0`、上述下载地址、大小和 MD5，无需小米账号凭据。MD5 也与 [xiaoai-agent 固定修订的构建记录](https://github.com/stevenjoezhang/xiaoai-agent/blob/9fc56179fbdde610e95755a843615669dece0d08/deploy/OH2P_1.62.2_BUILD_NOTES.md)一致。此处提供官方链接，不把小米专有固件视为本仓库 MIT/LGPL 源码。

从该 OTA 提取的配套输入如下；它们不是可以互相替代的同一个文件：

| 文件 | 大小 bytes | SHA256 |
|---|---:|---|
| `root.squashfs` | 31,728,742 | `95e1238dd41d28c84b7d1f31b80e29f6368341ee78551f5b51fd6c0c290d8853` |
| `boot.img.encrypt` | 5,403,136 | `6f61057bf3f122081834b27bec6eb4fd86d346bc8e308b4d21b15e1f99b5c4fe` |
| `dtb.img.encrypt` | 107,552 | `c87cf7169f28ccd356f04e78cb5f400d807cb2fa139fa1b35c0cf45c8989e27d` |

原厂 rootfs 的版本文件标记 OH2P/1.62.2；`libvpm.so` SHA256 为 `f5d76a34dc40eb1b7c9045614da636fe5558344c3334a44e152d22fbfaed56d9`，与本项目已运行 AEC 所需 ABI 相符。这是静态一致性证据，不代替新镜像的刷写、联网与声音验证。

## 为什么不直接给其它仓库的 patched 文件

截至2026-10-24，核查结果为：

- [oh2p-macos-worldcup](https://github.com/ChouMax-1989/oh2p-macos-worldcup) 记录 USB WorldCup 识别方法，没有固件 Release；识别成功不证明镜像可刷。
- [xiaoai-agent Releases](https://github.com/stevenjoezhang/xiaoai-agent/releases) 提供的是 Agent 程序，没有现成 OH2P 1.62.2 SSH 镜像。其[固定修订部署说明](https://github.com/stevenjoezhang/xiaoai-agent/blob/9fc56179fbdde610e95755a843615669dece0d08/deploy/README.md)要求从当前源码重新制作 rootfs。
- [Open-XiaoAI OH2P 1.58.6 Release](https://github.com/idootop/open-xiaoai/releases/tag/OH2P_1.58.6) 确实有另一版本的预构建 rootfs。它不能用来满足本项目的1.62.2要求，也不能只因同为OH2P就混刷。

## 历史备选：从原厂包生成 SSH 候选

仓库提供 [firmware/build.sh](../firmware/build.sh) 与 [firmware/prepare.py](../firmware/prepare.py)。它只从上述官方 OTA 构建，不读取音箱、设备 dump、账号缓存、Wi-Fi、`/data`、语音、日志或现有 SSH 私钥，也不执行刷机。

在临时 Linux 构建环境中准备依赖，例如 Ubuntu：

```sh
sudo apt-get update
sudo apt-get install -y build-essential liblzma-dev zlib1g-dev liblzo2-dev \
  liblz4-dev libzstd-dev curl patch openssl python3 tar
```

选择一种凭据模式构建；已有输出目录时脚本会拒绝覆盖：

```sh
# 通用公开候选：root 的公开默认密码为 open-xiaoai。
sudo bash firmware/build.sh --public-default

# 或者：个性化候选，仅接受你自己的 RSA 公钥，禁用 SSH 密码登录。
# 输入必须是 .pub 公钥，私钥保留在自己的电脑上。
sudo bash firmware/build.sh --authorized-key /absolute/path/to/id_rsa.pub \
  --out /absolute/path/to/new-output-directory
```

通用模式沿用公开默认密码，**没有强制首次改密功能**。镜像根分区为只读 SquashFS，不能假定执行 `passwd` 就能持久修改它。不要把此默认密码当作秘密；需要个人访问控制时使用公钥模式生成自己的候选。公钥模式去掉公钥的账号/机器名注释，不包含私钥，关闭 `PasswordAuth`/`RootPasswordAuth`，并使用无人持有的随机密码 hash。公钥认证也需要刷写后实际验证，不应提前丢弃恢复材料。

生成器固定以下来源并检查 SHA256：

- xiaoai-agent `9fc56179fbdde610e95755a843615669dece0d08`，源码归档 SHA256 `a2134c76436e879ed9e0244037cba886150bb0128aa810ee840cc24f71f05394`。
- 标准 squashfs-tools `4.7.5`，源码归档 SHA256 `547b7b7f4d2e44bf91b6fc554664850c69563701deab9fd9cd7e21f694c88ea6`。

它应用上游 SSH、登录、禁 OTA、启动钩子补丁，但保留原来的 UART 登录配置；然后删除原厂镜像中现成的 SSH host key 与授权 key。SSH 配置继续指向 `/data/etc/dropbear/dropbear_rsa_host_key`，由音箱首次启动时生成自己的 host key，不把同一把私钥分发给所有人。刷入时保留的已有 `/data` 可能仍有旧 host key；生成器不会读取或处理它。

原厂 `mico_record` 与 `noop` 都指向 `Capture`。候选只把原生 `mico_record` 改到 `hw:0,0`，保留 `noop → Capture → hw:0,3` 及原有播放配置，避免原生与定制采音共用真实输入。

重新打包使用此前实际成功流程中的标准参数，不用 `replace-backhand` 原地替换方式：

```sh
mksquashfs ROOT OUTPUT -noappend -comp xz -b 131072 \
  -no-tailends -no-xattrs -all-root -exports \
  -p 'dev/console c 600 0 0 5 1'
```

打包后会重新解包，检查版本、VPM、启动钩子、认证模式与缺失 SSH 私钥，并限制 system 镜像小于40MiB。新候选 SHA256 与大小以实际构建生成的清单为准；历史成功镜像的旧 hash 不适用于这次新产物。

默认输出 `firmware/out/release/`：

| 输出 | 用途 |
|---|---|
| `boot.img.encrypt`、`dtb.img.encrypt` | 同一官方1.62.2包的配套输入 |
| `root-original.squashfs` | 官方原始 rootfs |
| `root-ssh-candidate.squashfs` | 新生成 SSH/启动钩子/音频隔离候选，仅供 system 分区 |
| `firmware-manifest.json`、`SHA256SUMS` | 来源、认证模式、大小、hash与未实机状态 |

`OH2P-1.62.2-firmware-candidate.zip` 封装这一输出目录，并附本说明。**它是一组分区输入，不是小米签名的完整 OTA，也不是 USB Burning Tool 全盘镜像。** 它不包含本项目NAS配置、API Key或应用运行数据；小智应用仍通过[部署说明](DEPLOYMENT.md)安装。

## 兼容边界与跨版本配套

只面向中国版 OH2P，不能扩展到其它小爱型号。历史实机记录表明：1.60.3 的 boot 配1.62.2补丁 rootfs失败；仅升级boot/rootfs而保留旧DTB也失败。两版版本文件里的DTB版本即使都写`0.0.1`，实际字节仍可能不同。

因此，同版本1.62.2环境的SSH准备与从旧版跨版本升级是两件事。跨版本必须先确认匹配的1.62.2 boot、官方rootfs和DTB配套，先验收纯官方系统，再对同版system/rootfs应用补丁。不要只按上游“刷 system0”步骤跨版本；也不要把切换 `boot1` 当作任何设备都有效的恢复保证。保留原厂/恢复材料和分区对应关系，BL2/TPL不属于本生成器发布输出。

WorldCup 写入 `_aml_dtb` 时涉及解密，写后回读不能直接与加密输入做字节比较，应按实际工具与有效 FDT 结构验收。本仓库没有自动刷机脚本；刷写工具与线材说明可参考[上游 macOS 文档](https://github.com/stevenjoezhang/xiaoai-agent/blob/9fc56179fbdde610e95755a843615669dece0d08/deploy/flash-tool/README.md)。

本次已完成官方文件下载、hash、分段与rootfs静态核查；构建候选的解包检查由生成器执行。未执行当前音箱刷写、分区修改、恢复测试或新候选的实机/声学验收。

候选构建及重新解包检查已于 2026-10-24 [通过 GitHub Actions](https://github.com/ChouMax-1989/oh2p-xiaozhi-aec/actions/runs/40103311378)。这不包含音箱刷写验收。
