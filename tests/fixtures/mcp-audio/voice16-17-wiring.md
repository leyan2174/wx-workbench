# Voice 16/17 接线审阅

> 2026-09-07 文档核对：本页保留实施前设计及旧服务契约，不是当前注册清单。语音工具、`toolkit/audio/publish.rs::publish_wav_noclobber`、独立 prepared-audio IPC 预算和宿主 `Pending::try_cached` 均已有生产接线。下文“当前”“不存在”“待增”“未完成”属于当时源码快照；不要按旧建议重复接线。现行回归入口见 [语音运行时说明](../mcp-voice-runtime/README.md)、[WAV 发布说明](../wav-publish/README.md) 和 [receipt 说明](../voice-cache-receipt/README.md)。本次没有运行 oracle 或测试。

本文件是设计与已读源码契约，不代表两个工具已公开注册。生产 root、IPC、protocol、host 本轮未改。旧服务函数体由同目录 legacy_contract_test.py 离线验证，不导入旧服务。

## 必须先决定的兼容边界

- 旧 get_voice_messages/decode_voice/transcribe_voice 使用 VoiceInfo.local_id；q_prepare_voice 通过 resolve_legacy_audio 保持媒体 ID 语义：先唯一媒体候选，再按 server_id 反查唯一消息，最后由共享 join_voice 复核。歧义不得猜首条。chat 采用唯一精确名称解析，不做模糊名称猜测。旧消息 ID 适配入口已删除；显式消息身份解析仍由共享 database_media::resolve_voice_sources 提供。
- 旧成功结果是纯文本，不是 JSON。新 PreparedAudio 只是本地 IPC 中间结果，不得透传为公开 decode_voice 成功。
- 旧转录缓存命中可跳过数据库，源消息删除后仍成功；新 cached 字节入口必须取得已验证 SILK，不支持这条旧行为。需要保留此特性时另做显式账号+source+摘要索引，不能退回 username/local_id 弱键。

## 旧精确返回契约

来源 vendor/wechat-decrypt/mcp_server.py:3538、3555、3651、3707、4005。

decode_voice(chat_name: str, local_id: int) -> str：

```text
解码成功!
  文件: {out_path}
  时长: {pcm_len / 48000:.1f}秒
  大小: {WAV文件字节数:,} bytes
```

无末尾换行；缺依赖优先返回“缺少依赖: pip install silk-python”，随后才解析联系人。联系人不存在返回“找不到聊天对象: {chat_name}”；媒体无记录返回“找不到 local_id={local_id} 的语音消息”。SQL、解码、写出异常未被此函数捕获，不能伪装成成功文本。旧文件名是 username_本地时间YYYYMMDD_HHMMSS_local_id.wav，目录为 DECODED_VOICE_DIR；会覆盖同名文件，不原子发布。

transcribe_voice(chat_name: str, local_id: int) -> str：成功及缓存命中均为 `[{本地时间YYYY-MM-DD HH:MM}] ({language})\n{text}`，空文本也是成功。旧缓存缺时间使用 `-`，缺 language 使用 `unknown`，缺 backend 视为 local；命中只要求 text 存在、backend 和 model_size 匹配，不访问数据库/解码器。未命中先检查 local whisper，再检查 pysilk，再查媒体。对应缺依赖字符串分别为“缺少依赖: pip install openai-whisper”和“缺少依赖: pip install silk-python”。联系人/媒体不存在文本同上。

未命中在转录前写出永久 WAV；仅捕获转录调用的 RuntimeError 并返回 str(error)，所以转录失败可能留下 WAV。其他异常可传播。成功后缓存 text/language/create_time/backend/model_size，写缓存失败仅 stderr 警告，成功文本不变。空文本也缓存。新实现不应继承永久残留 WAV、弱缓存身份、自动下载/后端回退或不安全错误原文。

后端 RuntimeError 的旧文本分支（读取源码确认，不执行后端）：

- OpenAI 超限：`音频 {size / 1024 / 1024:.1f}MB 超过 OpenAI 25MB 上限，提前拒绝以避免无谓上传`；缺包：`缺少依赖: pip install openai`。
- OpenAI 认证/限流：`OpenAI 鉴权失败 (401)：检查 openai_api_key`、`OpenAI 限流 (429)：稍后重试`；其他 API 异常：`OpenAI API 错误: {e}`。
- whisper.cpp 缺程序：`whisper-cpp binary 未找到，请配置 Windows whisper_cpp_binary`。
- whisper.cpp 缺模型：`whisper.cpp 模型未找到。通过 config.json whisper_cpp_model 指定路径，或下载: https://huggingface.co/ggerganov/whisper.cpp`。
- whisper.cpp 超时：`whisper-cpp 超时 (120s)`；其他异常：`whisper-cpp 转录失败: {e}`。旧程序未检查子进程退出码，缺结果文件仍返回成功空文本；原生流程不应继承这一缺陷。
- 旧 local 后端默认 base，可能自动下载；openai 配置缺 key、whisper_cpp 找不到程序会回退 local。原生应采用显式后端并失败关闭，不照搬回退逻辑或错误中的潜在敏感底层原文。

## 已有可复用 API

| 文件 | 入口 | 用途/限制 |
| --- | --- | --- |
| src/daemon/query/mcp_audio.rs | q_prepare_voice(db,names,chat,media_local_id,Limits) | 内部媒体 ID 准备入口，返回 prepared_audio 包装的有界 JSON，无音频输出 |
| src/daemon/query/mcp_audio.rs | resolve_legacy_audio(db,names,chat,media_local_id) | 旧媒体 ID 入口，Found/两类显式歧义；由 q_prepare_voice 编码并清理错误链 |
| src/toolkit/asr/database_media.rs | resolve_voice_media_id(sources,username,media_local_id) | 明确账号清单下反向候选证明；错误kind区分媒体/消息歧义，最终共用join_voice |
| src/toolkit/asr/prepared_audio.rs | decode(payload,Limits) | 验证完整 JSON、base64、size、SHA256、证据、SILK 头；不是完整 SILK 帧解码，也不是签名认证 |
| src/toolkit/asr/mod.rs | prepare_wav_bytes(&silk) -> Result<Vec<u8>> | 内存 WAV，固定 SILK 输出 24kHz/单声道/PCM16；WAV 上限32MiB |
| src/toolkit/audio/mod.rs | normalize_silk / decode_silk_to_pcm | SILK16MiB、6000包、单包1..1024字节；先检帧再SDK解码 |
| src/toolkit/asr/cached.rs | transcribe_cached(&CachedRequest,&Backend) | 成功缓存薄适配，命中不重解码，缓存失效则复用字节转录 |
| src/toolkit/asr/mod.rs | transcribe_audio_bytes(&silk,&Backend) | 无缓存路径；本地临时 WAV 自动清理，云端必须显式允许 |
| src/attachment/native_image.rs | HostOutputGuard::new/protect | Windows 路径/祖先固定与目录隔离；不是 WAV 发布函数，protect 最多128项 |

不存在现成公共 WAV 发布 API。convert_silk_to_mp3 不是替代品：它走 ffmpeg、输出 MP3 并允许原子覆盖。图片 export_image 含图片专用查询，不应拿音频调用。

## Decode Voice 建议流程

1. 宿主从显式 policy 取得独立、已存在的 output_root，固定账号及输出路径；未配置时在数据库访问前拒绝。请求 schema 不接受任意根、密钥或后端。保护账号源/解密缓存/config/keys/模型等目录，别把数千个分片逐项塞进128项保护表。
2. 调用内部准备 IPC，宿主用 prepared_audio::decode 校验；检查账号仍是 pinned account、请求 local_id 和非零 create_time 与 evidence 匹配。checksum 只校验音频完整性，不认证来源。显示名请求不能直接与 evidence.username 字符串比较。
3. spawn_blocking 执行 prepare_wav_bytes；完整 WAV 已在内存且长度/头通过后，才开始发布。时长采用 `(wav.len()-44)/48000.0`，仅适用于本入口生成的固定44字节头，不能泛用于外部 WAV。
4. 待增的发布 API 建议放 src/toolkit/audio/publish.rs：publish_wav_noclobber(wav, output_root, filename, guard) -> Result<PublishedWav {path,size,pcm_bytes}>。guard 必须真实持有路径保护并在发布前复核，不能仅是未使用的参数。文件名使用安全固定格式/摘要，不直接拼 username。
5. 同目录 NamedTempFile -> write_all -> sync_all -> 最后账号/取消/输出保护检查 -> persist_noclobber。既有文件/目录/链接一律不覆盖。发布前预构建并限制成功响应字节数，避免输出成功后才发现响应超限。发布后不安排可失败的 metadata 查询；size 用内存长度。公开成功文本沿用上方模板，但路径和命名规则明确为新安全契约。

失败语义：准备/解码/限额/路径/取消检查失败时无最终 WAV；临时文件 RAII 尽力清理，既有目标不变。发布完成后若进程崩溃、通道断开或取消，文件可能已存在但用户未收到成功，不能承诺跨文件系统与IPC原子事务；不在错误路径盲删它。超时不能仅丢弃迟到回复却让后台继续发布，提交前须校验 context；提交后的不可撤销点要明确。

## Transcribe Voice 与 voice_cache

当前宿主 src/cli/mcp.rs 无 voice_cache 字段。建议新增宿主策略中的显式可选 cache_path 和已构建 Backend；不从 MCP args 接收任意路径/凭据。cache_path 父目录须已存在、独立可信，账号命名空间使用 pinned RuntimeContext.id。不能共享旧全局 voice_transcriptions.json 或直接导入未分账号旧缓存。

宿主先验证后端选择/授权和配置，再访问音频及缓存；云端授权失败不得因缓存命中绕过。BackendArgs::build 可供已有CLI构造参考，不在MCP工具里猜默认模型、读环境密钥或自动回退。new host 策略构造属于 main 后续工作。

准备 IPC -> prepared_audio::decode -> 证据/账号核对 -> spawn_blocking(cached::transcribe_cached)，请求填 cache_path/account/username/message_source/message_local_id/create_time/silk。无cache_path则调用 transcribe_audio_bytes。不要先调用 decode_voice 或持久写 WAV；本地引擎已有受控临时 WAV 路径。最终返回上面的旧文本模板，内部可以保留 backend/cache_state/evidence，但不伪装成旧 JSON 契约。

原生缓存身份包括账号命名空间、username、source、message_local_id、原始音频SHA256、配置摘要。配置摘要含后端/模型/语言/选项/时间；本地包含可执行文件与模型内容摘要，云端含端点但不含凭据。缓存文件总限额64MiB。CacheState 为 hit/stored/already_present/read_unavailable/write_unavailable。

成功空文本照样缓存；识别失败不缓存。缓存损坏、账号不匹配、写锁/写入失败不会改成成功缓存：cached 仍可完成识别，并返回 read_unavailable/write_unavailable；原文件保留。缓存采用协作锁+同目录原子发布，不宣称防非协作写者的CAS。缓存访问前必须将目录与音频源/账号库/配置/后端文件隔离，不能仅依赖缓存格式检查防止写到别的输入。

## 限额与接线验收

- SILK硬上限16MiB；base64完整上限22,369,624字节，仅base64尚未计证据/JSON/envelope。音频桥 caller Limits 分别控制音频和准备JSON。
- 当前 src/cli/mcp.rs 将 max_frame_bytes 同时用于 MCP 与 IPC响应；默认1MiB，CLI最高16MiB。因此不能在当前共享预算下支持全部16MiB音频。首批保持现限额并提前拒绝；若要求全量支持，main 显式增加独立内部IPC预算，不让base64进入最终MCP文本。不得静默取消上限。
- 完整SILK检查还有限6000包；WAV最终32MiB，非空偶数字节PCM。宿主转录结果同样须服从MCP响应预算；超长结果可以在识别后拒绝交付且缓存可能已存成功，不能把缓存成功误报为未发生副作用。
- 测试：消息id/媒体id不等、重复时间与source歧义、损坏准备响应零输出、现有目标不覆盖、路径别名与重解析、取消/超时提交边界、磁盘写失败、缓存只读/损坏/账号不符、成功空文本、缓存命中不解码、云端未授权零访问、最终响应超限与重试语义。
- 本轮只验证旧文本/缓存契约并提供设计，不声称已验证真实模型识别、发布事务或16/17公开MCP接线。
