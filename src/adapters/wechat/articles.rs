//! Article projection from decoded WeChat push evidence. No database discovery here.
use super::messages::{Snapshot, MAX_DECODED_BYTES};
use super::xml_fragments::{element_text as extract_xml_text, unescape_entities as unescape_html};
use crate::business::articles::{Article, EvidenceRef, Inventory, Issue, IssueKind};
use crate::business::{
    articles as domain,
    messages::{Conversation, MessageRef, SourceKind},
};
use std::collections::HashMap;

pub(crate) fn source_error(error: &anyhow::Error) -> domain::Error {
    use crate::business::messages::Error;
    match error.downcast_ref::<Error>() {
        Some(Error::Unsupported) => domain::Error::UnsupportedFormat,
        Some(Error::Unavailable | Error::NotFound | Error::Expired) => domain::Error::Unavailable,
        _ => domain::Error::InvalidData,
    }
}

pub(crate) struct Source<'a> {
    snapshot: &'a Snapshot,
    names: &'a HashMap<String, String>,
}

impl<'a> Source<'a> {
    pub(crate) fn new(snapshot: &'a Snapshot, names: &'a HashMap<String, String>) -> Self {
        Self { snapshot, names }
    }
}

impl domain::ArticleSource for Source<'_> {
    fn scan(&mut self, query: &domain::Query) -> Result<Inventory, domain::Error> {
        let mut result = Inventory::default();
        let filter = crate::business::messages::Filter {
            since: query.received_since,
            until: query.received_until,
            kinds: vec![crate::business::messages::Kind::Structured],
        };
        const LIMIT: usize = 100_000;
        let mut read = 0usize;
        for (index, stream) in self.snapshot.streams().iter().enumerate() {
            if self
                .snapshot
                .source_kind(index)
                .map_err(|_| domain::Error::Unavailable)?
                != SourceKind::OfficialPush
            {
                continue;
            }
            let publisher = match &stream.conversation {
                Conversation::Known(value) => value.as_str(),
                Conversation::Unmapped(_) => "",
            };
            let name = self
                .names
                .get(publisher)
                .map(String::as_str)
                .unwrap_or(publisher);
            // Selection is authoritative in the business operation. Only unread membership
            // is safe to prune here; unresolved ownership must still be reported.
            if !publisher.is_empty()
                && query
                    .unread_publishers
                    .as_ref()
                    .is_some_and(|set| !set.contains(publisher))
            {
                continue;
            }
            if read == LIMIT {
                result.unfinished = true;
                break;
            }
            let remaining = LIMIT - read;
            let rows = self
                .snapshot
                .read_page(index, &filter, remaining, false)
                .map_err(|error| source_error(&error))?;
            read += rows.len();
            if rows.len() == remaining {
                result.unfinished = true;
            }
            for raw in rows {
                let decoded = raw
                    .bounded_decode(MAX_DECODED_BYTES)
                    .map_err(|_| domain::Error::InvalidData)?;
                let xml = std::str::from_utf8(&decoded).map_err(|_| domain::Error::InvalidData)?;
                let mut parsed = parse_push(&raw.reference, publisher, name, raw.timestamp, xml);
                if result.articles.len().saturating_add(parsed.articles.len()) > LIMIT {
                    result.unfinished = true;
                    return Ok(result);
                }
                result.articles.append(&mut parsed.articles);
                result.issues.append(&mut parsed.issues);
            }
        }
        Ok(result)
    }
}

/// Recover XML fragments without presenting recovered input as complete evidence.
pub(crate) fn parse_push(
    evidence: &MessageRef,
    publisher: &str,
    publisher_name: &str,
    received_at: i64,
    xml: &str,
) -> Inventory {
    let mut result = Inventory::default();
    if publisher.is_empty() {
        result.issues.push(Issue {
            evidence: Some(EvidenceRef::new(evidence.clone(), None)),
            kind: IssueKind::UnknownPublisher,
        });
        return result;
    }
    if roxmltree::Document::parse(xml).is_err() {
        result.issues.push(Issue {
            evidence: Some(EvidenceRef::new(evidence.clone(), None)),
            kind: IssueKind::InvalidContent,
        });
    }
    let mut search_from = 0;
    let mut index = 0;
    while let Some(item_start) = xml[search_from..].find("<item>") {
        let abs_start = search_from + item_start;
        let Some(item_end) = xml[abs_start..].find("</item>") else {
            break;
        };
        let abs_end = abs_start + item_end + 7;
        let item_xml = &xml[abs_start..abs_end];
        let title = extract_cdata(item_xml, "title").unwrap_or_default();
        let url = extract_cdata(item_xml, "url").unwrap_or_default();
        let item_evidence = EvidenceRef::new(evidence.clone(), Some(index));
        index += 1;
        search_from = abs_end;
        // Empty title/URL also represents known non-article app messages.
        if title.is_empty() || url.is_empty() {
            continue;
        }
        let published_at = extract_xml_text(item_xml, "pub_time")
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(received_at);
        result.articles.push(Article {
            evidence: item_evidence,
            publisher: publisher.into(),
            publisher_name: publisher_name.into(),
            received_at,
            published_at,
            title,
            url,
            digest: extract_cdata(item_xml, "digest").unwrap_or_default(),
            cover_url: extract_cdata(item_xml, "cover").unwrap_or_default(),
        });
    }
    result
}

fn extract_cdata(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)?;
    let inner = xml[start..start + end].trim();
    if let Some(body) = inner.strip_prefix("<![CDATA[") {
        // inner = `<![CDATA[content]]>` → strip 9-char `<![CDATA[` prefix + 3-char `]]>` suffix
        // Strip `]]>` (normal) or `]]` (edge case)
        let cdata_end = b"]]>";
        let cdata_end2 = b"]]";
        let content: &str = if body.as_bytes().ends_with(cdata_end) {
            &body[..body.len() - 3]
        } else if body.as_bytes().ends_with(cdata_end2) {
            &body[..body.len() - 2]
        } else {
            body
        };
        let content = content.trim();
        if content.is_empty() {
            None
        } else {
            Some(content.to_string())
        }
    } else if inner.is_empty() {
        None
    } else {
        Some(unescape_html(inner))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query() -> domain::Query {
        domain::Query {
            limit: 20,
            publisher: None,
            received_since: None,
            received_until: None,
            unread_publishers: None,
        }
    }

    fn database(
        root: &std::path::Path,
        shard: usize,
        publisher: &str,
        mapped: bool,
        rows: &[(i64, i64, Vec<u8>)],
    ) -> super::super::messages::SourceFile {
        let path = root.join(format!("push-{shard}.db"));
        let conn = rusqlite::Connection::open(&path).unwrap();
        let table = format!("Msg_{:x}", md5::compute(publisher));
        conn.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,local_type INTEGER,create_time INTEGER,WCDB_CT_message_content INTEGER,message_content BLOB)")).unwrap();
        if mapped {
            conn.execute_batch("CREATE TABLE Name2Id(user_name TEXT)")
                .unwrap();
            conn.execute("INSERT INTO Name2Id VALUES(?1)", [publisher])
                .unwrap();
        }
        for (time, compression, content) in rows {
            conn.execute(
                &format!("INSERT INTO [{table}] VALUES(7,49,?1,?2,?3)"),
                rusqlite::params![time, compression, content],
            )
            .unwrap();
        }
        super::super::messages::SourceFile {
            logical_name: format!("message/biz_message_{shard}.db"),
            path,
            kind: SourceKind::OfficialPush,
        }
    }

    #[test]
    fn sqlite_shared_snapshot_decodes_all_shards_without_sessions_or_local_id_identity() {
        let root = tempfile::tempdir().unwrap();
        let xml = b"<msg><item><title>A</title><url>same</url><pub_time>40</pub_time></item><item><title>B</title><url>same</url><pub_time>30</pub_time></item></msg>";
        let first = database(root.path(), 0, "gh_a", true, &[(10, 0, xml.to_vec())]);
        let compressed = zstd::stream::encode_all(xml.as_slice(), 1).unwrap();
        let second = database(root.path(), 1, "gh_b", true, &[(20, 4, compressed)]);
        let snapshot = Snapshot::open(vec![first, second], Vec::<String>::new()).unwrap();
        let names = HashMap::new();
        let mut source = Source::new(&snapshot, &names);
        let page = domain::list(
            &mut source,
            &domain::Query {
                received_since: Some(10),
                received_until: Some(20),
                ..query()
            },
        )
        .unwrap();
        assert_eq!(page.articles.len(), 4);
        assert!(page.issues.is_empty());
        assert!(!page.source_unfinished);
        assert_ne!(page.articles[0].evidence, page.articles[1].evidence);
        assert_eq!(page.articles[0].published_at, 40);
        let unread = domain::list(
            &mut source,
            &domain::Query {
                unread_publishers: Some(std::collections::BTreeSet::from(["gh_a".into()])),
                ..query()
            },
        )
        .unwrap();
        assert_eq!(unread.articles.len(), 1);
        assert_eq!(unread.articles[0].publisher, "gh_a");
    }

    #[test]
    fn sqlite_unmapped_stream_is_partial_but_bad_content_is_failure() {
        let root = tempfile::tempdir().unwrap();
        let file = database(
            root.path(),
            0,
            "unknown",
            false,
            &[(1, 0, b"<msg/>".to_vec())],
        );
        let snapshot = Snapshot::open(vec![file], Vec::<String>::new()).unwrap();
        let names = HashMap::new();
        let page = domain::list(&mut Source::new(&snapshot, &names), &query()).unwrap();
        assert!(page.articles.is_empty());
        assert_eq!(page.issues[0].kind, IssueKind::UnknownPublisher);
        let bad = database(root.path(), 1, "gh_a", true, &[(1, 4, vec![0xff])]);
        let snapshot = Snapshot::open(vec![bad], Vec::<String>::new()).unwrap();
        assert!(matches!(
            domain::list(&mut Source::new(&snapshot, &names), &query()),
            Err(domain::Error::InvalidData)
        ));
    }

    #[test]
    fn sqlite_missing_schema_does_not_become_empty_success() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("unsupported.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE Msg_00000000000000000000000000000000(unrelated TEXT)")
            .unwrap();
        let opened = Snapshot::open(
            vec![super::super::messages::SourceFile {
                logical_name: "message/biz_message_0.db".into(),
                path,
                kind: SourceKind::OfficialPush,
            }],
            Vec::<String>::new(),
        );
        assert!(opened.is_err());
    }

    fn message_ref() -> MessageRef {
        use std::sync::{Arc, OnceLock};
        static OWNER: OnceLock<Arc<()>> = OnceLock::new();
        let owner = OWNER.get_or_init(|| Arc::new(()));
        MessageRef(crate::business::messages::EvidenceRef {
            snapshot: Arc::downgrade(owner),
            stream: 0,
            record: 1,
        })
    }

    #[test]
    fn recovered_fragment_is_partial_and_unknown_publisher_is_not_invented() {
        let xml = "<item><title>T</title><url>https://example.invalid/a</url></item><broken";
        let evidence = message_ref();
        let recovered = parse_push(&evidence, "publisher", "Publisher", 7, xml);
        assert_eq!(recovered.articles.len(), 1);
        assert_eq!(recovered.articles[0].published_at, 7);
        assert_eq!(recovered.issues[0].kind, IssueKind::InvalidContent);
        let unknown = parse_push(&evidence, "", "Display is not identity", 7, xml);
        assert!(unknown.articles.is_empty());
        assert_eq!(unknown.issues[0].kind, IssueKind::UnknownPublisher);
    }

    #[test]
    fn item_identity_preserves_position_and_does_not_depend_on_url() {
        let xml = "<msg><item><title>empty</title></item><item><title>A</title><url>same</url></item><item><title>B</title><url>same</url></item></msg>";
        let page = parse_push(&message_ref(), "a", "a", 1, xml);
        assert!(page.issues.is_empty());
        assert_eq!(page.articles[0].evidence.item_index(), Some(1));
        assert_eq!(page.articles[1].evidence.item_index(), Some(2));
    }

    fn parse_biz_xml_items(received: i64, publisher: &str, xml: &str) -> Vec<Article> {
        parse_push(&message_ref(), publisher, publisher, received, xml).articles
    }

    #[test]
    fn extract_cdata_normal() {
        let xml = "<title><![CDATA[TencentResearch]]></title>";
        assert_eq!(extract_cdata(xml, "title"), Some("TencentResearch".into()));
    }

    #[test]
    fn extract_cdata_empty() {
        let xml = "<cover><![CDATA[]]></cover>";
        assert_eq!(extract_cdata(xml, "cover"), None);
    }

    #[test]
    fn extract_cdata_url() {
        let xml = "<url><![CDATA[http://mp.weixin.qq.com/s?__biz=abc&mid=123]]></url>";
        let result = extract_cdata(xml, "url");
        assert!(result.is_some());
        let url = result.unwrap();
        assert!(url.starts_with("http://mp.weixin.qq.com"));
        assert!(!url.contains("CDATA"));
    }

    #[test]
    fn extract_cdata_no_cdata_wrapper() {
        let xml = "<pub_time>1700000000</pub_time>";
        assert_eq!(extract_cdata(xml, "pub_time"), Some("1700000000".into()));
    }

    #[test]
    fn parse_biz_xml_items_single_article() {
        let xml = r#"<msg><appmsg><mmreader><category><item>
            <title><![CDATA[Test Article Title]]></title>
            <url><![CDATA[http://mp.weixin.qq.com/s?test=1]]></url>
            <digest><![CDATA[Test Digest]]></digest>
            <cover><![CDATA[https://example.com/cover.jpg]]></cover>
            <pub_time>1700000000</pub_time>
        </item></category></mmreader></appmsg></msg>"#;

        let items = parse_biz_xml_items(1699999999, "gh_test123", xml);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Test Article Title");
        assert_eq!(items[0].url, "http://mp.weixin.qq.com/s?test=1");
        assert_eq!(items[0].digest, "Test Digest");
        assert_eq!(items[0].published_at, 1700000000);
        assert_eq!(items[0].publisher, "gh_test123");
    }

    #[test]
    fn parse_biz_xml_items_skips_no_url() {
        let xml = r#"<msg><mmreader><category><item>
            <title><![CDATA[Has Title No URL]]></title>
            <url><![CDATA[]]></url>
            <pub_time>1700000001</pub_time>
        </item></category></mmreader></msg>"#;
        let items = parse_biz_xml_items(1700000001, "gh_test", xml);
        assert_eq!(items.len(), 0);
    }

    #[test]
    fn parse_biz_xml_items_multi_article() {
        let xml = r#"<msg><mmreader><category>
        <item>
            <title><![CDATA[Article 1]]></title>
            <url><![CDATA[http://mp.weixin.qq.com/s?a=1]]></url>
            <pub_time>1700000010</pub_time>
        </item>
        <item>
            <title><![CDATA[Article 2]]></title>
            <url><![CDATA[http://mp.weixin.qq.com/s?a=2]]></url>
            <pub_time>1700000020</pub_time>
        </item>
        </category></mmreader></msg>"#;
        let items = parse_biz_xml_items(1700000000, "gh_multi", xml);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Article 1");
        assert_eq!(items[1].title, "Article 2");
    }

    #[test]
    fn parse_biz_xml_items_pub_time_fallback() {
        // When pub_time is missing, should fall back to recv_time
        let xml = r#"<item>
            <title><![CDATA[No PubTime]]></title>
            <url><![CDATA[http://mp.weixin.qq.com/s?x=1]]></url>
        </item>"#;
        let items = parse_biz_xml_items(1700000099, "gh_fallback", xml);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].published_at, 1700000099); // falls back to recv_time
    }
}
