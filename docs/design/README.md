# Pitwall 설계 문서

> IntelliJ에서 "에이전트가 대신하지 못하는 부분"만 남긴 가벼운 프로젝트 통합 관리 앱.
> 코드 작성은 Orca/에이전트가, **보고 · 조회 · 실행 · 디버깅 · 깃 관리**는 Pitwall이 맡는다.

| 문서 | 내용 |
|---|---|
| [01-product.md](01-product.md) | 목표, 비목표, 사용 시나리오, 핵심 원칙 |
| [02-tech-stack.md](02-tech-stack.md) | 기술 스택 선정과 근거, 대안 비교 |
| [03-architecture.md](03-architecture.md) | 프로세스 구조, 모듈 구성, 데이터 저장, 원격 구조 |
| [04-features.md](04-features.md) | 기능 정의 (기능별 요구사항 · 범위 · 우선순위) |
| [05-mcp.md](05-mcp.md) | 에이전트용 MCP 서버 설계 (툴 목록, 권한 모델) |
| [06-roadmap.md](06-roadmap.md) | 마일스톤, MVP 범위, 리스크, **결정 기록** |
| [07-extensibility.md](07-extensibility.md) | 테마 · UI 언어 · 코드 언어 · 플러그인 확장 구조 |
| [08-remote-mobile.md](08-remote-mobile.md) | Relay 기반 원격 연결 (Zed 방식 설치 · 재연결), 모바일(보류) |
| [09-references.md](09-references.md) | 참고 앱 조사와 시사점 |
| [10-ui.md](10-ui.md) | 화면 구성 · UI 스택 · 키보드/마우스 조작 |
| [11-m0-plan.md](11-m0-plan.md) | M0(기반) 작업 분할 |

## 한 줄 요약

- **Tauri 2 + Rust 코어 + React/TypeScript UI**
- 코어는 UI와 분리된 **헤드리스 "Host" 서비스**로 설계 → 같은 API를 UI · MCP · 원격(SSH)이 공유
- Git = 읽기 `gitoxide`, 쓰기 `git CLI` / DB = `sqlx`+드라이버 / 디버깅 = **DAP** / 코드 인텔리전스 = **LSP + tree-sitter**
- 에디터는 "뷰어 우선, 가벼운 편집" (CodeMirror 6)
- **macOS 우선**, Linux/Windows 지원
- 확장: 선언형 확장(테마 · 언어 · 키맵) + WASM 플러그인, 내장 기능도 확장 API 위에서 구현
- 원격: 원격 머신에 **Pitwall Relay** 설치 → 직접 연결(LAN · Tailscale), SSH 대체. 중계 서버 없음
- 모바일: **보류** (MVP · 로드맵 제외)
- MCP: 툴별 on/off · 프리셋 · 등급별 자동 승인
- DB 계층은 DBX(Apache-2.0) 구조 참고
- MVP: 탐색 · Git · DB(PG/MySQL/SQLite/Redis). 실행 · 디버그는 후순위
- 라이선스: MIT
