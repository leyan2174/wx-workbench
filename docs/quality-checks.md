# 分阶段质量检查

入口为 `scripts/check-quality.ps1`，使用 PowerShell 7（`pwsh`），默认 `Check`。适用于开发中按需检查，避免每轮都执行完整测试矩阵。不引入依赖、测试缓存或猜测性跳过；仅复用 Cargo 自身的增量产物。

```powershell
pwsh -NoProfile -File scripts/check-quality.ps1
pwsh -NoProfile -File scripts/check-quality.ps1 -Stage Check -Offline
pwsh -NoProfile -File scripts/check-quality.ps1 -Stage Test
pwsh -NoProfile -File scripts/check-quality.ps1 -Stage Fixtures
pwsh -NoProfile -File scripts/check-quality.ps1 -Stage All -Offline
```

| 阶段 | 实际执行 |
| --- | --- |
| Check（默认） | 根包 `cargo check --all-targets`，随后 `cargo clippy --all-targets -- -D warnings` |
| Test | 根包 `cargo test --all-targets --no-fail-fast -- --test-threads=1`，串行运行各目标内的测试，目标失败后继续执行其余目标 |
| Fixtures | 调用现有 `check-fixtures.ps1`；它执行夹具编译检查，不代表测试已运行 |
| All | `cargo fmt --all -- --check`、Check、Test、Fixtures，依次执行 |

根包 check/clippy/test 均带 `--locked --target x86_64-pc-windows-msvc`；Offline 为这些命令添加 `--offline`，并传给夹具脚本。fmt 不接受也不需要该参数。任一步失败后继续收集后续步骤结果，总退出码为 1；全部步骤成功为 0。启动、归属或日志错误同样不能返回成功。Check 通过不能称为测试通过；All 也不额外运行夹具测试或 doctest。

## Target 归属

默认使用当前 checkout 的 `target`，尊重显式设置的独立 `CARGO_TARGET_DIR`；相对路径按仓库根目录解释。所有子进程收到归一化后的绝对路径。不跨 checkout 共用 target。

```powershell
$env:CARGO_TARGET_DIR = 'C:/build/wx-workbench-dedicated'
pwsh -NoProfile -File scripts/check-quality.ps1 -Stage Check -Offline
```

归属规则与 `check-fixtures.ps1` 一致：`.checkout-owner` 必须指向当前仓库；外置非空且没有 owner 的目录拒绝使用；允许新建专属目录或沿用 checkout 内目录。不要通过伪造 owner 绕过隔离。脚本不清理已有产物、不修改源码、不提交或推送。

## 日志与验收边界

每轮写入 `<target>/quality-checks/<UTC时间>-<随机ID>/`，不会覆盖此前轮次。各步骤合并保存完整输出；屏幕只显示命令和每步骤最多 4000 UTF-8 字节摘要（含至多约 1800 字节输出尾部）。`summary.json` 记录实际命令、起止时间、结果和原始退出码；总退出码只表示成功或失败。完整输出先落盘再读取摘要，不会为了截断而提前终止命令。

夹具脚本仍写其原有 `quality-fixtures` 目录，本入口在该步骤结束后将其完整归档到本轮 `fixture-logs`；夹具实际 Cargo 命令在 `fixtures.log` 中，逐夹具退出码在归档的 `summary.json` 中。起止时间记录到阶段级。夹具历史目录可能含本轮没有重写的旧日志，应以本轮夹具 summary 为准。本入口用 target 内独占文件锁阻止自身并发运行；不要同时单独运行夹具脚本或在同一 target 中启动其他构建。

运行前后记录 `git diff --binary HEAD` 与 `git status --porcelain=v1 --untracked-files=all`，并对 `git ls-files --others --exclude-standard -z -- '*.rs'` 列出的未跟踪 Rust 源码逐文件计算 SHA-256，保存路径和内容哈希到本轮 `before/after-untracked-hashes.json`。比较这些快照的 SHA-256，即使未跟踪 `.rs` 路径和状态不变，内容变化也会被检测。发现差异时，结果标记 `changed-intermediate-state`，只针对运行期间中间状态；无法取得快照则标记 `unknown` 并失败。

即使 `no-change-observed` 也不宣称稳定验收：前后快照无法发现改动后又恢复的内容，也不能发现忽略文件或未跟踪非 Rust 文件的纯内容变化。稳定验收须由协调者在源码停止变化后统一执行；本脚本不构造复杂内容缓存，也不将编译检查算作测试。
