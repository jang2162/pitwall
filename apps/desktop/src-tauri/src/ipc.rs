//! Tauri IPC transport: 웹뷰의 JSON-RPC 메시지를 Host 연결로 실어 나른다.
//!
//! - `rpc_connect({ target, channel })`: [`HostRouter`] 로 Host 를 골라 연결을 열고 연결 id 를 돌려준다.
//!   Host 가 보내는 메시지(응답 · 스트림 청크 · 이벤트)는 `channel` 로 온다.
//! - `rpc_send({ conn, frame })`: JSON-RPC 메시지 하나를 그 연결로 보낸다.
//! - `rpc_disconnect({ conn })`: 연결을 닫는다.
//!
//! 연결은 그것을 연 웹뷰에 묶인다. 다른 웹뷰는 그 id 로 보내거나 닫을 수 없고, 웹뷰가 닫히거나 새로 로드되면
//! 그 웹뷰의 연결을 모두 닫는다 — 연결을 버리면 Host 가 진행 중 요청 · 스트림을 취소하고 구독을 걷는다
//! ([`pw_core::FrameSink`] 계약).
//!
//! 로컬 창은 데스크톱 사용자 본인이므로 호출자 role 은 owner 다.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use pw_core::{FrameSink, HostError, HostRouter, Target};
use pw_rpc::{Caller, Role};
use serde::Serialize;
use serde_json::Value;
use tauri::ipc::Channel;
use tauri::webview::PageLoadEvent;
use tauri::{Builder, Manager, Runtime, State, Webview, Window, WindowEvent};

pub type ConnId = u64;

/// 웹뷰 label → 그 웹뷰가 연 연결들.
type ConnTable = HashMap<String, HashMap<ConnId, Box<dyn FrameSink>>>;

pub struct IpcState {
    router: Arc<HostRouter>,
    next_conn: AtomicU64,
    conns: Mutex<ConnTable>,
}

impl IpcState {
    pub fn new(router: Arc<HostRouter>) -> Self {
        Self {
            router,
            next_conn: AtomicU64::new(1),
            conns: Mutex::default(),
        }
    }

    fn conns(&self) -> MutexGuard<'_, ConnTable> {
        self.conns.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 이 웹뷰의 연결을 모두 닫는다. 닫은 수를 돌려준다.
    pub fn close_webview(&self, label: &str) -> usize {
        // 잠금 밖에서 버린다. 연결을 버리면 Host 쪽 정리(세션 close)가 돈다.
        let removed = self.conns().remove(label);
        removed.map_or(0, |conns| conns.len())
    }

    #[cfg(test)]
    pub fn connection_count(&self, label: &str) -> usize {
        self.conns().get(label).map_or(0, HashMap::len)
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum IpcError {
    /// 라우팅 · 연결 실패.
    Host { error: HostError },
    /// 이 웹뷰가 연 연결이 아니거나 이미 닫혔다.
    UnknownConnection { conn: ConnId },
}

/// transport 를 앱에 붙인다: 상태, command, 웹뷰가 닫히거나 다시 로드될 때의 정리.
pub fn install<R: Runtime>(builder: Builder<R>, router: Arc<HostRouter>) -> Builder<R> {
    builder
        .manage(IpcState::new(router))
        .invoke_handler(tauri::generate_handler![
            rpc_connect,
            rpc_send,
            rpc_disconnect
        ])
        .on_window_event(on_window_event)
        .on_page_load(|webview, payload| {
            // 새로고침하면 프론트엔드는 연결 id 와 채널을 잃는다. 남은 연결은 아무도 닫지 못하므로 여기서 닫는다.
            if payload.event() == PageLoadEvent::Started {
                webview.state::<IpcState>().close_webview(webview.label());
            }
        })
}

/// 창이 사라지면 그 창의 연결을 닫는다. 창마다 웹뷰가 하나라 창 label 이 곧 웹뷰 label 이다.
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if matches!(event, WindowEvent::Destroyed) {
        window.state::<IpcState>().close_webview(window.label());
    }
}

// Session 이 메시지를 처리하며 tokio 태스크를 띄우므로 command 는 async 로 둔다 — Tauri 의 async command 는
// tokio 런타임 위에서 돌고, 동기 command 는 메인 스레드에서 돌아 런타임이 없다.
#[tauri::command]
async fn rpc_connect<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, IpcState>,
    target: Target,
    channel: Channel<Value>,
) -> Result<ConnId, IpcError> {
    let conn = state
        .router
        .connect(&target, Caller::new(Role::Owner))
        .await
        .map_err(|error| IpcError::Host { error })?;
    let id = state.next_conn.fetch_add(1, Ordering::Relaxed);
    let label = webview.label().to_owned();
    state
        .conns()
        .entry(label.clone())
        .or_default()
        .insert(id, conn.sink);
    // 연결을 기다리는 사이에 웹뷰가 닫혔으면 정리 훅은 이미 지나갔다. 여기서 직접 닫는다.
    if webview.app_handle().get_webview_window(&label).is_none() {
        state.close_webview(&label);
    }
    let mut source = conn.source;
    tauri::async_runtime::spawn(async move {
        // 연결이 닫히면 Host 가 남은 메시지(취소된 스트림의 stream.end 등)를 보낸 뒤 None 을 돌려준다.
        while let Some(frame) = source.recv().await {
            if channel.send(frame).is_err() {
                break;
            }
        }
    });
    Ok(id)
}

#[tauri::command]
async fn rpc_send<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, IpcState>,
    conn: ConnId,
    frame: Value,
) -> Result<(), IpcError> {
    let conns = state.conns();
    let sink = conns
        .get(webview.label())
        .and_then(|c| c.get(&conn))
        .ok_or(IpcError::UnknownConnection { conn })?;
    sink.send(frame);
    Ok(())
}

#[tauri::command]
async fn rpc_disconnect<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, IpcState>,
    conn: ConnId,
) -> Result<(), IpcError> {
    let removed = state
        .conns()
        .get_mut(webview.label())
        .and_then(|c| c.remove(&conn));
    match removed {
        Some(_) => Ok(()),
        None => Err(IpcError::UnknownConnection { conn }),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use pw_core::methods::{DemoTaskUpdate, Pong};
    use pw_core::task::{TOPIC_FINISHED, TaskFinishedEvent};
    use pw_core::{Filter, LocalHost, Received, Scope, Subscription, TaskStatus};
    use pw_rpc::protocol::StreamOpened;
    use serde_json::json;
    use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
    use tauri::test::{
        INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets,
    };
    use tauri::webview::InvokeRequest;
    use tauri::{App, WebviewWindow, WebviewWindowBuilder};

    use super::*;

    struct Fixture {
        app: App<MockRuntime>,
        host: Arc<LocalHost>,
        /// 채널로 나간 메시지: (채널 id, JSON).
        frames: mpsc::Receiver<(u32, Value)>,
    }

    fn fixture() -> Fixture {
        let host = Arc::new(LocalHost::new("local"));
        let router = Arc::new(HostRouter::new());
        router.add_host(host.clone()).unwrap();
        router.bind_project("p1".into(), "local").unwrap();
        let (tx, frames) = mpsc::channel();
        // mock 웹뷰에는 JS 가 없다. 채널로 나가는 메시지를 가로채 테스트로 돌린다.
        let builder =
            mock_builder().channel_interceptor(move |_webview, callback, _index, body| {
                if let InvokeResponseBody::Json(json) = body {
                    let _ = tx.send((callback.0, serde_json::from_str(json).unwrap()));
                }
                true
            });
        let app = install(builder, router)
            .build(mock_context(noop_assets()))
            .unwrap();
        Fixture { app, host, frames }
    }

    fn window(app: &App<MockRuntime>, label: &str) -> WebviewWindow<MockRuntime> {
        WebviewWindowBuilder::new(app, label, Default::default())
            .build()
            .unwrap()
    }

    fn invoke(
        webview: &WebviewWindow<MockRuntime>,
        cmd: &str,
        args: Value,
    ) -> Result<Value, Value> {
        get_ipc_response(
            webview,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|body| body.deserialize::<Value>().unwrap())
    }

    fn connect(webview: &WebviewWindow<MockRuntime>, channel: u32, target: Value) -> ConnId {
        let id = invoke(
            webview,
            "rpc_connect",
            json!({"target": target, "channel": format!("__CHANNEL__:{channel}")}),
        )
        .unwrap();
        serde_json::from_value(id).unwrap()
    }

    fn send(
        webview: &WebviewWindow<MockRuntime>,
        conn: ConnId,
        frame: Value,
    ) -> Result<Value, Value> {
        invoke(webview, "rpc_send", json!({"conn": conn, "frame": frame}))
    }

    fn next_frame(f: &Fixture, channel: u32) -> Value {
        loop {
            let (id, frame) = f
                .frames
                .recv_timeout(Duration::from_secs(5))
                .expect("no frame on the channel");
            if id == channel {
                return frame;
            }
        }
    }

    fn finished_events(host: &LocalHost) -> Subscription {
        host.bus()
            .subscribe(Filter::all().scope(Scope::Host).topics([TOPIC_FINISHED]))
    }

    fn wait_finished(sub: &mut Subscription) -> TaskStatus {
        tauri::async_runtime::block_on(async {
            match tokio_timeout(sub.recv()).await {
                Some(Received::Event(e)) => {
                    serde_json::from_value::<TaskFinishedEvent>(e.payload.clone())
                        .unwrap()
                        .status
                }
                other => panic!("expected a finished event, got {other:?}"),
            }
        })
    }

    async fn tokio_timeout<F: std::future::Future>(fut: F) -> F::Output {
        tokio::time::timeout(Duration::from_secs(5), fut)
            .await
            .expect("timed out")
    }

    /// 데모 작업을 열고 첫 진행 청크까지 받는다. 스트림 id 를 돌려준다.
    fn start_demo(
        f: &Fixture,
        webview: &WebviewWindow<MockRuntime>,
        conn: ConnId,
        channel: u32,
    ) -> u64 {
        send(
            webview,
            conn,
            json!({"jsonrpc": "2.0", "id": 1, "method": "host.demoTask",
                   "params": {"steps": 1000, "intervalMs": 10}}),
        )
        .unwrap();
        let opened: StreamOpened =
            serde_json::from_value(next_frame(f, channel)["result"].clone()).unwrap();
        send(
            webview,
            conn,
            json!({"jsonrpc": "2.0", "method": "stream.credit",
                   "params": {"streamId": opened.stream_id, "credit": 1}}),
        )
        .unwrap();
        let chunk = next_frame(f, channel);
        assert_eq!(chunk["method"], "stream.chunk");
        assert!(matches!(
            serde_json::from_value(chunk["params"]["data"].clone()).unwrap(),
            DemoTaskUpdate::Progress { .. }
        ));
        opened.stream_id
    }

    #[test]
    fn ping_round_trips_through_ipc() {
        let f = fixture();
        let welcome = window(&f.app, "welcome");
        let conn = connect(&welcome, 7, json!({"hostId": "local"}));

        send(
            &welcome,
            conn,
            json!({"jsonrpc": "2.0", "id": 1, "method": "host.ping", "params": {"nonce": "n1"}}),
        )
        .unwrap();
        let frame = next_frame(&f, 7);
        assert_eq!(frame["id"], 1);
        let pong: Pong = serde_json::from_value(frame["result"].clone()).unwrap();
        assert_eq!(pong.nonce, "n1");
        assert_eq!(pong.host_id, "local");
    }

    #[test]
    fn connecting_by_project_routes_and_unknown_project_is_an_error() {
        let f = fixture();
        let w = window(&f.app, "project-1");
        connect(&w, 1, json!({"projectId": "p1"}));
        let err = invoke(
            &w,
            "rpc_connect",
            json!({"target": {"projectId": "nope"}, "channel": "__CHANNEL__:2"}),
        )
        .unwrap_err();
        assert_eq!(
            err,
            json!({"kind": "host", "error": {"kind": "unknownProject", "id": "nope"}})
        );
        assert_eq!(f.app.state::<IpcState>().connection_count("project-1"), 1);
    }

    #[test]
    fn a_window_cannot_use_another_windows_connection() {
        let f = fixture();
        let a = window(&f.app, "welcome");
        let b = window(&f.app, "settings");
        let conn = connect(&a, 1, json!({"hostId": "local"}));
        let frame = json!({"jsonrpc": "2.0", "id": 1, "method": "host.ping"});
        assert_eq!(
            send(&b, conn, frame).unwrap_err(),
            json!({"kind": "unknownConnection", "conn": conn})
        );
        assert!(invoke(&b, "rpc_disconnect", json!({"conn": conn})).is_err());
        assert_eq!(f.app.state::<IpcState>().connection_count("welcome"), 1);
    }

    #[test]
    fn disconnect_closes_the_connection_and_cancels_its_streams() {
        let f = fixture();
        let w = window(&f.app, "welcome");
        let conn = connect(&w, 3, json!({"hostId": "local"}));
        let mut finished = finished_events(&f.host);
        start_demo(&f, &w, conn, 3);

        invoke(&w, "rpc_disconnect", json!({"conn": conn})).unwrap();
        assert_eq!(wait_finished(&mut finished), TaskStatus::Cancelled);
        assert!(send(&w, conn, json!({})).is_err());
    }

    #[test]
    fn closing_a_window_closes_its_connections_and_cancels_their_work() {
        let f = fixture();
        let welcome = window(&f.app, "welcome");
        let other = window(&f.app, "settings");
        let conn = connect(&welcome, 5, json!({"hostId": "local"}));
        connect(&welcome, 6, json!({"hostId": "local"}));
        connect(&other, 9, json!({"hostId": "local"}));
        let mut finished = finished_events(&f.host);
        let stream_id = start_demo(&f, &welcome, conn, 5);

        // mock 런타임은 창을 없애도 Destroyed 를 내지 않는다. 앱에 건 것과 같은 훅을 직접 부른다.
        on_window_event(&welcome.as_ref().window(), &WindowEvent::Destroyed);

        let state = f.app.state::<IpcState>();
        assert_eq!(state.connection_count("welcome"), 0);
        assert_eq!(state.connection_count("settings"), 1);
        assert_eq!(wait_finished(&mut finished), TaskStatus::Cancelled);
        // 닫힌 연결의 스트림은 cancelled 로 끝났다고 웹뷰 쪽에 알린다.
        let end = loop {
            let frame = next_frame(&f, 5);
            if frame["method"] == "stream.end" {
                break frame;
            }
        };
        assert_eq!(end["params"]["streamId"], stream_id);
        assert_eq!(end["params"]["status"], "cancelled");
    }
}
