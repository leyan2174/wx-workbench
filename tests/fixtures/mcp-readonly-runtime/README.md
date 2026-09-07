# MCP 新增只读工具端到端

> 2026-09-07 文档核对：入口仍为主仓 `mcp_readonly_runtime` 集成目标。下方工具数量与“本轮只新增”等说明保留首次只读阶段的历史范围；现行工具清单以生产协议及真实运行时测试断言为准，不据此声称当前只有原数量。本次没有运行测试或改变 golden。

仅合成加密 SQLite、隔离的运行目录及真实 wx 子进程；不 mock 查询、IPC 或 MCP，不访问真实账号，不运行写工具或云转录。

链路：SQLite 4096/80 页 -> AES-256-CBC/HMAC-SHA512 合成密文 -> 真实 daemon 解密/查询 -> 实际命名管道 -> 真实 wx mcp -> JSON-RPC 结果断言。明文构建库在启动前删除；源密文、配置、合成密钥及 msg 媒体树前后逐字节不变，并比较目录和文件清单。

历史首次覆盖：两个共享 runtime home 的独立账号；相同 local_id 的不同引用内容；14 工具清单；ContactTags 严格计数投影；完整 TagMembers；引用成功结构、真实 IPC exit_code=2、MCP 安全错误及时间消歧；VoiceInfo 的实际长度/NULL、时间窗口和分页；decode_image、decode_voice、transcribe_voice 三个未授权写工具全部拒绝；交错重复查询的账号隔离。

附件使用真实 msg/file 与 msg/attach/<peer-md5>/月份/Rec/card/{F,Img,A,V} 文件。断言完整消息身份、原始文件路径归属、实际字节、MD5、大小及绑定级别；两账号同身份不同字节，A 不得找到仅 B 存在的文件。覆盖 found/missing/text/metadata_only、61 项且超过 20K 的记录尾项、非 49 消息、XML 类型不符、索引越界、错误 MD5、无 hash 多候选、跨分片同身份混合类型歧义必须先于类型和索引检查、任意 base 参数拒绝、普通文件正文 20K 上限。返回媒体仅为原始本地引用，不解码或转录。

测试仅新增 mcp_readonly_runtime.rs 和本目录。现有 voice_runtime/delta_runtime 的 fixture 未公开可直接引用的 helper；本目录仅复用两段小型 SQLite 预留页尾/加密算法，不复制其整套测试。建议主线程后续将 encrypted_sqlite 的两个函数提为公共 helper，并让原测试所有者替换重复实现。

执行 `cargo test --offline --test mcp_readonly_runtime -- --nocapture`。子进程命令、JSON-RPC/IPC 收发和退出时日志均由测试输出；外层 Tee-Object 保留完整记录。此测试不宣称验证其余八工具的实际查询结果，也不替代 mcp_refer 的 uppercase/view/strict-discovery 专项。
