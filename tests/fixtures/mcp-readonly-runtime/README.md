# MCP 只读工具进程测试


仅合成加密 SQLite、隔离的运行目录及真实 wx 子进程；不 mock 查询、IPC 或 MCP，不访问真实账号，不运行写工具。

链路：SQLite 4096/80 页 -> AES-256-CBC/HMAC-SHA512 合成密文 -> 真实 daemon 解密/查询 -> 实际命名管道 -> 真实 wx mcp -> JSON-RPC 结果断言。明文构建库在启动前删除；源密文、配置、合成密钥及 msg 媒体树前后逐字节不变，并比较目录和文件清单。

覆盖：两个共享 runtime home 的独立账号；相同 local_id 的不同引用内容；公开工具清单；ContactTags 严格计数投影；完整 TagMembers；引用成功结构、真实 IPC exit_code=2、MCP 安全错误及时间消歧；VoiceInfo 的实际长度/NULL、时间窗口和分页；未授权的 decode_image 写工具拒绝；交错重复查询的账号隔离。

附件使用真实 msg/file 与 msg/attach/<peer-md5>/月份/Rec/card/{F,Img,A,V} 文件。断言完整消息身份、原始文件路径归属、实际字节、MD5、大小及绑定级别；两账号同身份不同字节，A 不得找到仅 B 存在的文件。覆盖 found/missing/text/metadata_only、61 项且超过 20K 的记录尾项、非 49 消息、XML 类型不符、索引越界、错误 MD5、无 hash 多候选、跨分片同身份混合类型歧义必须先于类型和索引检查、任意 base 参数拒绝、普通文件正文 20K 上限。返回媒体仅为原始本地引用，不解码。


执行 `cargo test --offline --test mcp_readonly_runtime -- --nocapture`。子进程命令、JSON-RPC/IPC 收发和退出时日志均由测试输出；外层 Tee-Object 保留完整记录。此测试不覆盖所有工具的实际业务结果，也不替代 mcp_refer 的 uppercase/view/strict-discovery 专项。

运行环境见[测试说明](../../README.md)，日志保存于仓库外的私有目录。
