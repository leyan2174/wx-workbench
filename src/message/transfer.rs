//! 微信转账消息的结构化字段与中文展示；展示金额保持原值，不做金额推算。

use anyhow::{ensure, Context, Result};
use chrono::{Datelike, Local, TimeZone};
use roxmltree::{Document, Node};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Transfer {
    pub title: String,
    pub description: String,
    #[serde(flatten)]
    pub info: TransferInfo,
}

#[derive(Debug, Serialize)]
pub struct TransferInfo {
    pub paysubtype: String,
    pub paysubtype_label: String,
    pub fee_desc: String,
    pub pay_memo: String,
    pub transcation_id: String,
    pub transfer_id: String,
    pub pay_msg_id: String,
    pub begin_transfer_time: String,
    pub invalid_time: String,
    pub effective_date: String,
    pub payer_username: String,
    pub receiver_username: String,
}

fn text(node: Node<'_, '_>, tags: &[&str]) -> String {
    for tag in tags {
        if let Some(child) = node.children().find(|child| child.has_tag_name(*tag)) {
            let value = child
                .text()
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if !value.is_empty() {
                return value;
            }
        }
    }
    String::new()
}

impl Transfer {
    /// 旧聊天导出采用紧凑字段集，空值不输出，金额不转换为浮点数。
    pub fn export_fields(&self) -> Option<serde_json::Map<String, serde_json::Value>> {
        let info = &self.info;
        let mut out = serde_json::Map::new();
        for (key, value) in [
            ("direction", &info.paysubtype_label),
            ("paysubtype", &info.paysubtype),
            ("fee_desc", &info.fee_desc),
            ("pay_memo", &info.pay_memo),
            ("payer_username", &info.payer_username),
            ("receiver_username", &info.receiver_username),
            ("transfer_id", &info.transfer_id),
            ("transcation_id", &info.transcation_id),
            ("pay_msg_id", &info.pay_msg_id),
        ] {
            if !value.is_empty() {
                out.insert(key.into(), value.clone().into());
            }
        }
        for (key, value) in [
            ("begin_transfer_time", &info.begin_transfer_time),
            ("invalid_time", &info.invalid_time),
        ] {
            if let Some(number) = export_integer(value) {
                out.insert(key.into(), number.into());
            }
        }
        (!out.is_empty()).then_some(out)
    }
}

fn export_integer(value: &str) -> Option<serde_json::Number> {
    let value = value.trim();
    let (negative, digits) = if let Some(rest) = value.strip_prefix('-') {
        (true, rest)
    } else {
        (false, value.strip_prefix('+').unwrap_or(value))
    };
    // 接受 Python int 常用的十进制下划线形式，但不接受连续/首尾下划线。
    if digits.is_empty()
        || digits
            .split('_')
            .any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let digits = digits.replace('_', "");
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return None;
    }
    let normalized = format!("{}{digits}", if negative { "-" } else { "" });
    normalized.parse().ok()
}

fn extract(appmsg: Node<'_, '_>) -> Option<TransferInfo> {
    let info = appmsg
        .children()
        .find(|child| child.has_tag_name("wcpayinfo"))?;
    let subtype = text(info, &["paysubtype"]);
    let label = match subtype.as_str() {
        "1" => "发起转账".into(),
        "3" => "已收款".into(),
        "4" => "已退还".into(),
        "5" => "过期已退还".into(),
        "7" => "待领取".into(),
        "8" => "已领取".into(),
        "" => String::new(),
        _ => format!("未知(paysubtype={subtype})"),
    };
    Some(TransferInfo {
        paysubtype: subtype,
        paysubtype_label: label,
        fee_desc: text(info, &["feedesc", "feeDesc"]),
        pay_memo: text(info, &["pay_memo", "paymemo"]),
        // 微信原字段就是 transcationid，不能擅自改成 transactionid 而漏读旧数据。
        transcation_id: text(info, &["transcationid", "transcationId"]),
        transfer_id: text(info, &["transferid", "transferId"]),
        pay_msg_id: text(info, &["paymsgid", "payMsgId"]),
        begin_transfer_time: text(info, &["begintransfertime", "beginTransferTime"]),
        invalid_time: text(info, &["invalidtime", "invalidTime"]),
        effective_date: text(info, &["effectivedate", "effectiveDate"]),
        payer_username: text(info, &["payer_username", "payerUsername"]),
        receiver_username: text(info, &["receiver_username", "receiverUsername"]),
    })
}

pub fn parse(xml: &str) -> Result<Transfer> {
    let doc = Document::parse(xml.trim()).context("无法解析消息 XML")?;
    let appmsg = doc
        .descendants()
        .find(|node| node.has_tag_name("appmsg"))
        .context("消息中没有 appmsg 段（不像转账）")?;
    let kind = text(appmsg, &["type"]).parse::<i64>().unwrap_or(0);
    ensure!(
        kind == 2000,
        "不是转账消息（appmsg type={kind}）。转账要求 appmsg type=2000"
    );
    let info = extract(appmsg).context("消息是 type=2000 但缺 <wcpayinfo> 节点（schema 异常）")?;
    let title = text(appmsg, &["title"]);
    Ok(Transfer {
        title: if title.is_empty() {
            "微信转账".into()
        } else {
            title
        },
        description: text(appmsg, &["des"]),
        info,
    })
}

pub fn summary(appmsg: Node<'_, '_>, title: &str) -> String {
    let Some(info) = extract(appmsg) else {
        return if title.is_empty() {
            "[转账]".into()
        } else {
            format!("[转账] {title}")
        };
    };
    let mut parts = vec![if info.paysubtype_label.is_empty() {
        "[转账]".into()
    } else {
        format!("[转账·{}]", info.paysubtype_label)
    }];
    if !info.fee_desc.is_empty() {
        parts.push(info.fee_desc);
    }
    if !info.pay_memo.is_empty() {
        parts.push(format!("备注: {}", info.pay_memo));
    }
    parts.join(" ")
}

fn timestamp(raw: &str) -> String {
    let digits = raw
        .strip_prefix('+')
        .or_else(|| raw.strip_prefix('-'))
        .unwrap_or(raw);
    let value = match raw.parse::<i64>() {
        Ok(value) => value,
        Err(_) if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) => {
            return format!("(无效 ts={raw})")
        }
        Err(_) => return String::new(),
    };
    if value == 0 {
        return String::new();
    }
    Local
        .timestamp_opt(value, 0)
        .single()
        .filter(|date| (1..=9999).contains(&date.year()))
        .map(|date| date.format("%Y-%m-%dT%H:%M:%S").to_string())
        .unwrap_or_else(|| format!("(无效 ts={raw})"))
}

impl Transfer {
    pub fn render(&self) -> String {
        let info = &self.info;
        let mut lines = vec![format!("转账消息: {}", self.title)];
        if !self.description.is_empty() {
            lines.push(format!("  描述: {}", self.description));
        }
        lines.push(format!(
            "  方向: {} (paysubtype={})",
            if info.paysubtype_label.is_empty() {
                "(未知)"
            } else {
                &info.paysubtype_label
            },
            if info.paysubtype.is_empty() {
                "?"
            } else {
                &info.paysubtype
            }
        ));
        for (label, value) in [
            ("金额", info.fee_desc.clone()),
            ("备注", info.pay_memo.clone()),
            ("付款方 wxid", info.payer_username.clone()),
            ("收款方 wxid", info.receiver_username.clone()),
            ("发起时间", timestamp(&info.begin_transfer_time)),
            ("失效时间", timestamp(&info.invalid_time)),
            ("转账 ID", info.transfer_id.clone()),
            ("支付交易号", info.transcation_id.clone()),
            ("paymsgid", info.pay_msg_id.clone()),
        ] {
            if !value.is_empty() {
                lines.push(format!("  {label}: {value}"));
            }
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn export_fields_match_legacy_fixtures() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/transfer-golden.json"))
                .unwrap();
        for case in cases.as_array().unwrap() {
            let actual = super::parse(case["xml"].as_str().unwrap())
                .ok()
                .and_then(|transfer| transfer.export_fields());
            assert_eq!(
                serde_json::to_value(actual).unwrap(),
                case["extras"],
                "{case}"
            );
        }
    }

    #[test]
    fn export_integers_preserve_precision_and_reject_malformed_values() {
        for (raw, expected) in [
            ("+001_234", "1234"),
            ("-0005", "-5"),
            (
                "999999999999999999999999999999999999999999999",
                "999999999999999999999999999999999999999999999",
            ),
        ] {
            assert_eq!(super::export_integer(raw).unwrap().to_string(), expected);
        }
        for raw in ["", "0", "-0", "_1", "1_", "1__2", "1.5", "+", "--1", "1e5"] {
            assert!(super::export_integer(raw).is_none(), "{raw}");
        }
    }
    use super::*;

    #[test]
    fn aliases_status_and_missing_amount_are_preserved() {
        let parsed = parse("<msg><appmsg><type>2000</type><wcpayinfo><paysubtype>99</paysubtype><feeDesc> ¥0.01 </feeDesc><paymemo> hello\n world </paymemo><payerUsername>payer</payerUsername></wcpayinfo></appmsg></msg>").unwrap();
        assert_eq!(parsed.info.fee_desc, "¥0.01");
        assert_eq!(parsed.info.pay_memo, "hello world");
        assert_eq!(parsed.info.paysubtype_label, "未知(paysubtype=99)");
        assert_eq!(parsed.info.payer_username, "payer");
        let empty = parse("<msg><appmsg><type>2000</type><wcpayinfo/></appmsg></msg>").unwrap();
        assert!(!empty.render().contains("金额"));
    }

    #[test]
    fn rejects_other_card_types_missing_info_and_entities() {
        assert!(parse("<msg><appmsg><type>6</type></appmsg></msg>").is_err());
        assert!(parse("<msg><appmsg><type>2000</type></appmsg></msg>").is_err());
        assert!(
            parse("<!DOCTYPE x [<!ENTITY y SYSTEM 'file:///private'>]><msg>&y;</msg>").is_err()
        );
    }

    #[test]
    fn matches_legacy_fields_summary_and_detail_fixtures() {
        let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/transfer-golden.json"
        )))
        .unwrap();
        for case in cases {
            let xml = case["xml"].as_str().unwrap();
            let doc = Document::parse(xml).unwrap();
            let appmsg = doc
                .descendants()
                .find(|node| node.has_tag_name("appmsg"))
                .unwrap();
            assert_eq!(
                summary(appmsg, &text(appmsg, &["title"])),
                case["summary"].as_str().unwrap(),
                "{}",
                case["name"]
            );
            if case["info"].is_null() {
                assert!(parse(xml).is_err());
                continue;
            }
            let transfer = parse(xml).unwrap();
            assert_eq!(
                serde_json::to_value(&transfer.info).unwrap(),
                case["info"],
                "{}",
                case["name"]
            );
            if let Some(render) = case["render"].as_str() {
                assert_eq!(transfer.render(), render, "{}", case["name"]);
            }
        }
    }
}
