//! 壳内 RPC 服务面（W0-3）。
//!
//! 只做「频道注册 → 分派 → 回帧」的语义，不含传输：出站帧通过 `mpsc` 交给调用方，
//! 入站帧是已经剥掉 stdio 长度前缀的 RPC payload。

mod channel;
mod router;
mod server;

pub use channel::{
    BoxFuture, CancellationToken, ChannelError, EventCallback, ServerChannel, Subscription,
};
pub use router::{FrameRoute, FrameRouter};
pub use server::ChannelServer;
