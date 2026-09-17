use crate::application::image_publication;
use anyhow::Result;

pub(super) fn cache(
    attach_dir: Option<String>,
    decoded_dir: Option<String>,
    aes_key: Option<String>,
    xor_key: Option<String>,
    force: bool,
) -> Result<()> {
    let runtime = crate::runtime::RuntimeContext::for_operation()?;
    let needs_stored =
        runtime.config.key_store.is_some() && (aes_key.is_none() || xor_key.is_none());
    let stored = needs_stored
        .then(|| super::image_keys::publication_material(&runtime))
        .transpose()?;
    image_publication::decode_images_current(
        attach_dir,
        decoded_dir,
        aes_key,
        xor_key,
        force,
        stored,
    )?;
    if needs_stored {
        crate::service::worker_keys::verify_image_revision(&runtime)?;
    }
    Ok(())
}

pub(super) fn image(dat_file: String, output_file: Option<String>) -> Result<()> {
    let runtime = crate::runtime::RuntimeContext::load()?;
    let stored = super::image_keys::publication_material(&runtime)?;
    image_publication::decode_image_for(&runtime, dat_file, output_file, stored)?;
    crate::service::worker_keys::verify_image_revision(&runtime)
}

pub(super) fn directory(input_dir: String, output_dir: Option<String>) -> Result<()> {
    let runtime = crate::runtime::RuntimeContext::load()?;
    let stored = super::image_keys::publication_material(&runtime)?;
    image_publication::batch_images_for(&runtime, input_dir, output_dir, stored)?;
    crate::service::worker_keys::verify_image_revision(&runtime)
}
