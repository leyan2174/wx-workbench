# SNS 相册图片测试

## 模块

生产 `album_images.rs` 由 `sns/mod.rs` 注册，`sns/album.rs` 调用图片复用与下载入口。

现有注册为：

```rust
pub(crate) mod album_images;
```

依赖： reqwest、regex、serde_json、tempfile、same-file、zeroize、anyhow。
HTML5 实体解析复用 `adapters/wechat/moments/decode.rs::html_unescape`，实体表位于同目录 `html_entities.json`。

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
    engine: impl FnMut() -> Result<&'a SnsKeystream, ImageError>)
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

调用者可使用 `OnceLock<SnsKeystream>` 实现 lazy engine：

```rust
let runtime = std::sync::OnceLock::new();
let result = album_images::download_sns_image(url, key, token, name, &guard, || {
    if runtime.get().is_none() {
        let loaded = SnsKeystream::bundled(RuntimeLimits::default())
            .map_err(|_| album_images::ImageError::EngineUnavailable)?;
        let _ = runtime.set(loaded);
    }
    runtime.get().ok_or(album_images::ImageError::EngineUnavailable)
});
```

已有图、明文图以及 key 为空或 `0` 的未知格式响应均不调用 engine。
不允许解密时，可直接传 `|| Err(ImageError::EngineUnavailable)`。

## 与旧 Python 的差异和限制

- 通用密钥流上限为 25 MiB，视频解码前缀仍为 128 KiB；预算见[WASM 契约](../../../src/adapters/wechat/media/SNS_KEYSTREAM.md)。
- 图片响应上限仍为 25 MiB，明文图不受 runtime 上限影响。
- 无覆盖原子发布替代 Python 的覆盖写入；已存在但无效的同后缀文件保留，报告 Output，不删除源文件或其他调用的文件。
- 路径越界、重解析点、硬链接与保护边界失败报告 Output；不能像旧脚本一样忽略这些安全异常。
- 网络不使用代理；10 秒超时、最多 5 次重定向、仅 HTTP(S)、禁止 HTTPS 降级及 URL 用户信息，URL/原始 token 最大 64 KiB。TLS 默认验证不放宽。
- HTTP 输入在候选生成时强制升级 HTTPS；loopback HTTP 测试只调用私有 transport。
- 固定 UA `MicroMessenger Client`、Accept `*/*`、Referer `https://mp.weixin.qq.com/`，禁用自动 Referer，避免重定向带出 token。
- 错误使用固定分类，不保留旧 Python 的任意异常文本。

## 针对性验收

从仓库根运行生产模块测试：

```powershell
cargo test --bin wx application::moments::album_images::tests -- --nocapture --test-threads=1
```

测试覆盖 URL 候选 oracle、图片 magic、真实后缀、延迟初始化、WASM XOR、候选回退、错误脱敏、不覆盖发布、硬链接及响应大小限制。本机回环用例检查请求头、重定向、状态、截断、无长度响应与超时。

`oracle.json` 保存固定的纯 URL 合成结果，普通回归只读取该文件，不执行 Python。run.ps1 委托 tests/run-module.ps1，使用根 Cargo.toml 和 wx 测试目标，可显式指定 TargetDir，无需手工选择 rlib。环境与跳过规则见[测试说明](../../README.md)。
