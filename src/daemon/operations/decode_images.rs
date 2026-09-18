use crate::application::image_publication;
use anyhow::Result;

pub(super) fn cache(
    attach_dir: Option<String>,
    decoded_dir: Option<String>,
    xor_key: Option<String>,
    force: bool,
) -> Result<()> {
    let runtime = crate::runtime::RuntimeContext::for_operation()?;
    let stored = super::image_keys::publication_material(&runtime)?;
    image_publication::decode_images_for(
        &runtime,
        attach_dir,
        decoded_dir,
        xor_key,
        force,
        stored,
    )?;
    crate::service::worker_keys::verify_image_revision(&runtime)
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
