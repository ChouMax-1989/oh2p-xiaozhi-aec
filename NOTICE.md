# 许可证和来源

这套工程组合了几部分代码，各自保留许可证：

- `edge/aec/preserved/vpm-owner`：LGPL-3.0-or-later。VPM ABI 工作源自 stevenjoezhang/xiaoai-agent 的 9fc56179fbdde610e95755a843615669dece0d08；包括本项目的增强音频共享与断流恢复修改。许可证见 LICENSES/LGPL-3.0.txt 和 GPL-3.0.txt。
- open-xiaoai 服务端、客户端和相应修改：MIT，原许可见 LICENSES/open-xiaoai-MIT.txt。上游固定提交 bc3396c64e2a435f354eb5cb12a203981f1fe422。
- 本项目新增 reader、脚本与说明：MIT（见 LICENSE）。
- OH2P 固件内置 libvpm.so 等库属于设备固件，本仓库不分发。安装时使用设备已有的库。
- 容器依赖及模型沿用各自上游许可；本项目许可证不替代第三方许可证。

设备端的发行 client 按文件 SHA256 固定；它的发行地址不是不可变版本地址，不能声称其与参考源码提交逐字节可复现。构建或分发二进制时，应同时提供相应源码和许可证。
