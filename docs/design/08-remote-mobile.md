# 08. 원격 & 모바일 연동

## 1. 개요

- **원격**: 원격 머신에 **Pitwall Relay**를 설치해서 연결한다. SSH는 설치를 돕는 경로와 대체 연결 수단으로만 쓴다
- **모바일**: 데스크톱 또는 원격 Relay에 붙어서 **내용 확인 + 간단한 조작**을 하는 컴패니언 앱
- 원격과 모바일은 **같은 연결 계층(Relay 네트워크)**을 공유한다 → 기기 간 연결을 한 번 설계해서 두 기능에 같이 사용

## 2. 구성 요소

| 구성 요소 | 설치 위치 | 역할 |
|---|---|---|
| **Pitwall Desktop** | 개발자 PC | 메인 UI. 내부에 Local Host + 내장 Relay 엔드포인트 포함(모바일이 붙을 수 있도록) |
| **Pitwall Relay** (`pitwall-relay`) | 원격 서버 / 개발 VM / 집 PC 등 | Host 코어(fs/git/lsp/dap/run/db)를 데몬으로 실행 + 암호화된 연결 엔드포인트 제공. 서비스로 상주(launchd/systemd/Windows Service) |
| **Relay Hub** (선택) | 공용 서버 또는 셀프호스팅 | NAT/방화벽 뒤 기기들의 랑데부 + 트래픽 중계. 내용은 종단간 암호화되어 Hub가 볼 수 없음 |
| **Pitwall Mobile** | iOS / Android | 컴패니언 앱 |

```
           ┌──────────── Relay Hub (선택, 중계만) ────────────┐
           │      E2E 암호화된 바이트만 전달 · 기기 랑데부          │
           └───▲──────────────────▲───────────────────▲──────┘
               │ 아웃바운드 WS/QUIC  │                   │
     ┌─────────┴────────┐ ┌───────┴────────┐  ┌───────┴────────┐
     │ Pitwall Desktop  │ │ Pitwall Relay  │  │ Pitwall Mobile │
     │ (Local Host +    │◄┤ (원격 머신)      │  │                │
     │  내장 Relay)      │ │ Host 코어 데몬   │  │                │
     └──────────────────┘ └────────────────┘  └────────────────┘
          ▲   직접 연결(LAN / Tailscale / 포트 개방) 가능하면 Hub를 거치지 않음
```

## 3. 연결 방식

| 우선순위 | 방식 | 조건 |
|---|---|---|
| 1 | **직접 연결** (QUIC/TLS) | 같은 LAN, Tailscale/WireGuard, 포트 개방 |
| 2 | **Hub 경유** | NAT 뒤, 인바운드 불가 — Relay/Desktop이 Hub로 아웃바운드 연결을 유지 |
| 3 | **SSH 터널** | Relay를 데몬으로 둘 수 없는 환경 — `ssh host pitwall-relay --stdio` (기존 SSH 방식) |

- 연결 수립 시 직접 연결을 먼저 시도하고, 실패하면 Hub로 넘어감 (ICE와 유사한 단순 버전)
- 전송 위 프로토콜은 03-architecture의 JSON-RPC 그대로 → UI/MCP 입장에서는 로컬과 원격이 같다

## 4. 설치 & 페어링

### Relay 설치
1. 원격 머신에서 한 줄 설치: `curl -fsSL https://.../install.sh | sh` (또는 Homebrew/apt/winget/단일 바이너리)
   - 또는 Desktop에서 "SSH로 Relay 설치" — SSH 접속 → 바이너리 업로드 → 서비스 등록까지 자동
2. `pitwall-relay pair` → 터미널에 **QR 코드 + 6자리 코드** 표시
3. Desktop/Mobile에서 QR 스캔 또는 코드 입력 → 페어링 완료

### 보안 모델
- 기기마다 Ed25519 키쌍 생성, 페어링 시 공개키 교환 (TOFU + 짧은 코드로 확인)
- 세션 암호화: **Noise 프로토콜(XX 패턴)** — Hub는 암호문만 중계
- Relay는 페어링된 기기 목록과 **기기별 권한 범위(role)**를 보유
  - `owner`(Desktop): 전체
  - `mobile`: 조회 + 허용된 조작만 (아래 6절)
  - `readonly`: 조회만
- 기기 해제(revoke) 즉시 반영, 페어링 코드는 5분 만료
- Relay가 노출하는 프로젝트 경로는 화이트리스트 (`relay.toml`의 `roots`)

### Relay 설정 예

```toml
# ~/.config/pitwall/relay.toml
name = "build-server"
roots = ["~/work"]                 # 이 아래만 프로젝트로 노출
[hub]
url = "wss://hub.example.com"     # 미설정 시 직접 연결만
[listen]
quic = "0.0.0.0:7420"             # 선택
[idle]
stop_lsp_after = "30m"
```

## 5. 원격 기능 범위

| 기능 | 원격 동작 |
|---|---|
| 파일 트리 · 에디터 · 검색 | Relay에서 처리, 파일 내용은 요청 시 전송 + 로컬 캐시 |
| Git | Relay에서 처리 (그래프 데이터는 스트리밍) |
| LSP · 인덱스 | Relay에서 언어 서버 실행 |
| 실행 · 디버그 | Relay에서 프로세스 실행, DAP 중계, **포트 포워딩**(원격 웹서버/디버그 포트 → 로컬) |
| 터미널 | Relay의 PTY |
| DB | Relay에서 직접 연결 (내부망 DB 접근에 유리) |
| MCP | 원격 머신의 에이전트도 Relay의 로컬 MCP 엔드포인트 사용 가능 → 승인 요청은 페어링된 Desktop/Mobile로 전달 |
| 세션 유지 | 연결이 끊겨도 Relay는 상주하므로 실행 중 프로세스 · 로그가 유지됨, 재접속 시 이어보기 |

## 6. 모바일

### 6.1 목표
"자리를 비웠을 때 확인하고, 막힌 것을 풀어주는 앱" — 코딩/편집은 하지 않는다.

### 6.2 기능

| 분류 | 기능 | 우선 |
|---|---|---|
| 연결 | QR 페어링, 여러 Desktop/Relay 목록, 연결 상태 | P0 |
| 프로젝트 | 프로젝트 목록, 현재 브랜치 · 변경 개수 · 실행 상태 요약 대시보드 | P0 |
| **알림** | 실행 종료/크래시, 빌드·테스트 실패, 디버거 브레이크포인트 정지, **MCP 승인 요청** | P0 |
| **MCP 승인** | 에이전트의 설정 변경/실행/DB 쓰기 요청을 diff와 함께 보고 승인·거절 | P0 |
| 실행 | 실행 구성 목록, 시작/중지/재시작, 로그 실시간 tail · 검색 | P0 |
| Git 조회 | 상태(변경 파일), diff 보기, 커밋 로그/그래프(간소화), 커밋 상세 | P0 |
| Git 조작 | fetch/pull, 브랜치 체크아웃, 전체 스테이징 후 커밋(메시지 입력), push | P1 |
| 파일 | 파일 트리 탐색, 읽기 전용 뷰어(하이라이트), 파일 검색 | P1 |
| 진단 | LSP 문제 목록 | P1 |
| DB | 저장된 쿼리 실행(읽기 전용), 결과 표 보기 | P1 |
| 디버그 | 정지 위치 · 스택 · 변수 보기, 계속/스텝 | P2 |
| 터미널 | 간단한 터미널 (명령 입력) | P2 |
| 위젯 | iOS/Android 홈 위젯: 실행 상태, 대기 중인 승인 수 | P2 |

**모바일에서 일부러 막는 것**: 파일 편집, force push, reset --hard, 머지 충돌 해결, 프로덕션 DB 쓰기

### 6.3 기술 스택

| 항목 | 선택 | 근거 |
|---|---|---|
| 앱 | **React Native (Expo)** | 푸시 알림 · 백그라운드 · 위젯 · 생체인증이 성숙. TS로 데스크톱과 코드 공유 |
| 공유 코드 | `packages/rpc-client`(생성된 타입+클라이언트), `packages/domain`(포맷터, 상태 로직), i18n `.ftl` 리소스 | UI 컴포넌트는 공유하지 않음 (모바일 UX 별도 설계) |
| 연결/암호화 | Rust 연결 계층(`pw-link`)을 **uniffi**로 iOS/Android 네이티브 모듈화 | Noise/QUIC 구현을 데스크톱과 하나로 유지 |
| 코드 뷰어 | tree-sitter 하이라이트 결과를 Relay가 토큰으로 내려주고 모바일은 렌더링만 | 모바일에 문법 번들 불필요 |
| 푸시 | APNs / FCM — 알림 내용은 E2E 암호화된 페이로드, 앱에서 복호화(Notification Service Extension) | Hub/푸시 서버가 내용을 볼 수 없게 |
| 보안 | 앱 잠금(Face ID/지문), 승인 동작은 생체인증 재확인 | |

- 대안 검토: Tauri 2 mobile은 데스크톱 UI 재사용이 장점이나 푸시/위젯/백그라운드 플러그인 생태계가 아직 약함 → 모바일은 RN 채택
- 푸시 전송은 Hub가 담당(앱이 Hub에 푸시 토큰 등록). Hub 없이 직접 연결만 쓰는 경우 앱이 포그라운드일 때만 알림

## 7. 모듈 영향

| 크레이트/패키지 | 변경 |
|---|---|
| `pw-link` (신규) | 기기 키, 페어링, Noise 세션, 직접/Hub/SSH 전송 추상 |
| `pw-remote` → `pw-relay` | Relay 데몬, 서비스 등록, roots 화이트리스트, 기기별 권한 |
| `apps/relay` (신규) | `pitwall-relay` 바이너리 (기존 `apps/host` 대체·통합) |
| `apps/hub` (신규) | Relay Hub 서버 (axum, 셀프호스팅용 단일 바이너리/도커 이미지) |
| `apps/mobile` (신규) | Expo 앱 |
| RPC | 모든 메서드에 필요한 권한(role) 메타데이터 부여 → Relay가 기기 role로 필터링 |
