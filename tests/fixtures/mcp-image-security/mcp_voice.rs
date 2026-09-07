//! Unrelated host voice orchestration is not exercised by the image host probe.
use crate::{
    ipc::Response,
    protocol::{CallContext, DispatchError},
    runtime::RuntimeContext,
};
#[derive(clap::Args, Debug, Clone, Default)]
#[group(id = "AuditVoiceBoundaryArgs")]
pub struct Args {}
pub enum Operation {
    Decode,
    Transcribe,
}
#[derive(Clone, Copy)]
pub struct Pending;
impl Args {
    pub fn prepare(
        &self,
        _: Operation,
        _: i64,
        _: Option<&std::path::Path>,
        _: &CallContext,
    ) -> Result<Pending, DispatchError> {
        panic!("voice host orchestration is outside this image probe; actual voice publisher is tested separately")
    }
}
impl Pending {
    pub fn bind(self, _: &RuntimeContext) -> Result<Self, DispatchError> {
        panic!("unrelated voice host")
    }
    pub fn finish(
        self,
        _: Response,
        _: &RuntimeContext,
        _: &CallContext,
        _: impl Fn() -> Result<(), DispatchError>,
    ) -> Result<Response, DispatchError> {
        panic!("unrelated voice host")
    }
}
