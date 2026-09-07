use crate::local_files::HostOutputGuard;
use std::{
    cell::{Cell, RefCell},
    path::Path,
};
#[path = "../../../src/toolkit/audio/publish.rs"]
mod voice;
pub use voice::PublishedWav;

pub struct Guard(HostOutputGuard);
impl Guard {
    pub fn new(output: &Path, protected: &Path) -> anyhow::Result<Self> {
        let mut guard = HostOutputGuard::new(output)?;
        guard.protect(protected)?;
        Ok(Self(guard))
    }
}
pub fn voice(
    wav: &[u8],
    guard: &Guard,
    callback: impl FnOnce(&PublishedWav) -> anyhow::Result<()>,
) -> anyhow::Result<PublishedWav> {
    voice::publish_wav_noclobber(wav, &guard.0, callback)
}
thread_local! {
    static AFTER_SYNC:RefCell<Option<Box<dyn FnOnce()>>>=RefCell::new(None);
    static FIRED:Cell<bool>=const { Cell::new(false) };
}
pub(crate) fn after_image_sync() {
    let callback = AFTER_SYNC.with(|slot| slot.borrow_mut().take());
    if let Some(callback) = callback {
        FIRED.with(|f| f.set(true));
        callback();
    }
}
pub fn image(
    request: crate::instrumented_image::ImageRequest<'_>,
    guard: &Guard,
    callback: impl FnOnce() + 'static,
) -> anyhow::Result<crate::instrumented_image::ImageOutput> {
    AFTER_SYNC.with(|slot| assert!(slot.borrow_mut().replace(Box::new(callback)).is_none()));
    FIRED.with(|f| f.set(false));
    let result = crate::instrumented_image::export_image_with_guard(request, &guard.0);
    AFTER_SYNC.with(|slot| slot.borrow_mut().take());
    assert!(
        FIRED.with(Cell::get),
        "test must reach the actual after-sync publication window"
    );
    result
}
