# SNS 相册图片独立模块交接

## 状态与注册

生产文件 `src/toolkit/sns/album_images.rs` 及其 `album_images_tests.rs` 已由 `sns/mod.rs` 注册，`sns/album.rs` 实际调用图片复用和下载入口。初次独立交付未修改其他模块，不应再将当时的“尚未接线”作为现状。

2026-09-07 文档核对：清理的是本目录生成的 `album-images-tests.exe` / `album-images-tests.pdb`；`harness.rs`、`run.ps1`、oracle 和历史日志保留。独立验收数字属于当时运行，本次没有重新执行测试。

现有注册为：

```rust
pub(crate) mod album_images;
```

不需要新增依赖；使用当前 reqwest、regex、serde_json、tempfile、same-file、zeroize、anyhow。
HTML5 实体解析已复用 `sns/decode.rs::html_unescape`，沿用 `sns/html_entities.json`，不再保留私有 unescape 副本。

## API

```rust
sns_image_url_candidates(url: &str, token: &str) -> Vec<String>
fix_sns_image_url(url: &str, token: &str) -> String
reuse_existing_image(name: &str, guard: &HostOutputGuard)
    -> Result<Option<Outcome>, ImageError>
save_image(data: &[u8], name: &str, guard: &HostOutputGuard)
    -> Result<Outcome, ImageError>
download_sns_image<'a>(url: &str, key: &str, token: &str,
    name: &str, guard: &HostOutputGuard,
    engine: impl FnMut() -> Result<&'a VideoRuntime, ImageError>)
    -> Result<Outcome, Failure>
```

`name` 为直属文件名（如 `post_1.jpg`），不是路径；真实后缀由 magic 决定。
调用者创建 images 输出目录，并给 HostOutputGuard 显式保护账号源库、缓存、配置、密钥及 runtime 输入。
模块不发现账号、不读取环境、不初始化目录或 WASM。不同账号必须使用隔离的 guard/输出目录及显式元数据。

`Outcome { filename, source, bytes }` 的 filename 仅含文件名，编排层组合 `images/{filename}`。
`source.as_str()` 为 `existing` / `remote` / `remote_decrypted`；bytes 为文件长度。
成功后编排层应清除旧 image_error；失败写入 Failure 的固定分类文本，不拼入输入或底层错误链。
`save_image` 自身返回 Remote，下载层完成解密后更正为 RemoteDecrypted。
`Failure.errors` 保留按候选顺序的每次失败分类；此前失败不会污染后续成功 Outcome。
URL helper 的返回值包含签名数据，禁止写入日志。

调用者可使用 `OnceLock<VideoRuntime>` 实现 lazy engine：

```rust
let runtime = std::sync::OnceLock::new();
let result = album_images::download_sns_image(url, key, token, name, &guard, || {
    if runtime.get().is_none() {
        let loaded = VideoRuntime::bundled(RuntimeLimits::default())
            .map_err(|_| album_images::ImageError::EngineUnavailable)?;
        let _ = runtime.set(loaded);
    }
    runtime.get().ok_or(album_images::ImageError::EngineUnavailable)
});
```

已有图、明文图以及 key 为空或 `0` 的未知格式响应均不调用 engine。
不允许解密时，可直接传 `|| Err(ImageError::EngineUnavailable)`。

## 与旧 Python 的差异和限制

- 早期 `VideoRuntime::keystream` 的 128 KiB 图片限制已解除，通用密钥流上限为 25 MiB；视频解密前缀仍为 128 KiB。修复原理和当时实测见 [KEYSTREAM-25M.md](../sns-video-native/KEYSTREAM-25M.md)，不得把早期限制或该次通过数字当作当前全仓状态。
- 图片响应上限仍为 25 MiB，明文图不受 runtime 上限影响。
- 无覆盖原子发布替代 Python 的覆盖写入；已存在但无效的同后缀文件保留，报告 Output，不删除源文件或其他调用的文件。
- 路径越界、重解析点、硬链接与保护边界失败报告 Output；不能像旧脚本一样忽略这些安全异常。
- 网络不使用代理；10 秒超时、最多 5 次重定向、仅 HTTP(S)、禁止 HTTPS 降级及 URL 用户信息，URL/原始 token 最大 64 KiB。TLS 默认验证不放宽。
- HTTP 输入在候选生成时强制升级 HTTPS；loopback HTTP 测试只调用私有 transport。
- 固定 UA `MicroMessenger Client`、Accept `*/*`、Referer `https://mp.weixin.qq.com/`，禁用自动 Referer，避免重定向带出 token。
- 错误使用固定分类，不保留旧 Python 的任意异常文本。

## 针对性验收

当前优先从仓库根运行已注册的生产模块测试：

```powershell
cargo test --bin wx toolkit::sns::album_images::tests -- --nocapture --test-threads=1
```

保留的独立 harness 入口如下；它需要匹配的已构建依赖，并会重写固定名称的编译/测试日志。保留历史证据时优先使用上面的主仓命令，另存本次输出，不直接覆盖旧日志。

```powershell
pwsh -NoProfile -File tests/fixtures/sns-album-images/run.ps1
```

脚本从本机已有 `target/debug/deps` 引用依赖，直接 rustc 编译真实生产源文件、守卫与 runtime；不改 Cargo，也不负责生产接线。生成 exe/pdb 已清理不等于 harness 被删除，重新运行会再生成测试产物。
当前 Windows 原生库路径为本机已安装的 windows_x86_64_msvc-0.52.6；换机需调整 run.ps1。
执行仅筛选 `album_images::tests`，`--test-threads=1`。
历史独立验收最终 **16 passed，0 failed，13 filtered out**；compile.log / tests.log 保留该次本机完整输出，不是本次文档核对的新结果。
覆盖 68 组旧 Python AST 提取 URL oracle、四种 magic、逐扩展头检查、真实后缀、明文 lazy、真实 WASM XOR、runtime 上限、各阶段候选 fallback、分类脱敏、无覆盖/硬链接、25 MiB 边界、loopback headers/重定向/状态/截断/无长度/10 秒超时。
oracle.py 仅提取两个纯 URL 函数，不导入供应商脚本、不读取真实微信数据。

先前已尝试全仓检查，配置现有 libclang 后 Windows cargo check 通过。
全仓测试曾在 native_migration_security 的日期参数测试因 config.json 格式错误失败；按用户最新要求不追查、不重跑，统一回归归 main。此结果不作为本模块验收依据。
