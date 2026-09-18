//! Validate host-sealed material locally before the existing process-bound CAS transaction.
use crate::{
    runtime::RuntimeContext,
    service::{config_pin::ConfigPin, image_import::Args, worker_keys},
};
use anyhow::{ensure, Result};

fn rejected() -> anyhow::Error {
    crate::service::protocol::ServiceError::new(
        "image_import_rejected",
        "Image material import validation failed",
    )
    .into()
}

pub(crate) fn execute(args: Args) -> Result<()> {
    let runtime = RuntimeContext::load()?;
    let pin = ConfigPin::new(&runtime)?;
    let opened = args.open(&runtime).map_err(|_| rejected())?;
    worker_keys::verify_image_import_revision(&runtime, opened.expected_revision)?;
    let (root_guard, identity) = crate::attachment::image_key::pin_image_root(&opened.sample_root)
        .map_err(|_| rejected())?;
    ensure!(identity == opened.sample_identity, rejected());
    let samples = crate::attachment::image_key::validate_material_for_image_root(
        &opened.sample_root,
        &opened.material.aes,
        std::time::Duration::from_secs(opened.options.timeout),
        (opened.options.max_mib as u64)
            .checked_mul(1024 * 1024)
            .ok_or_else(rejected)?,
    )
    .map_err(|_| rejected())?;
    pin.verify(&runtime).map_err(|_| rejected())?;
    samples.verify().map_err(|_| rejected())?;
    root_guard.verify().map_err(|_| rejected())?;
    worker_keys::verify_image_import_revision(&runtime, opened.expected_revision)?;
    let revision = if opened.options.no_save {
        opened.expected_revision
    } else {
        worker_keys::commit_image_import_sync(&runtime, opened.expected_revision, &opened.material)?
    };
    println!(
        "{}",
        serde_json::json!({"verified":true,"validation":"v2_aes_template","saved":!opened.options.no_save,"revision":revision})
    );
    Ok(())
}
