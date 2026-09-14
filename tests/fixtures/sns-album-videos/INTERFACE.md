# 相册视频媒体接口

仅支持 Windows x64 MSVC。生产 `album_videos` 由 `sns/mod.rs` 注册，`sns/album.rs` 调用。

## 接口

```rust
reuse_existing_video(name: &str, guard: &HostOutputGuard)
    -> Result<Option<Outcome>, VideoError>
copy_cached_video(source: &Path, name: &str, guard: &HostOutputGuard, allow_partial: bool)
    -> Result<Option<Outcome>, VideoError>
download_video<'a>(url: &str, key: &str, name: &str, guard: &HostOutputGuard,
    engine: impl FnMut() -> Result<&'a VideoRuntime, VideoError>)
    -> Result<Outcome, VideoError>
```

上述函数为 pub(crate)。Outcome 的 filename 是直属文件名（统一换为 .mp4），source 为 VideoSource，可 as_str() 得到 existing/cache/remote；complete 为 bool，bytes 为 u64。加密下载亦沿用旧 remote 来源值。

调用方 负责创建 videos 输出目录、建立守卫并保护显式账号数据库、缓存、配置、密钥及其他源目录。函数不读取任何默认账号目录或密钥文件。惰性 engine 初始化错误应映射为 VideoError::EngineUnavailable，不附加底层路径/密钥错误链。

复用、缓存、远程的优先级和 JSON/统计由 编排层 决定；download_video 本身不先执行复用，成功下载可原子替换旧目标。调用方 使用已有 cache::build_cache_index 和 cache::find_cached_video，传 entry.path 给复制函数；不需要新增扫描器或修改 helper 可见性。复制函数在调用时重新固定并校验源文件，不承诺验证索引时刻快照。complete 沿用源扩展名（忽略大小写 .mp4 为完整）；其他扩展名需 allow_partial=true。已有 MP4 复用沿用旧 complete=true 语义，不能从此前部分缓存的改名结果恢复历史标记，编排层 可自行保留该元数据。

## 行为和加强边界

- 视频上限 2 GiB；前缀读取循环至 128 KiB 或 EOF，仅前缀调用 keystream，不调用全量 decode。尾部以 64 KiB 缓冲区原样流式写入。明文不初始化 WASM。
- 请求头 User-Agent: MicroMessenger Client、Accept: */*；不添加 Referer；no_proxy 禁用自动环境代理。
- 30 秒网络总期限（含响应 body），不是旧 urllib 的逐次 socket 超时；WASM 使用自身 fuel/memory 限制。磁盘 I/O 不受这个网络期限控制。
- HTTP/HTTPS 允许显式 loopback，未宣称公网专属 SSRF 防护。拒绝 URL 凭据、片段、控制字符、超长 URL；重定向同样验证，最多 5 次，禁止 HTTPS 降级 HTTP。
- 仅接受 200，拒绝 206 和 Content-Range，防止片段冒充完整下载。验证声明长度及流式累计限额；有 Content-Length 或 chunked 的截断失败。无长度且以连接关闭结束的合法 HTTP 无法判断源内容是否语义截断；与旧版一致，不解析完整 MP4 容器。
- HostOutputGuard 固定本地输出祖先并在提交前复核目标。已有目标/源均通过文件句柄拒绝重解析点、硬链接及路径身份变化；缓存源祖先额外固定，禁止源与输出目录重叠。
- 同目录 NamedTempFile，成功 sync 后原子 persist 替换；失败只清理本次临时文件，不删除旧目标，不改缓存源。缓存复制保留修改时间。
- 错误为无错误链枚举，不包含 URL/token/key/路径。安全边界拒绝返回 Err，缺失/非 MP4/禁用部分缓存返回 None。

## 验证

现行生产模块可从仓库根定向运行：

```powershell
cargo test --bin wx toolkit::sns::album_videos::tests -- --nocapture --test-threads=1
```

run.ps1 委托 tests/run-module.ps1，使用根 Cargo.toml、Windows MSVC 和 wx 测试目标，筛选 album_videos::tests，单线程执行。

运行器可显式指定 TargetDir，不手工查找 rlib 或复制生产源码。

所有媒体字节和路径来自临时目录及 loopback；不读真实微信、账号钥匙或稳定安装。测试覆盖前缀短读、WASM 合成 oracle、超过 25 MiB 视频、缓存完整/部分/源不变、硬链接/别名、句柄锁定、HTTP 错误/长度/超限/chunked 截断/超时、惰性引擎、URL 和重定向边界。

2 GiB 边界通过实际常量、声明长度、稀疏文件长度和流式计数器精准验证；不在测试中下载 2 GiB。超限流式控制通过相同私有实现注入较小限额测试。


环境与输出约定见[测试说明](../../README.md)。
