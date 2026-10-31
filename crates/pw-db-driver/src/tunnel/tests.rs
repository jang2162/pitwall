//! 터널 시험. 실 sshd 없이 도는 것(가짜 ssh 스크립트 — unix)과 로컬 sshd 컨테이너에 붙는 것(`live_ssh_tunnel_*`, 표지
//! `cfg(pw_live_ssh)` — `testing` 머리 주석)이 있다.

use std::ffi::OsString;

use super::*;

#[cfg(unix)]
fn key(destination: &str) -> TunnelKey {
    TunnelKey {
        project: "p1".to_owned(),
        destination: destination.to_owned(),
        db_host: "127.0.0.1".to_owned(),
        db_port: testing::ECHO_PORT,
    }
}

#[test]
fn the_argv_is_fixed_and_the_values_come_last() {
    let args = ssh_args(40001, "db.internal", 5432, "bastion").unwrap();
    let args: Vec<&str> = args.iter().map(|a| a.to_str().unwrap()).collect();
    assert_eq!(
        args,
        [
            "-N",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "ServerAliveInterval=30",
            "-o",
            "ControlPath=none",
            "-L",
            "127.0.0.1:40001:db.internal:5432",
            "--",
            "bastion",
        ]
    );
    let v6 = ssh_args(40001, "::1", 5432, "ssh://me@b:2222").unwrap();
    assert_eq!(v6[8], OsString::from("127.0.0.1:40001:[::1]:5432"));
    assert_eq!(v6[10], OsString::from("ssh://me@b:2222"));
}

#[test]
fn values_that_ssh_would_read_as_options_are_refused() {
    for bad in [
        "-oProxyCommand=touch /tmp/x",
        "-oProxyCommand=x",
        "-J",
        "a b",
        "a\tb",
        "a\nb",
        "",
    ] {
        let e = ssh_args(40001, "db", 5432, bad).unwrap_err();
        assert_eq!(e.kind, DbErrorKind::Invalid, "{bad:?}");
        let e = ssh_args(40001, bad, 5432, "bastion").unwrap_err();
        assert_eq!(e.kind, DbErrorKind::Invalid, "{bad:?}");
    }
    assert!(ssh_args(40001, "db", 0, "bastion").is_err());
}

/// 목적지 · DB 호스트는 허용 글자 목록이다 — 셸 메타문자(사람의 `ProxyCommand … %h` 를 거치면 OpenSSH 9.6 미만에서 명령이 된다)는
/// 어느 자리에도 들어가지 못한다. IPv6 · 밑줄 든 호스트 · `~/.ssh/config` 별칭은 받는다.
#[test]
fn destinations_and_db_hosts_take_only_allowed_characters() {
    for bad in [
        "a$(touch${IFS}x)",
        "a;id",
        "a`id`",
        "ssh://x$(id)@h",
        "ssh://h$(id)",
        "ssh://h;id:22",
        "ssh://[::1$(id)]:22",
        "ssh://[::1]x",
        "ssh://[::1]:22;id",
        "ssh://h:+22",
        "ssh://h:",
        "ssh://h:0",
        "ssh://@h",
        "ssh://-x@h",
        "ssh://h/x",
        "user@host",
        "host:22",
        "a|b",
        "a&b",
        "a'b",
        "a\"b",
        "a>b",
        "a*b",
        "a%b",
        "호스트",
    ] {
        assert!(check_destination(bad).is_err(), "{bad:?}");
        let e = ssh_args(40001, "db", 5432, bad).unwrap_err();
        assert_eq!(e.kind, DbErrorKind::Invalid, "{bad:?}");
    }
    for good in [
        "bastion",
        "prod-db_01.internal",
        "my_host",
        "ssh://jump.example",
        "ssh://me@jump.example:2222",
        "ssh://deploy.user@10.0.0.5:22/",
        "ssh://[::1]:22",
        "ssh://[2001:db8::1]",
        "ssh://[fe80::1%25]:2222",
    ] {
        check_destination(good).unwrap_or_else(|why| panic!("{good:?}: {why}"));
        ssh_args(40001, "db", 5432, good).unwrap();
    }
    for bad in [
        "db;id", "db$(id)", "a`b`", "db/x", "-db", "db host", "", "[db;id]",
    ] {
        assert!(check_db_host(bad).is_err(), "{bad:?}");
        assert!(ssh_args(40001, bad, 5432, "bastion").is_err(), "{bad:?}");
    }
    for good in [
        "db",
        "db_1.internal",
        "10.0.0.5",
        "::1",
        "[::1]",
        "2001:db8::5",
    ] {
        check_db_host(good).unwrap_or_else(|why| panic!("{good:?}: {why}"));
        ssh_args(40001, good, 5432, "bastion").unwrap();
    }
}

#[cfg(unix)]
mod fake {
    //! 가짜 ssh: 받은 인자를 적고, 정한 대로 끝나거나 멈춘다.

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::super::testing::{TempDir, shell_quote, write_script};
    use super::super::*;
    use super::key;

    struct Fake {
        dir: TempDir,
    }

    impl Fake {
        /// `body` 는 스크립트 본문. 앞에 호출 기록(`calls` 에 한 줄)과 pid 기록(`pid`)이 붙는다.
        fn new(body: &str) -> Self {
            let dir = TempDir::new("pw-fake-ssh", None);
            let calls = dir.path().join("calls");
            let pid = dir.path().join("pid");
            write_script(
                &dir.path().join("ssh"),
                &format!(
                    "#!/bin/sh\necho \"$*\" >> {}\necho $$ > {}\n{body}\n",
                    shell_quote(&calls),
                    shell_quote(&pid)
                ),
            );
            Self { dir }
        }

        fn launch(&self, askpass: Option<AskpassGuard>) -> Launch {
            Launch {
                program: self.dir.path().join("ssh"),
                env: vec![("PATH".into(), "/usr/bin:/bin".into())],
                askpass,
            }
        }

        fn calls(&self) -> usize {
            fs::read_to_string(self.dir.path().join("calls")).map_or(0, |t| t.lines().count())
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }
    }

    fn read_pid(path: &Path) -> u32 {
        fs::read_to_string(path).unwrap().trim().parse().unwrap()
    }

    async fn gone_within(pid: u32, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        while tokio::time::Instant::now() < deadline {
            if !alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        !alive(pid)
    }

    #[tokio::test]
    async fn an_option_like_destination_runs_nothing() {
        let fake = Fake::new("exit 0");
        let marker = fake.path("x");
        let tunnels = Tunnels::new();
        let e = tunnels
            .open(
                key(&format!("-oProxyCommand=touch{}", marker.display())),
                || async { Ok(fake.launch(None)) },
            )
            .await
            .unwrap_err();
        assert_eq!(e.kind, DbErrorKind::Invalid);
        assert_eq!(fake.calls(), 0, "ssh must not run");
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn an_ssh_that_gives_up_reports_its_words_and_leaves_nothing() {
        let fake = Fake::new("echo 'pitwall@b: Permission denied (publickey).' >&2\nexit 255");
        let e = Tunnels::new()
            .open(key("bastion"), || async { Ok(fake.launch(None)) })
            .await
            .unwrap_err();
        assert_eq!(e.kind, DbErrorKind::Auth);
        assert!(
            e.message.contains("Permission denied (publickey)"),
            "{}",
            e.message
        );
        assert_eq!(fake.calls(), 1);
        assert!(lock(&GROUPS).is_empty() || !lock(&GROUPS).contains(&read_pid(&fake.path("pid"))));
    }

    #[tokio::test]
    async fn an_ssh_that_never_listens_times_out_and_its_whole_group_ends() {
        // 그룹의 다른 프로세스(ProxyJump 의 자식 ssh 같은)도 함께 끝나는지 — 뒤에 띄운 sleep 의 pid 를 적는다.
        let fake = Fake::new(&format!(
            "sleep 30 &\necho $! > {}\nexec sleep 30",
            shell_quote(&std::path::PathBuf::from("CHILD"))
        ));
        let child_file = fake.dir.path().join("child");
        // 스크립트의 CHILD 자리를 절대 경로로.
        let script = fs::read_to_string(fake.path("ssh"))
            .unwrap()
            .replace("'CHILD'", &shell_quote(&child_file));
        write_script(&fake.path("ssh"), &script);
        // 400 ms 는 스크립트가 pid 파일 둘을 다 쓴 뒤부터 센다 — 준비 제한 시간을 멈추는 장치(askpass 의 「묻는 중」)로 그때까지 시계를
        // 세운다. 그러지 않으면 바쁜 기계에서 스크립트가 파일을 쓰기 전에 그룹이 끝나 시험이 시험할 것을 못 본다.
        let written = child_file.clone();
        let setup = AskpassGuard::new(Vec::new(), move || !written.exists(), ());
        let tunnels = Tunnels::with_ready_timeout(Duration::from_millis(400));
        let started = tokio::time::Instant::now();
        let e = tokio::time::timeout(
            Duration::from_secs(30),
            tunnels.open(key("bastion"), || async { Ok(fake.launch(Some(setup))) }),
        )
        .await
        .expect("the fake ssh never wrote its pid files")
        .unwrap_err();
        assert_eq!(e.kind, DbErrorKind::Timeout, "{}", e.message);
        assert!(started.elapsed() >= Duration::from_millis(400));
        let ssh = read_pid(&fake.path("pid"));
        let sleeper = read_pid(&child_file);
        assert!(
            gone_within(ssh, Duration::from_secs(1)).await,
            "ssh {ssh} still runs"
        );
        assert!(
            gone_within(sleeper, Duration::from_secs(1)).await,
            "the ssh's child {sleeper} still runs"
        );
        assert_eq!(tunnels.live_count(), 0);
    }

    /// 남이 첫 포트의 와일드카드(`0.0.0.0:<포트>`)를 잡았다: 처음 찌르기는 남의 리스너에 닿지만, ssh 가 `127.0.0.1:<포트>` 를
    /// 바인드하면(macOS) 그 포트로 준비되고, 바인드하지 못하면(Linux) 다른 포트로 준비된다 — 어느 쪽이든 제한 시간까지 기다리지 않고,
    /// 준비된 뒤 그 주소로 가는 연결은 남에게 가지 않는다.
    #[tokio::test]
    async fn a_wildcard_listener_on_the_port_does_not_stall_a_tunnel_that_listens() {
        use std::sync::atomic::AtomicUsize;

        let fake = Fake::new(concat!(
            "prev=''\nfor a in \"$@\"; do\n  if [ \"$prev\" = -L ]; then port=$(echo \"$a\" | cut -d: -f2); fi\n  prev=\"$a\"\ndone\n",
            // 남의 리스너가 준비 확인에 찔린 뒤에야 바인드한다(경합을 늘 같은 순서로).
            "while [ ! -f 'SEEN' ]; do sleep 0.02; done\n",
            "exec perl -MIO::Socket::INET -e '$s = IO::Socket::INET->new(LocalAddr => \"127.0.0.1\", LocalPort => $ARGV[0], Listen => 5, ReuseAddr => 1) or die \"bind [127.0.0.1]:$ARGV[0]: $!\\n\"; while (my $c = $s->accept) { close $c }' \"$port\"",
        ));
        let seen = fake.path("seen");
        let script = fs::read_to_string(fake.path("ssh"))
            .unwrap()
            .replace("'SEEN'", &shell_quote(&seen));
        write_script(&fake.path("ssh"), &script);

        // 첫 포트만 남이 와일드카드로 잡는다. 남이 받은 연결 수를 센다.
        let accepted = Arc::new(AtomicUsize::new(0));
        let taken = Arc::new(Mutex::new(None::<(u16, tokio::task::JoinHandle<()>)>));
        let (count, slot) = (Arc::clone(&accepted), Arc::clone(&taken));
        let tunnels = Tunnels::with_port_picker(move || {
            let port = free_port()?;
            let mut slot = lock(&slot);
            if slot.is_none() {
                let listener = std::net::TcpListener::bind(("0.0.0.0", port))?;
                listener.set_nonblocking(true)?;
                let listener = tokio::net::TcpListener::from_std(listener)?;
                let (count, seen) = (Arc::clone(&count), seen.clone());
                let task = tokio::spawn(async move {
                    while let Ok((_conn, _)) = listener.accept().await {
                        count.fetch_add(1, Ordering::SeqCst);
                        let _ = fs::write(&seen, "");
                    }
                });
                *slot = Some((port, task));
            }
            Ok(port)
        });
        let started = tokio::time::Instant::now();
        let lease = tokio::time::timeout(
            Duration::from_secs(10),
            tunnels.open(key("bastion"), || async { Ok(fake.launch(None)) }),
        )
        .await
        .expect("a wildcard listener stalled the tunnel")
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "ready only after {:?}",
            started.elapsed()
        );
        let (first, task) = lock(&taken).take().unwrap();
        let probes = accepted.load(Ordering::SeqCst);
        assert!(
            probes >= 1,
            "the readiness probe never met the other listener"
        );
        if cfg!(target_os = "macos") {
            assert_eq!(
                lease.local().port,
                first,
                "ssh listens on loopback beside the wildcard"
            );
        }
        // 준비된 주소로 가는 연결은 남이 받지 않는다(드라이버가 비밀번호를 보낼 곳).
        let conn = tokio::net::TcpStream::connect((LOOPBACK, lease.local().port))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(conn);
        assert_eq!(
            accepted.load(Ordering::SeqCst),
            probes,
            "a connection reached the other listener"
        );
        let ssh = lease.pid();
        drop(lease);
        assert!(
            gone_within(ssh, Duration::from_secs(2)).await,
            "ssh {ssh} still runs"
        );
        task.abort();
    }

    #[tokio::test]
    async fn the_ready_limit_waits_while_askpass_asks_a_person() {
        let fake = Fake::new("exec sleep 30");
        let asking = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&asking);
        let guard = AskpassGuard::new(
            vec![("SSH_ASKPASS".into(), "/nonexistent".into())],
            move || flag.load(Ordering::SeqCst),
            (),
        );
        let tunnels = Tunnels::with_ready_timeout(Duration::from_millis(300));
        let release = Arc::clone(&asking);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1200)).await;
            release.store(false, Ordering::SeqCst);
        });
        let started = tokio::time::Instant::now();
        let e = tunnels
            .open(key("bastion"), || async { Ok(fake.launch(Some(guard))) })
            .await
            .unwrap_err();
        assert_eq!(e.kind, DbErrorKind::Timeout);
        // 묻는 1.2 s 는 세지 않는다 — 그 뒤 0.3 s 가 지나야 끝난다.
        assert!(
            started.elapsed() >= Duration::from_millis(1400),
            "timed out after {:?} while a person was answering",
            started.elapsed()
        );
        assert!(gone_within(read_pid(&fake.path("pid")), Duration::from_secs(1)).await);
    }

    #[tokio::test]
    async fn a_taken_port_is_tried_once_more_on_another_port() {
        let fake = Fake::new(
            "echo 'bind [127.0.0.1]:1: Address already in use' >&2\necho 'Could not request local forwarding.' >&2\nexit 255",
        );
        let e = Tunnels::new()
            .open(key("bastion"), || async { Ok(fake.launch(None)) })
            .await
            .unwrap_err();
        assert_eq!(fake.calls(), 2, "one more try on another port");
        let calls = fs::read_to_string(fake.path("calls")).unwrap();
        let ports: Vec<&str> = calls
            .lines()
            .map(|l| l.split_whitespace().nth(8).unwrap())
            .collect();
        assert_ne!(ports[0], ports[1], "the second try uses another port");
        assert!(e.message.contains("twice"), "{}", e.message);
    }

    #[tokio::test]
    async fn the_child_gets_only_the_given_environment_and_askpass() {
        let fake = Fake::new(&format!(
            "env > {}\nexit 1",
            shell_quote(&PathBuf::from("ENV"))
        ));
        let env_file = fake.path("env");
        let script = fs::read_to_string(fake.path("ssh"))
            .unwrap()
            .replace("'ENV'", &shell_quote(&env_file));
        write_script(&fake.path("ssh"), &script);
        // SAFETY: 이 시험만 이 이름을 쓴다(다른 스레드가 읽지 않는다).
        unsafe { std::env::set_var("PW_TUNNEL_TEST_APP_ONLY", "leak") };
        let guard = AskpassGuard::new(
            vec![
                ("SSH_ASKPASS".into(), "/helper".into()),
                ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
            ],
            || false,
            (),
        );
        let _ = Tunnels::new()
            .open(key("bastion"), || async { Ok(fake.launch(Some(guard))) })
            .await;
        let env = fs::read_to_string(&env_file).unwrap();
        assert!(env.contains("SSH_ASKPASS=/helper"), "{env}");
        assert!(env.contains("SSH_ASKPASS_REQUIRE=force"), "{env}");
        assert!(
            !env.contains("PW_TUNNEL_TEST_APP_ONLY"),
            "the app environment leaked: {env}"
        );
    }
}

#[cfg(pw_live_ssh)]
mod live {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    use super::super::testing::{self, SshFixture};
    use super::super::*;
    use super::key;

    async fn gone_within(pid: u32, limit: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + limit;
        while tokio::time::Instant::now() < deadline {
            if !alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        !alive(pid)
    }

    /// 로컬 포트에 LISTEN 이 있는가(누구 것이든) — 붙어 본다.
    fn listening(port: u16) -> bool {
        TcpStream::connect_timeout(
            &(std::net::Ipv4Addr::LOCALHOST, port).into(),
            Duration::from_millis(300),
        )
        .is_ok()
    }

    fn echo(port: u16, text: &[u8]) -> Vec<u8> {
        let mut stream = TcpStream::connect((LOOPBACK, port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream.write_all(text).unwrap();
        let mut back = vec![0u8; text.len()];
        stream.read_exact(&mut back).unwrap();
        back
    }

    #[test]
    fn live_ssh_tunnel_ready() {
        let f = SshFixture::from_env();
        testing::wait_sshd(&f.host, f.port, Duration::from_secs(120)).unwrap();
    }

    async fn ready(f: &SshFixture) {
        let (host, port) = (f.host.clone(), f.port);
        tokio::task::spawn_blocking(move || {
            testing::wait_sshd(&host, port, Duration::from_secs(120))
        })
        .await
        .unwrap()
        .unwrap();
    }

    /// ① 터널 너머 가짜 TCP 서버(컨테이너 안의 에코)에 바이트가 오간다.
    #[tokio::test]
    async fn live_ssh_tunnel_carries_bytes_both_ways() {
        let f = SshFixture::from_env();
        ready(&f).await;
        let tunnels = Tunnels::new();
        let lease = tunnels
            .open(key(testing::HOST), || async { Ok(f.launch(None)) })
            .await
            .unwrap();
        let port = lease.local().port;
        let back = tokio::task::spawn_blocking(move || echo(port, b"pitwall-tunnel-bytes\n"))
            .await
            .unwrap();
        assert_eq!(back, b"pitwall-tunnel-bytes\n");
        assert_eq!(listener_owned_by(lease.pid(), port).await, Owner::Ours);
    }

    /// ② 두 연결이 한 터널을 함께 쓰고, 마지막이 닫히면 1 s 안에 ssh pid 가 사라지고 로컬 포트에 LISTEN 이 없다.
    #[tokio::test]
    async fn live_ssh_tunnel_is_shared_and_ends_within_a_second_after_the_last_lease() {
        let f = SshFixture::from_env();
        ready(&f).await;
        let tunnels = Tunnels::new();
        let first = tunnels
            .open(key(testing::HOST), || async { Ok(f.launch(None)) })
            .await
            .unwrap();
        let second = tunnels
            .open(key(testing::HOST), || async {
                panic!("the second connection must share the running tunnel")
            })
            .await
            .unwrap();
        assert_eq!(first.pid(), second.pid());
        assert_eq!(first.local(), second.local());
        assert_eq!(tunnels.live_count(), 1);
        let (pid, port) = (first.pid(), first.local().port);
        drop(first);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(alive(pid), "one connection still uses the tunnel");
        assert!(listening(port));
        let lost = second.lost();
        drop(second);
        assert!(
            gone_within(pid, Duration::from_secs(1)).await,
            "ssh {pid} still runs"
        );
        assert!(!listening(port), "the local port still listens");
        assert!(lost.is_cancelled());
        assert_eq!(tunnels.live_count(), 0);
    }

    /// ③ 옵션처럼 읽히는 목적지는 진짜 ssh 를 띄우기 전에 거절된다 — `ProxyCommand` 의 파일이 생기지 않는다.
    #[tokio::test]
    async fn live_ssh_tunnel_refuses_an_option_like_host_ref_and_runs_no_command() {
        let f = SshFixture::from_env();
        let marker = f.dir.path().join("proxy-ran");
        let script = f.dir.path().join("proxy.sh");
        testing::write_script(
            &script,
            &format!("#!/bin/sh\ntouch {}\n", testing::shell_quote(&marker)),
        );
        let x_existed = std::path::Path::new("/tmp/x").exists();
        let tunnels = Tunnels::new();
        for destination in [
            "-oProxyCommand=touch /tmp/x".to_owned(),
            format!("-oProxyCommand={}", script.display()),
        ] {
            let e = tunnels
                .open(key(&destination), || async { Ok(f.launch(None)) })
                .await
                .unwrap_err();
            assert_eq!(e.kind, DbErrorKind::Invalid, "{destination}");
        }
        assert!(!marker.exists(), "the proxy command ran");
        if !x_existed {
            assert!(
                !std::path::Path::new("/tmp/x").exists(),
                "/tmp/x was created"
            );
        }
        assert_eq!(tunnels.live_count(), 0);
    }

    /// ④ 고른 로컬 포트를 그 사이 다른 소켓이 잡으면 ssh 가 끝나고 다른 포트로 성공한다 — 남의 리스너를 「준비됨」 으로 보지 않는다.
    #[tokio::test]
    async fn live_ssh_tunnel_moves_to_another_port_when_the_first_is_taken() {
        let f = SshFixture::from_env();
        ready(&f).await;
        let squatter = TcpListener::bind((LOOPBACK, 0)).unwrap();
        let taken = squatter.local_addr().unwrap().port();
        let picks = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = std::sync::Arc::clone(&picks);
        let tunnels = Tunnels::with_port_picker(move || {
            if counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Ok(taken)
            } else {
                let l = std::net::TcpListener::bind((LOOPBACK, 0))?;
                Ok(l.local_addr()?.port())
            }
        });
        let lease = tunnels
            .open(key(testing::HOST), || async { Ok(f.launch(None)) })
            .await
            .unwrap();
        assert_eq!(picks.load(std::sync::atomic::Ordering::SeqCst), 2);
        let port = lease.local().port;
        assert_ne!(port, taken);
        assert_eq!(listener_owned_by(lease.pid(), port).await, Owner::Ours);
        let back = tokio::task::spawn_blocking(move || echo(port, b"moved\n"))
            .await
            .unwrap();
        assert_eq!(back, b"moved\n");
        drop(squatter);
    }

    /// ⑥(드라이버 쪽) ssh 를 `kill -9` 하면 리스의 `lost` 가 깨어나고, 다음 open 은 새 ssh 를 띄운다(다시 붙는 것은 사람이 누를 때).
    #[tokio::test]
    async fn live_ssh_tunnel_reports_a_killed_ssh_as_lost() {
        let f = SshFixture::from_env();
        ready(&f).await;
        let tunnels = Tunnels::new();
        let lease = tunnels
            .open(key(testing::HOST), || async { Ok(f.launch(None)) })
            .await
            .unwrap();
        let lost = lease.lost();
        let pid = lease.pid();
        // SAFETY: 이 시험이 띄운 ssh 하나에만 보낸다.
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        tokio::time::timeout(Duration::from_secs(2), lost.cancelled())
            .await
            .expect("lost wakes up after kill -9");
        let again = tunnels
            .open(key(testing::HOST), || async { Ok(f.launch(None)) })
            .await
            .unwrap();
        assert_ne!(again.pid(), pid);
    }

    /// 앱이 끝날 때 남은 ssh 가 없다.
    #[tokio::test]
    async fn live_ssh_tunnel_shutdown_leaves_no_ssh() {
        let f = SshFixture::from_env();
        ready(&f).await;
        let tunnels = Tunnels::new();
        let mut leases = Vec::new();
        for db_port in [testing::ECHO_PORT, 22] {
            let key = TunnelKey {
                db_port,
                ..key(testing::HOST)
            };
            leases.push(
                tunnels
                    .open(key, || async { Ok(f.launch(None)) })
                    .await
                    .unwrap(),
            );
        }
        let pids: Vec<u32> = leases.iter().map(TunnelLease::pid).collect();
        assert_ne!(pids[0], pids[1]);
        shutdown();
        for pid in pids {
            assert!(
                gone_within(pid, Duration::from_secs(1)).await,
                "ssh {pid} survived shutdown"
            );
        }
        for lease in &leases {
            assert!(
                lease.lost().is_cancelled() || {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    lease.lost().is_cancelled()
                }
            );
        }
    }

    /// 사람의 ControlMaster 는 쓰지도 끄지도 않는다(`ControlPath=none`): 마스터가 떠 있어도 터널은 우리 ssh 가 직접 LISTEN 하고, 끄면
    /// 그 LISTEN 이 사라지며 마스터는 그대로다.
    #[tokio::test]
    async fn live_ssh_tunnel_does_not_use_or_touch_the_humans_control_master() {
        let f = SshFixture::from_env();
        ready(&f).await;
        let ssh = f.bin.join("ssh");
        let master = std::process::Command::new(&ssh)
            .args(["-fN", testing::HOST_MASTER])
            .env_clear()
            .envs(f.env())
            .status()
            .unwrap();
        assert!(master.success());
        let check = |ssh: &std::path::Path| {
            std::process::Command::new(ssh)
                .args(["-O", "check", testing::HOST_MASTER])
                .env_clear()
                .envs(f.env())
                .output()
                .unwrap()
                .status
                .success()
        };
        assert!(check(&ssh), "the master runs");
        let tunnels = Tunnels::new();
        let lease = tunnels
            .open(key(testing::HOST_MASTER), || async { Ok(f.launch(None)) })
            .await
            .unwrap();
        let (pid, port) = (lease.pid(), lease.local().port);
        assert_eq!(listener_owned_by(pid, port).await, Owner::Ours);
        drop(lease);
        assert!(gone_within(pid, Duration::from_secs(1)).await);
        assert!(
            !listening(port),
            "the forward stayed somewhere (the master?)"
        );
        assert!(check(&ssh), "the human's master must stay");
        let _ = std::process::Command::new(&ssh)
            .args(["-O", "exit", testing::HOST_MASTER])
            .env_clear()
            .envs(f.env())
            .output();
    }
}
