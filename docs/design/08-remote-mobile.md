# 08. 원격 & 모바일 연동

## 1. 개요

- **원격**: 원격 머신에 **Pitwall Relay**를 설치해서 연결한다. 연결은 **직접 연결(LAN · Tailscale)**만 사용하고, SSH는 Relay 설치 보조와 대체 연결 수단으로 쓴다
- **모바일**: 데스크톱 또는 원격 Relay에 붙어서 **내용 확인 + 간단한 조작**을 하는 컴패니언 앱 (**Android 먼저**)
- 원격과 모바일은 **같은 연결 계층(`pw-link`)**을 공유한다
- **중계 서버(Relay Hub)는 만들지 않는다.** NAT · 방화벽 문제는 Tailscale에 맡긴다. 단, 전송 계층을 추상화해 두어 필요해지면 Hub를 나중에 추가할 수 있게 한다

## 2. 구성 요소

| 구성 요소 | 설치 위치 | 역할 |
|---|---|---|
| **Pitwall Desktop** | 개발자 PC | 메인 UI. 내부에 Local Host + 내장 Relay 엔드포인트(모바일 접속용) |
| **Pitwall Relay** (`pitwall-relay`) | 원격 서버 / 개발 VM / 집 PC | Host 코어(fs/git/lsp/dap/run/db)를 데몬으로 실행 + 암호화된 연결 엔드포인트. 서비스로 상주(launchd/systemd/Windows Service) |
| **Pitwall Mobile** | Android (이후 iOS) | 컴패니언 앱 |

```
   ┌──────────── Tailscale tailnet 또는 같은 LAN ─────────────┐
   │                                                          │
   │  ┌──────────────────┐   QUIC + Noise   ┌───────────────┐ │
   │  │ Pitwall Desktop  │◄────────────────►│ Pitwall Relay │ │
   │  │ (Local Host +    │                  │ (원격 머신)     │ │
   │  │  내장 Relay)      │                  └───────▲───────┘ │
   │  └────────▲─────────┘                          │         │
   │           │        QUIC + Noise                │         │
   │           └──────────────┬─────────────────────┘         │
   │                  ┌───────┴────────┐                       │
   │                  │ Pitwall Mobile │                       │
   │                  └────────────────┘                       │
   └──────────────────────────────────────────────────────────┘
        tailnet 밖이거나 Relay를 상주시킬 수 없으면: SSH stdio로 연결
```

## 3. 연결 방식

| 우선순위 | 방식 | 조건 |
|---|---|---|
| 1 | **직접 연결** (QUIC, 기본 포트 7420) | 같은 LAN 또는 **Tailscale** tailnet. 주소는 Tailscale MagicDNS 이름 / IP / mDNS(LAN) |
| 2 | **SSH 터널** | Relay를 데몬으로 둘 수 없거나 직접 연결이 막힌 환경 — `ssh host pitwall-relay --stdio` |

- **Tailscale 연동 (D)**
  - Relay는 기본적으로 **Tailscale 인터페이스와 LAN에만 바인드** (공인 IP에 열지 않음)
  - Desktop/Mobile은 tailnet 기기 목록(로컬 Tailscale API)에서 Relay가 떠 있는 기기를 자동 검색
  - Tailscale이 없는 환경은 LAN(mDNS) 또는 SSH로 동작
- Tailscale이 이미 WireGuard로 암호화하지만, **Pitwall 자체 인증(Noise + 기기 키)도 항상 적용** → tailnet의 다른 기기라도 페어링하지 않으면 접근 불가
- 전송 위 프로토콜은 03-architecture의 JSON-RPC 그대로 → UI/MCP 입장에서는 로컬과 원격이 같다
- `pw-link`의 전송은 `Transport` trait으로 추상화 (`Quic`, `Ssh`, 추후 `Hub`)

## 4. 설치 & 페어링

### Relay 설치
1. 원격 머신에서 한 줄 설치: `curl -fsSL https://.../install.sh | sh` (또는 Homebrew/apt/단일 바이너리)
   - 또는 Desktop에서 "SSH로 Relay 설치" — SSH 접속 → 바이너리 업로드 → 서비스 등록까지 자동
2. `pitwall-relay pair` → 터미널에 **QR 코드 + 6자리 코드** 표시
3. Desktop/Mobile에서 QR 스캔 또는 코드 입력 → 페어링 완료

### 보안 모델
- 기기마다 Ed25519 키쌍 생성, 페어링 시 공개키 교환 (TOFU + 짧은 코드로 확인)
- 세션 암호화/인증: **Noise 프로토콜(XX 패턴)**
- Relay는 페어링된 기기 목록과 **기기별 권한 범위(role)**를 보유
  - `owner`(Desktop): 전체
  - `mobile`: 조회 + 허용된 조작만 (6절)
  - `readonly`: 조회만
- 기기 해제(revoke) 즉시 반영, 페어링 코드는 5분 만료
- Relay가 노출하는 프로젝트 경로는 화이트리스트 (`relay.toml`의 `roots`)

### Relay 설정 예

```toml
# ~/.config/pitwall/relay.toml
name = "build-server"
roots = ["~/work"]                 # 이 아래만 프로젝트로 노출
[listen]
port = 7420
interfaces = ["tailscale", "lan"]  # 기본값. "all"은 명시적으로만
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
| MCP | 원격 머신의 에이전트도 Relay의 로컬 MCP 엔드포인트 사용 → 승인 요청은 페어링된 Desktop/Mobile로 전달 |
| 세션 유지 | Relay가 상주하므로 연결이 끊겨도 실행 중 프로세스 · 로그 유지, 재접속 시 이어보기 |

## 6. 모바일

### 6.1 목표
"자리를 비웠을 때 확인하고, 막힌 것을 풀어주는 앱" — 코딩/편집은 하지 않는다.

### 6.2 기능

| 분류 | 기능 | 우선 |
|---|---|---|
| 연결 | QR 페어링, 여러 Desktop/Relay 목록, 연결 상태 | P0 |
| 프로젝트 | 프로젝트 목록, 현재 브랜치 · 변경 개수 · 실행 상태 요약 대시보드 | P0 |
| **알림** | 실행 종료/크래시, 빌드 실패, 디버거 브레이크포인트 정지, **MCP 승인 요청** (6.4 참고) | P0 |
| **MCP 승인** | 에이전트의 설정 변경/실행/DB 쓰기 요청을 diff와 함께 보고 승인·거절 | P0 |
| 실행 | 실행 구성 목록, 시작/중지/재시작, 로그 실시간 tail · 검색 | P1 (데스크톱 실행 기능 일정에 맞춤) |
| Git 조회 | 상태(변경 파일), diff 보기, 커밋 로그/그래프(간소화), 커밋 상세 | P0 |
| Git 조작 | fetch/pull, 브랜치 체크아웃, 전체 스테이징 후 커밋(메시지 입력), push | P1 |
| 파일 | 파일 트리 탐색, 읽기 전용 뷰어(하이라이트), 파일 검색 | P1 |
| 진단 | LSP 문제 목록 | P1 |
| DB | 저장된 쿼리 실행(읽기 전용), 결과 표 보기, Redis 키 조회 | P1 |
| 디버그 | 정지 위치 · 스택 · 변수 보기, 계속/스텝 | P2 |
| 터미널 | 간단한 터미널 (명령 입력) | P2 |
| 위젯 | 홈 화면 위젯: 실행 상태, 대기 중인 승인 수 | P2 |

**모바일에서 일부러 막는 것**: 파일 편집, force push, reset --hard, 머지 충돌 해결, 프로덕션 DB 쓰기

### 6.3 기술 스택

| 항목 | 선택 | 근거 |
|---|---|---|
| 앱 | **React Native (Expo, dev build)** | 포그라운드 서비스 · 생체인증 · 위젯 등 네이티브 기능 접근, TS로 데스크톱과 코드 공유. iOS 추가 시 같은 코드베이스 |
| 플랫폼 | **Android 먼저** (minSdk 28 / Android 9+), iOS는 이후 | 사이드로드(APK)로 개인 배포가 쉬움, 상시 연결을 위한 포그라운드 서비스 허용 |
| 공유 코드 | `packages/rpc-client`, `packages/domain`, i18n `.ftl` 리소스 | UI 컴포넌트는 공유하지 않음 (모바일 UX 별도 설계) |
| 연결/암호화 | Rust `pw-link`를 **uniffi**로 Kotlin 바인딩 → Expo 네이티브 모듈 | Noise/QUIC 구현을 데스크톱과 하나로 유지 |
| 코드 뷰어 | Relay가 tree-sitter 하이라이트 토큰을 내려주고 모바일은 렌더링만 | 모바일에 문법 번들 불필요 |
| 보안 | 앱 잠금(지문/얼굴, BiometricPrompt), 승인 동작은 생체인증 재확인, 기기 키는 Android Keystore | |
| 배포 | 1단계: GitHub Releases APK (서명), 2단계: Play 스토어 / F-Droid 검토 | MIT 오픈소스와 궁합 |

### 6.4 알림 — Hub/푸시 서버 없이

Hub를 두지 않으므로 FCM 같은 서버 푸시를 쓰지 않는다.

| 방식 | 내용 | 기본 |
|---|---|---|
| **포그라운드 서비스 상시 연결** | 앱이 Android 포그라운드 서비스로 tailnet 위 Desktop/Relay와 연결을 유지, 이벤트 수신 시 **로컬 알림** 생성 | ✅ 기본 |
| 주기 확인 | WorkManager로 15분 간격 상태 조회 (배터리 절약 모드) | 선택 |
| UnifiedPush (ntfy 등) | 사용자가 ntfy 서버를 이미 쓰는 경우 연동. 페이로드는 "새 이벤트 있음"만, 내용은 앱이 직접 조회 | P2 |

- 연결은 QUIC keep-alive를 길게(예: 60s) 잡고, 화면 꺼짐/Doze 시 재연결 백오프
- 상시 연결은 사용자가 켤 때만 (상단 고정 알림 표시 — Android 정책)
- iOS를 추가할 때는 백그라운드 연결이 제한되므로 그때 푸시 경로(APNs, 즉 Hub 또는 ntfy 연동)를 다시 결정

## 7. 모듈 영향

| 크레이트/패키지 | 내용 |
|---|---|
| `pw-link` | 기기 키, 페어링, Noise 세션, `Transport` 추상(QUIC/SSH), Tailscale 기기 검색, mDNS |
| `pw-relay` | Relay 데몬, 서비스 등록, roots 화이트리스트, 기기별 권한, SSH 설치 부트스트랩 |
| `apps/relay` | `pitwall-relay` 바이너리 |
| `apps/mobile` | Expo 앱 (Android 우선) |
| RPC | 모든 메서드에 필요한 권한(role) 메타데이터 → Relay가 기기 role로 필터링 |
