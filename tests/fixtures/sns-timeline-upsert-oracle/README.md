# SNS 时间线更新参考测试

该 fixture 保存迁移时由历史 wechat-decrypt SNS 导出流程生成的合成结果。当前回归只消费固定输入与 golden，不再执行或携带 Python oracle。

## 运行

从仓库根目录使用 Python 标准库运行：

Rust 核心定向入口：

```powershell
cargo test --bin wx application::moments::tests -- --nocapture
```

环境与输出要求见[测试说明](../../README.md)。生成或更新 golden 是单独操作，不应为了得到通过结果覆盖预期文件。

## 隔离方式

数据库与输出路径显式放在 TemporaryDirectory。缓存替身只提供合成 PNG，下载替身只记录失败，不调用网络。测试只覆盖普通 XML，不验证 zstd 或真实缓存匹配、解密。

UTC+08:00 与固定导出时刻只属于测试时钟；生产参考实现仍使用宿主本地时间。input.json 是公开虚构输入，golden.json 保存第二次运行的完整结果、文件清单、HTML 投影和身份/重名预期。

## 更新规则

- 再次导出会替换同名帖子、timeline.json 和 timeline.html；不把源中已经删除的帖子合并回当前时间线。
- 旧的额外帖子、媒体和无关文件保持原字节及修改时间。因此扫描全部零散 JSON 可能比读取 timeline.json 得到更多帖子。
- 本次没有缓存命中且下载失败时，不根据磁盘残留图片补造新 HTML 引用。
- 空源行不改输出，也不构建缓存索引；联系人和评论读取仍按参考顺序发生。
- summary.user_name 是数据库分组身份，post.db_user_name 是数据库作者，post.username 是 XML 作者。空数据库作者归 unknown，不能用显示目录或 XML 昵称证明账号归属。
- 同秒文件后缀每次从本轮输入重新分配，不是稳定帖子 ID；宽度至少三位，不能把名称固定为17位。
- 参考导出是多文件非事务写入，文件保留与当前汇总是不同概念。

## 原生发布

Rust 导出使用媒体和发布接口，绑定目录的认领、未知文件保留及部分提交见[SNS 发布说明](../sns-publish/README.md)。显式旧目录认领保留未验证来源标记，不能借参考结果宣称认证了旧目录来源。

原生恢复媒体可能位于 images/videos 并附 local_file，与参考平铺媒体不同。发布计划须包含实际目标与父目录，不为恢复媒体而静默合并旧帖子集合。

HTML 只作结构检查，不是完整样式 golden。真实网络、文件系统竞态、账号内容和播放器效果不属于此 oracle 的覆盖。
