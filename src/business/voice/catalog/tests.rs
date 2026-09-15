use super::*;
use std::cell::Cell;

struct Memory {
    page: Page,
    reads: Cell<usize>,
    unavailable: bool,
}
impl Source for Memory {
    type Error = Error;
    fn read(&self, _: &Query) -> Result<Page, Error> {
        self.reads.set(self.reads.get() + 1);
        if self.unavailable {
            Err(Error::Unavailable)
        } else {
            Ok(self.page.clone())
        }
    }
}
fn query() -> Query {
    Query {
        username: "alice".into(),
        limit: 3,
        offset: 0,
        since: Some(0),
        until: Some(0),
    }
}
fn memory() -> Memory {
    Memory {
        page: Page {
            entries: [None, Some(0), Some(7)]
                .into_iter()
                .map(|byte_len| Entry {
                    username: "alice".into(),
                    timestamp: 0,
                    byte_len,
                    source: SourceRef::new("private source evidence"),
                })
                .collect(),
            offset: 0,
            limit: 3,
            continuation: PageContinuation::MayHaveMore,
        },
        reads: Cell::new(0),
        unavailable: false,
    }
}

#[test]
fn previews_preserve_order_duplicates_endpoints_and_unknown_size() {
    let source = memory();
    let page = list(&source, &query()).unwrap();
    assert_eq!(page.entries, source.page.entries);
    assert_eq!(
        page.entries.iter().map(|v| v.byte_len).collect::<Vec<_>>(),
        vec![None, Some(0), Some(7)]
    );
    assert_ne!(page.entries[0].source, page.entries[1].source);
    assert_eq!(
        format!("{:?}", page.entries[0].source),
        "SourceRef(<opaque>)"
    );
    assert!(!format!("{page:?}").contains("private source evidence"));
}

#[test]
fn invalid_queries_do_not_read_the_source() {
    let source = memory();
    let mut q = query();
    q.username = " ".into();
    assert_eq!(list(&source, &q).unwrap_err(), Error::EmptyUsername);
    q = query();
    q.limit = 0;
    assert_eq!(list(&source, &q).unwrap_err(), Error::EmptyLimit);
    q = query();
    q.since = Some(1);
    assert_eq!(list(&source, &q).unwrap_err(), Error::InvalidRange);
    q = query();
    q.offset = usize::MAX;
    assert_eq!(list(&source, &q).unwrap_err(), Error::PaginationOverflow);
    assert_eq!(source.reads.get(), 0);
}

#[test]
fn complete_inventory_does_not_mean_pagination_is_exhausted() {
    for offset in [0, 9] {
        for (returned, expected) in [
            (0, PageContinuation::Exhausted),
            (1, PageContinuation::Exhausted),
            (3, PageContinuation::MayHaveMore),
        ] {
            let mut source = memory();
            let q = Query { offset, ..query() };
            source.page.offset = offset;
            source.page.entries.truncate(returned);
            source.page.continuation = q.continuation(returned);
            let page = list(&source, &q).unwrap();
            assert_eq!(page.continuation, expected);
            assert_eq!(source.reads.get(), 1, "no pagination probe");
        }
    }
}

#[test]
fn a_full_last_page_cannot_claim_exhaustion_without_a_probe() {
    let mut source = memory();
    assert_eq!(
        list(&source, &query()).unwrap().continuation,
        PageContinuation::MayHaveMore
    );
    source.page.continuation = PageContinuation::Exhausted;
    assert_eq!(list(&source, &query()).unwrap_err(), Error::InvalidPage);
    source.page.entries.clear();
    source.page.continuation = PageContinuation::MayHaveMore;
    assert_eq!(list(&source, &query()).unwrap_err(), Error::InvalidPage);
}

#[test]
fn incomplete_sources_and_invalid_pages_fail_closed() {
    let mut source = memory();
    source.unavailable = true;
    assert_eq!(list(&source, &query()).unwrap_err(), Error::Unavailable);
    for defect in 0..6 {
        let mut source = memory();
        match defect {
            0 => source.page.offset += 1,
            1 => source.page.limit += 1,
            2 => source.page.entries.push(source.page.entries[0].clone()),
            3 => source.page.entries[0].username = "bob".into(),
            4 => source.page.entries[0].timestamp = -1,
            _ => source.page.entries[0].timestamp = 1,
        }
        assert_eq!(list(&source, &query()).unwrap_err(), Error::InvalidPage);
    }
}

#[test]
fn exact_chat_resolution_does_not_guess_identity_from_labels() {
    let names = HashMap::from([
        ("alice".into(), "shared".into()),
        ("bob".into(), "shared".into()),
        ("carol".into(), "alice".into()),
        ("dave".into(), "Unique".into()),
    ]);
    assert_eq!(resolve_exact_chat("alice", &names).unwrap(), "alice");
    assert_eq!(resolve_exact_chat("Unique", &names).unwrap(), "dave");
    for chat in ["shared", "unique", "Unique ", "ali"] {
        assert_eq!(resolve_exact_chat(chat, &names), Err(Error::AmbiguousChat));
    }
    assert_eq!(resolve_exact_chat(" ", &names), Err(Error::EmptyChat));
}
