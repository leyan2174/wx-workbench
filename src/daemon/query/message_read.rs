//! Account host preparation and compatibility projections; message storage lives in the adapter.
use super::*;
use crate::{
    adapters::wechat::messages::{
        pages::{LegacyMessageProjection, PageDiagnostics},
        LegacyReadPolicy, Snapshot, SourceFile,
    },
    business::messages as domain,
};
use std::{collections::BTreeMap, path::PathBuf};

pub(super) struct Prepared {
    pub files: Vec<SourceFile>,
    origins: BTreeMap<String, (PathBuf, CacheMode)>,
}

impl Prepared {
    fn open(&self, identities: impl IntoIterator<Item = String>) -> Result<Snapshot> {
        Snapshot::open(self.files.clone(), identities)
    }

    // Explicit legacy diagnostics only; page selection never consumes host paths.
    fn history_metadata(
        &self,
        diagnostics: &PageDiagnostics,
        session_ts: Option<i64>,
        windowed: bool,
        options: MetaOptions,
    ) -> Result<Meta> {
        let shards = diagnostics
            .shards
            .iter()
            .map(|shard| {
                let (path, mode) = self
                    .origins
                    .get(&shard.logical_source)
                    .context("message origin unavailable")?;
                Ok(MessageShard {
                    rel_key: shard.logical_source.clone(),
                    path: path.clone(),
                    table: shard.table.clone(),
                    max_ts: shard.latest_timestamp,
                    cache_mode: *mode,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(meta_for_shards(
            self.files.len(),
            &shards,
            diagnostics.hits,
            Vec::new(),
            session_ts,
            windowed,
            options,
        ))
    }

    fn global_metadata(&self, diagnostics: &PageDiagnostics, options: MetaOptions) -> Meta {
        let modes = self
            .origins
            .iter()
            .map(|(key, (_, mode))| (key.clone(), mode.as_str().to_owned()))
            .collect();
        let paths = self
            .origins
            .iter()
            .map(|(key, (path, _))| (key.clone(), path.to_string_lossy().into_owned()))
            .collect();
        meta_for_global_query(
            self.files.len(),
            diagnostics.hits,
            Vec::new(),
            true,
            options,
            Some(modes),
            Some(paths),
        )
    }
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
    let username = chat_identity::resolve(db, names, chat).await?;
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
    let message_usernames = if selected
        .iter()
        .any(|&index| chat_identity::is_folded(&records[index].session.username))
    {
        chat_identity::message_usernames(
            db,
            names,
            records
                .iter()
                .map(|record| record.session.username.clone())
                .collect(),
        )
        .await
        .ok()
    } else {
        None
    };
    let mut slots: Vec<_> = records.into_iter().map(Some).collect();
    let mut results = Vec::new();
    let mut nickname_cache = HashMap::new();
    for index in selected {
        let mut value = session_view(
            db,
            names,
            slots[index].take().context("duplicate session selection")?,
            &mut nickname_cache,
        )
        .await;
        let has_message_table = message_usernames.as_ref().map(|usernames| {
            value["username"]
                .as_str()
                .is_some_and(|username| usernames.contains(username))
        });
        chat_identity::mark_exportability(&mut value, has_message_table);
        results.push(value);
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
    let result = tokio::task::spawn_blocking(move || -> Result<_> {
        let mut identities: Vec<_> = names_copy.map.keys().cloned().collect();
        identities.extend(changed.iter().map(|(name, _)| name.clone()));
        let snapshot = prepared.open(identities)?;
        let read = snapshot.new_messages_page(&changed, &page)?;
        let empty = HashMap::new();
        let mut messages = Vec::new();
        let mut delivered = Vec::new();
        for message in &read.page.messages {
            let domain::Conversation::Known(username) = &message.conversation else {
                anyhow::bail!(domain::Error::InvalidData);
            };
            let mut value = project(
                message,
                read.legacy.message(&message.reference)?,
                &names_copy,
                nicknames.get(username).unwrap_or(&empty),
            )?;
            value["username"] = Value::String(username.clone());
            value["chat"] = Value::String(names_copy.display(username));
            value["is_group"] = Value::Bool(username.ends_with("@chatroom"));
            value["chat_type"] = Value::String(chat_type_of(username, &names_copy).into());
            delivered.push((username.clone(), message.timestamp));
            messages.push(value);
        }
        Ok((
            messages,
            delivered,
            prepared.global_metadata(&read.diagnostics, options),
        ))
    })
    .await?;
    check_inventory(db, names, domain::SourceKind::Ordinary)?;
    let (messages, delivered, meta) = result?;
    Ok(json!({"count": messages.len(), "messages": messages,
        "new_state": subscription.advance(&delivered), "meta": meta}))
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
    message: &domain::Message,
    legacy: &LegacyMessageProjection,
    names: &Names,
    nicknames: &HashMap<String, String>,
) -> Result<Value> {
    let username = match &message.conversation {
        domain::Conversation::Known(name) => name.as_str(),
        _ => "",
    };
    let is_group = username.ends_with("@chatroom");
    let sender = message.sender.as_deref().unwrap_or("");
    let mut value = serde_json::to_value(legacy)?;
    value["timestamp"] = json!(message.timestamp);
    value["time"] = json!(fmt_time(message.timestamp, "%Y-%m-%d %H:%M"));
    value["sender"] = json!(if sender.is_empty() {
        String::new()
    } else {
        sender_display(sender, "", &names.map, nicknames)
    });
    value["content"] = json!(message.preview);
    match &message.conversation {
        domain::Conversation::Known(username) => {
            value["identity_status"] = json!("mapped");
            value["username"] = json!(username);
        }
        domain::Conversation::Unmapped(_) => {
            value["identity_status"] = json!("unmapped");
            value["username"] = Value::Null;
        }
    }
    add_sender_identity(&mut value, is_group, sender, &names.map, nicknames);
    match &message.content {
        domain::Content::Structured(rich) => {
            value["rich"] = crate::message::structured_message::project(rich);
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
    if let Some(url) = &message.url {
        value["url"] = Value::String(url.clone());
    }
    if let Some(call) = &message.call {
        value["call"] = json!({"media": "unknown", "status_text": call.status_text, "duration_text": call.duration_text});
    }
    Ok(value)
}

pub(super) fn validate_history(options: &HistoryQuery<'_>) -> Result<usize> {
    let invalid =
        |message: &'static str| anyhow::Error::new(domain::Error::InvalidData).context(message);
    if options.filter.msg_type.is_some() && options.msg_types.is_some_and(|v| !v.is_empty()) {
        return Err(invalid("conflicting history msg_type and msg_types"));
    }
    if options.msg_types.is_some_and(|v| v.len() > 100) {
        return Err(invalid("too many history types"));
    }
    if options.page.limit == 0 {
        return Err(invalid("history limit must be positive"));
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
        .ok_or_else(|| invalid("history page overflow"))?;
    i64::try_from(size).map_err(|_| invalid("history page exceeds SQLite integer range"))?;
    Ok(size)
}

pub(super) async fn history(
    db: &DbCache,
    names: &Names,
    chat: &str,
    options: HistoryQuery<'_>,
) -> Result<Value> {
    validate_history(&options)?;
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
    let username = chat_identity::resolve(db, names, chat).await?;
    let prepared = prepare(db, names, domain::SourceKind::Ordinary).await?;
    let nicknames = if username.ends_with("@chatroom") {
        load_group_nicknames(db, &username).await?
    } else {
        HashMap::new()
    };
    let session_ts = session_last_timestamp(db, &username).await;
    let windowed = options.page.offset > 0
        || options.filter.since.is_some()
        || options.filter.until.is_some()
        || options.filter.msg_type.is_some()
        || options.msg_types.is_some_and(|v| !v.is_empty())
        || options.oldest_first;
    let meta_options = options.meta;
    let names_copy = names.clone();
    let username_copy = username.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<_> {
        let mut identities: Vec<_> = names_copy.map.keys().cloned().collect();
        identities.push(username_copy.clone());
        let snapshot = prepared.open(identities)?;
        let read = snapshot.history_page(&username_copy, &filter, &legacy, &page)?;
        let messages = read
            .page
            .messages
            .iter()
            .map(|message| {
                project(
                    message,
                    read.legacy.message(&message.reference)?,
                    &names_copy,
                    &nicknames,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let meta =
            prepared.history_metadata(&read.diagnostics, session_ts, windowed, meta_options)?;
        Ok((messages, meta))
    })
    .await?;
    check_inventory(db, names, domain::SourceKind::Ordinary)?;
    let (messages, meta) = result?;
    Ok(
        json!({"chat": names.display(&username), "username": username,
        "is_group": username.ends_with("@chatroom"), "chat_type": chat_type_of(&username, names),
        "count": messages.len(), "messages": messages, "meta": meta}),
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
    let targets = if let Some(chats) = chats {
        let mut targets = HashSet::new();
        for chat in chats {
            targets.insert(chat_identity::resolve(db, names, &chat).await?);
        }
        Some(targets)
    } else {
        None
    };
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
    let result = tokio::task::spawn_blocking(move || -> Result<_> {
        let mut identities: Vec<_> = names_copy.map.keys().cloned().collect();
        if let Some(targets) = &targets {
            identities.extend(targets.iter().cloned());
        }
        let snapshot = prepared.open(identities)?;
        let read =
            snapshot.search_page(targets.as_ref(), &filter, &legacy, &keyword_copy, &page)?;
        let unresolved = read.page.unresolved_conversations();
        let empty = HashMap::new();
        let results = read
            .page
            .messages
            .iter()
            .map(|message| {
                let wire = read.legacy.message(&message.reference)?;
                let username = match &message.conversation {
                    domain::Conversation::Known(name) => name.as_str(),
                    domain::Conversation::Unmapped(_) => "",
                };
                let mut value = project(
                    message,
                    wire,
                    &names_copy,
                    nicknames.get(username).unwrap_or(&empty),
                )?;
                value["chat"] = Value::String(match &message.conversation {
                    domain::Conversation::Known(name) => names_copy.display(name),
                    domain::Conversation::Unmapped(_) => wire
                        .unmapped_chat_label()
                        .context("legacy unresolved chat display unavailable")?
                        .to_owned(),
                });
                Ok(value)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((
            results,
            unresolved,
            prepared.global_metadata(&read.diagnostics, options),
        ))
    })
    .await?;
    check_inventory(db, names, domain::SourceKind::Ordinary)?;
    let (results, unresolved, meta) = result?;
    let mut meta = serde_json::to_value(meta)?;
    meta["identity_complete"] = json!(unresolved == 0);
    meta["unresolved_identities"] = json!(unresolved);
    Ok(json!({"keyword": keyword, "count": results.len(), "results": results, "meta": meta}))
}
