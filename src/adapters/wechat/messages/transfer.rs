//! WeChat transfer field aliases and strict detail decoding.
use crate::business::structured_message::{TransferContent, TransferDetails, TransferStatus};
use anyhow::{ensure, Context, Result};
use roxmltree::{Document, Node};

pub(crate) fn text(node: Node<'_, '_>, tags: &[&str]) -> String {
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

pub(crate) fn extract(appmsg: Node<'_, '_>) -> Option<TransferDetails> {
    let info = appmsg
        .children()
        .find(|child| child.has_tag_name("wcpayinfo"))?;
    let subtype = text(info, &["paysubtype"]);
    let status = match subtype.as_str() {
        "1" => TransferStatus::Initiated,
        "3" => TransferStatus::Received,
        "4" => TransferStatus::Returned,
        "5" => TransferStatus::ExpiredReturned,
        "7" => TransferStatus::PendingCollection,
        "8" => TransferStatus::Collected,
        "" => TransferStatus::Missing,
        _ => TransferStatus::Unknown,
    };
    Some(TransferDetails {
        raw_subtype: subtype,
        status,
        amount_text: text(info, &["feedesc", "feeDesc"]),
        memo: text(info, &["pay_memo", "paymemo"]),
        // 微信原字段就是 transcationid，不能擅自改成 transactionid 而漏读旧数据。
        transaction_id: text(info, &["transcationid", "transcationId"]),
        transfer_id: text(info, &["transferid", "transferId"]),
        payment_message_id: text(info, &["paymsgid", "payMsgId"]),
        started_at: text(info, &["begintransfertime", "beginTransferTime"]),
        expires_at: text(info, &["invalidtime", "invalidTime"]),
        effective_date: text(info, &["effectivedate", "effectiveDate"]),
        payer: text(info, &["payer_username", "payerUsername"]),
        receiver: text(info, &["receiver_username", "receiverUsername"]),
    })
}

pub fn parse(xml: &str) -> Result<TransferContent> {
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
    Ok(TransferContent {
        title,
        description: text(appmsg, &["des"]),
        details: info,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::transfer::{summary, Transfer};

    fn card(kind: &str, fields: &str) -> String {
        format!("<msg><appmsg><type>{kind}</type><wcpayinfo>{fields}</wcpayinfo></appmsg></msg>")
    }

    #[test]
    fn complete_aliases_preserve_typed_values_and_original_json_spelling() {
        let aliases = [
            ("feedesc", "feeDesc", " 0.010 CNY "),
            ("pay_memo", "paymemo", " hello\n world "),
            ("transcationid", "transcationId", "txn"),
            ("transferid", "transferId", "transfer"),
            ("paymsgid", "payMsgId", "payment"),
            ("begintransfertime", "beginTransferTime", "+001_234"),
            ("invalidtime", "invalidTime", "future-format"),
            ("effectivedate", "effectiveDate", "tomorrow"),
            ("payer_username", "payerUsername", "payer"),
            ("receiver_username", "receiverUsername", "receiver"),
        ];
        let fields = |alternate: bool| {
            let mut out = "<paysubtype>99</paysubtype>".to_owned();
            for (original, alias, value) in aliases {
                let tag = if alternate { alias } else { original };
                out.push_str(&format!("<{tag}>{value}</{tag}>"));
            }
            out
        };
        let first = parse(&card("2000", &fields(false))).unwrap();
        assert_eq!(first, parse(&card("2000", &fields(true))).unwrap());
        assert_eq!(first.details.status, TransferStatus::Unknown);
        let projected = Transfer::from(first);
        assert_eq!(
            serde_json::to_value(&projected).unwrap(),
            serde_json::json!({
                "title": "微信转账", "description": "",
                "paysubtype": "99", "paysubtype_label": "未知(paysubtype=99)",
                "fee_desc": "0.010 CNY", "pay_memo": "hello world",
                "transcation_id": "txn", "transfer_id": "transfer", "pay_msg_id": "payment",
                "begin_transfer_time": "+001_234", "invalid_time": "future-format",
                "effective_date": "tomorrow", "payer_username": "payer", "receiver_username": "receiver"
            })
        );
        let extras = projected.export_fields().unwrap();
        assert_eq!(extras["begin_transfer_time"], 1234);
        assert!(!extras.contains_key("invalid_time"));
        assert!(!extras.contains_key("effective_date"));
        assert!(!extras.contains_key("transaction_id"));
    }

    #[test]
    fn missing_unknown_and_first_nonempty_alias_rules_are_not_reinterpreted() {
        let empty = parse(&card("2000", "")).unwrap();
        assert_eq!(empty.details.status, TransferStatus::Missing);
        assert_eq!(summary(Some(&empty.details), "ignored"), "[转账]");
        assert_eq!(summary(None, "fallback"), "[转账] fallback");
        assert!(parse("<msg><appmsg><type>2000</type></appmsg></msg>").is_err());
        let xml = card("2000", "<feedesc> </feedesc><feeDesc>raw amount</feeDesc><transcationid>old</transcationid><transcationId>alias</transcationId><transactionid>wrong spelling</transactionid>");
        let decoded = parse(&xml).unwrap();
        assert_eq!(decoded.details.amount_text, "raw amount");
        assert_eq!(decoded.details.transaction_id, "old");
        for kind in ["", "2_000", "future", "6"] {
            let xml = card(kind, "<feedesc>raw amount</feedesc>");
            assert!(parse(&xml).is_err());
            let doc = Document::parse(&xml).unwrap();
            let app = doc
                .descendants()
                .find(|node| node.has_tag_name("appmsg"))
                .unwrap();
            assert_eq!(summary(extract(app).as_ref(), ""), "[转账] raw amount");
        }
    }
}
