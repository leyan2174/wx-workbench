use anyhow::Result;
use serde::Serialize;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Default, Serialize)]
pub(super) struct Report {
    pub(super) total: usize,
    pub(super) written: usize,
    pub(super) skipped: usize,
    pub(super) skipped_no_key: usize,
    pub(super) formats: BTreeMap<String, usize>,
    pub(super) planned: usize,
    pub(super) failures: Vec<Failure>,
}

#[derive(Serialize)]
pub(super) struct Failure {
    pub(super) path: PathBuf,
    pub(super) error: String,
}

impl Report {
    pub(super) fn finish(&self) -> Result<()> {
        println!("{}", serde_json::to_string_pretty(self)?);
        crate::ipc::outcome::BusinessOutcome::from_counts(
            self.written
                .saturating_add(self.skipped)
                .saturating_add(self.planned) as u64,
            self.failures.len() as u64,
        )
        .require_success()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::outcome::{BusinessFailure, BusinessOutcome};

    #[test]
    fn finish_distinguishes_partial_without_counting_missing_keys() {
        for (written, skipped, planned, failed, expected) in [
            (1, 0, 0, false, BusinessOutcome::Success),
            (1, 0, 0, true, BusinessOutcome::Partial),
            (0, 1, 0, true, BusinessOutcome::Partial),
            (0, 0, 1, true, BusinessOutcome::Partial),
            (0, 0, 0, true, BusinessOutcome::Failure),
            (0, 0, 0, false, BusinessOutcome::Success),
        ] {
            let mut report = Report {
                written,
                skipped,
                planned,
                skipped_no_key: 3,
                ..Default::default()
            };
            if failed {
                report.failures.push(Failure {
                    path: "synthetic.db".into(),
                    error: "synthetic item failure".into(),
                });
            }
            let actual = report.finish().map_or_else(
                |error| error.downcast_ref::<BusinessFailure>().unwrap().0,
                |_| BusinessOutcome::Success,
            );
            assert_eq!(actual, expected);
        }
    }
}
