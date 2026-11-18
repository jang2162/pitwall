//! 트리 가르기 · 포트 묶기의 단위 시험(프로세스 없이). 실제 PTY 로 띄우는 시험은 `runs/ports_tests.rs`.

use std::net::SocketAddr;

use super::*;

fn proc(pid: u32, parent: u32, session: u32, start: u64) -> Proc {
    Proc {
        pid,
        parent: Some(parent),
        session: Some(session),
        start: Some(start),
    }
}

fn pids(tree: &BTreeMap<u32, Option<u64>>) -> Vec<u32> {
    tree.keys().copied().collect()
}

/// 리더(10)의 세션 · 자손(세션을 떠난 30 과 그 자식 31)은 들고, 다른 실행(20)의 세션과 그 자식은 들지 않는다.
#[test]
fn a_tree_is_the_session_and_the_descendants_only() {
    let snapshot = Snapshot::of(vec![
        proc(1, 0, 1, 1),
        proc(10, 1, 10, 100),
        proc(11, 10, 10, 110),
        proc(12, 11, 10, 120),
        // 세션을 떠난 자식과 그 자식.
        proc(30, 12, 30, 130),
        proc(31, 30, 30, 131),
        // 다른 실행.
        proc(20, 1, 20, 200),
        proc(21, 20, 20, 210),
    ]);
    let mut known = Known::default();
    assert_eq!(pids(&snapshot.tree(10, &mut known)), [10, 11, 12, 30, 31]);
    assert_eq!(pids(&snapshot.tree(20, &mut Known::default())), [20, 21]);
}

/// 앞선 폴링에서 찾은 것은 부모가 끝나 init 으로 넘어가도 트리에 남는다 — 같은 시작 시각일 때만(번호가 남에게 다시 나가면 빠진다).
#[test]
fn a_known_descendant_stays_until_its_pid_is_reused() {
    let mut known = Known::default();
    let first = Snapshot::of(vec![
        proc(10, 1, 10, 100),
        proc(11, 10, 10, 110),
        proc(30, 11, 30, 130),
    ]);
    assert_eq!(pids(&first.tree(10, &mut known)), [10, 11, 30]);

    // 11 이 끝나 30 이 init 으로 넘어갔다.
    let orphaned = Snapshot::of(vec![proc(10, 1, 10, 100), proc(30, 1, 30, 130)]);
    assert_eq!(pids(&orphaned.tree(10, &mut known)), [10, 30]);

    // 30 이 끝나고 그 번호가 다른 사용자의 프로세스로 다시 나갔다.
    let reused = Snapshot::of(vec![proc(10, 1, 10, 100), proc(30, 1, 30, 999)]);
    assert_eq!(pids(&reused.tree(10, &mut known)), [10]);
    // 그 뒤로도 닿지 않는다(기억에서 빠졌다).
    assert_eq!(pids(&reused.tree(10, &mut known)), [10]);
}

/// 리더가 끝나 거둬진 뒤 그 번호를 받은 남의 프로세스(와 그 자식)는 실행의 것이 아니다.
#[test]
fn a_reused_leader_pid_is_not_the_run() {
    let mut known = Known::default();
    let first = Snapshot::of(vec![proc(10, 1, 10, 100)]);
    assert_eq!(pids(&first.tree(10, &mut known)), [10]);
    // 새 세션의 리더가 된 남의 프로세스 — 그 세션의 것도 들지 않는다.
    let reused = Snapshot::of(vec![proc(10, 1, 10, 500), proc(51, 10, 10, 510)]);
    assert!(reused.tree(10, &mut known).is_empty());
}

/// 부모보다 먼저 태어난 「자식」 은 부모 pid 칸이 다시 쓰인 번호를 가리키는 것이다(Windows 는 부모가 끝나도 칸이 남는다).
#[test]
fn a_child_older_than_its_parent_is_not_a_descendant() {
    let snapshot = Snapshot::of(vec![
        Proc {
            pid: 10,
            parent: Some(4),
            session: None,
            start: Some(100),
        },
        Proc {
            pid: 40,
            parent: Some(10),
            session: None,
            start: Some(50),
        },
        Proc {
            pid: 41,
            parent: Some(10),
            session: None,
            start: Some(150),
        },
    ]);
    assert_eq!(pids(&snapshot.tree(10, &mut Known::default())), [10, 41]);
}

#[test]
fn listeners_group_by_port_and_mark_every_address() {
    let addrs: Vec<SocketAddr> = [
        "127.0.0.1:5173",
        "[::1]:5173",
        "0.0.0.0:8080",
        "[::]:8080",
        "192.168.0.2:9000",
        "127.0.0.1:5173",
    ]
    .iter()
    .map(|a| a.parse().unwrap())
    .collect();
    assert_eq!(
        group(addrs),
        [
            RunPort {
                port: 5173,
                all_addresses: false,
                addresses: vec!["127.0.0.1".into(), "::1".into()],
            },
            RunPort {
                port: 8080,
                all_addresses: true,
                addresses: vec!["0.0.0.0".into(), "::".into()],
            },
            RunPort {
                port: 9000,
                all_addresses: false,
                addresses: vec!["192.168.0.2".into()],
            },
        ]
    );
}

/// `/proc/net/tcp{,6}` 의 꼴(리틀 엔디언 호스트에서 커널이 찍은 값). 듣는 줄(`0A`)만, 접속된 줄(`01`)은 뺀다.
#[cfg(all(target_os = "linux", target_endian = "little"))]
#[test]
fn proc_net_tcp_lines_are_parsed() {
    let v4 = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4242 1 0000000000000000 100 0 0 10 0\n   1: 0100007F:C350 0100007F:1F90 01 00000000:00000000 00:00000000 00000000  1000        0 4343 1 0000000000000000 20 4 30 10 -1\n   2: 00000000:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 77 1 0000000000000000 100 0 0 10 0\n";
    assert_eq!(
        super::linux::parse_table(v4, false),
        [
            (4242, "127.0.0.1:8080".parse().unwrap()),
            (77, "0.0.0.0:80".parse().unwrap())
        ]
    );
    let v6 = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 00000000000000000000000001000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 5151 1 0000000000000000 100 0 0 10 0\n   1: 00000000000000000000000000000000:0016 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 52 1 0000000000000000 100 0 0 10 0\n";
    assert_eq!(
        super::linux::parse_table(v6, true),
        [
            (5151, "[::1]:8080".parse().unwrap()),
            (52, "[::]:22".parse().unwrap())
        ]
    );
}

/// 부모의 시작 시각을 알고 자식의 것을 모르면(Windows 에서 열지 못한 프로세스 — 다른 사용자 · 보호 프로세스) 자손으로 보지 않는다.
/// 묵은 부모 pid 칸이 실행의 프로세스가 다시 받은 번호를 가리켜도 그 포트가 실행에 붙지 않게.
#[test]
fn a_child_whose_start_is_unknown_is_not_a_descendant() {
    let snapshot = Snapshot::of(vec![
        Proc {
            pid: 10,
            parent: Some(4),
            session: None,
            start: Some(100),
        },
        Proc {
            pid: 40,
            parent: Some(10),
            session: None,
            start: None,
        },
    ]);
    assert_eq!(pids(&snapshot.tree(10, &mut Known::default())), [10]);
}

/// 부모가 끝나 init 으로 넘어갔지만 세션에 남은 것(`nohup … &` 를 띄운 하위 셸이 끝난 뒤)도 실행의 것이다.
#[test]
fn a_session_member_whose_parent_ended_is_in_the_tree() {
    let snapshot = Snapshot::of(vec![proc(10, 1, 10, 100), proc(12, 1, 10, 120)]);
    assert_eq!(pids(&snapshot.tree(10, &mut Known::default())), [10, 12]);
}

/// 중지에 넘기는 것은 시작 시각을 아는 것만이다(모르는 것은 남의 프로세스인지 가를 수 없다).
#[test]
fn only_known_starts_are_handed_to_the_stop() {
    let mut known = Known::default();
    Snapshot::of(vec![
        proc(10, 1, 10, 100),
        Proc {
            pid: 11,
            parent: Some(10),
            session: Some(10),
            start: None,
        },
    ])
    .tree(10, &mut known);
    assert_eq!(known.verified(), [(10, 100)]);
}
