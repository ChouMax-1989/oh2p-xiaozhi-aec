# Edge 构建与产物

`prebuilt/` 保留当前实际运行的 ARMv7 hard-float 三件套，以 `SHA256SUMS` 固定；无需在音箱安装编译器。owner/reader 对应源码均完整保留。上游发行 client 的源码修订与发行文件不存在可证明的一一对应关系，不宣称二进制可复现。

Linux、Docker 或 NAS Docker 主机，在仓库根目录执行：

```sh
sh edge/build.sh
```

该入口构建 `rust:1.93.0-bookworm` 镜像，固定 Zig 0.16.0、cargo-zigbuild 0.22.0，运行 owner/reader 的 Linux 测试并交叉构建目标 `armv7-unknown-linux-gnueabihf.2.25`；读 ELF 的动态加载器与 GLIBC 符号并拒绝高于2.25的需求。产物为 `edge/build/out/{vpm-owner,arecord,SHA256SUMS}`。构建容器限2 CPU/2GiB，不访问设备。owner/reader 的 Cargo.lock 无第三方依赖。Docker 镜像及工具安装初次需要网络；镜像 tag 不构成完全可复现环境。

可选构建 client 候选：

```sh
sh edge/build.sh --client
```

固定源码为 [idootop/open-xiaoai bc3396c64e2a435f354eb5cb12a203981f1fe422](https://github.com/idootop/open-xiaoai/tree/bc3396c64e2a435f354eb5cb12a203981f1fe422)，下载归档 SHA256 为 `26f26b367746ce438721e23ca87f97f508431e93dfe04a0fc6db616e276d584e`。`tools/fetch-client-source.sh` 可独立下载并校验到 `build/client-source`。构建额外应用 `tools/client-fvsh-s16.patch`：该上游修订会将16bit请求改为S32，补丁使已增强的FVSH音频继续按S16请求，符合reader精确契约。只构建 `--bin client`，不构建监控程序。

候选输出 `edge/build/out/client-candidate`，不覆盖发行 client，不自动用于安装。它未在音箱验证，且不宣称与 SHA256 `7c19a777bf4bbadb305dd44beb29b344708a35a70923f803ba896542ba5fea20` 的原发行文件相同。重新运行 `--client` 时先自行保留/移走已有 `edge/build/client-source`；脚本拒绝覆盖。

owner/reader 的重新构建与修改可以保留原 client：将新的 owner/reader 复制到一个本地安装包的 `prebuilt/`，在其中运行 `sha256sum arecord vpm-owner client >SHA256SUMS`。安装时保留更新后的清单；boot对owner/reader检查此清单，client另外固定当前发行hash。使用自行构建的client候选需要自行验证其契约，并相应更新boot中的client hash。公开仓库不会替用户修改在运行的设备。

本地可独立运行的无设备测试：

```sh
sh edge/tests/test-alsa-routes.sh
cargo test --locked --offline --manifest-path edge/aec/reader/Cargo.toml
cargo test --locked --offline --manifest-path edge/aec/preserved/vpm-owner/Cargo.toml
```

Cargo 完整测试需要 Linux，reader 的 socket 测试须在 root 容器内运行（检查 root socket 所有权）；Windows 可独立执行 `rustc --test edge/aec/preserved/vpm-owner/src/stream_health.rs` 和 `native_trial_protocol.rs`。编译或协议测试不证明音箱的声学效果、真人连续对话、插话和长期稳定性。
