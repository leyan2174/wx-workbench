# wx-cli 协作规则

本仓库只面向 Windows x64 MSVC。

- 修改 Rust 后运行 cargo check，不提交检查失败的 Rust 代码。
- 交付前运行 cargo check --target x86_64-pc-windows-msvc 和 cargo test。
- 修改包版本时运行 cargo update --workspace。
- 保持账号隔离；不提交密钥、私人数据或解密缓存。
- 每次提交后推送到已配置的 origin 远端。
- 未经用户要求，不恢复已移除的平台代码或发布目标。
