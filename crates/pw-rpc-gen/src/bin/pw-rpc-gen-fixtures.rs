//! `packages/rpc-client` 테스트의 고정 입력을 만든다: 시험용 메서드의 TS 타입과, 실제 [`Session`] 이 주고받은
//! 골든 메시지. TS 테스트는 이 TS 타입으로 호출을 컴파일하고, 골든 메시지를 재생해 클라이언트가 같은 JSON 을
//! 만들고 서버의 JSON 을 읽는지 본다 — Rust 와 TS 가 같은 와이어 형식을 쓰는지의 증거다.
//!
//! 시험용 메서드는 이 실행 파일에만 링크된다. 그래서 `pw-rpc-gen` 의 생성물(`pw_rpc::methods`)에 섞이지 않는다.
//! 여기서도 레지스트리가 아니라 목록을 직접 넘긴다.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use pw_rpc::protocol::{
    CancelRequestParams, EVENTS_SUBSCRIBE, EVENTS_UNSUBSCRIBE, REQUEST_CANCEL, STREAM_CANCEL,
    STREAM_CREDIT, StreamCancelParams, StreamCreditParams, StreamOpened, SubscribeParams,
    Subscribed, UnsubscribeParams,
};
use pw_rpc::{
    Caller, EventHub, Id, MethodInfo, Outbound, RequestMessage, Role, Router, RpcError, Session,
    rpc_method,
};
use pw_rpc_gen::{Output, REGENERATE, emit, or_exit, typescript};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

#[derive(Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EchoParams {
    pub text: String,
    pub repeat_count: u32,
    pub mode: EchoMode,
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum EchoMode {
    Plain,
    Upper,
}

#[derive(Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EchoResult {
    pub text: String,
    pub length: u32,
}

#[derive(Serialize, Deserialize, specta::Type, schemars::JsonSchema)]
pub struct CountParams {
    pub n: u32,
}

rpc_method! {
    /// 입력을 `repeatCount` 번 이어 붙여 돌려준다.
    pub Echo: "fixture.echo" {
        params: EchoParams,
        result: EchoResult,
        tier: Read,
    }

    /// 0 부터 n-1 까지 청크로 보낸다.
    pub Count: "fixture.count" {
        params: CountParams,
        stream: u32,
        tier: Read,
    }

    /// 애플리케이션 오류를 돌려준다.
    pub Fail: "fixture.fail" {
        params: (),
        result: (),
        tier: Read,
    }

    /// 끝나지 않는다. 취소 시험용.
    pub Wait: "fixture.wait" {
        params: (),
        result: (),
        tier: Exec,
        roles: [Owner, Mobile],
    }
}

const APP_ERROR: i32 = 1001;

fn router() -> Router {
    let mut router = Router::new();
    router
        .unary(Echo, |_ctx, p: EchoParams| async move {
            let mut text = p.text.repeat(p.repeat_count as usize);
            if matches!(p.mode, EchoMode::Upper) {
                text = text.to_uppercase();
            }
            let length = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
            Ok(EchoResult { text, length })
        })
        .stream(Count, |_ctx, p: CountParams, sink| async move {
            for i in 0..p.n {
                sink.send(i).await?;
            }
            Ok(())
        })
        .unary(Fail, |_ctx, ()| async {
            Err(RpcError::new(APP_ERROR, "boom").with_data(json!({"reason": "fixture"})))
        })
        .unary(Wait, |_ctx, ()| std::future::pending());
    router
}

/// 클라이언트 쪽 메시지를 세션에 넣고 서버 쪽 메시지를 받아 순서대로 적는다.
struct Recorder {
    session: Session,
    rx: UnboundedReceiver<Outbound>,
    exchanges: Vec<Value>,
}

impl Recorder {
    fn send(&mut self, message: RequestMessage) {
        let value = serde_json::to_value(&message).expect("request serializes");
        self.exchanges
            .push(json!({"from": "client", "message": value}));
        self.session.handle_value(value);
    }

    fn request<P: Serialize>(&mut self, id: i64, method: &str, params: &P) {
        let params = serde_json::to_value(params).expect("params serialize");
        self.send(RequestMessage::request(Id::Number(id), method, params));
    }

    fn notify<P: Serialize>(&mut self, method: &str, params: &P) {
        let params = serde_json::to_value(params).expect("params serialize");
        self.send(RequestMessage::notification(method, params));
    }

    /// 서버 메시지 `n` 개를 받아 적고 마지막 것을 돌려준다.
    async fn receive(&mut self, n: usize) -> Value {
        let mut last = Value::Null;
        for _ in 0..n {
            let message = tokio::time::timeout(Duration::from_secs(5), self.rx.recv())
                .await
                .expect("timed out waiting for the server")
                .expect("session closed");
            last = serde_json::from_str(&message.to_text()).expect("outbound is JSON");
            self.exchanges
                .push(json!({"from": "server", "message": last}));
        }
        last
    }

    async fn assert_quiet(&mut self) {
        let extra = tokio::time::timeout(Duration::from_millis(100), self.rx.recv()).await;
        assert!(extra.is_err(), "unexpected server message: {extra:?}");
    }
}

fn result_as<T: serde::de::DeserializeOwned>(response: &Value) -> T {
    serde_json::from_value(response["result"].clone()).expect("typed result")
}

/// `packages/rpc-client/test/golden.test.ts` 가 같은 순서로 재생한다. 순서를 바꾸면 그 테스트도 바꾼다.
async fn record() -> Vec<Value> {
    let events = EventHub::new();
    let (session, rx) = Session::new(Arc::new(router()), events.clone(), Caller::new(Role::Owner));
    let mut r = Recorder {
        session,
        rx,
        exchanges: Vec::new(),
    };

    let echo = EchoParams {
        text: "hi".into(),
        repeat_count: 2,
        mode: EchoMode::Upper,
        note: None,
    };
    r.request(1, "fixture.echo", &echo);
    r.receive(1).await;

    r.request(2, "fixture.count", &CountParams { n: 3 });
    let opened: StreamOpened = result_as(&r.receive(1).await);
    r.notify(
        STREAM_CREDIT,
        &StreamCreditParams {
            stream_id: opened.stream_id,
            credit: 3,
        },
    );
    r.receive(4).await; // 청크 3 + stream.end

    r.request(3, "fixture.fail", &());
    r.receive(1).await;

    let subscribe = SubscribeParams {
        project_id: Some("p1".into()),
        topics: vec!["fs.changed".into()],
    };
    r.request(4, EVENTS_SUBSCRIBE, &subscribe);
    let subscribed: Subscribed = result_as(&r.receive(1).await);
    events
        .publish("p1", "fs.changed", &json!({"path": "src/main.rs"}))
        .expect("payload serializes");
    r.receive(1).await;
    r.request(
        5,
        EVENTS_UNSUBSCRIBE,
        &UnsubscribeParams {
            subscription_id: subscribed.subscription_id,
        },
    );
    r.receive(1).await;

    r.request(6, "fixture.wait", &());
    r.notify(REQUEST_CANCEL, &CancelRequestParams { id: Id::Number(6) });
    r.receive(1).await;

    r.request(7, "fixture.count", &CountParams { n: 100 });
    let opened: StreamOpened = result_as(&r.receive(1).await);
    r.notify(
        STREAM_CREDIT,
        &StreamCreditParams {
            stream_id: opened.stream_id,
            credit: 1,
        },
    );
    r.receive(1).await;
    r.notify(
        STREAM_CANCEL,
        &StreamCancelParams {
            stream_id: opened.stream_id,
        },
    );
    r.receive(1).await;

    r.assert_quiet().await;
    r.exchanges
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let methods: [&MethodInfo; 4] = [
        &MethodInfo::unary::<Echo>(),
        &MethodInfo::stream::<Count>(),
        &MethodInfo::unary::<Fail>(),
        &MethodInfo::unary::<Wait>(),
    ];
    let ts = match or_exit(typescript(&methods)) {
        Ok(ts) => ts,
        Err(code) => return code,
    };
    let golden = json!({
        "$comment": format!("@generated by pw-rpc-gen-fixtures from a real pw-rpc Session. Do not edit. Regenerate: {REGENERATE}"),
        "exchanges": record().await,
    });
    emit(&[
        Output {
            path: "packages/rpc-client/test/fixtures/rpc.ts",
            contents: ts,
        },
        Output {
            path: "packages/rpc-client/test/fixtures/golden.json",
            contents: serde_json::to_string_pretty(&golden).expect("golden serializes") + "\n",
        },
    ])
}
