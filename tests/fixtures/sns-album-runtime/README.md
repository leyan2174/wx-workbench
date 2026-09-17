# SNS Album 独立进程回归


测试入口为 `tests/sns_album_runtime.rs`。所有 contact/SNS SQLite、SQLCipher 加密页、saved keys、配置和媒体字节均由测试在唯一临时目录内合成，不读取真实微信资料或稳定安装。

运行指定目标：

```powershell
cargo test --target x86_64-pc-windows-msvc --test sns_album_runtime -- --nocapture
cargo check --target x86_64-pc-windows-msvc
```

## 接线契约

- CLI 使用位置参数 `sns-album USER`，只接受 `--output`；已移除的 `--output-root` 明确拒绝。`--output-dir` 指定可复用目录，和显式 `--output` 互斥。
- `--image-workers`、`--video-workers` 接受 0 并沿用旧夹值；`-n 0` 返回空 timeline。
- stdout 为 summary 对象，必须等于 `export_summary.json`。`timeline.json` 是帖子数组，不是带 posts 字段的对象。
- 逐项断言旧 Python summary 的全部 16 个计数字段；`image_cache` 始终为 0。
- 已有图片 `images/00001_30_01.png` 和视频 `videos/00001_30_02.mp4`、`videos/00001_30_03.mp4` 按旧脚本命名；无 binding 的旧目录默认拒绝，旧输出树必须不变且不得联网。只允许 publisher 新增指定的空同步锁 `.wx-sns-publish.lock`，不允许生成 binding 或业务元数据。显式 `--adopt-existing` 后才能复用，必须保留原始字节并返回 `legacy_unverified=true`。
- 空的 `--output-dir` 无需认领即可写入 `_source_binding.json`；同账号同联系人自动更新无需 adopt。已认领旧目录的来源警告在自动更新后必须保留。
- 两个合成加密账号使用相同联系人名字；另一账号或同账号另一联系人均不能通过 adopt 覆盖已知绑定。拒绝前后逐字节比较整个输出树，包括 binding、timeline 和 summary。
- `--no-videos` 即使有已存在视频也不处理，只累计 skipped；HTML 不应引用被跳过的视频。
- 明文视频由 loopback 临时 HTTP 服务提供，图片只用于离线或 existing 分支，不降低 TLS 校验。非阻塞 listener 接受 socket 后显式改为阻塞模式。
- 子进程使用空 PATH、不存在的 Python 路径和唯一 `WX_CLI_HOME`；A/B/A 使用同一 runtime 根，检查账号隔离及源数据库、config、keys 字节不变。
- Drop 只向本 fixture 记录的账号配置发出 daemon stop。

## 边界

本测试使用真实 CLI 生成 binding，不生成伪造绑定文件。完整/部分视频缓存优先级应由媒体核心测试覆盖；本文件验证 existing 优先、实际 HTTP 下载、no-remote 和 no-videos 的跨进程行为。不把 legacy 失败转换为跳过。小型 loopback 视频必须实际下载成功，不声称通过小样本验证了 2 GiB 上限或 128 KiB 解密前缀边界。

外部 `sns-feed --json` 仍断言帖子数组，内部 Feed metadata 不改变此输出契约。该目标为定向回归，测试范围不等于全仓覆盖。

## 加密视频与缓存

- `loopback_oracle_encrypted_video_decrypts_prefix_and_preserves_tail` 只读复用 `sns-video-native/vectors.json` 的公开 key=1、size=131072 固定 oracle。该向量来源于 Node/WASM 包装器输出；测试不执行 Node、Python，也不调用生产 Rust runtime 来制造预期值。
- 合成 MP4 字节只在前 128 KiB 与 oracle 异或，32779 字节的非零尾部保持明文；loopback 分段发送完整密文，真实 CLI 从加密 SNS 数据库解析 `<enc key="1"/>` 后下载。断言最终文件完整字节、前缀、原始 tail，以及 timeline 的 key/local_file/source/complete/bytes、HTML 引用和全部 summary 计数。
- 继承现有空 PATH 和不存在的 Python 路径，Node 也无法通过 PATH 查找；不降低 TLS 校验，不运行外部生成器。
- `complete_and_partial_cache_keep_mtime_in_final_album` 使用真实缓存命名布局，分别验证完整及部分缓存经 staging/publisher 后的最终文件 mtime，并检查源字节和源 mtime 不变、no-remote 零连接。

环境、输出和人工审核规则见[测试说明](../../README.md)。
