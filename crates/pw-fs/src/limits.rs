//! 대용량 · 특수 파일 기준값(`docs/design/12-m1-plan.md` 결정 M1-D9 · M1-D10). 판정([`crate::judge`]) · 텍스트 검색(M1-07) ·
//! 구문 강조 서비스(M1-17, `pw-syntax`) · hex 페이지 읽기(M1-23)가 이 상수를 함께 쓴다. 기준을 바꾸려면 여기만 고친다.
//!
//! 에디터(TS)도 같은 값을 `apps/desktop/src/editor/limits.ts` 에 둔다 — 그 파일의 시험이 이 파일을 읽어 값이 같은지 본다.

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;

/// 바이너리 판정에 보는 파일 앞부분.
pub const SNIFF_BYTES: usize = 8 * 1024;
/// 이 크기까지는 편집 + 강조.
pub const EDITABLE_MAX: u64 = 5 * MIB;
/// 이 크기까지는 읽기 전용(강조 · 줄바꿈 끔). 넘으면 열지 않는다. 범위 읽기 한 번 · 텍스트 검색 한 파일의 상한이기도 하다.
pub const READ_ONLY_MAX: u64 = 50 * MIB;
/// 한 줄이 이 글자 수를 넘으면 강조 · 줄바꿈을 끈다.
pub const LONG_LINE_CHARS: usize = 10_000;
/// 열지 않는 파일의 [앞부분 보기]가 읽는 크기.
pub const HEAD_PREVIEW_BYTES: u64 = MIB;
/// hex 뷰어가 한 번에 읽는 페이지(결정 M1-D10). `fs.readBytes` 한 번의 상한이다.
pub const HEX_PAGE_BYTES: u64 = 64 * KIB;
/// 조각 읽기 · 쓰기(M7-13b) 한 조각 내용의 와이어 바이트 상한(직렬화한 JSON 문자열). 링크 틀(64 MiB)보다 넉넉히 작게 — 큰 틀
/// 하나가 그 머신의 다른 채널(터미널)을 오래 막지 않게.
pub const CHUNK_WIRE_MAX: u64 = 16 * MIB;
/// 조각 쓰기 하나가 모을 수 있는 바이트(원격 문서의 상한 — 열 수 있는 가장 큰 파일과 같다).
pub const UPLOAD_MAX: u64 = READ_ONLY_MAX;
/// Host 전체에서 동시에 열린 조각 쓰기 모음의 수.
pub const UPLOADS_MAX: usize = 4;
/// 한 연결이 동시에 열 수 있는 조각 쓰기 모음의 수.
pub const UPLOADS_PER_SESSION: usize = 2;
