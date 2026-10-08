# 09. 참고 앱

조사일: 2026-10. 기능 · 구조를 참고할 앱과 각각에서 가져올 점.

## 1. 겹치는 영역 — 먼저 확인할 것

| 앱 | 내용 | Pitwall에 주는 시사점 |
|---|---|---|
| **Orca** (stablyai/orca) | 에이전트 병렬 실행 · worktree 격리, 내장 에디터(VS Code 기반), 소스 컨트롤(diff 리뷰 · 커밋 · AI diff 주석), **SSH 원격 worktree**(재연결 · 포트 포워딩), **모바일 컴패니언(iOS/Android)** | 원격 · 모바일 · diff 리뷰가 Orca와 겹침. Pitwall의 차별점은 **DB · git 그래프/머지 · 실행/디버그 · LSP 탐색**에 둬야 함. 원격(M7) · 모바일(M8) 범위는 Orca와 중복되지 않는 쪽(DB, 실행 상태, Pitwall MCP 승인)으로 좁히는 것을 검토 |
| **JetBrains Air** | Fleet 플랫폼 기반 에이전트 개발 환경. 작업별 worktree/Docker 격리, 내장 Git 클라이언트 · 터미널. macOS 먼저 | 방향이 Orca와 같음(에이전트 중심). DB · 디버깅 등 "IDE 잔여 기능"을 가볍게 묶는 제품은 아직 없음 → Pitwall 포지션 유효 |
| **JetBrains Fleet** (2025-12 종료) | 경량 IDE를 표방했으나 프리뷰를 벗어나지 못하고 종료 | 반면교사: "가벼운 범용 IDE"로 IntelliJ와 경쟁하면 실패. **IDE가 아닌 '허브'** 포지션 유지, 비목표 엄수 |

## 2. 영역별 참고 앱

### 아키텍처 · 앱 셸
| 앱 | 참고 포인트 |
|---|---|
| **GitButler** | Tauri + Rust + Svelte. GUI와 CLI(`but`)가 같은 Rust API 계층 공유 → Pitwall의 "헤드리스 Host + 여러 클라이언트" 구조와 동일. 대규모 cargo workspace 구성, 커밋 그래프 모듈 참고 |
| **Zed** | 원격 개발: UI(로컬) / 헤드리스 서버(원격) 분리, SSH로 서버 자동 설치(버전 정확히 일치), 데몬 유지로 재연결, 미저장 편집은 로컬 보관. 초기 자체 중계 서버 방식을 버리고 SSH 직결로 전환 → Hub 없이 가는 D11 결정과 같은 방향. 확장 모델(선언형 + WASM)도 참고 |
| VS Code Remote | 원격 서버 설치 · 확장의 UI/워크스페이스 측 분리 |

### Git
| 앱 | 참고 포인트 |
|---|---|
| Fork / Sublime Merge | 그래프 가독성, 빠른 필터, 라인 단위 스테이징, 머지 충돌 UI |
| GitButler | 브랜치/커밋 조작 UX, 드래그로 커밋 이동 |
| lazygit / gitui | 키보드 중심 조작, gitui는 Rust(gitoxide 일부) 구현 참고 |

### DB
| 앱 | 라이선스 | 참고 포인트 |
|---|---|---|
| **DBX** (t8y2/dbx) | **Apache-2.0** (MIT 프로젝트에서 차용 가능, 고지 필요) | Tauri 2 + Vue 3 + CodeMirror 6, `sqlx`/`tiberius`/`redis-rs`/`mongodb`. 스타 2.5만 · 커밋 7.9천으로 성숙. 크레이트 분할(types/sql/drivers/driver-*), SQL 위험도 분류, JDBC agent, SQLite 격리 워커, MCP · CLI 동반 → **02 §2.7에 반영** |
| TablePlus | 상용 | 경량 네이티브 DB 클라이언트 UX 기준점, 연결 색상 태그(프로덕션 경고) |
| Beekeeper Studio | GPLv3(Community) | SQL 콘솔/그리드 UX. **GPL이라 코드 차용 불가**(MIT 프로젝트) — UX만 참고 |
| DbGate | 오픈소스(라이선스 확인 필요) | SQL + NoSQL 통합, 데스크톱/웹 동시 제공 |
| DataGrip | 상용 | 스키마 기반 자동완성, 결과 편집, 실행 계획 — 기능 기준점 |
| Redis Insight / Another Redis Desktop Manager | — | Redis 키 브라우저 · 타입별 뷰어 UX |

### 모바일 · 에이전트 승인
| 앱 | 참고 포인트 |
|---|---|
| **CC Pocket** (MIT) | PC에 브리지 서버 → 폰이 **Tailscale/Wi-Fi로 WebSocket 직결**, 주 용도는 툴 호출 승인 — Pitwall D11/D12와 같은 구조. 연결 · 승인 UX 참고 |
| Claude Code Remote Control | 세션은 로컬 유지, 폰은 제어 화면. QR로 연결 |
| Orca 모바일 | 에이전트 상태 모니터링 · 완료 알림 |

### MCP
| 앱 | 참고 포인트 |
|---|---|
| **JetBrains IDE 내장 MCP 서버** | 실행 구성 실행 · 터미널 명령 등 툴 제공, **노출 툴 개별 on/off**, 실행 시 확인 프롬프트(+"brave mode"로 생략), 클라이언트 설정 자동 등록(전역/프로젝트) → 05-mcp 권한 모델 · F10-5와 동일 방향, 툴 on/off 설정 추가 검토 |

## 3. 설계 반영 결과

| 제안 | 결과 |
|---|---|
| 포지션 재확인 (Orca와 겹치는 원격 · 모바일 · diff 리뷰) | 모바일 **보류**(D13). Pitwall 고유 가치(DB · git · 실행/디버그 · LSP)에 집중 |
| MCP 툴 개별 on/off | **반영** — 05 §3-1, F10-6 (D14) |
| DBX 코드 검토 | **확인 완료: Apache-2.0** — 02 §2.7 크레이트 구조, F6-9 위험도 분류, F6-3b JDBC 방식 (D15) |
| Zed 원격 방식 | **반영** — 08 §4~5, F9 (D16) |
