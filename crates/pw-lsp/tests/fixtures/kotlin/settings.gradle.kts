// live 시험 픽스처(../README.md). kotlin-lsp 는 Kotlin Gradle 플러그인을 적용한 빌드만 분석한다(빌드 파일이 없거나 java 플러그인만이면
// hover · 완성 · 진단이 비어 있다 — 15-m4-plan.md M4-18 실측). 그래서 이 픽스처만 플러그인 · 표준 라이브러리를 받는다(판 고정, 그 밖의
// 의존 없음 — 메인 판정 2026-11-03).
rootProject.name = "fixture"
