//! `remote.*` RPC(18-m7-plan M7-08 ①). 모두 owner 만이고 **Host 범위 연결만** 받는다(웰컴 · 설정 창 — 프로젝트 창 · 파일 창 · MCP
//! 세션은 거절). MCP 툴이 아니다. 읽기는 `read`, 붙기 · 끊기 · 믿기는 `write` 다.
//!
//! - `remote.connect` 는 **스트림**이다: 열려 있는 동안 그 호스트를 「쓴다」([`RemoteHosts::hold`]) — 지금 상태부터 바뀔 때마다
//!   보낸다. 스트림을 모두 닫으면 링크를 닫는다(연결은 쓰는 머신만 — 08 「창과 활성 체크아웃」).
//! - `remote.prompts` 는 원격 호스트를 붙이는 ssh 의 askpass 질문(호스트 키 확인 · 암호)을 듣는 스트림, `remote.prompt.answer` 가
//!   그 답이다(git 의 `git.prompts` 와 같은 꼴 — 호스트 키 질문은 힌트 없이 온다, 창이 글로 알아본다).

use pw_git::write::{AnswerError, GitPromptAnswer, GitPromptEvent};
use pw_rpc::{ConnectionScope, Ctx, Router, RpcError, code, rpc_method};

use crate::{
    PROMPTS_KEY, RemoteError, RemoteHostCheck, RemoteHostList, RemoteHostParams, RemoteHosts,
    RemoteLinkCommandParams, RemoteLogEntry, RemotePromptAnswerParams, RemoteRelays, RemoteStatus,
    RemoteStopOldParams, RemoteStopOutcome, RemoteTrustMachineParams,
};

rpc_method! {
    /// 원격 호스트 목록: 전역 `[[remote_hosts]]` 와 `~/.ssh/config` 의 `Host` 이름(와일드카드 없음), 각각의 지금 상태 · 기억한
    /// 머신 id · 마지막 링크 상태. 링크를 열지 않는다.
    pub RemoteHostsList: "remote.hosts.list" {
        params: (),
        result: RemoteHostList,
        tier: Read,
        roles: [Owner],
    }

    /// 링크 없이 가볍게 본다: 사람의 ControlMaster 가 떠 있는가(`ssh -O check`)와 마지막 링크 상태.
    pub RemoteHostsCheck: "remote.hosts.check" {
        params: RemoteHostParams,
        result: RemoteHostCheck,
        tier: Read,
        roles: [Owner],
    }

    /// 그 호스트의 연결 로그(단계 · 실패 · ssh stderr 끝부분). askpass 답은 없다.
    pub RemoteLog: "remote.log" {
        params: RemoteHostParams,
        result: Vec<RemoteLogEntry>,
        tier: Read,
        roles: [Owner],
    }

    /// 지금 링크에서 본 원격 Relay(18-m7-plan M7-07b ②): 떠 있는 옛 판 데몬(판 · 실행 · 터미널 · 붙은 링크 수 — 사람이 본 그대로
    /// `remote.stopOld` 에 싣는다) · 답하지 않는 옛 판 · `pitwall` 명령 링크 · 이번 정리에서 지운 것. 링크가 없거나 아직 보지 않았으면
    /// `null`. 바뀌면 `remote.relays` 이벤트.
    pub RemoteRelaysGet: "remote.relays" {
        params: RemoteHostParams,
        result: Option<RemoteRelays>,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 그 호스트를 쓴다: 링크가 없으면 붙고(SSH → 설치 → 데몬 → 핸드셰이크), 스트림이 열린 동안 링크를 둔다. 지금 상태부터 바뀔
    /// 때마다 보낸다. 사람이 끊어 둔 호스트도 다시 붙는다. 모든 스트림이 닫히면 링크를 닫는다.
    pub RemoteConnect: "remote.connect" {
        params: RemoteHostParams,
        stream: RemoteStatus,
        tier: Write,
        roles: [Owner],
    }

    /// 원격 호스트를 붙이는 ssh 의 질문 스트림(호스트 키 확인 · 암호). 열 때 아직 열린 질문이 먼저 온다.
    pub RemotePrompts: "remote.prompts" {
        params: (),
        stream: GitPromptEvent,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 사람이 끊는다. 쓰는 창이 남아 있어도 다시 붙지 않는다(새 `remote.connect` · `remote.retry` 까지).
    pub RemoteDisconnect: "remote.disconnect" {
        params: RemoteHostParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }

    /// 다시 시도: 쓰는 창이 있으면 기다림 없이 지금 붙는다.
    pub RemoteRetry: "remote.retry" {
        params: RemoteHostParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }

    /// 「이 머신을 새로 믿기」: 그 호스트가 `machineChanged` 이고 `found` 가 `machine` 일 때만 — 묶음 멤버의 머신 id 를 옮기고 새
    /// id 를 기억한 뒤 다시 붙는다.
    pub RemoteTrustMachine: "remote.trustMachine" {
        params: RemoteTrustMachineParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }

    /// [옛 판 끝내기](M7-07b ③): 그 호스트가 지금 `machine` 에 붙어 있고, 원격의 그 옛 판 데몬이 사람이 본 `seen` 그대로(pid · 실행 ·
    /// 터미널 · 붙은 링크 수)일 때만 SIGTERM — 실행 트리는 그 데몬이 끝낸다. 바뀌었으면 끝내지 않고 `changed` 로 지금 것을 돌려준다.
    pub RemoteStopOld: "remote.stopOld" {
        params: RemoteStopOldParams,
        result: RemoteStopOutcome,
        tier: Write,
        roles: [Owner],
    }

    /// 원격의 `~/.local/bin/pitwall` 링크를 이 판 Relay 로 두거나(`on`) 걷는다(메인 판정 M7-P12 — 사람이 고를 때만). 그 자리에 사람의
    /// 다른 파일이 있으면 Relay 가 거절한다. 그 호스트가 지금 `machine` 에 붙어 있을 때만.
    pub RemoteLinkCommand: "remote.linkCommand" {
        params: RemoteLinkCommandParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }

    /// 원격 askpass 질문의 답. 처음 온 답만 받는다.
    pub RemotePromptAnswer: "remote.prompt.answer" {
        params: RemotePromptAnswerParams,
        result: (),
        tier: Write,
        roles: [Owner],
    }
}

/// Host 범위 연결만(웰컴 · 설정 창).
fn host_scope(ctx: &Ctx) -> Result<(), RpcError> {
    match ctx.scope() {
        ConnectionScope::Host => Ok(()),
        _ => Err(RpcError::new(
            code::PERMISSION_DENIED,
            "remote hosts are managed from a host connection (the welcome or settings window)",
        )),
    }
}

fn rpc(e: RemoteError) -> RpcError {
    match e {
        RemoteError::UnknownHost(_) => RpcError::new(code::NOT_FOUND, e.to_string()),
        RemoteError::Unavailable => RpcError::internal(e.to_string()),
        RemoteError::Conflict(_) => RpcError::invalid_params(e.to_string()),
        RemoteError::Remote(_) => RpcError::new(code::CONFLICT, e.to_string()),
    }
}

/// 핸들러를 붙인다.
pub fn register(router: &mut Router, remote: &RemoteHosts) {
    let r = remote.clone();
    router.unary(RemoteHostsList, move |ctx, ()| {
        let r = r.clone();
        async move {
            host_scope(&ctx)?;
            tokio::task::spawn_blocking(move || r.list())
                .await
                .map_err(|e| RpcError::internal(e.to_string()))
        }
    });
    let r = remote.clone();
    router.unary(RemoteHostsCheck, move |ctx, p: RemoteHostParams| {
        let r = r.clone();
        async move {
            host_scope(&ctx)?;
            r.check(&p.name).await.map_err(rpc)
        }
    });
    let r = remote.clone();
    router.unary(RemoteLog, move |ctx, p: RemoteHostParams| {
        let r = r.clone();
        async move {
            host_scope(&ctx)?;
            Ok(r.log(&p.name))
        }
    });
    let r = remote.clone();
    router.unary(RemoteRelaysGet, move |ctx, p: RemoteHostParams| {
        let r = r.clone();
        async move {
            host_scope(&ctx)?;
            Ok(r.relays(&p.name))
        }
    });
    let r = remote.clone();
    router.unary(RemoteStopOld, move |ctx, p: RemoteStopOldParams| {
        let r = r.clone();
        async move {
            host_scope(&ctx)?;
            r.stop_old(&p.name, &p.machine, &p.seen).await.map_err(rpc)
        }
    });
    let r = remote.clone();
    router.unary(RemoteLinkCommand, move |ctx, p: RemoteLinkCommandParams| {
        let r = r.clone();
        async move {
            host_scope(&ctx)?;
            r.link_command(&p.name, &p.machine, p.on).await.map_err(rpc)
        }
    });
    let r = remote.clone();
    router.stream(RemoteConnect, move |ctx, p: RemoteHostParams, sink| {
        let r = r.clone();
        async move {
            host_scope(&ctx)?;
            let _use = r.hold(&p.name).map_err(rpc)?;
            let mut status = r.watch(&p.name);
            loop {
                let now = status.borrow_and_update().clone();
                sink.send(now).await?;
                if status.changed().await.is_err() {
                    return Ok(());
                }
            }
        }
    });
    let r = remote.clone();
    router.stream(RemotePrompts, move |ctx, (), sink| {
        let r = r.clone();
        async move {
            host_scope(&ctx)?;
            let mut listener = r.prompts().listen(PROMPTS_KEY);
            while let Some(event) = listener.recv().await {
                sink.send(event).await?;
            }
            Ok(())
        }
    });
    let r = remote.clone();
    router.unary(RemoteDisconnect, move |ctx, p: RemoteHostParams| {
        let r = r.clone();
        async move {
            host_scope(&ctx)?;
            r.disconnect(&p.name).map_err(rpc)
        }
    });
    let r = remote.clone();
    router.unary(RemoteRetry, move |ctx, p: RemoteHostParams| {
        let r = r.clone();
        async move {
            host_scope(&ctx)?;
            r.retry(&p.name).map_err(rpc)
        }
    });
    let r = remote.clone();
    router.unary(
        RemoteTrustMachine,
        move |ctx, p: RemoteTrustMachineParams| {
            let r = r.clone();
            async move {
                host_scope(&ctx)?;
                let (name, machine) = (p.name, p.machine);
                tokio::task::spawn_blocking(move || r.trust_machine(&name, &machine))
                    .await
                    .map_err(|e| RpcError::internal(e.to_string()))?
                    .map_err(rpc)
            }
        },
    );
    let r = remote.clone();
    router.unary(
        RemotePromptAnswer,
        move |ctx, p: RemotePromptAnswerParams| {
            let r = r.clone();
            async move {
                host_scope(&ctx)?;
                if let GitPromptAnswer::Askpass { value } = &p.answer {
                    r.remember_secret(value);
                }
                r.prompts()
                    .answer(PROMPTS_KEY, p.prompt_id, p.answer)
                    .map_err(|e| match e {
                        AnswerError::NotFound => RpcError::new(
                            code::NOT_FOUND,
                            "no such open prompt (already answered or closed)",
                        ),
                        AnswerError::Mismatch => {
                            RpcError::invalid_params("the answer does not fit the prompt")
                        }
                    })
            }
        },
    );
}
