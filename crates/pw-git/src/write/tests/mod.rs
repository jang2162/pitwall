//! 쓰기 실행기 시험(13-m2-plan.md M2-06 완료 기준 ① ~ ⑤ 의 Rust 쪽). 진짜 git · 임시 폴더의 시험 저장소(`crate::testing`) · 진짜
//! git 을 PATH 에 둔 가짜 로그인 셸로 돈다(unix). 창은 `testing::write::Window` 가 흉내 내고, RPC 경로(`git.prompts` 스트림 ·
//! `git.prompt.answer`)는 [`rpc`] 가 Host 연결로 본다. 질문 표 · 경로 꼴은 OS 와 상관없이 돈다.

mod paths;
mod prompt;

#[cfg(unix)]
mod askpass;
#[cfg(unix)]
mod executor;
#[cfg(unix)]
mod operation;
#[cfg(unix)]
mod rpc;
#[cfg(unix)]
mod unsaved;
#[cfg(unix)]
mod untrusted;

#[cfg(unix)]
use pw_core::bus::{EventBus, Filter, Received, Subscription};

/// Task 의 진행 · 종료 알림과 창 알림을 받는 구독. 쓰기 전에 연다.
#[cfg(unix)]
fn subscribe_all(bus: &EventBus) -> Subscription {
    bus.subscribe(Filter::all())
}

/// 지금까지 받은 이벤트(토픽, 내용).
#[cfg(unix)]
fn drain(sub: &mut Subscription) -> Vec<(String, serde_json::Value)> {
    let mut events = Vec::new();
    while let Some(received) = sub.try_recv() {
        if let Received::Event(event) = received {
            events.push((event.topic.clone(), event.payload.clone()));
        }
    }
    events
}

/// 첫 `task.finished` 까지(그것 포함) 받은 이벤트. Task 는 표에서 먼저 빠지고 종료 알림이 뒤에 나간다(pw-core `task.rs`) — 표가 빈
/// 것을 보고 비우면 알림을 놓칠 수 있어 알림 자체를 기다린다.
#[cfg(unix)]
async fn until_finished(sub: &mut Subscription) -> Vec<(String, serde_json::Value)> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut events = Vec::new();
    loop {
        events.extend(drain(sub));
        if events
            .iter()
            .any(|(topic, _)| topic == pw_core::task::TOPIC_FINISHED)
        {
            return events;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no task.finished: {events:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// `topic` 의 마지막 이벤트 내용.
#[cfg(unix)]
fn last(events: &[(String, serde_json::Value)], topic: &str) -> Option<serde_json::Value> {
    events
        .iter()
        .rev()
        .find(|(t, _)| t == topic)
        .map(|(_, payload)| payload.clone())
}
