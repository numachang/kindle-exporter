//! CDP の配管。HTTP で対象タブを見つけ、WebSocket で命令を送る。
//!
//! ADR-0001 §4 のとおり async は使わない。ブラウザ 1 タブを逐次操作するだけなので、
//! ブロッキングの WebSocket で足りる。使う CDP メソッドは 10 個ほどしかない。

use std::net::TcpStream;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::error::{Error, Result};

/// 応答を待つ上限。これを超えたら接続が死んだものとして扱う。
///
/// 途中まで読んだフレームからは復帰できないので、**回復はせずに落とす。**
/// 黙って止まり続けるより、失敗として上位に返す方が扱いやすい。
const READ_TIMEOUT: Duration = Duration::from_secs(120);

/// 接続先。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// ブラウザ全体の WebSocket URL。
    pub browser_ws: String,
    /// 操作したいタブの target id。
    pub target_id: String,
    /// そのタブの URL（ログ用）。
    pub page_url: String,
}

#[derive(Debug, Deserialize)]
struct Version {
    #[serde(rename = "webSocketDebuggerUrl")]
    web_socket_debugger_url: String,
}

#[derive(Debug, Deserialize)]
struct Target {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    url: String,
}

impl Endpoint {
    /// 既定の接続先（`127.0.0.1:9222` の Kindle Cloud Reader タブ）。
    ///
    /// Chrome 136 以降は既定プロファイルだと `--remote-debugging-port` が
    /// 無視されるため、専用プロファイル（`--user-data-dir`）での起動が要る。
    pub fn discover(port: u16, url_contains: &str) -> Result<Self> {
        let version: Version = get_json(&format!("http://127.0.0.1:{port}/json/version"))?;
        let targets: Vec<Target> = get_json(&format!("http://127.0.0.1:{port}/json/list"))?;
        let page = targets
            .iter()
            .find(|t| t.kind == "page" && t.url.contains(url_contains))
            .ok_or_else(|| {
                Error::not_found(
                    "対象のタブ",
                    targets.iter().map(|t| format!("{}: {}", t.kind, t.url)),
                )
            })?;
        Ok(Self {
            browser_ws: version.web_socket_debugger_url,
            target_id: page.id.clone(),
            page_url: page.url.clone(),
        })
    }
}

fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T> {
    let body = ureq::get(url)
        .call()
        .map_err(|e| Error::Connect(format!("{url}: {e}")))?
        .body_mut()
        .read_to_string()
        .map_err(|e| Error::Connect(format!("{url}: {e}")))?;
    serde_json::from_str(&body).map_err(|e| Error::Unexpected(format!("{url}: {e}")))
}

/// 1 タブに繋がった CDP セッション。
#[derive(Debug)]
pub struct Cdp {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    session: String,
    next_id: u64,
}

impl Cdp {
    /// 対象タブに attach し、`Runtime` と `Page` を有効にする。
    pub fn attach(endpoint: &Endpoint) -> Result<Self> {
        let (socket, _) = tungstenite::connect(&endpoint.browser_ws)
            .map_err(|e| Error::Connect(format!("{}: {e}", endpoint.browser_ws)))?;
        set_read_timeout(&socket);
        let mut cdp = Self { socket, session: String::new(), next_id: 0 };

        let attached = cdp.send(
            "Target.attachToTarget",
            json!({ "targetId": endpoint.target_id, "flatten": true }),
        )?;
        cdp.session = attached
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Unexpected(format!("sessionId がありません: {attached}")))?
            .to_owned();

        cdp.send("Runtime.enable", json!({}))?;
        cdp.send("Page.enable", json!({}))?;
        Ok(cdp)
    }

    /// CDP メソッドを 1 つ呼び、結果を返す。イベント通知は読み飛ばす。
    pub fn send(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        let mut message = json!({ "id": id, "method": method, "params": params });
        if !self.session.is_empty() {
            message["sessionId"] = Value::String(self.session.clone());
        }
        self.socket
            .send(Message::Text(message.to_string().into()))
            .map_err(|e| Error::Transport(format!("{method} の送信に失敗: {e}")))?;
        self.await_reply(method, id)
    }

    fn await_reply(&mut self, method: &str, id: u64) -> Result<Value> {
        loop {
            let Some(text) = self.read_text(method)? else { continue };
            let reply: Value = serde_json::from_str(&text)
                .map_err(|e| Error::Unexpected(format!("{method} の応答が JSON でない: {e}")))?;
            // イベント通知（`method` を持つ）は自分の応答ではない。
            if reply.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(err) = reply.get("error") {
                return Err(Error::Protocol {
                    method: method.to_owned(),
                    message: err.to_string(),
                });
            }
            return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// テキストフレームを 1 つ読む。制御フレームなら `None`。
    fn read_text(&mut self, method: &str) -> Result<Option<String>> {
        match self.socket.read() {
            Ok(Message::Text(t)) => Ok(Some(t.to_string())),
            Ok(Message::Close(_)) => {
                Err(Error::Transport(format!("{method} の応答前に接続が閉じられました")))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(Error::Transport(format!("{method} の応答を読めません: {e}"))),
        }
    }
}

fn set_read_timeout(socket: &WebSocket<MaybeTlsStream<TcpStream>>) {
    if let MaybeTlsStream::Plain(stream) = socket.get_ref() {
        // 失敗しても致命的ではない（無期限に待つだけ）ので、結果は捨てる。
        drop(stream.set_read_timeout(Some(READ_TIMEOUT)));
    }
}
