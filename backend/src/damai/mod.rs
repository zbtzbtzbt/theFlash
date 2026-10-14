//! Event data from damai.cn: the PC detail page and its per-session tier
//! endpoint. Why these sources: docs/decisions/0001-damai-event-data-source.md.
//! What each field means: docs/subsystems/damai-event.md.
//!
//! Every request leaves through `access::Gate`, which paces this process's
//! requests and stops all of them once any theFlash process has run into
//! risk control: docs/subsystems/damai-access.md.

mod access;
mod client;
mod event;
mod page;
mod sku;
pub(crate) mod text;

pub use access::{Clock, Gate, MIN_REQUEST_GAP, Resumed, RiskStop, SystemClock, state_dir};
pub use client::{Client, ClockReading, DEFAULT_BASE_URL, DEFAULT_HOME_URL, parse_item_id};
pub use event::{
    Event, SaleState, Session, Tier, TierStatus, buy_button_code_is_mapped, sale_state_of_code,
    tier_code_is_mapped,
};
pub use page::{item_page_json, item_page_problem, perform_id};
pub use sku::{tier_answer_problem, tier_json};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "从“{0}”里找不到演出 ID：请给有效的大麦演出详情页链接（detail.damai.cn/item.htm?id=数字 或 m.damai.cn/damai/detail/item.html?itemId=数字）或纯数字 ID"
    )]
    InvalidInput(String),
    #[error("环境变量 {name} 的值无效：{expected}")]
    InvalidConfig {
        name: &'static str,
        expected: &'static str,
    },
    #[error("演出 {0} 不存在或已下架")]
    NotFound(u64),
    #[error("大麦触发了风控（{0} 返回了滑块验证页），已停止查询，没有重试")]
    RiskControl(String),
    /// Risk control happened and every theFlash process is now stopped,
    /// including this one, which may not be the one that ran into it.
    #[error(
        "大麦风控熔断生效中，这次请求没有发出：{0}。所有 theFlash 进程都不再请求大麦。确认 owner 已经看过这次风控后，运行 `theflash risk resume --yes` 解除"
    )]
    RiskStopped(Box<RiskStop>),
    /// Risk control happened but the breaker could not be written, so the
    /// other processes will not stop on their own.
    #[error(
        "大麦触发了风控（{url} 返回了滑块验证页），已停止查询；但熔断状态没能写进 {path}（{reason}），别的 theFlash 进程不会自动停下，请手动停掉它们并报告 owner"
    )]
    RiskNotRecorded {
        url: String,
        path: String,
        reason: String,
    },
    /// The breaker itself is unreadable or its directory unusable. Stopping
    /// is the only safe reading: an unreadable breaker is not "no breaker".
    #[error(
        "读不到大麦风控熔断状态（{path}）：{reason}。为了不在风控期间继续请求大麦，已停止；请修好这个位置，或让 owner 看过这次风控后用 `theflash risk resume --yes` 解除"
    )]
    RiskState { path: String, reason: String },
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

impl Error {
    /// Risk control, or not knowing whether risk control is in effect. No
    /// caller sends another request after one of these; `theflash risk
    /// resume --yes` is the only way on.
    pub fn stops_damai_access(&self) -> bool {
        matches!(
            self,
            Error::RiskControl(_)
                | Error::RiskStopped(_)
                | Error::RiskNotRecorded { .. }
                | Error::RiskState { .. }
        )
    }
}
