//! Incremental archive workflow. Raw export documents are opaque to this layer.
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Prepare,
    Read,
    Identity,
    SourceIdentity,
    ProcessedIdentity,
    Transform,
    Publish,
    Index,
    Manifest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub stage: Stage,
    pub detail: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.stage, self.detail)
    }
}
impl std::error::Error for Failure {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub start: i64,
    pub end: Option<i64>,
}

pub struct DeltaPlan {
    usernames: Vec<String>,
    range: Range,
}

impl DeltaPlan {
    pub fn new(usernames: &[String], range: Range) -> Result<Self, Failure> {
        if usernames.is_empty()
            || usernames.iter().any(String::is_empty)
            || range.end.is_some_and(|end| end < range.start)
        {
            return Err(Failure {
                stage: Stage::Read,
                detail: "delta archive requires nonempty targets and an ordered time range".into(),
            });
        }
        let mut seen = HashSet::new();
        Ok(Self {
            usernames: usernames
                .iter()
                .filter(|name| seen.insert(*name))
                .cloned()
                .collect(),
            range,
        })
    }
}

/// An explicit raw-export payload, not an ordinary message business object.
pub struct RawArchive<D> {
    pub username: String,
    pub document: D,
}

pub fn select_usernames<'a>(
    available: impl IntoIterator<Item = &'a str>,
    requested: &[&str],
) -> Result<HashSet<String>, Failure> {
    let requested: HashSet<_> = requested.iter().copied().collect();
    let selected: HashSet<_> = available
        .into_iter()
        .filter(|name| requested.contains(name))
        .map(str::to_owned)
        .collect();
    if selected.is_empty() {
        return Err(Failure {
            stage: Stage::Prepare,
            detail: "requested usernames do not match any available target".into(),
        });
    }
    Ok(selected)
}

pub struct PreparedArchive<D> {
    pub archive: RawArchive<D>,
    pub messages: usize,
    pub added_messages: usize,
}

/// A single fixed-account batch. Implementations own raw-format conversion and
/// guarded file access; the use case owns ordering and publication outcomes.
pub trait FullArchive {
    type Document;
    fn prepare(&mut self, username: &str) -> Result<(), Failure>;
    fn read(&mut self, username: &str) -> Result<RawArchive<Self::Document>, Failure>;
    fn transform(
        &mut self,
        username: &str,
        document: Self::Document,
    ) -> Result<PreparedArchive<Self::Document>, Failure>;
    fn publish(&mut self, document: &Self::Document) -> Result<(), Failure>;
    fn record(&mut self, username: &str) -> Result<(), Failure>;
}

pub struct FullTargetResult {
    pub username: String,
    pub result: Result<(usize, usize), Failure>,
    pub artifact_published: bool,
}

pub fn export_chats(usernames: &[String], archive: &mut impl FullArchive) -> Vec<FullTargetResult> {
    usernames
        .iter()
        .map(|username| {
            let mut artifact_published = false;
            let result = (|| {
                archive.prepare(username)?;
                let read = archive.read(username)?;
                if read.username != *username {
                    return Err(Failure {
                        stage: Stage::SourceIdentity,
                        detail: "returned chat identity does not match request".into(),
                    });
                }
                let prepared = archive.transform(username, read.document)?;
                if prepared.archive.username != *username {
                    return Err(Failure {
                        stage: Stage::ProcessedIdentity,
                        detail: "processed chat identity does not match request".into(),
                    });
                }
                archive.publish(&prepared.archive.document)?;
                artifact_published = true;
                archive.record(username)?;
                Ok((prepared.messages, prepared.added_messages))
            })();
            FullTargetResult {
                username: username.clone(),
                result,
                artifact_published,
            }
        })
        .collect()
}

pub trait DeltaSource {
    type Document;
    fn read(&mut self, username: &str, range: Range)
        -> Result<RawArchive<Self::Document>, Failure>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Publication {
    Written { messages: u64 },
    Skipped,
    Failed(Failure),
}

/// Records failures in the same manifest as successful artifacts. A failed finish
/// never yields a completed run; existing artifacts are not a resumable checkpoint.
pub trait DeltaPublisher<D> {
    type Manifest;
    fn record(&mut self, username: &str, document: Result<D, Failure>) -> Publication;
    fn finish(self) -> Result<Self::Manifest, Failure>;
}

#[derive(Debug, PartialEq, Eq)]
pub struct TargetResult {
    pub username: String,
    pub publication: Publication,
}

pub struct Completed<M> {
    pub manifest: M,
    pub targets: Vec<TargetResult>,
}

impl<M> Completed<M> {
    pub fn success(&self) -> bool {
        self.targets
            .iter()
            .all(|target| !matches!(target.publication, Publication::Failed(_)))
    }

    pub fn messages(&self) -> u64 {
        self.targets
            .iter()
            .map(|target| match target.publication {
                Publication::Written { messages } => messages,
                _ => 0,
            })
            .sum()
    }
}

pub fn export_delta<S: DeltaSource, P: DeltaPublisher<S::Document>>(
    plan: DeltaPlan,
    mut source: S,
    mut publisher: P,
) -> Result<Completed<P::Manifest>, Failure> {
    let mut targets = Vec::with_capacity(plan.usernames.len());
    for username in plan.usernames {
        let document = source.read(&username, plan.range).and_then(|read| {
            if read.username != username {
                Err(Failure {
                    stage: Stage::Identity,
                    detail: "delta source username mismatch".into(),
                })
            } else {
                Ok(read.document)
            }
        });
        // A publisher cannot turn a source/identity failure into an empty success.
        let source_failure = document.as_ref().err().cloned();
        let publication = publisher.record(&username, document);
        targets.push(TargetResult {
            username,
            publication: source_failure
                .map(Publication::Failed)
                .unwrap_or(publication),
        });
    }
    Ok(Completed {
        manifest: publisher.finish()?,
        targets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    struct Source(Rc<RefCell<Vec<String>>>);
    impl DeltaSource for Source {
        type Document = u64;
        fn read(&mut self, username: &str, range: Range) -> Result<RawArchive<u64>, Failure> {
            assert_eq!(
                range,
                Range {
                    start: 10,
                    end: Some(20)
                }
            );
            self.0.borrow_mut().push(username.into());
            Ok(RawArchive {
                username: if username == "wrong" {
                    "other"
                } else {
                    username
                }
                .into(),
                document: 2,
            })
        }
    }
    struct Sink {
        recorded: Rc<RefCell<Vec<String>>>,
        fail_finish: bool,
    }
    impl DeltaPublisher<u64> for Sink {
        type Manifest = ();
        fn record(&mut self, username: &str, value: Result<u64, Failure>) -> Publication {
            self.recorded.borrow_mut().push(username.into());
            match value {
                Err(error) => Publication::Failed(error),
                Ok(_) if username == "broken" => Publication::Failed(Failure {
                    stage: Stage::Publish,
                    detail: "synthetic write error".into(),
                }),
                Ok(messages) => Publication::Written { messages },
            }
        }
        fn finish(self) -> Result<(), Failure> {
            if self.fail_finish {
                Err(Failure {
                    stage: Stage::Manifest,
                    detail: "synthetic manifest error".into(),
                })
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn validates_before_io_and_deduplicates_targets_not_messages() {
        assert!(DeltaPlan::new(
            &[],
            Range {
                start: 10,
                end: None
            }
        )
        .is_err());
        assert!(DeltaPlan::new(
            &["a".into()],
            Range {
                start: 10,
                end: Some(9)
            }
        )
        .is_err());
        let names = ["a", "wrong", "a", "broken", "last"].map(String::from);
        let calls = Rc::new(RefCell::new(Vec::new()));
        let records = Rc::new(RefCell::new(Vec::new()));
        let report = export_delta(
            DeltaPlan::new(
                &names,
                Range {
                    start: 10,
                    end: Some(20),
                },
            )
            .unwrap(),
            Source(calls.clone()),
            Sink {
                recorded: records.clone(),
                fail_finish: false,
            },
        )
        .unwrap();
        assert_eq!(*calls.borrow(), ["a", "wrong", "broken", "last"]);
        assert_eq!(*records.borrow(), *calls.borrow());
        assert!(!report.success());
        assert_eq!(report.messages(), 4);
        assert!(matches!(
            &report.targets[1].publication,
            Publication::Failed(Failure {
                stage: Stage::Identity,
                ..
            })
        ));
    }
    #[test]
    fn manifest_failure_does_not_complete_a_run() {
        let result = export_delta(
            DeltaPlan::new(
                &["a".into()],
                Range {
                    start: 10,
                    end: Some(20),
                },
            )
            .unwrap(),
            Source(Rc::new(RefCell::new(Vec::new()))),
            Sink {
                recorded: Rc::new(RefCell::new(Vec::new())),
                fail_finish: true,
            },
        );
        assert!(matches!(
            result,
            Err(Failure {
                stage: Stage::Manifest,
                ..
            })
        ));
    }

    #[derive(Default)]
    struct Full {
        current: String,
        events: Vec<(String, Stage)>,
    }

    #[test]
    fn username_selection_preserves_partial_matches_without_display_name_lookup() {
        assert_eq!(
            select_usernames(["alpha", "beta"], &["alpha", "absent", "alpha"]).unwrap(),
            HashSet::from(["alpha".to_owned()])
        );
        assert!(select_usernames(["alpha"], &["display name"]).is_err());
        assert!(select_usernames(["alpha"], &[]).is_err());
    }

    impl Full {
        fn step(&mut self, stage: Stage) -> Result<(), Failure> {
            self.events.push((self.current.clone(), stage));
            if matches!(
                (self.current.as_str(), stage),
                ("bad-prepare", Stage::Prepare)
                    | ("bad-source", Stage::Read)
                    | ("bad-publish", Stage::Publish)
                    | ("bad-index", Stage::Index)
            ) {
                return Err(Failure {
                    stage,
                    detail: "synthetic failure".into(),
                });
            }
            Ok(())
        }
    }

    impl FullArchive for Full {
        type Document = String;
        fn prepare(&mut self, username: &str) -> Result<(), Failure> {
            self.current = username.into();
            self.step(Stage::Prepare)
        }
        fn read(&mut self, username: &str) -> Result<RawArchive<String>, Failure> {
            self.step(Stage::Read)?;
            Ok(RawArchive {
                username: if username == "bad-read-identity" {
                    "other"
                } else {
                    username
                }
                .into(),
                document: username.into(),
            })
        }
        fn transform(
            &mut self,
            username: &str,
            document: String,
        ) -> Result<PreparedArchive<String>, Failure> {
            self.step(Stage::Transform)?;
            Ok(PreparedArchive {
                archive: RawArchive {
                    username: if username == "bad-transformed-identity" {
                        "other"
                    } else {
                        username
                    }
                    .into(),
                    document,
                },
                messages: 4,
                added_messages: 2,
            })
        }
        fn publish(&mut self, document: &String) -> Result<(), Failure> {
            assert_eq!(document, &self.current);
            self.step(Stage::Publish)
        }
        fn record(&mut self, username: &str) -> Result<(), Failure> {
            assert_eq!(username, self.current);
            self.step(Stage::Index)
        }
    }

    #[test]
    fn full_archive_orders_stages_and_continues_without_indexing_failed_artifacts() {
        let names: Vec<_> = [
            "bad-prepare",
            "bad-source",
            "bad-read-identity",
            "bad-transformed-identity",
            "bad-publish",
            "bad-index",
            "after",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let mut archive = Full::default();
        let results = export_chats(&names, &mut archive);
        assert_eq!(results.len(), names.len());
        for result in &results[..5] {
            assert!(result.result.is_err());
            assert!(!result.artifact_published);
            assert!(!archive
                .events
                .contains(&(result.username.clone(), Stage::Index)));
        }
        assert_eq!(
            results[2].result.as_ref().unwrap_err().stage,
            Stage::SourceIdentity
        );
        assert_eq!(
            results[3].result.as_ref().unwrap_err().stage,
            Stage::ProcessedIdentity
        );
        assert!(results[5].artifact_published);
        assert_eq!(results[5].result.as_ref().unwrap_err().stage, Stage::Index);
        assert_eq!(results[6].result, Ok((4, 2)));
        assert!(results[6].artifact_published);
        let stages: Vec<_> = archive
            .events
            .iter()
            .filter(|(name, _)| name == "after")
            .map(|(_, stage)| *stage)
            .collect();
        assert_eq!(
            stages,
            [
                Stage::Prepare,
                Stage::Read,
                Stage::Transform,
                Stage::Publish,
                Stage::Index
            ]
        );
    }
}
