---
name: orca-worker
categories: [agent]
description: Orca 오케스트레이션으로 워크트리에 워커 세션을 띄우고 감독한다. 워크트리 작업을 시작할 때, 워커가 응답하지 않을 때, 배달이 오지 않을 때 쓴다.
requires: [rules/orca]
---

# orca-worker

워크트리 작업을 Orca 터미널 워커로 돌리는 **절차**다.

**규칙은 여기 없다.** 역할 분리는 `base` 스택 「워커에게 일을 맡길 때」가, 브리프·제출 확인은 「워커에게 브리프를 보낼 때」가, 워크트리 생성·붙이기·디스패치·정리에서 **이 도구가 조용히 실패하는 자리**는 이 디렉터리의 보조 파일이 갖는다 — 막혔을 때 아래 표에서 골라 연다. 이 파일은 그 규칙들을 **어떤 순서로 밟는가**만 말한다.

## 언제 쓰나

**트리거는 「병렬」이 아니라 「워크트리 작업」이다.** 워크트리를 하나만 쓰는 단일 작업도 똑같이 워커로 띄운다.

**판단 지점은 「워크트리를 만들 때」가 아니라 「일을 시작할 때」다.** 워크트리 생성은 `git worktree add` 로 해도 된다. 갈리는 것은 **그 안에서 실제 작업을 시작하는 순간**이다 — 메인 세션이 워크트리에 들어가 직접 파일을 고치거나 서브에이전트로 처리해버리는 것이 가장 흔한 이탈이다.

- **Why:** Orca 터미널 워커는 앱에 **독립 터미널**로 떠서 진행을 직접 보고 개입할 수 있고, 태스크·디스패치 이력이 남는다. 서브에이전트는 컨텍스트가 코디네이터 안에 숨어 안 보인다.
- **워크트리를 만들었으면 이미 직접 처리 대상이 아니다.** 만들어 놓고 「이 정도는 간단하니 내가 하겠다」로 되돌아가는 것이 이 이탈이다.

## 절차

**방식을 외우지 말고** `orca skills get orchestration` 으로 버전 맞춤 가이드를 읽는다 — 플래그·서브커맨드가 릴리스마다 바뀐다. 순서는 이렇다.

**워커를 띄우는 명령을 바꾸는 스택이 프로젝트 지침에 있으면 2~3단계와 모델 고르기는 그 스택을 따른다.** 나머지 단계는 그대로 밟는다.

1. **환경 확인을 워크트리보다 먼저 한다.** `orca status --json` 이 running runtime 을 보이면 Orca 환경이다. 아니면 워크트리만 만들고 서브에이전트로 진행하거나 사용자에게 경로를 알린다.
2. **워크트리를 만든다.** 기준 브랜치와 생성 위치에서 사고가 난다 — [worktree.md](worktree.md).
3. **Task 를 세우고 워커를 붙인다.** `worker-start` 에 `--agent` 를 주어 **Orca 가 터미널까지 띄우게 한다.** 터미널을 먼저 만들어 놓고 붙이지 않는다 — [worker-start.md](worker-start.md).
   ```bash
   orca orchestration task-create --spec <브리프> --task-title <역할:슬러그> --json
   orca orchestration worker-start --task <task_id> --worktree id:<fullId> \
       --agent claude --model <모델> --timeout-ms 180000 --json
   ```
4. **브리프가 제출됐는지 눈으로 확인한다 — 생략하지 않는다.** `orca terminal read --terminal <handle> --json` 으로 읽고 **마지막 프롬프트(`❯`) 줄 뒤가 비었는지** 본다. 비어 있으면 제출된 것이고, 텍스트가 남아 있으면 미제출이다. 판정 규칙은 `base` 스택 「『보냈다』는 『상대가 받았다』가 아니다」 절이 갖는다.
   미제출이면 엔터만 보내고 다시 읽어 확인한다.
   ```bash
   orca terminal send --terminal <handle> --text "" --enter --json
   ```
5. **곧바로 수신 대기에 들어간다.** 감독이면 `check --wait --types worker_done,escalation,decision_gate`. 완전 위임이면 소유권을 넘기고 lifecycle 프리앰블을 붙이지 않는다 — [dispatch.md](dispatch.md).
6. **끝나면 회수하고 정리한다** — [cleanup.md](cleanup.md).

## 막혔을 때 어디를 보나

| 증상 | 어느 절 |
| --- | --- |
| 붙이기가 실패했다 / 재시도해도 되나 | [worker-start.md](worker-start.md) |
| `ok: true` 를 받았는데 워커가 안 움직인다 | [delivery.md](delivery.md) |
| 대기자가 아무것도 못 받는다 / 안 깨어난다 | 같은 파일의 「폴링 루프를 만들지 말고」·「대기자를 버렸으면 기상 장치를」 |
| 배달이 안 오는데 dispatch 가 `completed` 다 | 같은 파일의 「배달이 안 오는데 dispatch 가 이미 `completed` 면」 |
| 수정 회차를 보냈는데 완료가 안 온다 / 워커의 질문이 거절된다 | 같은 파일의 「닫힌 Dispatch 의 워커에게 이어서 일을」 |
| 워커가 브리프 파일을 못 찾는다 | [worker-start.md](worker-start.md) 「Task 의 `--spec` 에는」 |
