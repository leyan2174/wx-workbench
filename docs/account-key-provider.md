# 账号密钥

本项目支持自动复用、只读内存扫描和显式账号级捕获。它们是不同的授权路径；普通查询只使用已保存密钥，不自动启动扫描或重启微信。

## 选择来源

| 参数 | 行为 | 条件 |
| --- | --- | --- |
| `--key-provider auto` | 先尝试 DPAPI 保存的账号密钥，逐库验证；无法使用时走版本对应的只读内存扫描。 | 默认值，不隐式重启或注入。 |
| `--key-provider memory` | 跳过账号密钥记录，使用只读扫描。 | 微信进程存在且可访问；仍需明确扫描授权。 |
| `--key-provider account` | 重启微信并通过 Frida 捕获候选，派生和验证各数据库密钥。 | 必须同时明确 `--force --restart-wechat`，可能需要手机确认登录。 |

内存扫描对低于 4.1.10 和不低于 4.1.10 的版本分别使用原始密钥与 Config.Cipher 路径。这是扫描算法选择，不是自动启用 Frida 捕获。微信内部布局会改变，版本分支不能替代数据库验证。

## 配置与授权

```powershell
$account = Join-Path $env:USERPROFILE 'wx-cli-data/synthetic-account'
$env:WX_CLI_CONFIG = Join-Path $account 'config.json'
$env:WX_CLI_HOME = Join-Path $account 'runtime'
wx toolkit setup --check
```

检查路径与后端不会捕获密钥或运行识别模型。`WX_CLI_CONFIG` 选择配置文件，`--db-dir` 选择数据库目录，二者不能混淆。现有配置及其密钥文件可使未指定 `--force` 的初始化直接返回。

强制初始化不授权跨账号覆盖，也不授权丢弃未知配置字段。只有明确得到扫描或重启授权后才执行：

```powershell
# $dbStorage 已由用户确认，不能自动取另一个账号的目录。
wx init --force --db-dir $dbStorage --key-provider memory
# 此命令会关闭并重新启动微信；必须有独立的重启授权。
wx init --force --db-dir $dbStorage --key-provider account --restart-wechat
```

账号捕获先拒绝来自其他安装位置的微信进程，再关闭匹配进程并启动目标程序。`--wechat-exe` 仅用于账号 provider；`--capture-timeout` 默认 300 秒，允许 10 至 1800 秒。需要扫码或手机确认时等待用户操作，不自动点击登录，也不无限重试。

## 捕获与派生

Frida 执行内嵌的 `account_hook.js`，不需要 Node.js 或 Python。Hook 通过 SHA-512 常量及指令引用定位实现，再识别 32 字节密钥和 ipad 布局的 HMAC 输入。候选以二进制消息传回，不写日志。

候选与每个数据库的 16 字节 salt 经过 256000 次 PBKDF2-HMAC-SHA512 派生。以消息库首页 HMAC 校验候选，再逐库验证，包括 WAL 首页变体。捕获路径排除顶层 `migrate`，要求收集到的目标数据库全部覆盖才保存账号密钥。

账号捕获的目录收集与内存扫描的 `CheckedInventory` 不同，不应假定两条路径具有相同的路径固定与遍历限额。

候选密钥通过首页校验不等于整库认证通过。正式缓存解密逐页验证 HMAC，WAL 还校验连续提交前缀和每个选入帧；主库与 WAL 全部处理成功后才发布完整缓存。具体保证与限制见[数据库认证与缓存发布](architecture.md#数据库认证与缓存发布)。

## 文件与复用

`account_key.dpapi` 与当前配置位于同一目录，保存版本化的 Windows DPAPI 记录，绑定规范化数据库目录和当前 Windows 用户。替换时保留时间戳备份；临时文件和备份也应按秘密材料保护。不要分发它们或假定另一台机器、另一个用户能解密。

daemon 使用配置 `keys_file` 指向的逐数据库 JSON，缺省名为 `all_keys.json`。相对路径基于配置目录。该 JSON 含原始密钥，不是 DPAPI 密文，必须保存在私有目录。

缓存命中前仍核对当前源库首页 HMAC。密钥失效或首页损坏时返回错误，不继续复用旧缓存，也不自动扫描或重启微信；先检查目标账号与密钥条件。

初始化拒绝畸形配置、冲突输出路径及检测到的并发修改，保留未知配置字段，并写入实际配置指定的密钥路径。密钥和配置分别原子发布，先密钥后配置；这不是跨文件事务。配置发布失败时已经保存的密钥仍可能存在。账号 DPAPI 文件也不属于跨文件事务。

`auto` 可复用保存的账号密钥为新增分片派生密钥，但仍需完整验证。记录不可读、目录不匹配或验证失败时保留原记录，再进入只读扫描。账号密钥不是登录密码，也不是永久跨设备身份；迁移或换钥后的数据库必须重新验证。

## 进程生命周期

启动时清除原始标准句柄的继承，显式子进程重定向仍有效，避免长期运行的微信持有调用者输出管道。Frida 使用默认 spawn 标准流，不额外创建 piped stdio。

worker 挂起启动、加入 Job 后才恢复。只有明确的 force/account/restart 操作使用允许子进程脱离的专用 Job：worker 仍被 daemon 回收，重启后的用户应用可继续运行。该标志作用于该 worker 的全部子进程，因此捕获路径不能借它运行无关工具。普通 Job 仍回收后代。

捕获失败或取消后先确认目标进程状态，不把失败当成再次关闭或启动微信的授权。已写入文件不因取消自动回滚。

## 构建与测试

Windows 构建通过 `frida-sys` 获取 Frida devkit，并用 libclang 生成绑定。构建下载与运行时捕获是不同条件。

默认单元测试覆盖派生、错误候选、完整覆盖、DPAPI、目录绑定和备份。Frida 附加与进程生命周期的可选测试使用临时合成进程，先核对测试体与依赖，不自动重启真实微信。执行规则见[测试说明](../tests/README.md)。

实现入口为 [provider](../src/scanner/mod.rs)、[账号捕获](../src/scanner/windows/account.rs)、[初始化](../src/daemon/operations/init.rs)及[worker 生命周期](../src/daemon/tasks/process.rs)。Hook 来源与许可保留在[第三方说明](../THIRD_PARTY_NOTICES.md)。
