# Delta / Plan 安全回归

入口为 `tests/delta_plan_security.rs`。数据全部在独占临时目录生成：SQLCipher 格式加密消息/联系人库、明文统计库、恶意 JSON 文本及 Windows junction。固定 `0x11` 密钥仅用于合成库。

不读取现有微信账号，不使用 Python，不请求网络。真实 CLI 由 `CARGO_BIN_EXE_wx` 指定；每个调用隔离配置和运行目录，结束时停止本测试账号的 daemon。

运行：`cargo test --test delta_plan_security -- --nocapture`。本目录可保存完整构建及测试日志；不存放真实用户资料。
