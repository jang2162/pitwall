# 08. 원격 & 모바일 연동

## 1. 개요

- **원격**: 원격 머신에 **Pitwall Relay**를 설치해서 연결한다. 연결은 **직접 연결(LAN · Tailscale)**만 사용하고, SSH는 Relay 설치 보조와 대체 연결 수단으로 쓴다
- **모바일**: **보류 (D13)** — 6절은 향후 검토용 초안. 연결 계층(`pw-link`)과 role 구조만 모바일을 고려해 유지
- 원격 설치 · 재연결 방식은 **Zed 원격 개발 구조**를 참고 (UI 로컬 / 헤드리스 서버 원격, 버전 일치 서버 자동 설치, 데몬 재사용)
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

**방법 A. 원격에서 직접 설치** — `curl -fsSL https://.../install.sh | sh` (또는 Homebrew/apt/단일 바이너리) → 서비스 등록

**방법 B. Desktop에서 SSH로 설치 (Zed 방식)**
1. 시스템 `ssh` 바이너리 사용 (사용자의 `~/.ssh/config`, 에이전트, ProxyJump, ControlMaster를 그대로 존중). 비밀번호는 설정에 저장하지 않고 키 인증 권장, 프롬프트는 GUI askpass로 처리
2. 원격 OS/아키텍처 감지 (`uname -sm`)
3. `~/.local/share/pitwall/relay/pitwall-relay-{channel}-{version}` 존재 여부 확인 (원격의 `$XDG_DATA_HOME` 존중) — **Desktop 버전과 정확히 일치**해야 함
4. 없으면 설치:
   - 기본: 원격이 릴리스 서버에서 직접 다운로드 + 해시 검증
   - `upload_binary_over_ssh = true`: Desktop이 로컬로 받아 SSH로 업로드 (원격 인터넷 제한 환경)
   - 수동: 릴리스 바이너리를 위 경로 · 이름 규칙으로 직접 배치
5. 상주 서비스 등록(systemd user / launchd) 또는 SSH 세션 기반 데몬으로 기동
6. 오래된 버전 바이너리 정리 (최근 2개 유지)

### 연결 · 재연결 (프록시 모드)
- 연결할 때마다 `pitwall-relay proxy`를 실행 → 데몬이 없으면 띄우고, 있으면 기존 데몬에 붙음
- 데몬 재사용에 실패하면(버전 불일치, 손상) 새 데몬으로 교체하고 로그 남김
- Tailscale/LAN 직접 연결(QUIC)이 가능하면 데이터 경로는 직접 연결, 불가하면 SSH 채널 위로 그대로 운영

### 페어링 (직접 연결용)
1. `pitwall-relay pair` → 터미널에 **QR 코드 + 6자리 코드** 표시 (SSH로 설치한 경우 Desktop이 SSH 채널로 자동 페어링)
2. Desktop에서 코드 입력 → 페어링 완료

### 보안 모델
- 기기마다 Ed25519 키쌍 생성, 페어링 시 공개키 교환 (TOFU + 짧은 코드로 확인)
- 세션 암호화/인증: **Noise 프로토콜(XX 패턴)**
- Relay는 페어링된 기기 목록과 **기기별 권한 범위(role)**를 보유
  - `owner`(Desktop): 전체
  - `mobile`: 조회 + 허용된 조작만 (6절)
  - `readonly`: 조회만
- 기기 해제(revoke) 즉시 반영, 페어링 코드는 5분 만료
- Relay가 노출하는 프로젝트 경로는 화이트리스트 (`relay.toml`의 `roots`)

### 원격 호스트 등록 예 (Desktop 쪽)

```toml
# ~/.config/pitwall/settings.toml
[[remote_hosts]]
nickname = "build-server"
host = "build.tail1234.ts.net"     # Tailscale MagicDNS 또는 ~/.ssh/config 호스트명
username = "dev"                   # 기본: 로컬 사용자명
port = 22
ssh_args = ["-o", "ServerAliveInterval=30"]
upload_binary_over_ssh = false
projects = ["~/work/billing-api"]

[[remote_hosts.port_forwards]]
local_port = 8080
remote_port = 8080

[[remote_hosts.port_forwards]]
local_port = 5005                  # 원격 JVM 디버그 포트
remote_port = 5005
remote_host = "localhost"          # 다른 호스트(Docker 등)로 바꿀 수 있음
local_host = "127.0.0.1"           # "0.0.0.0"이면 모든 로컬 인터페이스에서 수신
```

### Relay 설정 예 (원격 쪽)

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
| 런타임 환경 | Relay의 `runtime` 모듈이 원격 머신의 mise/셸 환경으로 해석 (F14-7) |
| 실행 · 디버그 | Relay에서 프로세스 실행, DAP 중계, **포트 포워딩**(원격 웹서버/디버그 포트 → 로컬) |
| 터미널 | Relay의 PTY |
| DB | Relay에서 직접 연결 (내부망 DB 접근에 유리) |
| MCP | 원격 머신의 에이전트도 Relay의 로컬 MCP 엔드포인트 사용 → 승인 요청은 연결된 Desktop으로 전달 |
| 세션 유지 | Relay가 상주하므로 연결이 끊겨도 실행 중 프로세스 · 로그 유지, 재접속 시 이어보기 |
| 미저장 편집 | **Desktop 로컬에 보관** (연결 끊김에도 유실 없음), 재접속 시 복원 · 원격 파일이 그사이 바뀌었으면 diff로 충돌 해결 |
| 원격 터미널에서 `pitwall <file>` | 원격 Relay가 연결된 Desktop에 "파일 열기" 이벤트 전달 (Zed가 지원하지 못하는 부분 — 에이전트가 원격에서 작업할 때 유용) |

### 로컬 / 원격 역할 분담 (Zed와 동일 원칙)

| Desktop (로컬) | Relay (원격) |
|---|---|
| UI, 테마, 키맵, 레이아웃 | 소스 코드, 파일 감시 |
| tree-sitter 하이라이트(열린 파일) | LSP, 심볼 인덱스, 텍스트 검색 |
| 미저장 편집, 최근 프로젝트 | git, 실행 · 디버그, 터미널, DB 연결 |
| 비밀(키체인) — 필요한 것만 세션 동안 전달 | 프로젝트 설정(`.pitwall/`) |

### 설정 위치
- **로컬 설정**: UI 관련 (폰트, 테마)
- **Relay 설정**: 서버 영향 (`relay.toml` — LSP 경로, 프록시, roots)
- **프로젝트 설정**: `.pitwall/` — Desktop과 Relay 양쪽이 읽음

### 제약
- 파일 10만 개 이상인 루트(`~`, `/`)는 성능 저하 → 프로젝트 폴더 단위로 열도록 유도 (F9-10)
- 지원 원격 플랫폼: Linux x64/arm64(musl 정적), macOS. 32bit 미지원, Windows 원격은 이후

## 6. 모바일 — 보류 (향후 검토용 초안)

> **D13: MVP 및 현재 로드맵에서 제외.** 모바일로 확인할 정보가 구체화되면 이 절을 기준으로 재검토한다.

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
