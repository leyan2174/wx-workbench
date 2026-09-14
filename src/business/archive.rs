//! Incremental archive workflow. Raw export documents are opaque to this layer.
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Read,
    Identity,
    Publish,
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
}
