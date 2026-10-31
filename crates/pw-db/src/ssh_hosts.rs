//! `~/.ssh/config` 의 호스트 이름 목록(`db.ssh.hosts`, 14-m3-plan.md M3-14 — 연결 편집 다이얼로그의 SSH 탭이 고르는 목록).
//!
//! - **이름만 읽는다.** `Host` 줄의 패턴 가운데 와일드카드(`*` · `?`) · 부정(`!…`)이 없는 것만, 터널 목적지 검사
//!   (`pw_db_driver::tunnel::check_destination`)를 지나는 것만 돌려준다. `HostName` · `User` · `ProxyJump` 등 다른 값은 읽지
//!   않는다 — 연결은 ssh 가 사람의 설정대로 한다(결정 M3-D11).
//! - `Match` 줄의 조건(`Match host …`)은 읽지 않는다 — 그것은 호스트 이름이 아니라 조건이다.
//! - `Include` 는 따라간다: 상대 경로는 `~/.ssh` 기준, `~/` 는 홈, 마지막 경로 조각의 `*` · `?` 만 펼친다. 깊이 [`MAX_DEPTH`] ·
//!   파일 [`MAX_FILES`] 개 · 파일당 [`MAX_FILE_BYTES`] · 이름 [`MAX_HOSTS`] 개에서 멈춘다.
//! - 일반 파일만 읽는다(FIFO · 장치 · 폴더는 건너뛴다 — 크기가 0 으로 보여 크기 검사로는 못 거르고, 열거나 읽으면 멈춘다). 파일당
//!   상한은 실제로 읽는 바이트에 건다.
//! - 읽기 실패(파일 없음 · 권한 · UTF-8 아님)는 오류가 아니라 빈 목록이다 — 로그에만 남긴다(메인 판정).
//! - 범위: 그 프로젝트 창의 연결 + **신뢰한 프로젝트만**(메인 판정 — 호스트 이름은 비밀은 아니지만 사람의 개인 정보다. 미신뢰
//!   프로젝트의 확장 · 웹 콘텐츠에 내주지 않는다). MCP 에 내지 않는다.
//!
//! 원격 「호스트 추가」(10-ui.md 「다이얼로그 · 팝업 목록」, M7)가 같은 목록을 쓸 때 공용 크레이트로 옮긴다.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pw_rpc::{Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::config::{Connections, DbProjectParams, require_project_window};

/// `Include` 를 따라가는 깊이(ssh 는 16).
pub const MAX_DEPTH: usize = 8;
/// 읽는 파일 수(처음 파일 포함).
pub const MAX_FILES: usize = 64;
/// 파일 하나에서 읽는 바이트. 넘으면 그 파일은 읽지 않는다.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// 돌려주는 이름 수.
pub const MAX_HOSTS: usize = 1000;

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbSshHosts {
    /// 파일에 나온 순서, 같은 이름은 한 번.
    pub hosts: Vec<String>,
}

rpc_method! {
    /// `~/.ssh/config`(과 그 `Include`)의 `Host` 이름 — 와일드카드 · 부정 패턴이 없고 터널 목적지로 쓸 수 있는 것만. 다른 값은
    /// 읽지 않는다. 읽지 못하면 빈 목록. 그 프로젝트 창의 연결 + 신뢰한 프로젝트만.
    pub DbSshHostsMethod: "db.ssh.hosts" {
        params: DbProjectParams,
        result: DbSshHosts,
        tier: Read,
        roles: [Owner],
    }
}

/// `config`(사람의 `~/.ssh/config`)에서 호스트 이름을 읽는다. `ssh_dir` 은 상대 `Include` 의 기준(`~/.ssh`), `home` 은 `~/` 를 푼다.
pub fn hosts_in(config: &Path, ssh_dir: &Path, home: &Path) -> Vec<String> {
    let mut reader = Reader {
        ssh_dir,
        home,
        files: 0,
        hosts: Vec::new(),
    };
    reader.read(config, 0);
    reader.hosts
}

struct Reader<'a> {
    ssh_dir: &'a Path,
    home: &'a Path,
    files: usize,
    hosts: Vec<String>,
}

impl Reader<'_> {
    fn read(&mut self, path: &Path, depth: usize) {
        if depth > MAX_DEPTH || self.files >= MAX_FILES || self.hosts.len() >= MAX_HOSTS {
            return;
        }
        self.files += 1;
        let Some(text) = read_limited(path) else {
            return;
        };
        for line in text.lines() {
            let Some((keyword, args)) = split_line(line) else {
                continue;
            };
            if keyword.eq_ignore_ascii_case("host") {
                for pattern in words(args) {
                    self.add(&pattern);
                }
            } else if keyword.eq_ignore_ascii_case("include") {
                for pattern in words(args) {
                    for file in self.expand(&pattern) {
                        self.read(&file, depth + 1);
                    }
                }
            }
        }
    }

    fn add(&mut self, pattern: &str) {
        // 와일드카드(`*` · `?`) · 부정(`!`) 패턴은 목적지 허용 글자 밖이라 이 검사 하나로 빠진다.
        if self.hosts.len() >= MAX_HOSTS
            || pw_db_driver::tunnel::check_destination(pattern).is_err()
            || self.hosts.iter().any(|h| h == pattern)
        {
            return;
        }
        self.hosts.push(pattern.to_owned());
    }

    /// `Include` 인자를 파일 목록으로. 마지막 조각의 `*` · `?` 만 펼치고 이름 순으로 돌려준다(ssh 도 glob 순서로 읽는다).
    fn expand(&self, pattern: &str) -> Vec<PathBuf> {
        let path = if let Some(rest) = pattern.strip_prefix("~/") {
            self.home.join(rest)
        } else if Path::new(pattern).is_absolute() {
            PathBuf::from(pattern)
        } else {
            self.ssh_dir.join(pattern)
        };
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            return Vec::new();
        };
        if !name.contains(['*', '?']) {
            return vec![path];
        }
        let Some(dir) = path.parent() else {
            return Vec::new();
        };
        if dir.to_str().is_some_and(|d| d.contains(['*', '?'])) {
            // 폴더 조각의 glob 은 펼치지 않는다 — 이름 목록일 뿐이라 놓쳐도 사람이 직접 친다.
            return Vec::new();
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_str().is_some_and(|n| glob_match(name, n)))
            .map(|e| e.path())
            .collect();
        files.sort();
        files
    }
}

/// 일반 파일만, [`MAX_FILE_BYTES`] 까지 읽는다. FIFO · 장치 · 폴더는 열지 않는다 — FIFO 는 여는 순간 쓰는 쪽을 기다리고 `/dev/zero`
/// 는 끝이 없으며, 둘 다 크기가 0 으로 보여 크기 검사로는 거르지 못한다. 상한은 실제로 읽는 바이트에 건다(검사와 읽기 사이에 파일이
/// 커져도 그 이상 읽지 않는다). 넘거나 읽지 못하면 `None`(로그만).
fn read_limited(path: &Path) -> Option<String> {
    use std::io::Read;

    let skip = |why: &str| {
        tracing::info!(target: "pw_db", path = %path.display(), why, "ssh config file not read for the host list");
        None
    };
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return skip("not a regular file"),
        Err(e) => {
            tracing::debug!(target: "pw_db", path = %path.display(), error = %e, "no ssh config");
            return None;
        }
    }
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return skip("cannot open"),
    };
    // 검사와 열기 사이에 바뀌었으면 연 것을 다시 본다.
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return skip("not a regular file");
    }
    let mut bytes = Vec::new();
    if file
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return skip("cannot read");
    }
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return skip("too large");
    }
    String::from_utf8(bytes).ok().or_else(|| skip("not utf-8"))
}

/// 한 줄을 (키워드, 나머지)로. 빈 줄 · 주석은 `None`. ssh 처럼 키워드 뒤의 공백 또는 `=` 하나로 가른다.
fn split_line(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let (keyword, rest) = line.split_at(end);
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim_start();
    Some((keyword, rest))
}

/// 공백으로 가른 인자. 큰따옴표로 감싼 것은 한 인자다. 줄 끝 주석(`#` 로 시작하는 인자)부터는 버린다.
fn words(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = args.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let Some(&first) = chars.peek() else {
            break;
        };
        if first == '#' {
            break;
        }
        let mut word = String::new();
        if first == '"' {
            chars.next();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                word.push(c);
            }
        } else {
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                word.push(c);
            }
        }
        if !word.is_empty() {
            out.push(word);
        }
    }
    out
}

/// `*`(아무 글자 0개 이상) · `?`(한 글자)만 아는 맞추기.
fn glob_match(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    let (mut pi, mut ni, mut star, mut mark) = (0, 0, None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

impl Connections {
    /// `db.ssh.hosts`. `home` 은 사람의 홈 — RPC 는 앱 프로세스의 홈을 준다(ssh 도 같은 사용자로 돈다). 시험은 임시 폴더를 준다.
    pub fn ssh_hosts(
        &self,
        scope: &pw_rpc::ConnectionScope,
        params: &DbProjectParams,
        home: Option<&Path>,
    ) -> Result<DbSshHosts, RpcError> {
        require_project_window(scope, &params.project_id)?;
        self.trust_gate(&params.project_id, params.worktree.as_deref())?;
        let Some(home) = home else {
            return Ok(DbSshHosts { hosts: Vec::new() });
        };
        let ssh_dir = home.join(".ssh");
        Ok(DbSshHosts {
            hosts: hosts_in(&ssh_dir.join("config"), &ssh_dir, home),
        })
    }
}

/// 핸들러를 붙인다. 파일 읽기는 블로킹 스레드에서 한다.
pub fn register(router: &mut Router, connections: Arc<Connections>) {
    router.unary(DbSshHostsMethod, move |ctx, params: DbProjectParams| {
        let (c, scope) = (Arc::clone(&connections), ctx.scope().clone());
        async move {
            tokio::task::spawn_blocking(move || {
                c.ssh_hosts(&scope, &params, std::env::home_dir().as_deref())
            })
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn only_plain_host_names_that_can_be_a_tunnel_destination_are_listed() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        let config = write(
            &ssh,
            "config",
            "# comment\n\
             Host bastion db-jump  # trailing comment\n\
             \tHostName 10.0.0.1\n\
             \tUser deploy\n\
             Host *.internal !secret prod-?\n\
             Host=eq-sign\n\
             host \"quoted-name\"\n\
             Host bad;name $(touch) -oProxyCommand=x\n\
             Match host matched-only exec \"true\"\n\
             Host bastion\n\
             HostName not-a-host-line\n",
        );
        assert_eq!(
            hosts_in(&config, &ssh, home.path()),
            ["bastion", "db-jump", "eq-sign", "quoted-name"]
        );
    }

    #[test]
    fn includes_are_followed_relative_to_the_ssh_dir_with_a_simple_glob() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        let config = write(
            &ssh,
            "config",
            "Include conf.d/*.conf ~/.other/ssh_config /nowhere/at/all\nHost main\n",
        );
        write(&ssh, "conf.d/b.conf", "Host from-b\n");
        write(&ssh, "conf.d/a.conf", "Host from-a\nInclude nested\n");
        write(&ssh, "conf.d/skip.txt", "Host not-included\n");
        write(&ssh, "nested", "Host from-nested\n");
        write(home.path(), ".other/ssh_config", "Host from-home\n");
        assert_eq!(
            hosts_in(&config, &ssh, home.path()),
            ["from-a", "from-nested", "from-b", "from-home", "main"]
        );
    }

    #[test]
    fn include_loops_and_missing_files_end_without_an_error() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        let config = write(&ssh, "config", "Host a\nInclude config\n");
        assert_eq!(hosts_in(&config, &ssh, home.path()), ["a"]);
        assert!(hosts_in(&ssh.join("absent"), &ssh, home.path()).is_empty());
    }

    #[test]
    fn the_include_depth_and_file_count_are_capped() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        // 사슬: config → f1 → f2 → … 각 파일이 이름 하나를 둔다.
        for i in 0..MAX_DEPTH + 4 {
            let name = if i == 0 {
                "config".to_owned()
            } else {
                format!("f{i}")
            };
            write(&ssh, &name, &format!("Host h{i}\nInclude f{}\n", i + 1));
        }
        let hosts = hosts_in(&ssh.join("config"), &ssh, home.path());
        assert_eq!(hosts.len(), MAX_DEPTH + 1, "{hosts:?}");

        // 넓게: 파일 수 상한.
        let wide = tempfile::tempdir().unwrap();
        let ssh = wide.path().join(".ssh");
        write(&ssh, "config", "Include many/*\n");
        for i in 0..MAX_FILES + 10 {
            write(&ssh, &format!("many/{i:03}"), &format!("Host w{i}\n"));
        }
        let hosts = hosts_in(&ssh.join("config"), &ssh, wide.path());
        assert_eq!(hosts.len(), MAX_FILES - 1, "the first file counts too");
    }

    #[test]
    fn a_file_over_the_byte_limit_is_not_read() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        let mut text = String::from("Host big\n");
        text.push_str(&"#".repeat(MAX_FILE_BYTES as usize));
        let config = write(&ssh, "config", &text);
        assert!(hosts_in(&config, &ssh, home.path()).is_empty());
    }

    /// FIFO · 장치 · 폴더를 가리키는 `Include` 는 건너뛰고 나머지를 읽는다 — 멈추지 않는다(시간 상한을 걸고 본다).
    #[cfg(unix)]
    #[test]
    fn includes_of_fifos_devices_and_folders_are_skipped_without_hanging() {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        let config = write(
            &ssh,
            "config",
            "Host a\nInclude pipe /dev/zero folder\nHost b\n",
        );
        let made = std::process::Command::new("mkfifo")
            .arg(ssh.join("pipe"))
            .status()
            .unwrap();
        assert!(made.success());
        std::fs::create_dir(ssh.join("folder")).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let (config2, ssh2, home2) = (config.clone(), ssh.clone(), home.path().to_path_buf());
        std::thread::spawn(move || {
            let _ = tx.send(hosts_in(&config2, &ssh2, &home2));
        });
        let hosts = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("reading a fifo or a device must not hang");
        assert_eq!(hosts, ["a", "b"]);
    }

    #[test]
    fn glob_matches_star_and_question_mark() {
        assert!(glob_match("*.conf", "a.conf"));
        assert!(glob_match("a?c", "abc"));
        assert!(glob_match("*", ""));
        assert!(!glob_match("*.conf", "a.conf.bak"));
        assert!(!glob_match("a?c", "ac"));
    }
}
