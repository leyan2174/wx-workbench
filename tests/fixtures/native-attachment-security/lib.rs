//! 独立安全审查：复用已有 fixture 的真实模块，不注册或修改生产 root。
pub use attachment_refs_contract::attachment_refs;

pub mod toolkit {
    pub use crate::attachment_refs;
}

#[cfg(test)]
mod query_boundary;

#[cfg(test)]
mod refs_security;

#[cfg(all(test, windows))]
mod image_security;
#[path = "../../support/message_read_adapters.rs"]
pub mod adapters;
#[path = "../../support/media_business.rs"]
pub mod business;
