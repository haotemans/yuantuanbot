//! NapCat access token 共享槽：adapter-qq 的 ws_handler 每次连接时 read，
//! WebUI 热应用 write 立即生效（不必重启进程换 token）。
//! listen_addr 仍然是一次性绑定，改它仍需重启进程。

use std::sync::{Arc, RwLock};

pub type SharedToken = Arc<RwLock<String>>;

pub fn shared_token(t: &str) -> SharedToken {
    Arc::new(RwLock::new(t.to_string()))
}
