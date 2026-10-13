//! Optional phone delivery. No configuration, request, or response is logged.
//! Decisions and limits: docs/decisions/0012-phone-push.md.

use std::future::Future;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

const TIMEOUT: Duration = Duration::from_secs(3);
const CONFIG_LIMIT: u64 = 65_536;

/// Each channel consumes the same desktop notification; channels do not deduplicate.
trait PushChannel: Send + 'static {
    fn send(&self, alert: &Alert) -> impl Future<Output = Result<(), &'static str>> + Send;
}

#[derive(Serialize)]
struct Alert {
    title: String,
    message: String,
}

#[derive(Deserialize)]
#[serde(tag = "channel", rename_all = "lowercase", deny_unknown_fields)]
enum Config {
    Ntfy {
        #[serde(default = "default_server")]
        server: String,
        topic: String,
        token: Option<String>,
    },
}

fn default_server() -> String {
    "https://ntfy.sh/".into()
}

fn config_path() -> Option<PathBuf> {
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(root.join("theflash/push.json"))
}

fn load() -> Result<Option<Config>, &'static str> {
    let Some(path) = config_path() else {
        return Ok(None);
    };
    load_file(&path)
}

fn load_file(path: &Path) -> Result<Option<Config>, &'static str> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("无法读取配置文件"),
    };
    if !meta.is_file() || meta.mode() & 0o7777 != 0o600 {
        return Err("配置必须是权限 0600 的普通文件（不接受符号链接）");
    }
    let file = std::fs::File::open(path).map_err(|_| "无法读取配置文件")?;
    let opened = file.metadata().map_err(|_| "无法读取配置文件")?;
    if !opened.is_file()
        || opened.mode() & 0o7777 != 0o600
        || opened.ino() != meta.ino()
        || opened.dev() != meta.dev()
    {
        return Err("配置文件读取时发生变化");
    }
    let mut bytes = Vec::new();
    file.take(CONFIG_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取配置文件")?;
    if bytes.len() as u64 > CONFIG_LIMIT {
        return Err("配置文件过大");
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "配置 JSON 无效或渠道不支持")
}

struct Ntfy {
    client: reqwest::Client,
    server: reqwest::Url,
    topic: String,
    token: Option<HeaderValue>,
}

impl Ntfy {
    fn new(server: String, topic: String, token: Option<String>) -> Result<Self, &'static str> {
        let server = reqwest::Url::parse(&server).map_err(|_| "服务器地址无效")?;
        let loopback = matches!(server.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if !(server.scheme() == "https" || server.scheme() == "http" && loopback)
            || server.host_str().is_none()
            || !server.username().is_empty()
            || server.password().is_some()
            || server.query().is_some()
            || server.fragment().is_some()
            || server.path() != "/"
        {
            return Err("服务器须为 HTTPS 根地址（本机测试可用 HTTP），不能包含凭据或参数");
        }
        if topic.is_empty()
            || topic.len() > 128
            || !topic
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err("topic 格式无效");
        }
        let token = token
            .map(|t| {
                if t.is_empty()
                    || !t
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                {
                    return Err("token 格式无效");
                }
                let mut header =
                    HeaderValue::from_str(&format!("Bearer {t}")).map_err(|_| "token 格式无效")?;
                header.set_sensitive(true);
                Ok(header)
            })
            .transpose()?;
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .connect_timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| "无法初始化推送客户端")?;
        Ok(Self {
            client,
            server,
            topic,
            token,
        })
    }
}

impl PushChannel for Ntfy {
    async fn send(&self, alert: &Alert) -> Result<(), &'static str> {
        // JSON handles Chinese titles without non-ASCII HTTP headers.
        // Topic is in the body, never in a request URL or error message.
        let body =
            serde_json::json!({"topic":self.topic, "title":alert.title, "message":alert.message});
        let mut request = self
            .client
            .post(self.server.clone())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string());
        if let Some(token) = &self.token {
            request = request.header(reqwest::header::AUTHORIZATION, token.clone());
        }
        let response = request.send().await.map_err(|_| "网络失败或超时")?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err("服务器拒绝推送")
        }
    }
}

pub(super) struct PhonePush {
    sender: Option<mpsc::Sender<Alert>>,
    task: tokio::task::JoinHandle<()>,
}

impl PhonePush {
    pub fn from_config() -> Option<Self> {
        let result = load().and_then(|config| match config {
            None => Ok(None),
            Some(Config::Ntfy {
                server,
                topic,
                token,
            }) => Ntfy::new(server, topic, token).map(|c| Some(Self::start(c))),
        });
        match result {
            Ok(p) => p,
            Err(why) => {
                println!("手机推送未启用（{why}；topic/token：***），继续盯票");
                None
            }
        }
    }

    fn start<C: PushChannel>(channel: C) -> Self {
        let (sender, mut receiver) = mpsc::channel::<Alert>(32);
        let task = tokio::spawn(async move {
            while let Some(alert) = receiver.recv().await {
                if let Err(why) = channel.send(&alert).await {
                    println!("手机推送失败（{why}；topic/token：***），继续盯票");
                }
            }
        });
        Self {
            sender: Some(sender),
            task,
        }
    }

    pub fn alert(&self, title: &str, lines: &[String]) {
        if self.sender.as_ref().is_some_and(|s| {
            s.try_send(Alert {
                title: title.into(),
                message: lines.join("\n"),
            })
            .is_err()
        }) {
            println!("手机推送失败（队列已满或已停止；topic/token：***），继续盯票");
        }
    }

    pub async fn finish(&mut self) {
        self.sender.take();
        if tokio::time::timeout(TIMEOUT, &mut self.task).await.is_err() {
            println!("手机推送结束等待超时（topic/token：***），取消剩余提醒");
        }
    }
}

impl Drop for PhonePush {
    fn drop(&mut self) {
        // Ctrl-C drops watch's future: no worker may outlive its watch.
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    #[test]
    fn config_rejects_symlinks_invalid_json_and_oversized_files_without_values() {
        let dir = std::env::temp_dir().join(format!("theflash-push-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("push.json");
        assert!(load_file(&path).unwrap().is_none());
        std::fs::write(&path, b"{invalid-secret-json").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(
            load_file(&path),
            Err("配置 JSON 无效或渠道不支持")
        ));
        std::fs::write(&path, vec![b'x'; CONFIG_LIMIT as usize + 1]).unwrap();
        assert!(matches!(load_file(&path), Err("配置文件过大")));
        std::fs::write(&path, r#"{"channel":"ntfy","topic":"fake-topic"}"#).unwrap();
        assert!(load_file(&path).unwrap().is_some());
        let link = dir.join("link.json");
        symlink(&path, &link).unwrap();
        assert!(load_file(&link).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn secrets_cannot_be_embedded_in_server_urls_and_remote_http_is_refused() {
        for server in [
            "http://ntfy.sh/",
            "https://fake-secret@ntfy.sh/",
            "https://ntfy.sh/fake-secret",
            "https://ntfy.sh/?token=fake-secret",
            "https://ntfy.sh/#fake-secret",
            "file:///fake-secret",
        ] {
            let result = Ntfy::new(server.into(), "fake-topic".into(), None);
            let Err(why) = result else {
                panic!("unsafe server accepted")
            };
            assert!(!why.contains("fake-secret"));
        }
    }

    struct HangingChannel {
        started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        cancelled: Arc<AtomicBool>,
    }

    struct Guard(Arc<AtomicBool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    impl PushChannel for HangingChannel {
        async fn send(&self, _: &Alert) -> Result<(), &'static str> {
            let _guard = Guard(self.cancelled.clone());
            self.started
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(())
                .unwrap();
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn a_hanging_replaceable_channel_does_not_block_alerts_and_is_cancelled_with_watch() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let (started, received) = tokio::sync::oneshot::channel();
        let push = PhonePush::start(HangingChannel {
            started: Mutex::new(Some(started)),
            cancelled: cancelled.clone(),
        });
        push.alert("first", &[]);
        received.await.unwrap(); // channel is now deliberately stuck, without a timing race
        push.alert("second", &[]); // watch can proceed while first delivery hangs
        drop(push);
        tokio::time::timeout(Duration::from_secs(5), async {
            while !cancelled.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
