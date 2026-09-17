# 结构化消息尾部边界

## 职责归属

- `transfer.rs` 负责微信 XML 遍历、字段别名、唯一的转账状态解码和严格详情校验，返回 `business::structured_message::TransferContent`。
- `summary.rs` 负责旧语音/视频时长、通话状态和名片属性提取；交给 `message::summary` 的是数字、普通文本和 typed 信息，不推断通话媒体类型。
- `location.rs` 负责位置属性、有限坐标校验、分类提取及微信 x 对应纬度、y 对应经度的映射，返回 `LocationContent`。
- `message::transfer`、`message::location` 保留详情 JSON 与展示；`message::summary` 保留阅读摘要格式，这些展示模块不解析 XML。
- `message::structured_message` 是 rich JSON 的显式展示投影，生产出口为 `daemon/query/message_read.rs`。业务枚举不直接序列化。
- `export_content.rs` 是显式 raw 导出兼容适配器，由微信消息适配器直接提供，不通过 `message` 生产 re-export。其职责包括 packed `local_type`、appmsg/表情私有字段、嵌套引用和历史 content/extras 投影，不是普通 typed 业务响应。普通导出、增量导出、目录导出、rich 预览与 reply 解码按需调用该适配器。

## 兼容边界

- 完整转账 JSON 保留全部字段、空字符串、flatten 结构、原始金额/时间文本和 `transcation_id` 拼写；业务成员为 `transaction_id`。默认标题与中文标签留在展示层。
- rich 转账业务成员改为 `status/raw_subtype/amount_text/memo`；旧 `direction/paysubtype/fee_desc/pay_memo` 只在展示投影恢复。详情与 rich 使用同一 `TransferStatus`，未知原始代码只作诊断证据，不反推业务状态。
- 字段别名顺序、首个非空别名、空白折叠和首个后代节点选择保持原算法。
- 严格转账详情仍要求 Rust 整数语法的 type 2000 和 `wcpayinfo`；缺支付节点仍允许标题摘要回退。未知状态不被猜测，业务层不推算金额或转换时间。
- raw 导出正文允许 packed 高位 subtype 回退；extras 仍要求 XML 显式 type。其 `2_000` 等十进制下划线规范化不会放宽严格详情入口。
- 紧凑 extras 保留整数规范化、零值/无效值省略和大整数精度；有效日期留在完整详情，不进入紧凑 extras。
- 旧摘要保留 XML 限制、后代节点选择及异常回退。视频时长允许非数字文本；语音仍用 u64 和一位小数。未知通话状态保留原文，畸形 voip 标记仍回退通话摘要。
- rich 预览保留独立的大小、节点、安全文本和数值限制；未知转账方向仍输出空字符串，不能代替严格详情或 raw 导出入口。

## 覆盖与未执行项

transfer、summary、location、export-content golden 测试通过真实 adapter 核对固定期望值。合成 XML 测试覆盖完整别名/JSON 等价、缺字段、未知状态与时长格式、坐标方向、名片凭据排除、raw subtype 回退，以及严格详情/raw 导出/rich 预览的差异。另有展示测试保证不会从原始诊断代码推断业务状态。

`tests/support/structured_content_adapters.rs` 为 attachment-refs 和 mcp-readonly-security fixture 注册真实模块，不访问账号、网络或微信进程。执行命令与依赖见[测试说明](../../../../tests/README.md)。
