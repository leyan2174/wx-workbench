# SNS 发布测试

生产时间线和相册共用 publish 模块及 HostOutputGuard。夹具只使用 Windows 临时合成文件，不发现账号、不读取真实数据库或下载媒体。

## 调用契约

绑定信息必须来自调用方固定的账号和数据库上下文；source ID 原样比较，不构成来源认证。prepare 应接收所有候选相对文件，包括可选扩展名和 sidecar。

根文件使用空相对路径的 guard；图片和视频分别使用 images、videos guard。计划文件的父目录按需创建，相册即使没有媒体任务也准备两个空媒体目录；目录深度和子目录数量受预算限制。

下载暂存位于输出树之外。publish_all 在替换前检查所有源与目标，按调用方顺序提交。错误报告已提交文件数，之前的文件不回滚，后面的汇总文件不发布。

绑定清单是所有权声明，不是完成凭据。显式认领旧目录后，legacy_unverified 在后续更新仍保留。未知文件和子树不递归扫描、删除或宣称已验证。持久锁文件不删除，Windows 独占写句柄随释放或进程退出而解锁。

该机制协调合作写者，不提供对抗性 compare-and-swap 或跨文件崩溃事务。

## 运行

按[测试说明](../../README.md)准备依赖，从仓库根目录执行：

```powershell
cargo test --bin wx infrastructure::output_tree::tests -- --nocapture
cargo test --manifest-path tests/fixtures/sns-publish/Cargo.toml --target x86_64-pc-windows-msvc publish::tests -- --nocapture
```

合成发布测试不代替整个相册业务或实际媒体内容验证。
