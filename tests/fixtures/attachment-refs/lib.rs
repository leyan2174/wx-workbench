// 路径引用真实模块与共享 XML helper，不复制生产解析器，不加载账号或后台服务。
#[path = "../../../src/toolkit/attachment_refs.rs"]
pub mod attachment_refs;
#[path = "../../../src/message/mod.rs"]
pub mod message;
#[path = "../../../src/business/structured_message.rs"]
pub mod structured_message;
#[path = "../../support/structured_content_adapters.rs"]
pub mod structured_content_adapters;
#[path = "../../../src/business/attachment_content.rs"]
pub mod decoded_content;
pub mod business {
    pub use crate::structured_message;
    pub use super::decoded_content as attachment_content;
}
#[path = "../../../src/adapters/wechat/media/attachment_content.rs"]
pub mod wechat_content;
#[path = "../../../src/adapters/wechat/media/directory_layout.rs"]
pub mod directory_layout;
#[path = "../../../src/adapters/wechat/media/legacy_dat.rs"]
pub mod legacy_dat;
pub mod adapters {
    pub mod wechat {
        pub use crate::structured_content_adapters as messages;
        pub mod media {
            pub use crate::wechat_content as attachment_content;
            pub use crate::{directory_layout, legacy_dat};
        }
    }
}
