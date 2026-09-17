use crate::application::voice_batch_export;
use crate::infrastructure::audio;
use anyhow::Result;
use std::path::PathBuf;

pub(super) fn convert(input: String, output: Option<String>) -> Result<()> {
    let context = crate::application::publication_context::PublicationContext::current()?;
    let output = output.unwrap_or_else(|| {
        let mut path = PathBuf::from(&input);
        path.set_extension("mp3");
        path.to_string_lossy().into_owned()
    });
    let protected = context.protected(std::path::Path::new(&input))?;
    let result = audio::convert_silk_to_mp3_checked(
        std::path::Path::new(&input),
        std::path::Path::new(&output),
        &protected,
        || context.verify(),
    )?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

pub(super) fn export(
    config: PathBuf,
    output_dir: Option<PathBuf>,
    contacts: Option<String>,
) -> Result<()> {
    let mut options = voice_batch_export::BatchOptions::from_config_file(&config)?;
    if let Some(output) = output_dir {
        options.output_dir = output;
    }
    if let Some(contacts) = contacts {
        options.contacts = voice_batch_export::parse_contact_filter(&contacts);
    }
    // The operation worker's Job owns disconnect/cancellation cleanup.
    let report = voice_batch_export::convert_database_checked(&options, &[config], || false)?;
    finish_voice_batch(&report)
}

fn finish_voice_batch(report: &voice_batch_export::BatchReport) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(report)?);
    voice_batch_outcome(report).require_success()?;
    Ok(())
}

pub(super) fn voice_batch_outcome(
    report: &voice_batch_export::BatchReport,
) -> crate::ipc::outcome::BusinessOutcome {
    use crate::business::voice_export::BatchState;
    use crate::ipc::outcome::BusinessOutcome;
    match report.progress.state() {
        BatchState::Success => BusinessOutcome::Success,
        BatchState::Partial => BusinessOutcome::Partial,
        BatchState::Failure => BusinessOutcome::Failure,
    }
}

#[test]
fn voice_batch_report_preserves_partial_classification() {
    use crate::ipc::outcome::{BusinessFailure, BusinessOutcome};
    for (converted, skipped_existing, failed, expected) in [
        (1, 0, 0, BusinessOutcome::Success),
        (1, 0, 1, BusinessOutcome::Partial),
        (0, 1, 1, BusinessOutcome::Partial),
        (0, 0, 1, BusinessOutcome::Failure),
    ] {
        let report = voice_batch_export::BatchReport {
            progress: crate::business::voice_export::BatchProgress {
                converted,
                skipped_existing,
                failed,
                filtered: 3,
                ..Default::default()
            },
            ..Default::default()
        };
        let actual = finish_voice_batch(&report).map_or_else(
            |error| error.downcast_ref::<BusinessFailure>().unwrap().0,
            |_| BusinessOutcome::Success,
        );
        assert_eq!(actual, expected);
    }
}
