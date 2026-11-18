//! 실행의 env(17-m6-plan M6-03)의 완료 기준 ① ~ ④ 와 「사람이 본 구성」 해시(M6-11)의 env 몫.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use pw_runtime::RuntimeEnv;

use super::dotenv::{Problem, SyntaxError, parse};
use super::*;
use crate::config::read::from_file_value;

/// 시험의 비밀 값 — 어느 글에도 나오면 안 된다.
const SECRET: &str = "s3cr3t-VALUE-9f2c";

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(tmp.path()).unwrap();
        let root = base.join("proj");
        fs::create_dir_all(&root).unwrap();
        Self {
            _tmp: tmp,
            base,
            root,
        }
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
}

fn config(body: &str) -> RunConfig {
    let doc: toml_edit::DocumentMut = body.parse().unwrap();
    from_file_value(pw_core::config::merge::to_json(doc.as_table())).unwrap()
}

fn runtime(vars: &[(&str, &str)]) -> RuntimeEnv {
    RuntimeEnv::from_vars(
        vars.iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v))),
    )
}

fn pairs(text: &str) -> Vec<(String, String)> {
    parse(text).unwrap_or_else(|e| panic!("{e}"))
}

fn one(text: &str) -> String {
    let parsed = pairs(text);
    assert_eq!(parsed.len(), 1, "{text:?} → {parsed:?}");
    parsed.into_iter().next().unwrap().1
}

// ① 덮는 순서: 런타임 환경 → env_file(목록 순서) → env → 프로필 env.

#[test]
fn the_profile_wins_over_env_over_files_over_the_runtime() {
    let f = Fixture::new();
    f.write(".env", "WHO=file1\nONLY_FILES=file1\nFILE_VS_ENV=file1\n");
    f.write(".env.local", "WHO=file2\nONLY_FILES=file2\n");
    let cfg = config(
        r#"
name = "dev"
type = "shell"
command = "true"
env_file = [".env", ".env.local"]
env = { WHO = "env", FILE_VS_ENV = "env", ENV_VS_PROFILE = "env" }
[profiles.prod]
env = { WHO = "profile", ENV_VS_PROFILE = "profile" }
"#,
    );
    let base = runtime(&[
        ("PATH", "/usr/bin"),
        ("WHO", "runtime"),
        ("ONLY_RUNTIME", "runtime"),
    ]);

    // (키, 프로필 없이, 프로필 `prod`) — 한 키가 네 자리에 다 있으면 프로필, 그다음 env, 그다음 뒤 파일, 그다음 런타임.
    let table = [
        ("WHO", "env", "profile"),
        ("ENV_VS_PROFILE", "env", "profile"),
        ("FILE_VS_ENV", "env", "env"),
        ("ONLY_FILES", "file2", "file2"),
        ("ONLY_RUNTIME", "runtime", "runtime"),
    ];
    let plain = RunEnv::resolve(&cfg, None, &f.root).unwrap().apply(&base);
    let prod = RunEnv::resolve(&cfg, Some("prod"), &f.root)
        .unwrap()
        .apply(&base);
    for (key, without, with) in table {
        assert_eq!(
            plain.get(key).and_then(|v| v.to_str()),
            Some(without),
            "{key} without a profile"
        );
        assert_eq!(
            prod.get(key).and_then(|v| v.to_str()),
            Some(with),
            "{key} with profile `prod`"
        );
    }
    assert_eq!(prod.path, OsString::from("/usr/bin"));
}

#[test]
fn path_from_the_config_replaces_the_runtime_path() {
    let f = Fixture::new();
    let cfg = config(
        "name = \"x\"\ntype = \"shell\"\ncommand = \"true\"\nenv = { PATH = \"/opt/bin\" }\n",
    );
    let env = RunEnv::resolve(&cfg, None, &f.root)
        .unwrap()
        .apply(&runtime(&[("PATH", "/usr/bin"), ("HOME", "/h")]));
    assert_eq!(env.path, OsString::from("/opt/bin"));
    assert!(!env.vars.contains_key(&OsString::from("PATH")));
    assert_eq!(env.get("HOME").unwrap(), "/h");
}

#[test]
fn an_unknown_profile_is_an_error() {
    let f = Fixture::new();
    let cfg = config("name = \"x\"\ntype = \"shell\"\ncommand = \"true\"\n");
    let err = RunEnv::resolve(&cfg, Some("nope"), &f.root).unwrap_err();
    assert!(matches!(err, EnvError::UnknownProfile(_)), "{err}");
}

// ② `.env` 문법(메인 판정 M6-P5).

#[test]
fn dotenv_grammar_table() {
    let table: &[(&str, &str)] = &[
        ("A=plain", "plain"),
        ("A=", ""),
        ("export A=exported", "exported"),
        ("export\tA=tab", "tab"),
        ("  A = spaced  ", "spaced"),
        ("A=a b  c", "a b  c"),
        ("A='single $HOME \\n'", "single $HOME \\n"),
        ("A=\"double\"", "double"),
        ("A=\"line1\\nline2\"", "line1\nline2"),
        ("A=\"back\\tslash\\\\\"", "back\\tslash\\\\"),
        ("A='has \"double\" inside'", "has \"double\" inside"),
        ("A=\"has 'single' inside\"", "has 'single' inside"),
        ("A=1 # comment", "1"),
        ("A=1\t# comment", "1"),
        ("A=#fff", "#fff"),
        ("A=a#b", "a#b"),
        ("A= # only a comment", ""),
        (
            "A=\"quoted # not a comment\" # comment",
            "quoted # not a comment",
        ),
        ("A='x'#c", "x"),
        // 변수 확장은 하지 않는다.
        ("A=${OTHER}", "${OTHER}"),
        ("A=$OTHER/bin", "$OTHER/bin"),
        ("A=\"${OTHER}\"", "${OTHER}"),
        ("A=`cmd`", "`cmd`"),
        ("A=x=y=z", "x=y=z"),
        ("A=café ✓", "café ✓"),
    ];
    for (line, want) in table {
        assert_eq!(one(line), *want, "{line:?}");
    }
}

#[test]
fn dotenv_files_with_comments_crlf_bom_and_multiline_values() {
    let text = "\u{feff}# header\r\n\r\nexport FIRST=1\r\n  # indented comment\nKEY=\"-----BEGIN-----\nabc\n-----END-----\"\nexport=word\nFIRST=2\n";
    assert_eq!(
        pairs(text),
        vec![
            ("FIRST".to_owned(), "1".to_owned()),
            (
                "KEY".to_owned(),
                "-----BEGIN-----\nabc\n-----END-----".to_owned()
            ),
            ("export".to_owned(), "word".to_owned()),
            ("FIRST".to_owned(), "2".to_owned()),
        ]
    );
    assert_eq!(pairs(""), vec![]);
    assert_eq!(pairs("A=1"), vec![("A".to_owned(), "1".to_owned())]);
}

#[test]
fn crlf_is_one_line_feed_inside_quotes_too() {
    let text = "A=\"-----BEGIN-----\r\nabc\r\n-----END-----\"\r\nB='x\r\ny'\r\nC=\"one\\ntwo\"\r\nD=plain\r\n";
    assert_eq!(
        pairs(text),
        vec![
            (
                "A".to_owned(),
                "-----BEGIN-----\nabc\n-----END-----".to_owned()
            ),
            ("B".to_owned(), "x\ny".to_owned()),
            ("C".to_owned(), "one\ntwo".to_owned()),
            ("D".to_owned(), "plain".to_owned()),
        ]
    );
    // 줄 번호는 `\r\n` 을 한 줄로 센다.
    assert_eq!(
        parse("A=\"x\r\ny\"\r\nB\r\n"),
        Err(SyntaxError {
            line: 3,
            problem: Problem::MissingEquals
        })
    );
}

#[test]
fn dotenv_errors_name_the_line_and_never_the_text() {
    let table: &[(&str, usize, Problem)] = &[
        ("A=1\nnot a pair\n", 2, Problem::InvalidName),
        ("A=1\nLONELY\n", 2, Problem::MissingEquals),
        ("1A=x", 1, Problem::InvalidName),
        ("A.B=x", 1, Problem::InvalidName),
        ("=x", 1, Problem::InvalidName),
        ("A=1\nB=\"open\nC=2\n", 2, Problem::UnclosedQuote),
        ("A='open", 1, Problem::UnclosedQuote),
        ("A=\"x\" trailing", 1, Problem::TextAfterQuote),
        // `\"` 는 풀지 않는다 — 겹따옴표가 거기서 닫힌다.
        ("A=\"say \\\"hi\\\"\"", 1, Problem::TextAfterQuote),
        ("A=\"multi\nline\"\nB", 3, Problem::MissingEquals),
        ("A=x\0y", 1, Problem::Nul),
        // 외톨이 `\r`(옛 Mac 줄 끝) — 글자로 보면 `B` 가 조용히 `A` 의 값에 붙는다.
        ("A=1\rB=2\r", 1, Problem::LoneCr),
        ("A=1\r", 1, Problem::LoneCr),
        ("A=1\nB=\"x\ry\"\n", 2, Problem::LoneCr),
        ("A='x\r\ny\r'", 2, Problem::LoneCr),
        ("# comment\rB=2\n", 1, Problem::LoneCr),
    ];
    for (text, line, problem) in table {
        assert_eq!(
            parse(text),
            Err(SyntaxError {
                line: *line,
                problem: *problem
            }),
            "{text:?}"
        );
    }
    let message = parse(&format!("A=1\nB=\"{SECRET}"))
        .unwrap_err()
        .to_string();
    assert!(!message.contains(SECRET), "{message}");
    assert!(message.contains("line 2"), "{message}");
}

#[test]
fn a_syntax_error_in_a_file_names_the_file_and_line() {
    let f = Fixture::new();
    f.write(".env", &format!("OK=1\n{SECRET} oops\n"));
    let cfg = config("name = \"x\"\ntype = \"shell\"\ncommand = \"true\"\nenv_file = [\".env\"]\n");
    let err = RunEnv::resolve(&cfg, None, &f.root).unwrap_err();
    let message = err.to_string();
    assert!(matches!(err, EnvError::Syntax { .. }), "{message}");
    assert!(
        message.contains(".env") && message.contains("line 2"),
        "{message}"
    );
    assert!(!message.contains(SECRET), "{message}");
}

// ③ 루트 밖 `env_file` 거절.

#[test]
fn env_files_outside_the_root_are_refused() {
    let f = Fixture::new();
    fs::write(f.base.join("outside.env"), format!("LEAK={SECRET}\n")).unwrap();
    let mut refused = vec!["../outside.env".to_owned()];
    refused.push(f.base.join("outside.env").display().to_string());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(f.base.join("outside.env"), f.root.join(".env.link")).unwrap();
        refused.push(".env.link".to_owned());
        fs::create_dir_all(f.base.join("elsewhere")).unwrap();
        fs::write(f.base.join("elsewhere/.env"), format!("LEAK={SECRET}\n")).unwrap();
        std::os::unix::fs::symlink(f.base.join("elsewhere"), f.root.join("conf")).unwrap();
        refused.push("conf/.env".to_owned());
    }
    for rel in refused {
        let mut cfg = config("name = \"x\"\ntype = \"shell\"\ncommand = \"true\"\n");
        cfg.env_file = vec![rel.clone()];
        let err = RunEnv::resolve(&cfg, None, &f.root).unwrap_err();
        assert!(matches!(err, EnvError::Outside { .. }), "{rel}: {err}");
        assert!(!err.to_string().contains(SECRET), "{rel}: {err}");
    }
}

#[test]
fn a_link_inside_the_root_is_followed() {
    let f = Fixture::new();
    f.write("conf/real.env", "A=linked\n");
    #[cfg(unix)]
    std::os::unix::fs::symlink(f.root.join("conf/real.env"), f.root.join(".env")).unwrap();
    #[cfg(not(unix))]
    f.write(".env", "A=linked\n");
    let cfg = config("name = \"x\"\ntype = \"shell\"\ncommand = \"true\"\nenv_file = [\".env\"]\n");
    let env = RunEnv::resolve(&cfg, None, &f.root).unwrap();
    assert_eq!(env.vars().get("A").map(String::as_str), Some("linked"));
}

#[test]
fn missing_large_and_non_text_files_are_errors() {
    let f = Fixture::new();
    let resolve = |rel: &str| {
        let mut cfg = config("name = \"x\"\ntype = \"shell\"\ncommand = \"true\"\n");
        cfg.env_file = vec![rel.to_owned()];
        RunEnv::resolve(&cfg, None, &f.root)
    };
    assert!(matches!(
        resolve(".env.missing"),
        Err(EnvError::Read { .. })
    ));

    f.write("dir/.keep", "");
    assert!(matches!(resolve("dir"), Err(EnvError::Read { .. })));

    let mut big = "A=1\n".repeat((MAX_ENV_FILE_BYTES as usize) / 4);
    f.write(".env.exact", &big);
    assert_eq!(big.len() as u64, MAX_ENV_FILE_BYTES);
    assert!(resolve(".env.exact").is_ok());
    big.push('\n');
    f.write(".env.big", &big);
    assert!(matches!(
        resolve(".env.big"),
        Err(EnvError::TooLarge { .. })
    ));

    fs::write(f.root.join(".env.bin"), [b'A', b'=', 0xff, 0xfe, b'\n']).unwrap();
    assert!(matches!(resolve(".env.bin"), Err(EnvError::NotUtf8 { .. })));

    #[cfg(unix)]
    {
        let fifo = f.root.join(".env.fifo");
        let c = std::ffi::CString::new(fifo.display().to_string()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        // FIFO 를 열다 막히지 않고 일반 파일이 아니라고 거절한다. 막히는 회귀가 시험을 멈추지 않고 실패하게 따로 돌려 시한을 둔다.
        let mut cfg = config("name = \"x\"\ntype = \"shell\"\ncommand = \"true\"\n");
        cfg.env_file = vec![".env.fifo".to_owned()];
        let root = f.root.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(RunEnv::resolve(&cfg, None, &root));
        });
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("reading a FIFO env_file blocked instead of being refused");
        assert!(matches!(result, Err(EnvError::Read { .. })));
    }
}

// ④ 값이 든 `RunEnv` 의 `Debug` · 오류 글에 값이 없다.

#[test]
fn debug_and_errors_never_show_values() {
    let f = Fixture::new();
    f.write(".env.local", &format!("FROM_FILE={SECRET}\n"));
    let cfg = config(&format!(
        "name = \"x\"\ntype = \"shell\"\ncommand = \"true\"\nenv_file = [\".env.local\"]\nenv = {{ FROM_ENV = \"{SECRET}\" }}\n[profiles.p]\nenv = {{ FROM_PROFILE = \"{SECRET}\" }}\n"
    ));
    let env = RunEnv::resolve(&cfg, Some("p"), &f.root).unwrap();
    assert_eq!(env.vars().len(), 3);
    let debug = format!("{env:?} {env:#?}");
    assert!(!debug.contains(SECRET), "{debug}");
    // 이름 · 파일 경로 · 프로필은 보인다.
    for shown in [
        "FROM_FILE",
        "FROM_ENV",
        "FROM_PROFILE",
        ".env.local",
        "\"p\"",
    ] {
        assert!(debug.contains(shown), "{shown} missing from {debug}");
    }

    // 오류의 Display · Debug 에도 값이 없다.
    let mut broken = cfg.clone();
    f.write(".env.broken", &format!("A=\"{SECRET}\n"));
    broken.env_file = vec![".env.broken".to_owned()];
    let err = RunEnv::resolve(&broken, Some("p"), &f.root).unwrap_err();
    let text = format!("{err} {err:?}");
    assert!(!text.contains(SECRET), "{text}");
}

#[test]
fn the_plan_view_masks_secret_names_only() {
    let f = Fixture::new();
    f.write(
        ".env",
        &format!("API_KEY={SECRET}\nDB_PASSWORD={SECRET}\nPORT=5173\n"),
    );
    let cfg = config(
        "name = \"x\"\ntype = \"shell\"\ncommand = \"true\"\nenv_file = [\".env\"]\nenv = { authToken = \"t\", KEYMAP = \"vim\" }\n",
    );
    let masked = RunEnv::resolve(&cfg, None, &f.root).unwrap().masked();
    let want: BTreeMap<String, String> = [
        ("API_KEY", pw_core::secret::REDACTED),
        ("DB_PASSWORD", pw_core::secret::REDACTED),
        ("PORT", "5173"),
        ("authToken", pw_core::secret::REDACTED),
        ("KEYMAP", "vim"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    assert_eq!(masked, want);
}

// 「사람이 본 구성」 해시(M6-11)의 입력 — `.env` 파일마다의 다이제스트.

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn env_file_digests_are_the_sha256_of_the_bytes_that_gave_the_values() {
    let f = Fixture::new();
    // BOM · CRLF · 주석이 든 파일 — 다이제스트는 푼 글이 아니라 읽은 바이트 그대로의 것이다.
    let first = "\u{feff}DB=local\r\n# note\r\n";
    let second = "DB=second\n";
    f.write(".env", first);
    f.write("conf/.env.local", second);
    let cfg = config(
        "name = \"x\"\ntype = \"shell\"\ncommand = \"true\"\nenv_file = [\".env\", \"conf/.env.local\"]\n",
    );
    let env = RunEnv::resolve(&cfg, None, &f.root).unwrap();
    assert_eq!(
        env.files(),
        [
            EnvFileDigest {
                path: ".env".to_owned(),
                sha256: sha256(first.as_bytes()),
            },
            EnvFileDigest {
                path: "conf/.env.local".to_owned(),
                sha256: sha256(second.as_bytes()),
            },
        ]
    );
    assert_eq!(env.vars().get("DB").map(String::as_str), Some("second"));
    // `printf 'A=1\n' | shasum -a 256` — 도구 밖의 값과도 맞춘다.
    assert_eq!(
        sha256(b"A=1\n"),
        "91d6a3d55e9fea7911c537afae6607c77fa8bc0f3a76c375e104ac5a8cfa84db"
    );

    // 주석 한 줄만 바꿔도 다이제스트가 바뀐다(값은 같다).
    f.write(".env", "\u{feff}DB=local\r\n# other note\r\n");
    let again = RunEnv::resolve(&cfg, None, &f.root).unwrap();
    assert_eq!(again.vars(), env.vars());
    assert_ne!(again.files()[0].sha256, env.files()[0].sha256);
    assert_eq!(again.files()[1], env.files()[1]);
}

#[test]
fn env_file_digests_do_not_depend_on_the_worktree() {
    let a = Fixture::new();
    let b = Fixture::new();
    for f in [&a, &b] {
        f.write(".env", "A=1\n");
    }
    let cfg = config("name = \"x\"\ntype = \"shell\"\ncommand = \"true\"\nenv_file = [\".env\"]\n");
    let ea = RunEnv::resolve(&cfg, None, &a.root).unwrap();
    let eb = RunEnv::resolve(&cfg, None, &b.root).unwrap();
    assert_eq!(ea.files(), eb.files());
    assert_eq!(ea.files()[0].path, ".env");
}
