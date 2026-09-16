# MCP 语音查询安全回归

本夹具引用真实语音列表与数据库关联模块，DbCache 只提供合成分片路径和接口形状。测试不扫描账号、不读取密钥，也不使用真实录音。

## 必须拒绝的输入

- Name2Id、VoiceInfo 或证据表不是普通 rowid 表，或含 rowid、_rowid_、oid 同名用户列，包括生成列。仅改用另一个 SQLite 别名不足以防止遮蔽。
- 大小写变体分片隐藏重复媒体、规范来源重名、同一物理文件的硬链接别名。
- 完整账号清单中的缺片、额外未知片、缓存返回空路径或损坏库。
- 消息与媒体证据不完整、时间冲突或同一身份存在多候选。

用户列遮蔽可能让联系人编号错误关联到另一个联系人；忽略大写分片则可能将重复记录当成唯一匹配。回归要求明确模式或歧义错误，并检查源文件字节不变，不能以空列表代替拒绝。

## 正向语义

联系人使用本媒体库 Name2Id.rowid 与 VoiceInfo.chat_name_id 关联；跨库编号不通用。精确 username 比较不能被 NOCASE 排序规则放宽。

列表按时间降序、source 升序、local_id 降序、rowid 降序稳定分页。时间边界两端包含，零与负时间不能被当作未设置；NULL 音频长度保持未知，空 BLOB 长度为零。

ASR 使用 message.server_id 与 media.svr_id 关联，媒体 local_id 不等于消息 local_id。只有旧列表字段的模式可用于列表，但不足以证明 ASR 关联。离线入口依赖调用方提供完整快照，无法发现根目录或清单之外被省略的分片。

## 运行

按[测试说明](../../README.md)准备依赖，从仓库根目录执行：

```powershell
cargo test --offline --manifest-path tests/fixtures/mcp-voice-security/Cargo.toml --target x86_64-pc-windows-msvc -- --nocapture
```

完整证据与限制见[数据库媒体契约](../../../src/adapters/wechat/media/VOICE_DATABASE.md)。合成模式测试不代表覆盖所有微信版本。
