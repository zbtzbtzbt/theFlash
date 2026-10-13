//! Event data from damai.cn: the PC detail page and its per-session tier
//! endpoint. Why these sources: docs/decisions/0001-damai-event-data-source.md.
//! What each field means: docs/subsystems/damai-event.md.

mod client;
mod event;
mod page;
mod sku;
mod text;

pub use client::{Client, ClockReading, DEFAULT_BASE_URL, DEFAULT_HOME_URL, parse_item_id};
pub use event::{
    Event, SaleState, Session, Tier, TierStatus, buy_button_code_is_mapped, sale_state_of_code,
    tier_code_is_mapped,
};
pub use page::item_page_json;
pub use sku::tier_json;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "从“{0}”里找不到演出 ID：请给有效的大麦演出详情页链接（detail.damai.cn/item.htm?id=数字 或 m.damai.cn/damai/detail/item.html?itemId=数字）或纯数字 ID"
    )]
    InvalidInput(String),
    #[error("演出 {0} 不存在或已下架")]
    NotFound(u64),
    #[error("大麦触发了风控（{0} 返回了滑块验证页），已停止查询，没有重试")]
    RiskControl(String),
    #[error("网络请求失败（{url}）：{reason}")]
    Network { url: String, reason: String },
    /// This machine has no CA certificate to check damai.cn's with: a Linux
    /// without the ca-certificates package. Nothing was sent.
    #[error(
        "本机没有 CA 证书，HTTPS 没法验证大麦的证书，所以没有发出任何请求。先装 ca-certificates 这个包再运行：Debian/Ubuntu 上是 sudo apt-get update && sudo apt-get install -y ca-certificates（在容器里用 root 运行时去掉 sudo）。设置了 SSL_CERT_FILE 或 SSL_CERT_DIR 时，程序只从它们指向的位置读证书"
    )]
    NoCaCertificates,
    /// The HTTP client could not be built for another reason. Nothing was
    /// sent either.
    #[error("没法创建发请求用的 HTTP 客户端，所以没有发出任何请求：{0}")]
    ClientSetup(String),
    #[error("大麦返回了 HTTP {status}（{url}）")]
    Http { url: String, status: u16 },
    #[error("看不懂大麦返回的{what}：{reason}")]
    Parse { what: &'static str, reason: String },
}
