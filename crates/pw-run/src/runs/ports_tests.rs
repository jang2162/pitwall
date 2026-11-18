//! 포트 표시 시험(17-m6-plan M6-08 완료 기준 ① ~ ③) — 실제 PTY 로 띄운 실행의 프로세스 트리가 듣는 소켓. 듣는 프로그램은 perl 이다
//! (macOS · Linux 에 모두 있다). 폴링 간격은 100 ms 로 줄인다.

use super::*;

const POLL: Duration = Duration::from_millis(100);

fn fast() -> Fixture {
    Fixture::build(
        None,
        RunLimits {
            port_interval: POLL,
            ..RunLimits::default()
        },
    )
}

/// perl 스크립트를 파일로 쓰고 그 경로.
fn script(f: &Fixture, name: &str, body: &str) -> PathBuf {
    let path = f.tmp.path().join(name);
    fs::write(&path, body).unwrap();
    path
}

/// 출력의 `<key>=<숫자>` 하나.
fn number(text: &str, key: &str) -> u32 {
    let marker = format!("{key}=");
    text.split(&marker)
        .nth(1)
        .unwrap_or_else(|| panic!("no {marker} in {text:?}"))
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap()
}

/// 실행의 포트가 `wanted` 를 만족할 때까지(10초) 기다려 그 목록을 준다.
async fn ports_until(
    f: &Fixture,
    run_id: &str,
    wanted: impl Fn(&[RunPort]) -> bool,
) -> Vec<RunPort> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let ports = f
            .runs
            .list(&f.project)
            .into_iter()
            .find(|r| r.run_id == run_id)
            .map(|r| r.ports)
            .unwrap_or_default();
        if wanted(&ports) {
            return ports;
        }
        assert!(
            Instant::now() < deadline,
            "ports never matched, last {ports:?}"
        );
        tokio::time::sleep(POLL / 2).await;
    }
}

/// 몇 번의 폴링 동안 포트가 `wanted` 를 계속 만족하는가.
async fn ports_stay(f: &Fixture, run_id: &str, wanted: impl Fn(&[RunPort]) -> bool) {
    for _ in 0..8 {
        tokio::time::sleep(POLL).await;
        let ports = f
            .runs
            .list(&f.project)
            .into_iter()
            .find(|r| r.run_id == run_id)
            .map(|r| r.ports)
            .unwrap();
        assert!(wanted(&ports), "ports changed to {ports:?}");
    }
}

async fn stop(c: &mut Client, a: &mut Attached, run_id: &str) {
    c.ok::<RunInfo>("run.stop", json!({"runId": run_id})).await;
    tokio::time::timeout(Duration::from_secs(15), c.exit(a))
        .await
        .expect("the run did not end");
}

/// 듣고 `PORT=<포트>` 를 찍은 뒤 잔다.
const LISTEN: &str = r#"use IO::Socket::INET; $| = 1;
my $s = IO::Socket::INET->new(Listen => 5, LocalAddr => "$ARGV[0]:0", Proto => "tcp", ReuseAddr => 1) or die "listen: $!";
print "PORT=", $s->sockport, "\n";
sleep 300;
"#;

/// 완료 기준 ①: 손자 프로세스(실행의 셸 → `sh` → perl)가 연 리스닝 소켓이 `run.list` 와 이벤트 `run.ports` 에 보인다 — 리더(자식
/// pid)만 보면 실패한다. 루프백에만 듣는 포트는 「모든 주소」 가 아니다. 끝나면 포트가 빈다.
#[tokio::test(flavor = "multi_thread")]
async fn a_grandchild_listening_socket_is_shown() {
    let f = fast();
    let listen = script(&f, "listen.pl", LISTEN);
    f.shell_config(
        "dev",
        &format!("sh -c 'perl {} 127.0.0.1; :' & wait", listen.display()),
    );
    let mut events = f.bus.subscribe(
        Filter::all()
            .scope(Scope::Project(f.project.clone()))
            .topics([TOPIC_PORTS]),
    );
    let mut c = f.client();
    let run = info(
        c.call(
            "run.start",
            json!({"projectId": f.project, "configId": "dev"}),
        )
        .await,
    );
    let mut a = c.attach(&run.run_id, None).await;
    c.read_until(&mut a, "PORT=").await;
    c.read_until(&mut a, "\n").await;
    let port = u16::try_from(number(&a.text, "PORT")).unwrap();

    let ports = ports_until(&f, &run.run_id, |p| !p.is_empty()).await;
    assert_eq!(
        ports,
        [RunPort {
            port,
            all_addresses: false,
            addresses: vec!["127.0.0.1".into()],
        }]
    );
    let listed: methods::RunList = c.ok("run.list", json!({"projectId": f.project})).await;
    assert_eq!(listed.runs[0].ports, ports);
    let event = tokio::time::timeout(Duration::from_secs(10), events.recv())
        .await
        .expect("no run.ports event")
        .unwrap();
    let Received::Event(event) = event else {
        panic!("{event:?}");
    };
    let announced: RunInfo = serde_json::from_value(event.payload.clone()).unwrap();
    assert_eq!(announced.run_id, run.run_id);
    assert_eq!(announced.ports, ports);

    stop(&mut c, &mut a, &run.run_id).await;
    assert!(f.runs.list(&f.project)[0].ports.is_empty());
}

/// 완료 기준 ②: 접속(established) 소켓은 빠진다 — 프로그램이 제 포트에 붙어 양쪽(받은 소켓 · 건 소켓)이 established 여도 듣는 포트
/// 하나만 보인다. `0.0.0.0` 에 듣는 포트는 「모든 주소」 다.
#[tokio::test(flavor = "multi_thread")]
async fn established_sockets_are_left_out() {
    let f = fast();
    let connect = script(
        &f,
        "connect.pl",
        r#"use IO::Socket::INET; $| = 1;
my $s = IO::Socket::INET->new(Listen => 5, LocalAddr => "0.0.0.0:0", Proto => "tcp", ReuseAddr => 1) or die "listen: $!";
my $c = IO::Socket::INET->new(PeerAddr => "127.0.0.1:" . $s->sockport, Proto => "tcp") or die "connect: $!";
my $in = $s->accept or die "accept: $!";
print "PORT=", $s->sockport, " CLIENT=", $c->sockport, "\n";
sleep 300;
"#,
    );
    f.shell_config("dev", &format!("perl {}", connect.display()));
    let mut c = f.client();
    let run = info(
        c.call(
            "run.start",
            json!({"projectId": f.project, "configId": "dev"}),
        )
        .await,
    );
    let mut a = c.attach(&run.run_id, None).await;
    c.read_until(&mut a, "CLIENT=").await;
    c.read_until(&mut a, "\n").await;
    let port = u16::try_from(number(&a.text, "PORT")).unwrap();
    let client = u16::try_from(number(&a.text, "CLIENT")).unwrap();

    let ports = ports_until(&f, &run.run_id, |p| !p.is_empty()).await;
    ports_stay(&f, &run.run_id, |p| p.len() == 1).await;
    assert_eq!(
        ports,
        [RunPort {
            port,
            all_addresses: true,
            addresses: vec!["0.0.0.0".into()],
        }],
        "the connected port {client} must not show"
    );
    stop(&mut c, &mut a, &run.run_id).await;
}

/// 완료 기준 ③: 폴링은 끝나지 않은 실행이 있는 동안만 돈다 — 실행이 끝나면 폴링 스레드가 0, 다시 실행하면 다시 하나.
#[tokio::test(flavor = "multi_thread")]
async fn polling_stops_when_the_runs_end() {
    let f = fast();
    f.shell_config("dev", "sleep 300");
    let mut c = f.client();
    assert_eq!(f.runs.port_pollers(), 0);
    for _ in 0..2 {
        let first = info(
            c.call(
                "run.start",
                json!({"projectId": f.project, "configId": "dev"}),
            )
            .await,
        );
        let second = info(
            c.call(
                "run.start",
                json!({"projectId": f.project, "configId": "dev"}),
            )
            .await,
        );
        assert_eq!(f.runs.port_pollers(), 1, "one poller for every run");
        let mut a = c.attach(&first.run_id, None).await;
        stop(&mut c, &mut a, &first.run_id).await;
        tokio::time::sleep(POLL * 4).await;
        assert_eq!(f.runs.port_pollers(), 1, "a run is still going");
        let mut b = c.attach(&second.run_id, None).await;
        stop(&mut c, &mut b, &second.run_id).await;
        let deadline = Instant::now() + Duration::from_secs(5);
        while f.runs.port_pollers() != 0 {
            assert!(
                Instant::now() < deadline,
                "the poller kept running after every run ended"
            );
            tokio::time::sleep(POLL / 2).await;
        }
    }
}

/// 실행이 띄운 것이 `setsid` 로 세션을 떠나고 그 부모가 끝나 init 으로 넘어가도(실행 관리자가 앞서 본 후손) 그 포트를 계속 보인다.
#[tokio::test(flavor = "multi_thread")]
async fn a_descendant_that_left_the_session_keeps_its_ports() {
    let f = fast();
    let escape = script(
        &f,
        "escape.pl",
        r#"use POSIX; use IO::Socket::INET; $| = 1;
my $pid = fork() // die "fork: $!";
if ($pid == 0) {
    POSIX::setsid() or die "setsid: $!";
    my $s = IO::Socket::INET->new(Listen => 5, LocalAddr => "127.0.0.1:0", Proto => "tcp", ReuseAddr => 1) or die "listen: $!";
    print "ESC=$$ PORT=", $s->sockport, "\n";
    sleep 300;
    exit 0;
}
sleep 1;
print "PARENT_DONE\n";
exit 0;
"#,
    );
    // 부모 perl 이 끝난 뒤에도 실행이 돌도록 셸은 잔다.
    f.shell_config("dev", &format!("perl {}; sleep 300", escape.display()));
    let mut c = f.client();
    let run = info(
        c.call(
            "run.start",
            json!({"projectId": f.project, "configId": "dev"}),
        )
        .await,
    );
    let mut a = c.attach(&run.run_id, None).await;
    c.read_until(&mut a, "PORT=").await;
    c.read_until(&mut a, "\n").await;
    let escaped = number(&a.text, "ESC");
    let port = u16::try_from(number(&a.text, "PORT")).unwrap();
    let shown = |p: &[RunPort]| p.iter().any(|p| p.port == port);
    let result = async {
        ports_until(&f, &run.run_id, shown).await;
        c.read_until(&mut a, "PARENT_DONE").await;
        assert!(alive(escaped));
        ports_stay(&f, &run.run_id, shown).await;
    };
    let outcome = tokio::time::timeout(Duration::from_secs(30), result).await;
    // 세션을 떠나 init 으로 넘어간 것은 중지가 닿지 않는다(03 「프로세스 · 창 모델」 — 끝내기를 시작하기 전에 부모를 떠난 것).
    kill(escaped);
    outcome.expect("timed out");
    stop(&mut c, &mut a, &run.run_id).await;
}

/// 실행 20개 · 후손 300개에서 한 번 읽는 데 드는 시간(완료 기준 밖 — 보고서의 측정). `--ignored` 로 돌린다.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "measurement"]
async fn measure_twenty_runs_with_hundreds_of_descendants() {
    let f = Fixture::build(
        None,
        RunLimits {
            // 측정은 손으로 부른다.
            port_interval: Duration::from_secs(3600),
            ..RunLimits::default()
        },
    );
    let listen = script(&f, "listen.pl", LISTEN);
    f.shell_config(
        "dev",
        &format!(
            "perl {} 127.0.0.1 & for i in $(seq 14); do sleep 300 & done; wait",
            listen.display()
        ),
    );
    let mut c = f.client();
    let mut runs = Vec::new();
    for _ in 0..20 {
        runs.push(info(
            c.call(
                "run.start",
                json!({"projectId": f.project, "configId": "dev"}),
            )
            .await,
        ));
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    let mut took = Vec::new();
    for _ in 0..10 {
        let began = Instant::now();
        ports::poll(&f.runs.inner);
        took.push(began.elapsed());
    }
    let snapshot = ports::Snapshot::take();
    let with_ports = f
        .runs
        .list(&f.project)
        .iter()
        .filter(|r| !r.ports.is_empty())
        .count();
    took.sort();
    eprintln!(
        "processes {} · runs with a port {with_ports}/20 · one poll: min {:?} median {:?} max {:?}",
        snapshot.len(),
        took[0],
        took[took.len() / 2],
        took[took.len() - 1]
    );
    for run in runs {
        f.runs.stop(&f.project, &run.run_id).unwrap();
    }
    for run in f.runs.list(&f.project) {
        let found = f.runs.find(&f.project, &run.run_id).unwrap();
        tokio::time::timeout(Duration::from_secs(15), found.exited())
            .await
            .unwrap();
    }
    assert_eq!(with_ports, 20);
}
