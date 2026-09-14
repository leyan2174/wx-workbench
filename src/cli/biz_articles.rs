use super::history::{parse_time, parse_time_end};
use super::output::{print_value, resolve};
use super::transport;
use crate::ipc::Request;
use anyhow::Result;

pub fn cmd_biz_articles(
    limit: usize,
    account: Option<String>,
    since: Option<String>,
    until: Option<String>,
    unread: bool,
    json: bool,
) -> Result<()> {
    let since_ts = since.as_deref().map(parse_time).transpose()?;
    let until_ts = until.as_deref().map(parse_time_end).transpose()?;

    let req = Request::BizArticles {
        limit,
        account,
        since: since_ts,
        until: until_ts,
        unread,
    };
    let resp = transport::send(req)?;
    let data = article_array(&resp.data)?;
    warn_incomplete(&resp.data, &mut std::io::stderr().lock())?;
    print_value(data, &resolve(json))
}

fn article_array(response: &serde_json::Value) -> Result<&serde_json::Value> {
    response
        .get("articles")
        .filter(|value| value.is_array())
        .ok_or_else(|| anyhow::anyhow!("Invalid article response: articles must be an array"))
}

fn warn_incomplete(
    data: &serde_json::Value,
    output: &mut impl std::io::Write,
) -> std::io::Result<()> {
    let partial = data.get("partial").and_then(serde_json::Value::as_bool) == Some(true);
    let unfinished = data
        .get("source_unfinished")
        .and_then(serde_json::Value::as_bool)
        == Some(true);
    if partial || unfinished {
        writeln!(
            output,
            "警告：公众号文章结果不完整，部分来源或内容未能读取，或扫描尚未完成。"
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn malformed_article_response_is_an_error_not_an_empty_array() {
        for response in [
            json!({}),
            json!(null),
            json!([]),
            json!({"articles": null}),
            json!({"articles": {}}),
            json!({"articles": "synthetic-private-content"}),
            json!({"articles": 0}),
            json!({"articles": false}),
        ] {
            assert_eq!(
                article_array(&response).unwrap_err().to_string(),
                "Invalid article response: articles must be an array"
            );
        }
    }

    #[test]
    fn legacy_article_arrays_are_preserved_without_partial_metadata() {
        for response in [
            json!({"articles": []}),
            json!({"articles": [{"title": "Synthetic article"}]}),
        ] {
            let projected = article_array(&response).unwrap();
            assert_eq!(projected, &response["articles"]);
            let mut warnings = Vec::new();
            warn_incomplete(&response, &mut warnings).unwrap();
            assert!(warnings.is_empty());
        }
    }

    #[test]
    fn partial_and_unfinished_each_warn_once_without_changing_articles() {
        for (partial, unfinished) in [(true, false), (false, true), (true, true)] {
            let response = json!({
                "articles": [{"title": "Synthetic article"}],
                "partial": partial,
                "source_unfinished": unfinished,
            });
            let before = response.clone();
            let mut output = Vec::new();
            warn_incomplete(&response, &mut output).unwrap();
            let warning = String::from_utf8(output).unwrap();
            assert!(warning.contains("结果不完整"));
            assert_eq!(warning.lines().count(), 1);
            assert_eq!(response, before);
        }
    }

    #[test]
    fn ordinary_pagination_and_legacy_responses_remain_quiet() {
        for response in [
            json!({"articles": []}),
            json!({"articles": [], "partial": false, "source_unfinished": false}),
            json!({"articles": [], "has_more": true}),
        ] {
            let mut output = Vec::new();
            warn_incomplete(&response, &mut output).unwrap();
            assert!(output.is_empty());
        }
    }

    #[test]
    fn warning_does_not_echo_source_error_or_article_content() {
        let response = json!({
            "partial": true,
            "issues": ["synthetic-private-source-token"],
            "articles": [{"title": "synthetic-private-title"}],
        });
        let mut output = Vec::new();
        warn_incomplete(&response, &mut output).unwrap();
        let warning = String::from_utf8(output).unwrap();
        assert!(!warning.contains("synthetic-private"));
        assert!(!warning.is_empty());
    }

    #[test]
    fn warning_write_failure_is_not_ignored() {
        struct Broken;
        impl std::io::Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let result = warn_incomplete(&json!({"partial": true}), &mut Broken);
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
    }
}
