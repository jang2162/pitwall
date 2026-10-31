// 앞의 여섯 시험은 DBX `crates/dbx-driver-redis/src/lib.rs` 의 시험을 옮긴 것이다(머리 주석의 출처, 수정함 — 분류 결과를
// `KvCommandClass` 로, 마지막 시험은 줄마다 argv 로 나눠 분류하게). 나머지는 이 행(M3-09)이 더했다.

use super::*;

fn class_of(text: &str) -> KvCommandClass {
    classify_argv(&parse_command_argv(text).unwrap())
}

const CONFIRM: KvCommandClass = KvCommandClass::Confirm(RiskGrade::UnboundedWrite);

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
    assert_eq!(classify_command("flushdb"), KvCommandClass::Blocked);
    assert_eq!(classify_command("JSON.DEL"), CONFIRM);
    assert_eq!(classify_command("JSON.FORGET"), CONFIRM);
    assert_eq!(classify_command("JSON.CLEAR"), CONFIRM);
    assert_eq!(classify_command("KEYS"), KvCommandClass::Blocked);
    assert_eq!(classify_command("flushall"), KvCommandClass::Blocked);
    assert_eq!(classify_command("eval"), KvCommandClass::Blocked);
    assert_eq!(classify_command("FCALL"), KvCommandClass::Blocked);
    assert_eq!(classify_command("XGROUP"), KvCommandClass::Blocked);
    assert_eq!(classify_command("VENDOR.WRITE"), KvCommandClass::Blocked);
}

/// 여러 줄 중 하나라도 모르는 명령이면 묶음 전체가 거절이다(어느 자리에 있든). DBX 원본은 줄 전체(`"DEL victim"`)를 명령
/// 이름으로 넘겨 모든 줄이 「모르는 명령」 으로 거절되던 시험이었다 — 여기서는 줄마다 argv 로 나눠 `DEL` 은 확인, `FCALL` 은
/// 거절로 분류한 뒤 합친다.
#[test]
fn classify_command_batch_blocks_on_any_unknown_member() {
    for batch in [
        ["DEL victim", "FCALL wipe 0"],
        ["FCALL wipe 0", "DEL victim"],
    ] {
        let classes: Vec<KvCommandClass> = batch.iter().map(|line| class_of(line)).collect();
        assert!(classes.contains(&CONFIRM), "{batch:?}: {classes:?}");
        let highest = classes
            .iter()
            .map(|c| c.grade())
            .try_fold(RiskGrade::Read, |acc, g| g.map(|g| acc.max(g)));
        assert_eq!(
            highest, None,
            "batch {batch:?} must be refused because it contains FCALL"
        );
    }
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
        assert_eq!(class_of(text), KvCommandClass::Blocked, "{text}");
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
        assert_eq!(class_of(text), KvCommandClass::Blocked, "{text}");
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
        assert_eq!(class_of(text), KvCommandClass::Blocked, "{text}");
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
    assert_eq!(classify_argv::<Vec<u8>>(&[]), KvCommandClass::Blocked);
    assert_eq!(
        classify_argv(&[vec![0xff_u8, 0x47]]),
        KvCommandClass::Blocked
    );
    assert_eq!(classify_argv(&["get", "k"]), KvCommandClass::Allowed);
}
