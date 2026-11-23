//! `remote.*` RPC(18-m7-plan M7-08 ①). 모두 owner 만이고 **Host 범위 연결만** 받는다(웰컴 · 설정 창 — 프로젝트 창 · 파일 창 · MCP
//! 세션은 거절). MCP 툴이 아니다. 읽기는 `read`, 붙기 · 끊기 · 믿기는 `write` 다.
//!
//! - `remote.connect` 는 **스트림**이다: 열려 있는 동안 그 호스트를 「쓴다」([`RemoteHosts::hold`]) — 지금 상태부터 바뀔 때마다
//!   보낸다. 스트림을 모두 닫으면 링크를 닫는다(연결은 쓰는 머신만 — 08 「창과 활성 체크아웃」).
//! - `remote.prompts` 는 원격 호스트를 붙이는 ssh 의 askpass 질문(호스트 키 확인 · 암호)을 듣는 스트림, `remote.prompt.answer` 가
//!   그 답이다(git 의 `git.prompts` 와 같은 꼴 — 호스트 키 질문은 힌트 없이 온다, 창이 글로 알아본다).
//! - 포트 포워딩(18-m7-plan M7-22): `remote.forwards.list` · `remote.forward.close`(상태바 · 원격 호스트 시트), 실행 탭의
//!   `remote.forward.open` 스트림(열린 동안 포워딩과 그 호스트를 쓴다). 원격 쪽은 늘 `localhost` 다 — 설정
//!   (`[[remote_hosts.port_forwards]]`)의 `remote_host` 도 그 머신의 루프백만이다(메인 판정 2026-11-24). MCP 툴이 아니다.

use pw_git::write::{AnswerError, GitPromptAnswer, GitPromptEvent};
use pw_rpc::{ConnectionScope, Ctx, Router, RpcError, code, rpc_method};

use pw_link::devices::{RelayDeviceList, RelayPairCode, RelayUnpairResult};

use crate::{
    ForwardOpenError, PROMPTS_KEY, RemoteDevicesParams, RemoteDiscovery, RemoteError,
    RemoteForward, RemoteForwardCloseParams, RemoteForwardOpenParams, RemoteForwardsParams,
    RemoteHostCheck, RemoteHostList, RemoteHostParams, RemoteHosts, RemoteLinkCommandParams,
    RemoteLogEntry, RemotePairCodeParams, RemotePairDirectParams, RemotePairDirectResult,
    RemotePromptAnswerParams, RemoteRelays, RemoteResident, RemoteResidentParams,
    RemoteResidentStatusParams, RemoteStatus, RemoteStopOldParams, RemoteStopOutcome,
    RemoteTrustMachineParams, RemoteUnpairDeviceParams,
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

    /// 기기 화면(18-m7-plan M7-23 ③): 그 호스트가 지금 `machine` 에 붙어 있으면 그 Relay 의 페어링 표(`relay.devices`)를 읽는다.
    /// 링크를 새로 열지 않는다. role 판정은 Relay 가 한다(owner 기기만 — 두 겹).
    pub RemoteDevices: "remote.devices" {
        params: RemoteDevicesParams,
        result: RelayDeviceList,
        tier: Read,
        roles: [Owner],
    }

    /// 그 Relay 에서 기기 하나를 해제한다(`relay.unpair` — 그 기기의 열린 링크 · 구독 · 터미널 붙기 · 이어 붙기 토큰이 곧바로 끊긴다).
    /// 그 호스트가 지금 `machine` 에 붙어 있을 때만. 이 Desktop 자신을 해제하면 이 링크도 끊긴다.
    pub RemoteUnpairDevice: "remote.unpairDevice" {
        params: RemoteUnpairDeviceParams,
        result: RelayUnpairResult,
        tier: Write,
        roles: [Owner],
    }

    /// 그 Relay 에 다른 기기를 붙일 6자리 코드를 만든다(`relay.pairCode` — 5분, 세 번 틀리면 끝, 한 번 쓰면 끝). 그 호스트가 지금
    /// `machine` 에 붙어 있을 때만.
    pub RemotePairCode: "remote.pairCode" {
        params: RemotePairCodeParams,
        result: RelayPairCode,
        tier: Write,
        roles: [Owner],
    }

    /// 직접 연결 후보 검색(18-m7-plan M7-24 ③): 로컬 Tailscale 의 온라인 기기와 LAN mDNS 광고. 그 주소에 직접 연결 받는 쪽이 있는지만
    /// 본다(누구인지는 증명하지 않는다).
    pub RemoteDiscover: "remote.discover" {
        params: (),
        result: RemoteDiscovery,
        tier: Read,
        roles: [Owner],
    }

    /// 코드 입력 화면(18-m7-plan M7-24 · M7-23 판정 5): 주소와 6자리 코드로 수동 배치 Relay 에 이 Desktop 을 페어링하고 직접 연결
    /// 전용 호스트 항목으로 더한다.
    pub RemotePairDirect: "remote.pairDirect" {
        params: RemotePairDirectParams,
        result: RemotePairDirectResult,
        tier: Write,
        roles: [Owner],
    }

    /// 「이 머신에 상주시키기」(18-m7-plan M7-24 ⑤ — 사람이 고를 때만, 결정 M7-D2): 그 머신의 이 판 Relay 를 launchd · systemd user
    /// 로 상주시키거나(`on`) 내린다. 그 호스트가 지금 `machine` 에 붙어 있고 SSH 로 닿을 때만.
    pub RemoteResidentSet: "remote.resident" {
        params: RemoteResidentParams,
        result: RemoteResident,
        tier: Write,
        roles: [Owner],
    }

    /// 그 머신의 이 판 Relay 가 상주인가(읽기만 — 단위 파일이 있는가).
    pub RemoteResidentStatus: "remote.residentStatus" {
        params: RemoteResidentStatusParams,
        result: RemoteResident,
        tier: Read,
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

rpc_method! {
    /// 포트 포워딩 목록(`name` 이 없으면 모든 호스트): 설정의 것(붙을 때 연다)과 실행 탭의 것, 듣는 로컬 주소 · 원격 대상 · 상태
    /// (열지 못했으면 까닭 — 루프백이 아님 · 로컬 포트가 쓰임). 바뀌면 `remote.forwards` 이벤트.
    pub RemoteForwardsList: "remote.forwards.list" {
        params: RemoteForwardsParams,
        result: Vec<RemoteForward>,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 실행 탭 포트의 「로컬로 포워딩」: 그 호스트의 `localhost:remotePort` 를 이 컴퓨터의 루프백(`127.0.0.1`)에서 듣는다. 스트림이
    /// 열린 동안 포워딩과 그 호스트를 쓴다(링크가 없으면 붙는다). 처음 열린 포워딩을, 닫히면(`remote.forward.close`) 끝을 보낸다. 로컬
    /// 포트를 이미 누가 쓰면 `conflict`(data `{ reason: "portInUse", port }`) — 다른 `localPort` 로 다시 연다.
    pub RemoteForwardOpen: "remote.forward.open" {
        params: RemoteForwardOpenParams,
        stream: RemoteForward,
        tier: Write,
        roles: [Owner],
    }
}

rpc_method! {
    /// 포워딩 하나를 끈다. 실행 탭의 것이면 그 스트림이 끝나고, 설정의 것은 다음에 붙을 때 다시 연다.
    pub RemoteForwardClose: "remote.forward.close" {
        params: RemoteForwardCloseParams,
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

fn forward_rpc(e: ForwardOpenError) -> RpcError {
    match e {
        ForwardOpenError::Remote(e) => rpc(e),
        ForwardOpenError::PortInUse(port) => RpcError::new(code::CONFLICT, e.to_string())
            .with_data(serde_json::json!({ "reason": "portInUse", "port": port })),
        ForwardOpenError::Bind(_) => RpcError::internal(e.to_string()),
    }
}

/// 핸들러를 붙인다.
pub fn register(router: &mut Router, remote: &RemoteHosts) {
    let r = remote.clone();
    router.unary(RemoteForwardsList, move |ctx, p: RemoteForwardsParams| {
        let r = r.clone();
        async move {
            host_scope(&ctx)?;
            Ok(r.forwards(p.name.as_deref()))
        }
    });
    let r = remote.clone();
    router.unary(
        RemoteForwardClose,
        move |ctx, p: RemoteForwardCloseParams| {
            let r = r.clone();
            async move {
                host_scope(&ctx)?;
                if r.close_forward(p.id) {
                    Ok(())
                } else {
                    Err(RpcError::new(code::NOT_FOUND, "no such port forward"))
                }
            }
        },
    );
    let r = remote.clone();
    router.stream(
        RemoteForwardOpen,
        move |ctx, p: RemoteForwardOpenParams, sink| {
            let r = r.clone();
            async move {
                host_scope(&ctx)?;
                let _use = r.hold(&p.name).map_err(rpc)?;
                let mut forward = r
                    .open_forward(&p.name, p.remote_port, p.local_port)
                    .await
                    .map_err(forward_rpc)?;
                loop {
                    let now = forward.watch.borrow_and_update().clone();
                    let Some(now) = now else { return Ok(()) };
                    sink.send(now).await?;
                    if forward.watch.changed().await.is_err() {
                        return Ok(());
                    }
                }
            }
        },
    );
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
    router.unary(RemoteDiscover, move |ctx, ()| {
        let r = r.clone();
        async move {
            host_scope(&ctx)?;
            r.discover().await.map_err(rpc)
        }
    });
    let r = remote.clone();
    router.unary(RemotePairDirect, move |ctx, p: RemotePairDirectParams| {
        let r = r.clone();
        async move {
            host_scope(&ctx)?;
            r.pair_direct(p).await.map_err(rpc)
        }
    });
    let r = remote.clone();
    router.unary(RemoteResidentSet, move |ctx, p: RemoteResidentParams| {
        let r = r.clone();
        async move {
            host_scope(&ctx)?;
            r.resident(&p.name, &p.machine, Some(p.on))
                .await
                .map_err(rpc)
        }
    });
    let r = remote.clone();
    router.unary(
        RemoteResidentStatus,
        move |ctx, p: RemoteResidentStatusParams| {
            let r = r.clone();
            async move {
                host_scope(&ctx)?;
                r.resident(&p.name, &p.machine, None).await.map_err(rpc)
            }
        },
    );
    let r = remote.clone();
    router.unary(RemoteDevices, move |ctx, p: RemoteDevicesParams| {
        let r = r.clone();
        async move {
            host_scope(&ctx)?;
            r.relay_call(
                &p.name,
                &p.machine,
                "relay.devices",
                serde_json::Value::Null,
            )
            .await
            .map_err(rpc)
        }
    });
    let r = remote.clone();
    router.unary(
        RemoteUnpairDevice,
        move |ctx, p: RemoteUnpairDeviceParams| {
            let r = r.clone();
            async move {
                host_scope(&ctx)?;
                r.relay_call(
                    &p.name,
                    &p.machine,
                    "relay.unpair",
                    serde_json::json!({ "device": p.device }),
                )
                .await
                .map_err(rpc)
            }
        },
    );
    let r = remote.clone();
    router.unary(RemotePairCode, move |ctx, p: RemotePairCodeParams| {
        let r = r.clone();
        async move {
            host_scope(&ctx)?;
            r.relay_call(
                &p.name,
                &p.machine,
                "relay.pairCode",
                serde_json::json!({ "role": p.role, "name": p.device_name }),
            )
            .await
            .map_err(rpc)
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
