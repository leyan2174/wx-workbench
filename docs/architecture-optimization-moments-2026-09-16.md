# 朋友圈应用工作流迁移

## 边界

朋友圈时间线、缓存恢复、归档、相册生成和宿主授权后的媒体获取已从 `src/toolkit/sns` 迁至 `src/application/moments`。daemon 操作直接调用应用用例；业务查询继续由 `business::moments` 表达，微信数据库、XML/DAT、缓存布局和密钥流由 `adapters::wechat` 实现，文件发布由 `infrastructure` 负责。

朋友圈导出读取现已进一步收敛到 `adapters::wechat::moments::read_export_paths`：适配器负责只读打开 SQLite、持有一致性事务、探测联系人及互动能力，并把时间线 SQL、内容解码和 XML 解释转换为 `ExportSnapshot`。`application::moments::export` 只接收该类型化快照，负责显示名安全化、联系人分组、文件渲染和发布，不再引用 `rusqlite`，也不包含微信表名或 SQL。

本轮没有移动 stdio/HTTP/MCP 协议，没有改变 CLI 参数、JSON 字段或 daemon 生命周期，也没有把普通查询改成持久任务。媒体下载仍要求宿主显式授权，模型参数不能开启网络能力。

## 复杂度收敛

`write_export_with_publication` 原先同时执行整批身份预检、内容生成、媒体恢复和发布。现在先由 `prepare_timeline_trees` 对全部联系人完成绑定检查并持有发布锁，再逐联系人生成暂存内容，最后由 `publish_timeline` 按媒体、单帖、汇总、HTML、恢复报告的既有顺序提交。

这使晚出现的联系人冲突在任何内容生成和发布前失败，同时保留单联系人暂存清理、旧目录显式认领、部分媒体报告和不覆盖已有媒体的语义。

## 验证

- `cargo check --target x86_64-pc-windows-msvc --all-targets`：通过。
- `RUSTFLAGS=-D warnings cargo check --target x86_64-pc-windows-msvc --all-targets`：通过，零告警。
- `cargo test application::moments`：85 通过，2 项既有条件忽略。
- `cargo test adapters::wechat::moments`：29 通过。
- `sns_timeline_runtime`：8 通过；`sns_album_runtime`：12 通过；`sns_download_runtime`：4 通过。
- `sns-album-images`：16 通过；`sns-album-videos`：14 通过；下载与渲染独立夹具编译通过。
- 所有验证使用合成数据与隔离输出，未读取真实微信账号或缓存，未执行真实媒体下载、内存扫描、云上传或付费识别。

## 当前结构与剩余工作

`src/toolkit` 已完成物理删除。ASR 应用编排位于 `src/application/transcription`，具体后端位于 `src/infrastructure/transcription`；`src/cli/toolkit.rs` 与 `src/daemon/operations/toolkit.rs` 仅保留当前公开命令/操作分组名称，不是 Toolkit 架构层。

朋友圈应用仍直接使用微信适配器导出的 `ExportSnapshot` 与兼容导出投影。后续若需要支持第二种朋友圈数据源，应把该快照提升为业务契约并通过装配注入；在只有一个本地微信数据源的当前实现中，不再为假想扩展复制模型。缓存恢复、DAT 与 SNS 密钥流仍是微信格式能力，继续留在适配器；媒体下载和文件发布仍分别受宿主授权与输出保护约束。
