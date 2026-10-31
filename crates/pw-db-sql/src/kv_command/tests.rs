// 앞의 여섯 시험은 DBX `crates/dbx-driver-redis/src/lib.rs` 의 시험을 옮긴 것이다(머리 주석의 출처, 수정함 — 분류 결과를
// `KvCommandClass` 로, 마지막 시험은 줄마다 argv 로 나눠 분류하게). 나머지는 이 행(M3-09)이 더했다.

use super::*;

fn class_of(text: &str) -> KvCommandClass {
    classify_argv(&parse_command_argv(text).unwrap())
}

const CONFIRM: KvCommandClass = KvCommandClass::Confirm(RiskGrade::UnboundedWrite);
const BLOCKED: KvCommandClass = KvCommandClass::Blocked(BlockReason::NotAllowed);
const BLOCKING: KvCommandClass = KvCommandClass::Blocked(BlockReason::Blocking);
const SCAN_COUNT: KvCommandClass = KvCommandClass::Blocked(BlockReason::ScanCount);

#[test]
fn parses_command_text_with_quotes_and_escapes() {
    let argv = parse_command_argv(r#"SET "user:1" "Ada \"Lovelace\"""#).unwrap();
    assert_eq!(
        argv,
        vec![
            b"SET".to_vec(),
            b"user:1".to_vec(),
            b"Ada \"Lovelace\"".to_vec()
        ]
    );

    let argv = parse_command_argv(r#"DEL "cache\\session""#).unwrap();
    assert_eq!(argv, vec![b"DEL".to_vec(), b"cache\\session".to_vec()]);

    let argv = parse_command_argv(r#"DEL "bin\x00key\xff""#).unwrap();
    assert_eq!(argv, vec![b"DEL".to_vec(), b"bin\0key\xff".to_vec()]);
    // 0xff 는 바이트 하나로 남는다(U+00FF 의 UTF-8 두 바이트가 아니다).
    assert_eq!(
        argv[1],
        vec![b'b', b'i', b'n', 0x00, b'k', b'e', b'y', 0xff]
    );
}

#[test]
fn parses_empty_quoted_arguments() {
    let argv = parse_command_argv(r#"SET "" """#).unwrap();
    assert_eq!(argv, vec![b"SET".to_vec(), b"".to_vec(), b"".to_vec()]);
}

#[test]
fn parses_quoted_and_escaped_command_names_before_classification() {
    for command_text in [r#""JSON.SET" user:1 $ {}"#, r#"JSON\.SET user:1 $ {}"#] {
        let argv = parse_command_argv(command_text).unwrap();
        let cmd = String::from_utf8_lossy(&argv[0]).to_ascii_uppercase();
        assert_eq!(classify_command(&cmd), KvCommandClass::Write);
    }
}

#[test]
fn rejects_empty_command_text() {
    for command_text in ["   ", "\"\""] {
        assert_eq!(
            parse_command_argv(command_text).unwrap_err(),
            "Redis command is empty"
        );
    }
}

#[test]
fn classifies_safe_confirmed_and_blocked_commands() {
    assert_eq!(classify_command("GET"), KvCommandClass::Allowed);
    assert_eq!(classify_command("JSON.GET"), KvCommandClass::Allowed);
    assert_eq!(classify_command("set"), KvCommandClass::Write);
    assert_eq!(classify_command("hset"), KvCommandClass::Write);
    assert_eq!(classify_command("JSON.SET"), KvCommandClass::Write);
    assert_eq!(classify_command("GETEX"), KvCommandClass::Write);
    assert_eq!(classify_command("XREADGROUP"), KvCommandClass::Write);
    assert_eq!(classify_command("del"), CONFIRM);
    assert_eq!(classify_command("flushdb"), BLOCKED);
    assert_eq!(classify_command("JSON.DEL"), CONFIRM);
    assert_eq!(classify_command("JSON.FORGET"), CONFIRM);
    assert_eq!(classify_command("JSON.CLEAR"), CONFIRM);
    assert_eq!(classify_command("KEYS"), BLOCKED);
    assert_eq!(classify_command("flushall"), BLOCKED);
    assert_eq!(classify_command("eval"), BLOCKED);
    assert_eq!(classify_command("FCALL"), BLOCKED);
    assert_eq!(classify_command("XGROUP"), BLOCKED);
    assert_eq!(classify_command("VENDOR.WRITE"), BLOCKED);
}

/// 여러 줄 중 하나라도 모르는 명령이면 묶음 전체가 거절이다(어느 자리에 있든). DBX 원본은 줄 전체(`"DEL victim"`)를 명령
/// 이름으로 넘겨 모든 줄이 「모르는 명령」 으로 거절되던 시험이었다 — 여기서는 묶음 분류의 주인 `classify_script` 가 줄마다
/// argv 로 나눠 `DEL` 은 확인, `FCALL` 은 거절로 분류한 뒤 합친다.
#[test]
fn classify_command_batch_blocks_on_any_unknown_member() {
    for batch in ["DEL victim\nFCALL wipe 0", "FCALL wipe 0\nDEL victim"] {
        let (commands, class) = classify_script(batch).unwrap();
        assert_eq!(commands.len(), 2);
        assert!(
            commands.iter().any(|argv| classify_argv(argv) == CONFIRM),
            "{batch:?}"
        );
        assert_eq!(
            class, BLOCKED,
            "batch {batch:?} must be refused because it contains FCALL"
        );
    }
}

/// 여러 줄은 줄마다 명령이다 — 한 줄로 읽으면 `[GET, k, FLUSHALL]` 이 허용으로 보인다(리뷰 8). 빈 줄은 건너뛰고, 거절이 없으면
/// 가장 높은 등급이다.
#[test]
fn scripts_split_on_lines_and_take_the_highest_class() {
    assert_eq!(classify_script("GET k\nFLUSHALL").unwrap().1, BLOCKED);
    let (commands, class) = classify_script("GET a\n\n  \nSET b 1\nDEL c\n").unwrap();
    assert_eq!(commands.len(), 3);
    assert_eq!(class, CONFIRM);
    assert_eq!(classify_script("GET a").unwrap().1, KvCommandClass::Allowed);
    assert!(classify_script(" \n ").is_err());
    assert!(classify_script("GET \"a").is_err());
    assert_eq!(
        BLOCKING.max(BLOCKED),
        BLOCKING,
        "the first refusal's reason stays"
    );
    assert_eq!(
        KvCommandClass::Write.max(KvCommandClass::Allowed),
        KvCommandClass::Write
    );
}

/// 14-m3-plan.md M3-09 완료 기준 ② 의 표.
#[test]
fn the_plan_table_blocks_and_confirms_what_it_says() {
    for text in [
        "KEYS *",
        "FLUSHALL",
        "CONFIG SET maxmemory 1",
        "EVAL \"return 1\" 0",
        "SELECT 2",
        "NOSUCHCOMMAND a b",
    ] {
        assert_eq!(class_of(text), BLOCKED, "{text}");
    }
    for text in ["DEL a", "RENAME a b"] {
        assert_eq!(class_of(text), CONFIRM, "{text}");
    }
}

/// 연결을 점유하는 막히는 명령은 거절이다(DBX 는 확인 · 허용이었다 — 머리 주석).
#[test]
fn blocking_commands_are_refused() {
    for text in [
        "BLPOP q 0",
        "BRPOP q 0",
        "BLMOVE a b LEFT RIGHT 0",
        "BLMPOP 0 1 q LEFT",
        "BRPOPLPUSH a b 0",
        "BZPOPMIN z 0",
        "BZPOPMAX z 0",
        "BZMPOP 0 1 z MIN",
        "WAIT 1 0",
        "WAITAOF 1 0 0",
        "XREAD BLOCK 0 STREAMS s $",
        "xread count 10 block 100 streams s 0",
        "XREADGROUP GROUP g c BLOCK 0 STREAMS s >",
    ] {
        assert_eq!(class_of(text), BLOCKING, "{text}");
    }
    // `BLOCK` 이 없으면 막히지 않는다. `STREAMS` 뒤의 `BLOCK` 은 키 이름이다.
    assert_eq!(
        class_of("XREAD COUNT 10 STREAMS s 0"),
        KvCommandClass::Allowed
    );
    assert_eq!(class_of("XREAD STREAMS BLOCK 0"), KvCommandClass::Allowed);
    assert_eq!(
        class_of("XREADGROUP GROUP g c COUNT 1 STREAMS s >"),
        KvCommandClass::Write
    );
}

/// 연결의 상태를 바꾸거나 연결을 점유하는 명령은 거절이다(결정 M3-D15 — DB 번호마다 연결 하나를 함께 쓴다).
#[test]
fn connection_state_commands_are_refused() {
    for text in [
        "SELECT 0",
        "AUTH user pass",
        "HELLO 3",
        "CLIENT REPLY OFF",
        "CLIENT SETNAME x",
        "MULTI",
        "EXEC",
        "WATCH k",
        "RESET",
        "QUIT",
        "MONITOR",
        "SUBSCRIBE ch",
        "PSUBSCRIBE ch*",
        "SSUBSCRIBE ch",
        "READONLY",
        "SWAPDB 0 1",
        "FLUSHDB",
        "DEBUG SLEEP 3",
        "SHUTDOWN",
    ] {
        assert_eq!(class_of(text), BLOCKED, "{text}");
    }
}

#[test]
fn grades_share_the_sql_names_and_refusals_have_none() {
    assert_eq!(class_of("GET a").grade(), Some(RiskGrade::Read));
    assert_eq!(class_of("SET a 1").grade(), Some(RiskGrade::Write));
    assert_eq!(
        class_of("UNLINK a").grade(),
        Some(RiskGrade::UnboundedWrite)
    );
    assert_eq!(class_of("FLUSHALL").grade(), None);
    assert_eq!(class_of("KEYS *").grade(), None);
}

#[test]
fn empty_or_binary_names_are_refused() {
    assert_eq!(classify_argv::<Vec<u8>>(&[]), BLOCKED);
    assert_eq!(classify_argv(&[vec![0xff_u8, 0x47]]), BLOCKED);
    assert_eq!(classify_argv(&["get", "k"]), KvCommandClass::Allowed);
}

/// 리뷰 1: 그룹 · 소비자 · 키 이름이 `STREAMS` · `BLOCK` 같은 낱말이어도 자리로 가른다.
#[test]
fn stream_reads_find_block_by_position() {
    for text in [
        "XREADGROUP GROUP STREAMS c BLOCK 0 STREAMS s >",
        "XREADGROUP GROUP g STREAMS BLOCK 0 STREAMS s >",
        "xreadgroup group g streams block 0 streams s >",
        "XREADGROUP GROUP g c COUNT 5 NOACK BLOCK 10 STREAMS s >",
        "XREAD COUNT 1 BLOCK 0 STREAMS s $",
    ] {
        assert_eq!(class_of(text), BLOCKING, "{text}");
    }
    // 이름이 `BLOCK` 인 소비자 · 키는 막히지 않는다.
    assert_eq!(
        class_of("XREADGROUP GROUP g BLOCK COUNT 1 STREAMS s >"),
        KvCommandClass::Write
    );
    assert_eq!(
        class_of("XREADGROUP GROUP BLOCK c STREAMS BLOCK >"),
        KvCommandClass::Write
    );
    assert_eq!(class_of("XREAD STREAMS BLOCK 0"), KvCommandClass::Allowed);
    // 모르는 선택 인자 · `STREAMS` 가 없는 형식 · `GROUP` 이 없는 `XREADGROUP` 은 거절이다.
    for text in [
        "XREAD FOO 1 STREAMS s 0",
        "XREAD COUNT 1",
        "XREAD COUNT",
        "XREADGROUP g c STREAMS s >",
        "XREADGROUP GROUP g",
        "XREADGROUP GROUP g c CLAIM 10 STREAMS s >",
    ] {
        assert_eq!(class_of(text), BLOCKED, "{text}");
    }
}

/// 리뷰 3 · 메인 판정: 스캔의 `COUNT` 는 [`MAX_SCAN_COUNT`] 까지. 넘거나 수가 아니면 키 브라우저로 안내하는 거절이다.
#[test]
fn scans_over_the_count_limit_are_refused() {
    assert_eq!(class_of("SCAN 0 COUNT 10000"), KvCommandClass::Allowed);
    assert_eq!(
        class_of("SCAN 0 MATCH a* COUNT 1000 TYPE hash"),
        KvCommandClass::Allowed
    );
    assert_eq!(
        class_of("ZSCAN z 0 COUNT 5 NOSCORES"),
        KvCommandClass::Allowed
    );
    assert_eq!(class_of("HSCAN h 0 NOVALUES"), KvCommandClass::Allowed);
    for text in [
        "SCAN 0 COUNT 10001",
        "SCAN 0 COUNT 2000000",
        "HSCAN h 0 COUNT 100000000",
        "SSCAN s 0 MATCH * COUNT 99999",
        "ZSCAN z 0 COUNT many",
        "SCAN 0 COUNT",
        // 키 이름이 `COUNT` 여도 자리로 읽는다.
        "HSCAN COUNT 0 COUNT 99999",
    ] {
        assert_eq!(class_of(text), SCAN_COUNT, "{text}");
    }
    assert_eq!(class_of("SCAN 0 SORTED"), BLOCKED);
    assert_eq!(class_of("SCAN 0 MATCH"), BLOCKED);
}

/// 리뷰 4: 대상 키를 덮는 쓰기는 `RENAME` 처럼 확인이다.
#[test]
fn writes_that_overwrite_a_target_need_confirmation() {
    for text in [
        "BITOP AND dst a b",
        "GEORADIUS g 0 0 1 km STORE dst",
        "GEORADIUSBYMEMBER g m 1 km STOREDIST dst",
        "COPY a b DB 5 REPLACE",
        "RESTORE k 0 payload REPLACE",
        "SORT k STORE dst",
    ] {
        assert_eq!(class_of(text), CONFIRM, "{text}");
    }
    for text in [
        "GEORADIUS g 0 0 1 km COUNT 5 ANY",
        "COPY a b",
        "RESTORE k 0 payload",
    ] {
        assert_eq!(class_of(text), KvCommandClass::Write, "{text}");
    }
}

/// 14 M3-22 판정(1): 서버를 오래 붙잡는 명령은 그 등급 그대로 확인이다 — 읽기는 `Confirm(Read)`. 응답 상한은 응답만 끊는다.
#[test]
fn commands_that_hold_the_server_need_confirmation() {
    let heavy_read = KvCommandClass::Confirm(RiskGrade::Read);
    for text in [
        "LCS a b",
        "LCS a b IDX MINMATCHLEN 4",
        "GEOSEARCH g FROMLONLAT 0 0 BYRADIUS 1 km",
        // `ANY` 없는 `COUNT` 는 서버 일을 줄이지 않는다(재리뷰 C) — 큰 `COUNT` 로 비켜 가지 않는다.
        "GEOSEARCH g FROMLONLAT 0 0 BYRADIUS 1 km COUNT 10",
        "GEOSEARCH g FROMLONLAT 0 0 BYRADIUS 1 km COUNT 9223372036854775807",
        "GEORADIUS_RO g 0 0 1 km COUNT 5",
        "GEOSEARCH g FROMMEMBER ANY BYRADIUS 1 km COUNT 3",
        "GEOSEARCH g FROMLONLAT 0 0 BYRADIUS 1 km COUNT 0 ANY",
        // 재리뷰 2 C′: `ANY` 는 n 개를 찾으면 멈출 뿐이다 — 큰 n 은 끝까지 찾는다. 못 읽는 n 도 면제하지 않는다.
        "GEOSEARCH g FROMLONLAT 0 0 BYRADIUS 1 km COUNT 10001 ANY",
        "GEOSEARCH g FROMLONLAT 0 0 BYRADIUS 1 km COUNT 9223372036854775807 ANY",
        "GEOSEARCH g FROMLONLAT 0 0 BYRADIUS 1 km COUNT -1 ANY",
        "GEOSEARCH g FROMLONLAT 0 0 BYRADIUS 1 km COUNT ten ANY",
        "GEORADIUS_RO g 0 0 1 km COUNT 10001 ANY",
        "GEOSEARCH g FROMMEMBER COUNT BYBOX 1 1 km ASC",
        "GEOSEARCH COUNT FROMMEMBER m BYRADIUS 1 km",
        "GEOSEARCH g FROMMEMBER m BYRADIUS 1 km COUNT 0",
        "GEORADIUS_RO g 0 0 1 km WITHDIST",
        "GEORADIUSBYMEMBER_RO g COUNT 1 km",
    ] {
        assert_eq!(class_of(text), heavy_read, "{text}");
        assert_eq!(heavy_read.grade(), Some(RiskGrade::Read));
    }
    // 쓰기는 쓰기 등급의 확인, 덮는 쓰기는 그대로 범위 없는 쓰기.
    assert_eq!(
        class_of("GEORADIUS g 0 0 1 km"),
        KvCommandClass::Confirm(RiskGrade::Write)
    );
    assert_eq!(
        class_of("GEORADIUSBYMEMBER g m 1 km WITHCOORD"),
        KvCommandClass::Confirm(RiskGrade::Write)
    );
    assert_eq!(class_of("SORT k GET obj:*"), CONFIRM);
    assert_eq!(class_of("GEORADIUS g 0 0 1 km STORE dst"), CONFIRM);
    // 묶였거나 패턴이 없으면 그대로다.
    for text in [
        // `SORT_RO` 는 분류가 아니라 드라이버의 길이 사전 확인이 묶는다(재리뷰 D — 비용은 목록 길이).
        "SORT_RO k",
        "SORT_RO k LIMIT 0 10 ALPHA",
        "SORT_RO k BY w:* GET # GET obj:*",
        "GEOSEARCH g FROMLONLAT 0 0 BYRADIUS 1 km COUNT 10 ANY",
        "GEOSEARCH g FROMMEMBER m BYBOX 1 1 km COUNT 3 ANY ASC",
        "GEORADIUS_RO g 0 0 1 km COUNT 10 ANY",
        "GEORADIUSBYMEMBER_RO g m 1 km COUNT 10 ANY WITHDIST",
        "GEOSEARCH g FROMLONLAT 0 0 BYRADIUS 1 km COUNT 10000 ANY",
    ] {
        assert_eq!(class_of(text), KvCommandClass::Allowed, "{text}");
    }
    // 묶음에서 확인은 사라지지 않는다 — 더 높은 등급의 확인이 된다.
    assert_eq!(
        classify_script("GET a\nLCS a b").unwrap().1,
        heavy_read,
        "a heavy read keeps its confirmation"
    );
    assert_eq!(
        classify_script("LCS a b\nSET c 1").unwrap().1,
        KvCommandClass::Confirm(RiskGrade::Write)
    );
    assert_eq!(heavy_read.max(KvCommandClass::Allowed), heavy_read);
}
