# 参与开发

本项目目前仅支持 Windows x64 MSVC。安装与使用见 [README](README.md)，模块边界见[架构说明](docs/architecture.md)，测试方法见[测试说明](tests/README.md)。

提交问题时提供版本、入口、脱敏后的复现步骤和预期行为。使用合成数据库或合成媒体，不上传真实聊天、密钥、配置、缓存、内存转储或凭据。安全问题见[安全报告](SECURITY.md)。

修改业务逻辑时同时更新相关测试及文档；按账号隔离，保留授权、取消、失败和数据保全约束。每个 checkout 使用独立构建目录。执行 `cargo check --target x86_64-pc-windows-msvc` 和相关测试；完整自动检查使用 `pwsh -NoProfile -File scripts/check-quality.ps1 -Stage All`，独立夹具测试范围见 `tests/support/CHECK_MATRIX.md`。默认忽略的用例须先确认依赖和行为，不批量启动隐藏子进程入口。

新增第三方实现或资产时保留原作者、来源、版本及许可证；没有再分发依据的资产不加入候选。AI 生成或迁移代码同样需要审查、测试及来源说明，不以重写语言替代归属记录。

本项目原创贡献采用仓库 [Apache-2.0](LICENSE)；第三方材料遵循[第三方声明](THIRD_PARTY_NOTICES.md)。提交修改应具备对应权利，不将其他作者材料重新声明为原创。
