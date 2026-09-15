//! Account host preparation and compatibility projections; message storage lives in the adapter.
use super::*;
use crate::{
    adapters::wechat::messages::{LegacyReadPolicy, RawMessage, Snapshot, SourceFile},
    business::messages as domain,
};
use std::{collections::BTreeMap, path::PathBuf};

pub(super) struct Prepared {
    pub files: Vec<SourceFile>,
    origins: BTreeMap<String, (PathBuf, CacheMode)>,
}

pub(super) async fn find_shards(
    db: &DbCache,
    names: &Names,
    username: &str,
) -> Result<(Vec<MessageShard>, usize)> {
    let prepared = prepare(db, names, domain::SourceKind::Ordinary).await?;
    let username = username.to_owned();
    let result = tokio::task::spawn_blocking(move || -> Result<_> {
        let scanned = prepared.files.len();
        let snapshot = Snapshot::open(prepared.files, [username.clone()])?;
        let mut shards = Vec::new();
        for stream in snapshot.streams_for(&username, domain::SourceKind::Ordinary) {
            let source = snapshot.source_name(stream)?.to_owned();
            let (path, mode) = prepared
                .origins
                .get(&source)
                .context("message origin unavailable")?;
            shards.push(MessageShard {
                rel_key: source,
                path: path.clone(),
                table: snapshot.streams()[stream].table_name().into(),
                max_ts: snapshot.latest_timestamp(stream)?.unwrap_or(0),
                cache_mode: *mode,
            });
        }
        shards.sort_by(|a, b| b.max_ts.cmp(&a.max_ts).then(a.rel_key.cmp(&b.rel_key)));
        Ok((shards, scanned))
    })
    .await?;
    check_inventory(db, names, domain::SourceKind::Ordinary)?;
    result
}

pub(super) async fn stats(
    db: &DbCache,
    names: &Names,
    chat: &str,
    since: Option<i64>,
    until: Option<i64>,
    options: MetaOptions,
) -> Result<Value> {
    let filter = domain::Filter {
        since,
        until,
        kinds: Vec::new(),
    };
    filter.validate()?;
    let username = strict_message::username(chat, names)?;
    let prepared = prepare(db, names, domain::SourceKind::Ordinary).await?;
    let display = names.display(&username);
    let chat_type = chat_type_of(&username, names);
    let is_group = chat_type == "group";
    let nicknames = if is_group {
        load_group_nicknames(db, &username).await?
    } else {
        HashMap::new()
    };
    let scanned = prepared.files.len();
    let target = username.clone();
    let identities = names
        .map
        .keys()
        .cloned()
        .chain(std::iter::once(target.clone()))
        .collect::<Vec<_>>();
    let result = tokio::task::spawn_blocking(move || -> Result<_> {
        let snapshot = Snapshot::open(prepared.files, identities)?;
        let mut shards = Vec::new();
        for stream in snapshot.streams_for(&target, domain::SourceKind::Ordinary) {
            let source = snapshot.source_name(stream)?.to_owned();
            let (path, mode) = prepared
                .origins
                .get(&source)
                .context("message origin unavailable")?;
            shards.push(MessageShard {
                rel_key: source,
                path: path.clone(),
                table: snapshot.streams()[stream].table_name().into(),
                max_ts: snapshot.latest_timestamp(stream)?.unwrap_or(0),
                cache_mode: *mode,
            });
        }
        anyhow::ensure!(!shards.is_empty(), domain::Error::NotFound);
        shards.sort_by(|a, b| b.max_ts.cmp(&a.max_ts).then(a.rel_key.cmp(&b.rel_key)));
        let report =
            crate::adapters::wechat::messages::statistics::read(&snapshot, &target, &filter)?;
        Ok((report, shards))
    })
    .await?;
    check_inventory(db, names, domain::SourceKind::Ordinary)?;
    let (report, shards) = result?;
    let sender_counts = report
        .statistics
        .by_sender
        .iter()
        .map(|(name, count)| Ok((name.clone(), i64::try_from(*count)?)))
        .collect::<Result<HashMap<_, _>>>()?;
    let mut types = report.legacy_types.into_iter().collect::<Vec<_>>();
    types.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let by_type = types
        .into_iter()
        .map(|(kind, count)| json!({"type": kind, "count": count}))
        .collect::<Vec<_>>();
    let by_hour = report
        .statistics
        .by_hour
        .iter()
        .enumerate()
        .map(|(hour, count)| json!({"hour": hour, "count": count}))
        .collect::<Vec<_>>();
    let meta = meta_for_shards(
        scanned,
        &shards,
        report.hit_streams,
        Vec::new(),
        session_last_timestamp(db, &username).await,
        since.is_some() || until.is_some(),
        options,
    );
    Ok(
        json!({"chat": display, "username": username, "is_group": is_group, "chat_type": chat_type,
        "total": report.statistics.total, "by_type": by_type, "by_hour": by_hour,
        "top_senders": group_top_senders(&sender_counts, &names.map, &nicknames, 10), "meta": meta}),
    )
}

pub(super) async fn sessions(
    db: &DbCache,
    names: &Names,
    query: crate::business::sessions::Query,
    with_details: bool,
) -> Result<Value> {
    let path = db
        .get(crate::adapters::wechat::messages::sources::sessions().cache_key())
        .await?
        .context("session database unavailable")?;
    let verified = names.verify_flags.clone();
    let records = tokio::task::spawn_blocking(move || {
        crate::adapters::wechat::messages::sessions::read(&path, &verified)
    })
    .await??;
    let selected = crate::business::sessions::select(
        &records
            .iter()
            .map(|r| r.session.clone())
            .collect::<Vec<_>>(),
        &query,
    )?;
    let mut slots: Vec<_> = records.into_iter().map(Some).collect();
    let mut results = Vec::new();
    let mut nickname_cache = HashMap::new();
    for index in selected {
        results.push(
            session_view(
                db,
                names,
                slots[index].take().context("duplicate session selection")?,
                &mut nickname_cache,
            )
            .await,
        );
    }
    let meta = session_meta(db, names, &results, with_details);
    let mut value = json!({"sessions": results, "meta": meta});
    if query.unread_only {
        value["total"] = json!(value["sessions"].as_array().map_or(0, Vec::len));
    }
    Ok(value)
}

pub(super) async fn new_messages(
    db: &DbCache,
    names: &Names,
    state: Option<HashMap<String, i64>>,
    limit: usize,
    options: MetaOptions,
) -> Result<Value> {
    let session_path = db
        .get(crate::adapters::wechat::messages::sources::sessions().cache_key())
        .await?
        .context("session database unavailable")?;
    let current = tokio::task::spawn_blocking(move || {
        crate::adapters::wechat::messages::sessions::timestamps(&session_path)
    })
    .await??;
    let subscription = domain::TimestampSubscription {
        current: current.into_iter().collect(),
        previous: state,
        fallback: chrono::Utc::now().timestamp() - 86400,
    };
    let changed = subscription.changed();
    check_inventory(db, names, domain::SourceKind::Ordinary)?;
    if changed.is_empty() {
        return Ok(
            json!({"count": 0, "messages": [], "new_state": subscription.advance(&[]), "meta": meta_for_global_query(0, 0, Vec::new(), true, options, Some(HashMap::new()), Some(HashMap::new()))}),
        );
    }
    let page = domain::Page {
        limit,
        offset: 0,
        oldest_first: true,
    };
    page.candidate_limit()?;
    let prepared = prepare(db, names, domain::SourceKind::Ordinary).await?;
    let nicknames = load_group_nickname_maps(
        db,
        changed
            .iter()
            .filter(|(name, _)| name.ends_with("@chatroom"))
            .map(|(name, _)| name.clone())
            .collect(),
    )
    .await?;
    let names_copy = names.clone();
    let scanned = prepared.files.len();
    let modes = prepared
        .origins
        .iter()
        .map(|(k, (_, m))| (k.clone(), m.as_str().to_owned()))
        .collect();
    let paths = prepared
        .origins
        .iter()
        .map(|(k, (p, _))| (k.clone(), p.to_string_lossy().into_owned()))
        .collect();
    let result = tokio::task::spawn_blocking(move || -> Result<_> {
        let mut identities: Vec<_> = names_copy.map.keys().cloned().collect();
        identities.extend(changed.iter().map(|(name, _)| name.clone()));
        let snapshot = Snapshot::open(prepared.files, identities)?;
        let mut candidates = Vec::new();
        let mut hits = HashSet::new();
        let empty = HashMap::new();
        for (username, since) in changed {
            let filter = domain::Filter {
                since: Some(since.checked_add(1).context("subscription time overflow")?),
                until: None,
                kinds: Vec::new(),
            };
            for stream in snapshot.streams_for(&username, domain::SourceKind::Ordinary) {
                for raw in snapshot.read_page(stream, &filter, limit, true)? {
                    anyhow::ensure!(candidates.len() < 100_000, domain::Error::Limit);
                    hits.insert(raw.logical_source.clone());
                    let mut value = project(
                        &snapshot,
                        &raw,
                        &names_copy,
                        nicknames.get(&username).unwrap_or(&empty),
                    )?;
                    value["username"] = Value::String(username.clone());
                    value["chat"] = Value::String(names_copy.display(&username));
                    value["is_group"] = Value::Bool(username.ends_with("@chatroom"));
                    value["chat_type"] = Value::String(chat_type_of(&username, &names_copy).into());
                    candidates.push(domain::Candidate {
                        order: snapshot.order_key(&raw)?,
                        reference: raw.reference.clone(),
                        value: (value, username.clone(), raw.timestamp),
                    });
                }
            }
        }
        Ok((
            page.select(candidates, domain::Completeness::Complete)?,
            hits.len(),
        ))
    })
    .await?;
    check_inventory(db, names, domain::SourceKind::Ordinary)?;
    let (rows, hits) = result?;
    let delivered: Vec<_> = rows
        .iter()
        .map(|(_, name, timestamp)| (name.clone(), *timestamp))
        .collect();
    let messages: Vec<_> = rows.into_iter().map(|(value, _, _)| value).collect();
    Ok(
        json!({"count": messages.len(), "messages": messages, "new_state": subscription.advance(&delivered), "meta": meta_for_global_query(scanned, hits, Vec::new(), true, options, Some(modes), Some(paths))}),
    )
}

pub(super) async fn prepare(
    db: &DbCache,
    names: &Names,
    kind: domain::SourceKind,
) -> Result<Prepared> {
    check_inventory(db, names, kind)?;
    let mut keys = match kind {
        domain::SourceKind::Ordinary => names.msg_db_keys.clone(),
        domain::SourceKind::OfficialPush => names.biz_msg_db_keys.clone(),
    };
    keys.sort();
    keys.dedup();
    let mut files = Vec::new();
    let mut origins = BTreeMap::new();
    for key in keys {
        let source = db
            .get_with_mode(&key)
            .await
            .with_context(|| format!("消息分片未完整读取: {key}"))?
            .with_context(|| format!("消息分片未完整读取: {key}"))?;
        origins.insert(key.clone(), (source.path.clone(), source.mode));
        files.push(SourceFile {
            logical_name: key,
            path: source.path,
            kind,
        });
    }
    anyhow::ensure!(!files.is_empty(), domain::Error::Unavailable);
    Ok(Prepared { files, origins })
}

pub(super) fn check_inventory(db: &DbCache, names: &Names, kind: domain::SourceKind) -> Result<()> {
    if kind == domain::SourceKind::Ordinary {
        return ensure_complete_message_inventory(db, names);
    }
    crate::adapters::wechat::messages::sources::check_official_inventory(
        db.db_dir(),
        &names.biz_msg_db_keys,
    )
}

pub(super) fn project(
    snapshot: &Snapshot,
    raw: &RawMessage,
    names: &Names,
    nicknames: &HashMap<String, String>,
) -> Result<Value> {
    let message = snapshot.message(raw)?;
    let username = match &message.conversation {
        domain::Conversation::Known(name) => name.as_str(),
        _ => "",
    };
    let is_group = username.ends_with("@chatroom");
    let sender = message.sender.as_deref().unwrap_or("");
    let mut value = json!({
        "local_id": raw.local_id.context("ordinary message identity unavailable")?,
        "source": raw.logical_source.replace('\\', "/"),
        "timestamp": message.timestamp,
        "time": fmt_time(message.timestamp, "%Y-%m-%d %H:%M"),
        "sender": if sender.is_empty() { String::new() } else { sender_display(sender, "", &names.map, nicknames) },
        "content": message.preview,
        "type": fmt_type(raw.local_type),
    });
    match &message.conversation {
        domain::Conversation::Known(username) => {
            value["identity_status"] = json!("mapped");
            value["username"] = json!(username);
        }
        domain::Conversation::Unmapped(key) => {
            value["identity_status"] = json!("unmapped");
            value["username"] = Value::Null;
            value["unmapped_conversation"] = json!(
                crate::adapters::wechat::messages::read::diagnostics::legacy_unmapped_key(key)
            );
        }
    }
    add_sender_identity(&mut value, is_group, sender, &names.map, nicknames);
    match message.content {
        domain::Content::Structured(rich) => {
            value["rich"] = crate::message::structured_message::project(&rich);
        }
        domain::Content::Unavailable(issue) => {
            use crate::business::structured_message::ContentIssue;
            value["content_issue"] = json!(match issue {
                ContentIssue::UnsupportedKind => "unsupported_kind",
                ContentIssue::MalformedContent => "malformed_content",
                ContentIssue::InputTooLarge => "input_too_large",
                ContentIssue::NoSafePreview => "no_safe_preview",
            });
        }
        _ => {}
    }
    if let Some(url) = message.url {
        value["url"] = Value::String(url);
    }
    if let Some(call) = message.call {
        value["call"] = json!({"media": "unknown", "status_text": call.status_text, "duration_text": call.duration_text});
    }
    Ok(value)
}

pub(super) fn validate_history(options: &HistoryQuery<'_>) -> Result<usize> {
    let invalid =
        |message: &'static str| anyhow::Error::new(domain::Error::InvalidData).context(message);
    let limited = |message: &'static str| anyhow::Error::new(domain::Error::Limit).context(message);
    if options.filter.msg_type.is_some() && options.msg_types.is_some_and(|v| !v.is_empty()) {
        return Err(invalid("conflicting history msg_type and msg_types"));
    }
    if options.msg_types.is_some_and(|v| v.len() > 100) {
        return Err(limited("too many history types"));
    }
    if options.page.limit == 0 {
        return Err(limited("history limit must be positive"));
    }
    if options
        .filter
        .since
        .zip(options.filter.until)
        .is_some_and(|(since, until)| since > until)
    {
        return Err(invalid("invalid history time range"));
    }
    let size = options
        .page
        .offset
        .checked_add(options.page.limit)
        .ok_or_else(|| limited("history page overflow"))?;
    i64::try_from(size).map_err(|_| limited("history page exceeds SQLite integer range"))?;
    Ok(size)
}

pub(super) async fn history(
    db: &DbCache,
    names: &Names,
    chat: &str,
    options: HistoryQuery<'_>,
) -> Result<Value> {
    let per_stream = validate_history(&options)?;
    let page = domain::Page {
        limit: options.page.limit,
        offset: options.page.offset,
        oldest_first: options.oldest_first,
    };
    let legacy = LegacyReadPolicy {
        local_types: options
            .msg_types
            .filter(|v| !v.is_empty())
            .map(Vec::from)
            .unwrap_or_else(|| options.filter.msg_type.into_iter().collect()),
    };
    let filter = domain::Filter {
        since: options.filter.since,
        until: options.filter.until,
        kinds: Vec::new(),
    };
    filter.validate()?;
    let username = strict_message::username(chat, names)?;
    let prepared = prepare(db, names, domain::SourceKind::Ordinary).await?;
    let nicknames = if username.ends_with("@chatroom") {
        load_group_nicknames(db, &username).await?
    } else {
        HashMap::new()
    };
    let session_ts = session_last_timestamp(db, &username).await;
    let names_copy = names.clone();
    let username_copy = username.clone();
    let files = prepared.files;
    let scanned = files.len();
    let origins = prepared.origins;
    let result = tokio::task::spawn_blocking(move || -> Result<_> {
        let mut identities: Vec<_> = names_copy.map.keys().cloned().collect();
        identities.push(username_copy.clone());
        let snapshot = Snapshot::open(files, identities)?;
        let mut candidates = Vec::new();
        let mut shards = Vec::new();
        let mut hits = 0;
        let mut streams = snapshot
            .streams_for(&username_copy, domain::SourceKind::Ordinary)
            .into_iter()
            .map(|stream| Ok((stream, snapshot.latest_timestamp(stream)?.unwrap_or(0))))
            .collect::<Result<Vec<_>>>()?;
        streams.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for (rank, (stream, max_ts)) in streams.into_iter().enumerate() {
            let source = snapshot.source_name(stream)?.to_owned();
            let (path, mode) = origins.get(&source).context("message origin unavailable")?;
            shards.push(MessageShard {
                rel_key: source,
                path: path.clone(),
                table: snapshot.streams()[stream].table_name().into(),
                max_ts,
                cache_mode: *mode,
            });
            let rows = snapshot.read_legacy_page(
                stream,
                &filter,
                &legacy,
                per_stream,
                page.oldest_first,
            )?;
            if !rows.is_empty() {
                hits += 1;
            }
            for raw in rows {
                anyhow::ensure!(candidates.len() < 100_000, domain::Error::Limit);
                let mut order = snapshot.order_key(&raw)?;
                order.1 = rank;
                candidates.push(domain::Candidate {
                    order,
                    reference: raw.reference.clone(),
                    value: project(&snapshot, &raw, &names_copy, &nicknames)?,
                });
            }
        }
        shards.sort_by(|a, b| b.max_ts.cmp(&a.max_ts).then(a.rel_key.cmp(&b.rel_key)));
        Ok((
            page.select(candidates, domain::Completeness::Complete)?,
            shards,
            hits,
        ))
    })
    .await?;
    check_inventory(db, names, domain::SourceKind::Ordinary)?;
    let (messages, shards, hits) = result?;
    let windowed = options.page.offset > 0
        || options.filter.since.is_some()
        || options.filter.until.is_some()
        || options.filter.msg_type.is_some()
        || options.msg_types.is_some_and(|v| !v.is_empty())
        || options.oldest_first;
    let meta = meta_for_shards(
        scanned,
        &shards,
        hits,
        Vec::new(),
        session_ts,
        windowed,
        options.meta,
    );
    Ok(
        json!({"chat": names.display(&username), "username": username, "is_group": username.ends_with("@chatroom"), "chat_type": chat_type_of(&username, names), "count": messages.len(), "messages": messages, "meta": meta}),
    )
}

pub(super) async fn search(
    db: &DbCache,
    names: &Names,
    keyword: &str,
    chats: Option<Vec<String>>,
    limit: usize,
    filter: MessageFilter,
    options: MetaOptions,
) -> Result<Value> {
    let page = domain::Page {
        limit,
        offset: 0,
        oldest_first: false,
    };
    page.candidate_limit()?;
    let targets = chats
        .map(|chats| {
            chats
                .iter()
                .map(|chat| strict_message::username(chat, names))
                .collect::<Result<HashSet<_>>>()
        })
        .transpose()?;
    let prepared = prepare(db, names, domain::SourceKind::Ordinary).await?;
    let nicknames = load_group_nickname_maps(
        db,
        names
            .map
            .keys()
            .filter(|s| s.ends_with("@chatroom"))
            .cloned()
            .collect(),
    )
    .await?;
    let names_copy = names.clone();
    let keyword_copy = keyword.to_owned();
    let legacy = LegacyReadPolicy {
        local_types: filter.msg_type.into_iter().collect(),
    };
    let filter = domain::Filter {
        since: filter.since,
        until: filter.until,
        kinds: Vec::new(),
    };
    filter.validate()?;
    let scanned = prepared.files.len();
    let cache_modes = prepared
        .origins
        .iter()
        .map(|(k, (_, m))| (k.clone(), m.as_str().to_owned()))
        .collect();
    let paths = prepared
        .origins
        .iter()
        .map(|(k, (p, _))| (k.clone(), p.to_string_lossy().into_owned()))
        .collect();
    let result = tokio::task::spawn_blocking(move || -> Result<_> {
        let mut identities: Vec<_> = names_copy.map.keys().cloned().collect();
        if let Some(targets) = &targets {
            identities.extend(targets.iter().cloned());
        }
        let snapshot = Snapshot::open(prepared.files, identities)?;
        let mut candidates = Vec::new();
        let mut hits = HashSet::new();
        let empty = HashMap::new();
        for (stream, entry) in snapshot.streams().iter().enumerate() {
            if snapshot.source_kind(stream)? != domain::SourceKind::Ordinary {
                continue;
            }
            let username = match &entry.conversation {
                domain::Conversation::Known(name) => name.as_str(),
                _ => "",
            };
            if targets.as_ref().is_some_and(|set| !set.contains(username)) {
                continue;
            }
            for raw in
                snapshot.search_legacy_page(stream, &filter, &legacy, &keyword_copy, limit)?
            {
                anyhow::ensure!(candidates.len() < 100_000, domain::Error::Limit);
                hits.insert(raw.logical_source.clone());
                let mut value = project(
                    &snapshot,
                    &raw,
                    &names_copy,
                    nicknames.get(username).unwrap_or(&empty),
                )?;
                value["chat"] = Value::String(if username.is_empty() {
                    entry.table_name().into()
                } else {
                    names_copy.display(username)
                });
                if !username.is_empty() {
                    value["username"] = Value::String(username.into());
                }
                candidates.push(domain::Candidate {
                    order: snapshot.order_key(&raw)?,
                    reference: raw.reference.clone(),
                    value: (
                        value,
                        matches!(&entry.conversation, domain::Conversation::Known(_)),
                    ),
                });
            }
        }
        let mut page = page.select(candidates, domain::Completeness::Complete)?;
        page.reverse();
        let unresolved = page.iter().filter(|(_, mapped)| !mapped).count();
        Ok((
            page.into_iter().map(|(value, _)| value).collect::<Vec<_>>(),
            hits.len(),
            unresolved,
        ))
    })
    .await?;
    check_inventory(db, names, domain::SourceKind::Ordinary)?;
    let (results, hits, unresolved) = result?;
    let mut meta = serde_json::to_value(meta_for_global_query(
        scanned,
        hits,
        Vec::new(),
        true,
        options,
        Some(cache_modes),
        Some(paths),
    ))?;
    meta["identity_complete"] = json!(unresolved == 0);
    meta["unresolved_identities"] = json!(unresolved);
    Ok(json!({"keyword": keyword, "count": results.len(), "results": results, "meta": meta}))
}
