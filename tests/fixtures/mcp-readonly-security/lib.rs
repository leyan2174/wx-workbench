// 只替换缓存与名称容器边界；查询、XML、摘要和磁盘枚举均引用真实源码。
pub mod daemon {
    pub mod cache {
        use std::{
            collections::HashMap,
            path::{Path, PathBuf},
            sync::{
                atomic::{AtomicUsize, Ordering},
                Mutex,
            },
        };

        pub struct DbCache {
            pub root: PathBuf,
            pub paths: HashMap<String, PathBuf>,
            pub requests: Mutex<Vec<String>>,
            pub scans: AtomicUsize,
            pub publish_on_get: Mutex<Option<(usize, PathBuf, PathBuf)>>,
        }

        impl DbCache {
            pub fn new(root: PathBuf) -> Self {
                Self {
                    root,
                    paths: HashMap::new(),
                    requests: Mutex::new(Vec::new()),
                    scans: AtomicUsize::new(0),
                    publish_on_get: Mutex::new(None),
                }
            }

            pub fn db_dir(&self) -> &Path {
                &self.root
            }

            pub async fn get(&self, key: &str) -> anyhow::Result<Option<PathBuf>> {
                let count = {
                    let mut requests = self.requests.lock().unwrap();
                    requests.push(key.into());
                    requests.len()
                };
                let mut hook = self.publish_on_get.lock().unwrap();
                if hook.as_ref().is_some_and(|(at, _, _)| *at == count) {
                    let (_, source, destination) = hook.take().unwrap();
                    std::fs::copy(source, destination)?;
                }
                // 与真实缓存一样，不把已消失的源文件当成可用命中。
                if !self.root.join(key.replace('\\', "/")).exists() {
                    return Ok(None);
                }
                Ok(self.paths.get(key).cloned())
            }

            pub fn scan_count(&self) -> usize {
                self.scans.load(Ordering::SeqCst)
            }
        }
    }
}

#[path = "../../../src/daemon/query/mcp_contacts.rs"]
pub mod contacts;
#[path = "../../../src/message/mod.rs"]
pub mod message;
#[path = "../../../src/daemon/meta.rs"]
pub mod meta;
#[path = "../../../src/daemon/query/mcp_refer.rs"]
pub mod refer;
#[path = "../../../src/daemon/query/strict_message.rs"]
mod strict_message;

pub use daemon::cache::DbCache;
use std::{collections::HashMap, sync::atomic::Ordering};

#[derive(Clone, Default)]
pub struct Names {
    pub map: HashMap<String, String>,
    pub md5_to_uname: HashMap<String, String>,
    pub msg_db_keys: Vec<String>,
    pub biz_msg_db_keys: Vec<String>,
    pub verify_flags: HashMap<String, i64>,
}

// 与 query.rs 私有适配器同形，委托真实 meta helper；计数仅用于先拒绝断言。
fn ensure_complete_message_inventory(db: &DbCache, names: &Names) -> anyhow::Result<()> {
    db.scans.fetch_add(1, Ordering::SeqCst);
    let unknown = meta::discover_unknown_shards_checked(db.db_dir(), &names.msg_db_keys)?;
    anyhow::ensure!(
        unknown.is_empty(),
        "unknown message shards; complete inventory required"
    );
    Ok(())
}

#[path = "../../../src/adapters/wechat/contacts/mod.rs"]
pub mod contact_adapter;
#[path = "../../../src/business/contacts.rs"]
pub mod contact_business;
#[path = "../../support/media_business.rs"]
mod media_business;
#[path = "../../support/message_read_adapters.rs"]
mod message_read_adapters;
#[path = "../../../src/adapters/wechat/messages/reply.rs"]
pub mod reply_adapter;
#[path = "../../../src/adapters/wechat/messages/reply_read.rs"]
pub mod reply_read_adapter;
#[path = "../../support/structured_content_adapters.rs"]
pub mod structured_content_adapters;

pub mod adapters {
    pub use crate::message_read_adapters::messages;
    pub mod wechat {
        pub use crate::contact_adapter as contacts;
        pub mod messages {
            pub use crate::message_read_adapters::inventory;
            pub use crate::message_read_adapters::messages as read;
            pub use crate::message_read_adapters::messages::*;
            pub use crate::reply_adapter as reply;
            pub use crate::reply_read_adapter as reply_read;
            pub use crate::structured_content_adapters::*;
        }
    }
}
pub mod business {
    pub use crate::contact_business as contacts;
    pub use crate::media_business::*;
}
