//! 기본 번들 실 서버 시험 매트릭스(15-m4-plan.md M4-18 — CI `lsp-live`): basedpyright · ruff · jdtls · kotlin-lsp 를 Pitwall 의 서버
//! 관리자 · 기능 RPC 로 띄워 기대 능력 표(`scripts/lsp-live.toml` 의 `[capabilities]`)의 능력마다 시험 하나씩 돈다. 픽스처는
//! `tests/fixtures/{python,java,kotlin}/`(앵커 표는 `tests/fixtures/README.md`)를 임시 폴더에 복사해 쓴다. 각 시험은 먼저 그 능력을
//! 서버가 알리는지(`lsp.document.capabilities` — 정적 능력 · 동적 등록 모두, 기능 라우팅과 같은 판정) 보고, 알리지 않으면 실패한다
//! (빈 통과 없음). 시한은 서버별 — JVM 서버(jdtls · kotlin-lsp)는 첫 답까지 180 s, 나머지 60 s. 서버 실측(첫 진단까지 · 완성 응답
//! p50 · 서버 RSS)은 `METRIC` 줄로 찍는다(목표 없음 — 결정 M4-D19). 표지(`cfg(pw_live_lsp_<서버>)`)가 꺼져 있으면 그 서버의 시험이
//! 목록에 없다(`src/live.rs` 머리 주석).
//!
//! jdtls 의 작업 공간 색인 폴더는 서버 환경의 `JDTLS_DATA`(덮어쓰기 정의의 런처가 읽는다)로 시험 폴더 안에 둔다. Gradle 은
//! `~/.gradle` 을 쓴다(픽스처의 wrapper 판 · Kotlin Gradle 플러그인을 받는다 — CI 는 캐시한다).

#![cfg(unix)]

mod common;

#[cfg(any(
    pw_live_lsp_basedpyright,
    pw_live_lsp_ruff,
    pw_live_lsp_jdtls,
    pw_live_lsp_kotlin_lsp
))]
#[allow(
    dead_code,
    reason = "each server uses a part of the helpers; a server without its cfg leaves the rest unused"
)]
mod live {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use pw_lsp::edit::{LspFileChange, LspTextEdit, LspWorkspaceEdit};
    use pw_lsp::live::LiveServer;
    use pw_lsp::methods::LspInstanceState;
    use serde_json::{Value, json};

    use super::common;
    use super::common::env::{Conn, Env, EnvOptions};

    /// 서버 하나의 픽스처와 기대값.
    pub struct Spec {
        pub server: LiveServer,
        pub fixture: &'static str,
        /// `lsp.document.open` 의 언어(Linguist 이름).
        pub language: &'static str,
        /// 호출 자리(`call.*` · `signature.scale` · `complete.rect-members`).
        pub calls: &'static str,
        /// 정의 자리(`def.*` · `rename.scale`).
        pub shapes: &'static str,
        /// 오류 자리(`error.type-mismatch` · python 은 `unused.os` 도).
        pub errors: &'static str,
        /// 흐트러진 서식(`format.messy`).
        pub messy: &'static str,
        /// `scale` 의 사용처(선언 포함) 수.
        pub references: usize,
        /// rename 뒤 파일마다 새 이름이 나오는 수(정의 파일, 호출 파일).
        pub renamed: (usize, usize),
        /// 문서 심볼에 나와야 하는 이름(정의 파일).
        pub symbols: &'static [&'static str],
    }

    pub const PYTHON: Spec = Spec {
        server: LiveServer::Basedpyright,
        fixture: "python",
        language: "Python",
        calls: "calls.py",
        shapes: "shapes.py",
        errors: "errors.py",
        messy: "messy.py",
        // 정의 · import · 호출 셋.
        references: 5,
        renamed: (1, 4),
        symbols: &["Shape", "Rect", "scale"],
    };

    pub const JAVA: Spec = Spec {
        server: LiveServer::Jdtls,
        fixture: "java",
        language: "Java",
        calls: "src/main/java/fixture/Calls.java",
        shapes: "src/main/java/fixture/Shapes.java",
        errors: "src/main/java/fixture/Errors.java",
        messy: "src/main/java/fixture/Messy.java",
        // 정의 · 호출 셋(같은 패키지라 import 가 없다).
        references: 4,
        renamed: (1, 3),
        symbols: &["Shapes", "Shape", "Rect", "scale"],
    };

    pub const KOTLIN: Spec = Spec {
        server: LiveServer::KotlinLsp,
        fixture: "kotlin",
        language: "Kotlin",
        calls: "src/main/kotlin/fixture/Calls.kt",
        shapes: "src/main/kotlin/fixture/Shapes.kt",
        errors: "src/main/kotlin/fixture/Errors.kt",
        messy: "src/main/kotlin/fixture/Messy.kt",
        references: 4,
        renamed: (1, 3),
        symbols: &["Shape", "Rect", "scale"],
    };

    impl Spec {
        fn jvm(&self) -> bool {
            matches!(self.server, LiveServer::Jdtls | LiveServer::KotlinLsp)
        }

        /// 첫 답까지의 시한(계획 M4-18 — JVM 서버의 첫 진단 180 s, 나머지 60 s).
        pub fn deadline(&self) -> Duration {
            Duration::from_secs(if self.jvm() { 180 } else { 60 })
        }
    }

    /// 앵커 위치: (UTF-16 오프셋, 줄(0부터), 열(0부터, UTF-16)). 두 꼴을 읽는다 — `/*@이름*/`(표지 바로 뒤 글자)와 Python 의 줄 끝
    /// 주석 `# @이름 "앞글"`(그 줄의 코드에서 앞글 바로 뒤 글자 — 한 주석에 여럿 둘 수 있다).
    pub fn anchor(text: &str, name: &str) -> (u32, u32, u32) {
        let block = format!("/*@{name}*/");
        let at = if let Some(i) = text.find(&block) {
            i + block.len()
        } else {
            let tag = format!("@{name} \"");
            let mut start = 0;
            let mut found = None;
            for line in text.split_inclusive('\n') {
                if let Some(t) = line.find(&tag) {
                    let rest = &line[t + tag.len()..];
                    let before = &rest[..rest.find('"').expect("closing quote")];
                    let code = &line[..line.find(" # @").expect("anchor comment")];
                    let i = code
                        .find(before)
                        .unwrap_or_else(|| panic!("{before:?} is not on the line of {name}"));
                    found = Some(start + i + before.len());
                    break;
                }
                start += line.len();
            }
            found.unwrap_or_else(|| panic!("no anchor {name}"))
        };
        let before = &text[..at];
        let line = before.matches('\n').count() as u32;
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let column = before[line_start..].encode_utf16().count() as u32;
        (before.encode_utf16().count() as u32, line, column)
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    /// 서버를 Pitwall 의 관리자로 띄울 환경(픽스처 복사본 · 서버 경로). 문서는 아직 열지 않는다.
    pub fn env_for(spec: &Spec, options: EnvOptions) -> Env {
        let env = Env::new(options);
        let from = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(spec.fixture);
        copy_dir(&from, &env.root);
        env.configure(
            spec.server.server_id(),
            &spec.server.program(),
            &spec
                .server
                .args()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        );
        // Python 은 서버가 둘이다 — 시험하는 서버만 켠다(다른 하나가 PATH 에 있어 떠도 기능을 가로채지 않게).
        let other = match spec.server {
            LiveServer::Basedpyright => Some("ruff"),
            LiveServer::Ruff => Some("basedpyright"),
            _ => None,
        };
        if let Some(other) = other {
            let settings = env.paths.config.join("settings.toml");
            let mut text = std::fs::read_to_string(&settings).unwrap();
            text.push_str(&format!("[lsp.{other}]\nenabled = false\n"));
            std::fs::write(&settings, text).unwrap();
            env.global.reload().unwrap();
        }
        if spec.server == LiveServer::Jdtls {
            // 작업 공간 색인 폴더를 시험 폴더 안에(덮어쓰기 정의의 런처가 읽는다).
            let settings = env.paths.config.join("settings.toml");
            let mut text = std::fs::read_to_string(&settings).unwrap();
            text.push_str(&format!(
                "[lsp.jdtls.env]\nJDTLS_DATA = \"{}\"\n",
                env.base.join("jdtls-data").display()
            ));
            std::fs::write(&settings, text).unwrap();
            env.global.reload().unwrap();
        }
        env
    }

    pub struct Opened {
        pub env: Env,
        pub win: Conn,
        pub doc: String,
        pub path: PathBuf,
        pub text: String,
        /// 문서를 연 때(첫 진단까지 재는 기준).
        pub opened_at: Instant,
    }

    impl Opened {
        pub fn anchor(&self, name: &str) -> (u32, u32, u32) {
            anchor(&self.text, name)
        }
    }

    /// 픽스처의 `file` 을 창에서 열고 서버가 준비될 때까지 기다린다.
    pub async fn open(spec: &Spec, file: &str) -> Opened {
        let env = env_for(
            spec,
            EnvOptions {
                // 런타임(node · python · JDK)과 Gradle 을 찾도록 시험 프로세스의 환경을 넘긴다.
                isolated_shell: false,
                ..EnvOptions::default()
            },
        );
        open_in(spec, env, file).await
    }

    pub async fn open_in(spec: &Spec, env: Env, file: &str) -> Opened {
        let path = env.root.join(file);
        let text = std::fs::read_to_string(&path).unwrap();
        let mut win = env.window();
        let opened_at = Instant::now();
        let opened = win.open(&env, &path, spec.language, &text).await;
        let doc = opened["docId"].as_str().unwrap().to_owned();
        let server = spec.server.server_id();
        let ready = common::eventually(spec.deadline(), || {
            env.state_of(server) == Some(LspInstanceState::Ready)
        })
        .await;
        assert!(ready, "{server} is not ready: {:?}", env.states());
        Opened {
            env,
            win,
            doc,
            path,
            text,
            opened_at,
        }
    }

    /// 그 문서에서 서버가 이 기능을 알린다(`lsp.document.capabilities` — 정적 능력과 문서에 맞는 동적 등록, 기능 라우팅과 같은 판정).
    /// 알리지 않으면 실패한다 — 기대 능력 표에 있는 능력이다.
    pub async fn require(o: &mut Opened, spec: &Spec, feature: &str) {
        let server = spec.server.server_id();
        let deadline = Instant::now() + spec.deadline();
        loop {
            let got = o
                .win
                .call("lsp.document.capabilities", json!({ "docId": o.doc }))
                .await
                .unwrap();
            if got["features"][feature]["server"] == server {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{server} does not advertise `{feature}` (scripts/lsp-live.toml [capabilities]): {got}"
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// `method` 를 `done` 이 참일 때까지(서버가 색인하는 동안은 빈 답 · 오류) 다시 묻는다 — 시한은 서버별.
    pub async fn until(
        win: &mut Conn,
        spec: &Spec,
        method: &str,
        params: Value,
        done: impl Fn(&Value) -> bool,
    ) -> Value {
        let deadline = Instant::now() + spec.deadline();
        let mut last = Value::Null;
        while Instant::now() < deadline {
            match win.call(method, params.clone()).await {
                Ok(answer) if done(&answer) => return answer,
                Ok(answer) => last = answer,
                Err(e) => last = json!({ "error": e.message, "data": e.data }),
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!(
            "{}: {method} never answered as expected: {last}",
            spec.server.executable()
        );
    }

    /// 서버 프로세스(그룹 맨 앞)의 RSS(KiB).
    pub fn rss_kib(env: &Env, spec: &Spec) -> Option<u64> {
        let pid = *env
            .manager
            .pids_of(&env.id, Some(spec.server.server_id()))
            .first()?;
        // JVM 서버는 런처(셸 · python)가 exec 하므로 그 pid 가 JVM 이다. 자식까지 더한다(basedpyright 는 python → node).
        let out = std::process::Command::new("ps")
            .args(["-A", "-o", "pid=,ppid=,rss="])
            .output()
            .ok()?;
        let rows: Vec<(u32, u32, u64)> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace().map(|v| v.parse::<u64>().ok());
                Some((it.next()?? as u32, it.next()?? as u32, it.next()??))
            })
            .collect();
        let mut total = 0;
        let mut stack = vec![pid];
        while let Some(p) = stack.pop() {
            for (pid, ppid, rss) in &rows {
                if *pid == p {
                    total += rss;
                }
                if *ppid == p {
                    stack.push(*pid);
                }
            }
        }
        Some(total)
    }

    /// 진단 하나: 오류 파일을 열어 `anchor` 자리에서 시작하는 진단(심각도 `severity`, 코드 `code` — 주면)이 나올 때까지 진단 목록을
    /// 본다. 그 진단(LSP 꼴로)과 연 문서를 돌려준다.
    pub async fn diagnostic_at(
        spec: &Spec,
        anchor_name: &str,
        severity: &str,
        code: Option<&str>,
    ) -> (Opened, Value) {
        let mut o = open(spec, spec.errors).await;
        let (_, line, column) = o.anchor(anchor_name);
        let deadline = Instant::now() + spec.deadline();
        let found = loop {
            let list = o
                .win
                .call(
                    "lsp.diagnostics.list",
                    json!({ "projectId": o.env.id, "path": o.path, "limit": 1000 }),
                )
                .await
                .unwrap();
            let items = list["files"][0]["diagnostics"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let hit = items.iter().find(|d| {
                d["severity"] == severity
                    && d["server"] == spec.server.server_id()
                    && d["range"]["start"] == json!({ "line": line, "character": column })
                    && code.is_none_or(|c| d["code"] == c)
            });
            if let Some(d) = hit {
                break d.clone();
            }
            assert!(
                Instant::now() < deadline,
                "{}: no {severity} at {line}:{column} ({anchor_name}): {list}",
                spec.server.executable()
            );
            tokio::time::sleep(Duration::from_millis(300)).await;
        };
        eprintln!(
            "METRIC server={} first_diagnostic_ms={} rss_kib={:?}",
            spec.server.executable(),
            o.opened_at.elapsed().as_millis(),
            rss_kib(&o.env, spec)
        );
        (o, found)
    }

    /// 위 진단을 `textDocument/codeAction` 의 문맥(LSP `Diagnostic`)으로.
    pub fn as_lsp(d: &Value) -> Value {
        let severity = match d["severity"].as_str() {
            Some("error") => 1,
            Some("warning") => 2,
            Some("information") => 3,
            _ => 4,
        };
        let mut out = json!({ "range": d["range"], "severity": severity, "message": d["message"] });
        for key in ["code", "source", "data"] {
            if !d[key].is_null() {
                out[key] = d[key].clone();
            }
        }
        out
    }

    pub async fn diagnostics(spec: &Spec) {
        let (o, d) = diagnostic_at(spec, "error.type-mismatch", "error", None).await;
        eprintln!("{}: {d}", spec.server.executable());
        o.win.session.close();
    }

    /// 빠른 수정 하나 이상: 그 진단을 문맥으로 `quickfix` 만 묻는다.
    pub async fn code_action(spec: &Spec, anchor_name: &str, severity: &str, code: Option<&str>) {
        let (mut o, d) = diagnostic_at(spec, anchor_name, severity, code).await;
        require(&mut o, spec, "code_action").await;
        let (from, _, _) = o.anchor(anchor_name);
        let params = json!({ "docId": o.doc, "version": 0, "from": from, "to": from + 1,
                             "only": ["quickfix"],
                             "diagnostics": { "version": null, "items": [as_lsp(&d)] } });
        let answer = until(&mut o.win, spec, "lsp.codeAction.list", params, |a| {
            a["actions"].as_array().is_some_and(|l| {
                l.iter().any(|a| {
                    a["kind"]
                        .as_str()
                        .is_some_and(|k| k.starts_with("quickfix"))
                })
            })
        })
        .await;
        let titles: Vec<&str> = answer["actions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|a| a["title"].as_str())
            .collect();
        eprintln!("{}: quick fixes {titles:?}", spec.server.executable());
        o.win.session.close();
    }

    pub async fn hover(spec: &Spec) {
        let mut o = open(spec, spec.calls).await;
        require(&mut o, spec, "hover").await;
        let (offset, _, _) = o.anchor("call.scale-1");
        let answer = until(
            &mut o.win,
            spec,
            "lsp.hover",
            json!({ "docId": o.doc, "version": 0, "offset": offset }),
            |a| {
                a["hover"]["value"]
                    .as_str()
                    .is_some_and(|v| v.contains("scale"))
            },
        )
        .await;
        eprintln!("{}: hover {}", spec.server.executable(), answer["hover"]);
        o.win.session.close();
    }

    pub async fn definition(spec: &Spec) {
        let mut o = open(spec, spec.calls).await;
        require(&mut o, spec, "definition").await;
        let (offset, _, _) = o.anchor("call.scale-1");
        let shapes = std::fs::canonicalize(o.env.root.join(spec.shapes)).unwrap();
        let shapes_text = std::fs::read_to_string(&shapes).unwrap();
        let (_, line, column) = anchor(&shapes_text, "rename.scale");
        let answer = until(
            &mut o.win,
            spec,
            "lsp.definition",
            json!({ "docId": o.doc, "version": 0, "offset": offset }),
            |a| a["locations"].as_array().is_some_and(|l| !l.is_empty()),
        )
        .await;
        let first = &answer["locations"][0];
        assert_eq!(first["path"], shapes.display().to_string(), "{answer}");
        // 결과의 줄 · 열은 1 부터.
        assert_eq!(
            (first["line"].clone(), first["column"].clone()),
            (json!(line + 1), json!(column + 1)),
            "{answer}"
        );
        o.win.session.close();
    }

    pub async fn references(spec: &Spec) {
        let mut o = open(spec, spec.calls).await;
        require(&mut o, spec, "references").await;
        let (offset, _, _) = o.anchor("call.scale-1");
        let want = spec.references;
        let answer = until(
            &mut o.win,
            spec,
            "lsp.references",
            json!({ "docId": o.doc, "version": 0, "offset": offset, "includeDeclaration": true }),
            |a| a["locations"].as_array().is_some_and(|l| l.len() >= want),
        )
        .await;
        let files: BTreeMap<String, usize> =
            answer["locations"]
                .as_array()
                .unwrap()
                .iter()
                .fold(BTreeMap::new(), |mut acc, l| {
                    let name = Path::new(l["path"].as_str().unwrap())
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned();
                    *acc.entry(name).or_default() += 1;
                    acc
                });
        assert_eq!(
            files.values().sum::<usize>(),
            want,
            "{}: {answer}",
            spec.server.executable()
        );
        assert_eq!(files.len(), 2, "{files:?}");
        o.win.session.close();
    }

    pub async fn document_symbols(spec: &Spec) {
        let mut o = open(spec, spec.shapes).await;
        require(&mut o, spec, "document_symbol").await;
        let answer = until(
            &mut o.win,
            spec,
            "lsp.symbols.document",
            json!({ "docId": o.doc }),
            |a| {
                let names: Vec<&str> = a["symbols"]
                    .as_array()
                    .map(|s| s.iter().filter_map(|s| s["name"].as_str()).collect())
                    .unwrap_or_default();
                spec.symbols.iter().all(|want| {
                    names
                        .iter()
                        .any(|n| *n == *want || n.starts_with(&format!("{want}(")))
                })
            },
        )
        .await;
        assert_eq!(answer["server"], spec.server.server_id());
        o.win.session.close();
    }

    pub async fn workspace_symbols(spec: &Spec) {
        let mut o = open(spec, spec.calls).await;
        require(&mut o, spec, "workspace_symbol").await;
        let shapes = std::fs::canonicalize(o.env.root.join(spec.shapes)).unwrap();
        let id = o.env.id.clone();
        let answer = until(
            &mut o.win,
            spec,
            "lsp.workspaceSymbol",
            json!({ "projectId": id, "query": "Rect" }),
            |a| {
                a["symbols"].as_array().is_some_and(|s| {
                    s.iter().any(|s| {
                        s["name"] == "Rect" && s["location"]["path"] == shapes.display().to_string()
                    })
                })
            },
        )
        .await;
        eprintln!(
            "{}: {} symbol(s) for Rect",
            spec.server.executable(),
            answer["symbols"].as_array().unwrap().len()
        );
        o.win.session.close();
    }

    fn labels(list: &Value) -> Vec<String> {
        list["items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| i["label"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 라벨이 그 이름이다 — 서버가 꾸민 꼴(`width : int` · `area() : int` · `area(…)`)까지.
    fn names(label: &str, want: &str) -> bool {
        label
            .strip_prefix(want)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(['(', ' ', ':']))
    }

    pub async fn completion(spec: &Spec) {
        let mut o = open(spec, spec.calls).await;
        require(&mut o, spec, "completion").await;
        let (offset, _, _) = o.anchor("complete.rect-members");
        let params = json!({ "docId": o.doc, "version": 0, "offset": offset,
                              "trigger": { "kind": "character", "character": "." } });
        let want = ["area", "width", "height"];
        let list = until(&mut o.win, spec, "lsp.completion", params.clone(), |l| {
            let got = labels(l);
            want.iter().all(|w| got.iter().any(|g| names(g, w)))
        })
        .await;
        assert_eq!(list["staleVersion"], false);
        // 응답 시간(서버가 준비된 뒤 같은 자리 열 번 — Pitwall 왕복 포함).
        let mut times = Vec::new();
        for _ in 0..10 {
            let started = Instant::now();
            o.win.call("lsp.completion", params.clone()).await.unwrap();
            times.push(started.elapsed().as_millis());
        }
        times.sort_unstable();
        eprintln!(
            "METRIC server={} completion_p50_ms={} completion_max_ms={} rss_kib={:?}",
            spec.server.executable(),
            times[times.len() / 2],
            times[times.len() - 1],
            rss_kib(&o.env, spec)
        );
        o.win.session.close();
    }

    pub async fn signature(spec: &Spec) {
        let mut o = open(spec, spec.calls).await;
        require(&mut o, spec, "signature_help").await;
        let (offset, _, _) = o.anchor("signature.scale");
        let params = json!({ "docId": o.doc, "version": 0, "offset": offset,
                              "triggerKind": "invoked", "isRetrigger": false });
        let answer = until(&mut o.win, spec, "lsp.signatureHelp", params, |a| {
            !a["help"].is_null()
        })
        .await;
        let help = &answer["help"];
        let signature = &help["signatures"][help["activeSignature"].as_u64().unwrap_or(0) as usize];
        let label: Vec<u16> = signature["label"]
            .as_str()
            .unwrap()
            .encode_utf16()
            .collect();
        let active =
            &signature["parameters"][signature["activeParameter"].as_u64().unwrap_or(0) as usize];
        let (from, to) = (
            active["from"].as_u64().unwrap() as usize,
            active["to"].as_u64().unwrap() as usize,
        );
        let got = String::from_utf16(&label[from..to]).unwrap();
        assert!(got.contains("width"), "{got:?} in {}", signature["label"]);
        o.win.session.close();
    }

    /// LSP 위치(줄 · UTF-16) → 바이트 오프셋.
    pub fn offset(text: &str, line: u32, character: u32) -> usize {
        let mut start = 0;
        for _ in 0..line {
            match text[start..].find('\n') {
                Some(i) => start += i + 1,
                None => return text.len(),
            }
        }
        let mut units = 0;
        for (i, c) in text[start..].char_indices() {
            if units >= character as usize || c == '\n' {
                return start + i;
            }
            units += c.len_utf16();
        }
        text.len()
    }

    /// 편집을 넣은 글(뒤에서부터 — 웹뷰 적용기와 같은 규칙).
    pub fn apply(text: &str, edits: &[LspTextEdit]) -> String {
        let mut spans: Vec<(usize, usize, usize, &str)> = edits
            .iter()
            .enumerate()
            .map(|(i, e)| {
                (
                    offset(text, e.range.start.line, e.range.start.character),
                    offset(text, e.range.end.line, e.range.end.character),
                    i,
                    e.new_text.as_str(),
                )
            })
            .collect();
        spans.sort_by_key(|s| std::cmp::Reverse((s.0, s.2)));
        let mut out = text.to_owned();
        for (from, to, _, insert) in spans {
            out.replace_range(from..to, insert);
        }
        out
    }

    /// rename: 정의 자리에서 `scale` → `resize`. 편집을 파일에 넣은 뒤 파일마다 새 이름이 나오는 수가 기대값이다(서버가 낱말을
    /// 통째로 바꾸든 글자 차이만 주든 — kotlin-lsp 는 차이만 준다).
    pub async fn rename(spec: &Spec) {
        let mut o = open(spec, spec.shapes).await;
        require(&mut o, spec, "rename").await;
        let (offset, _, _) = o.anchor("rename.scale");
        let shapes = std::fs::canonicalize(o.env.root.join(spec.shapes)).unwrap();
        let calls = std::fs::canonicalize(o.env.root.join(spec.calls)).unwrap();
        let count = |text: &str| text.matches("resize").count();
        let deadline = Instant::now() + spec.deadline();
        loop {
            let answer = o
                .win
                .call(
                    "lsp.rename",
                    json!({ "docId": o.doc, "version": 0, "offset": offset, "newName": "resize" }),
                )
                .await;
            let last = if let Ok(answer) = &answer {
                let edit: LspWorkspaceEdit =
                    serde_json::from_value(answer["edit"].clone()).unwrap();
                let mut after = BTreeMap::new();
                for change in &edit.changes {
                    if let LspFileChange::Text { path, edits, .. } = change {
                        let path = std::fs::canonicalize(path).unwrap();
                        let text = std::fs::read_to_string(&path).unwrap();
                        after.insert(path, count(&apply(&text, edits)));
                    }
                }
                let got = (
                    after.get(&shapes).copied().unwrap_or(0),
                    after.get(&calls).copied().unwrap_or(0),
                );
                if got == spec.renamed && after.len() == 2 {
                    break;
                }
                format!("{got:?} from {edit:?}")
            } else {
                format!("{answer:?}")
            };
            assert!(
                Instant::now() < deadline,
                "{}: rename never gave {:?}: {last}\nserver log:\n{}",
                spec.server.executable(),
                spec.renamed,
                o.env.server_log_tail(spec.server.server_id(), 80)
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        o.win.session.close();
    }

    /// 포맷: 흐트러진 서식 파일을 공백 4칸으로 포맷한 편집을 넣은 글이 `expected` 이고, 그 글을 다시 포맷하면 바꿀 것이 없다.
    pub async fn format(spec: &Spec, formatter: LiveServer, expected: &str) {
        let mut o = open(spec, spec.messy).await;
        require(&mut o, spec, "formatting").await;
        let options = json!({ "tabSize": 4, "insertSpaces": true });
        let answer = until(
            &mut o.win,
            spec,
            "lsp.format",
            json!({ "docId": o.doc, "version": 0, "options": options }),
            |a| {
                a["staleVersion"] == false
                    && a["notReady"] != true
                    && a["edits"].as_array().is_some_and(|e| !e.is_empty())
            },
        )
        .await;
        assert_eq!(answer["server"], formatter.server_id(), "{answer}");
        let edits: Vec<LspTextEdit> = serde_json::from_value(answer["edits"].clone()).unwrap();
        let formatted = apply(&o.text, &edits);
        assert_eq!(formatted, expected, "{}", spec.server.executable());
        // 넣은 글을 다시 포맷하면 바꿀 것이 없다.
        let utf16 = o.text.encode_utf16().count();
        o.win
            .call(
                "lsp.document.change",
                json!({ "docId": o.doc, "version": 1,
                        "changes": [{ "from": 0, "to": utf16, "insert": formatted }] }),
            )
            .await
            .unwrap();
        let again = until(
            &mut o.win,
            spec,
            "lsp.format",
            json!({ "docId": o.doc, "version": 1, "options": options }),
            |a| a["staleVersion"] == false && a["notReady"] != true,
        )
        .await;
        let edits: Vec<LspTextEdit> = serde_json::from_value(again["edits"].clone()).unwrap();
        assert_eq!(apply(&formatted, &edits), formatted, "{again}");
        o.win.session.close();
    }
}

// ---- basedpyright(+ ruff) ---------------------------------------------------------------------------------------------------

#[cfg(pw_live_lsp_basedpyright)]
mod basedpyright {
    use super::live;
    use super::live::PYTHON as S;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_basedpyright_diagnostics() {
        live::diagnostics(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_basedpyright_hover() {
        live::hover(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_basedpyright_definition() {
        live::definition(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_basedpyright_references() {
        live::references(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_basedpyright_document_symbols() {
        live::document_symbols(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_basedpyright_workspace_symbols() {
        live::workspace_symbols(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_basedpyright_completion() {
        live::completion(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_basedpyright_signature() {
        live::signature(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_basedpyright_rename() {
        live::rename(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_basedpyright_code_action() {
        live::code_action(&S, "error.type-mismatch", "error", None).await;
    }
}

// ---- ruff(포맷 · 린트 — 정의 · 완성 · rename 이 없는 서버, 하한 3) ------------------------------------------------------------

#[cfg(pw_live_lsp_ruff)]
mod ruff {
    use pw_lsp::live::LiveServer;

    use super::live;

    const S: live::Spec = live::Spec {
        server: LiveServer::Ruff,
        ..live::PYTHON
    };

    /// ruff 의 포맷 결과(ruff 0.17.3 — `registry.toml` 의 고정 판).
    const FORMATTED: &str = "# 흐트러진 서식(포맷 — ruff).\n\
        def messy(a: int, b: int) -> int:  # @format.messy \"def   \"\n\
        \x20   c = a + b\n\
        \x20   if c > 10:\n\
        \x20       return c - 10\n\
        \x20   else:\n\
        \x20       return c\n";

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_ruff_diagnostics() {
        let (o, d) = live::diagnostic_at(&S, "unused.os", "warning", Some("F401")).await;
        eprintln!("ruff: {d}");
        o.win.session.close();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_ruff_format() {
        live::format(&S, LiveServer::Ruff, FORMATTED).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_ruff_code_action() {
        live::code_action(&S, "unused.os", "warning", Some("F401")).await;
    }
}

// ---- jdtls ------------------------------------------------------------------------------------------------------------------

#[cfg(pw_live_lsp_jdtls)]
mod jdtls {
    use pw_lsp::live::LiveServer;

    use super::live;
    use super::live::JAVA as S;

    /// jdtls 의 포맷 결과(1.63.0 — Eclipse 기본 서식, 공백 4칸. 블록 주석 안쪽에도 공백을 넣는다).
    const FORMATTED: &str = "// 흐트러진 서식(포맷).\n\
        package fixture;\n\
        \n\
        final class /* @format.messy */ Messy {\n\
        \x20   static int messy(int a, int b) {\n\
        \x20       int c = a + b;\n\
        \x20       if (c > 10) {\n\
        \x20           return c - 10;\n\
        \x20       } else {\n\
        \x20           return c;\n\
        \x20       }\n\
        \x20   }\n\
        }\n";

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_jdtls_diagnostics() {
        live::diagnostics(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_jdtls_hover() {
        live::hover(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_jdtls_definition() {
        live::definition(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_jdtls_references() {
        live::references(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_jdtls_document_symbols() {
        live::document_symbols(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_jdtls_workspace_symbols() {
        live::workspace_symbols(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_jdtls_completion() {
        live::completion(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_jdtls_signature() {
        live::signature(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_jdtls_rename() {
        live::rename(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_jdtls_format() {
        live::format(&S, LiveServer::Jdtls, FORMATTED).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_jdtls_code_action() {
        live::code_action(&S, "error.type-mismatch", "error", None).await;
    }

    /// 사람의 `[lsp.jdtls] settings` 가 jdtls 의 동작에 닿는다(15-m4-plan.md M4-18 — jdtls 는 설정을 `initialized` 뒤의
    /// `workspace/didChangeConfiguration` 으로 받는다): 시그니처 도움말을 사람 설정으로 끄면, 서버가 준비된 뒤(같은 자리의 hover 가
    /// 답한 뒤)에도 시그니처 도움말이 비어 있다 — 정의의 기본값(켬)으로는 답한다(`live_lsp_jdtls_signature`).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_jdtls_person_settings_reach_the_server() {
        use std::time::Duration;

        use serde_json::json;

        use super::common::env::EnvOptions;

        let mut options = EnvOptions {
            isolated_shell: false,
            ..EnvOptions::default()
        };
        options.manager.trace = true;
        let env = live::env_for(&S, options);
        let settings = env.paths.config.join("settings.toml");
        let mut text = std::fs::read_to_string(&settings).unwrap();
        text.push_str("[lsp.jdtls.settings.java.signatureHelp]\nenabled = false\n");
        std::fs::write(&settings, text).unwrap();
        env.global.reload().unwrap();
        let mut o = live::open_in(&S, env, S.calls).await;
        let (call, _, _) = o.anchor("call.scale-1");
        live::until(
            &mut o.win,
            &S,
            "lsp.hover",
            json!({ "docId": o.doc, "version": 0, "offset": call }),
            |a| {
                a["hover"]["value"]
                    .as_str()
                    .is_some_and(|v| v.contains("scale"))
            },
        )
        .await;
        let (offset, _, _) = o.anchor("signature.scale");
        for _ in 0..5 {
            let answer = o
                .win
                .call(
                    "lsp.signatureHelp",
                    json!({ "docId": o.doc, "version": 0, "offset": offset,
                            "triggerKind": "invoked", "isRetrigger": false }),
                )
                .await
                .unwrap();
            assert!(answer["help"].is_null(), "{answer}");
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        // 서버가 받은 설정(프로토콜 기록): 사람의 값이 `didChangeConfiguration` 으로 갔다.
        let logs = o.env.paths.state.join("logs/lsp");
        let mut log = String::new();
        for entry in std::fs::read_dir(&logs).unwrap().flatten() {
            log.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
        }
        let pushed = log
            .lines()
            .find(|l| l.contains("workspace/didChangeConfiguration"))
            .unwrap_or_else(|| panic!("no didChangeConfiguration in the protocol log"));
        assert!(
            pushed.contains("\"signatureHelp\":{\"enabled\":false}"),
            "{pushed}"
        );
        eprintln!(
            "jdtls asked workspace/configuration: {}",
            log.contains("\"method\":\"workspace/configuration\"")
        );
        o.win.session.close();
    }

    /// 서버를 돌릴 JDK 25 가 없으면(가짜 런타임 환경 — PATH 에 JDK 21 하나, mise 설치 없음) 띄우지 않고 `runtimeMissing { java,
    /// >=25 }` 이다(결정 M4-D6 — 안내 문구 「서버용 JDK >=25 가 필요하다 — `mise install java@25`」 는 이 값에서 나온다). macOS 는
    /// `/usr/libexec/java_home` 의 JDK 목록도 보므로 거기 25 이상이 있는 기기에서는 이 경우를 만들 수 없다 — 그렇다고 알리고 실패한다
    /// (건너뛰어 통과로 세지 않는다).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_jdtls_runtime_missing_without_a_jdk_25() {
        use pw_lsp::methods::LspInstanceState;

        use super::common::env::{EnvOptions, executable};

        if cfg!(target_os = "macos") {
            let list = std::process::Command::new("/usr/libexec/java_home")
                .arg("-V")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stderr).into_owned())
                .unwrap_or_default();
            let has_25 = list.lines().any(|l| {
                l.trim()
                    .split(['.', ' '])
                    .next()
                    .and_then(|v| v.parse::<u32>().ok())
                    .is_some_and(|major| major >= 25)
            });
            assert!(
                !has_25,
                "this Mac registers a JDK >= 25 with java_home, so the server runtime is always found here — \
                 the test runs on the Linux CI job: {list}"
            );
        }
        let env = live::env_for(
            &S,
            EnvOptions {
                isolated_shell: true,
                ..EnvOptions::default()
            },
        );
        executable(
            &env.bin.join("java"),
            "echo 'openjdk version \"21.0.5\" 2024-10-15' >&2",
        );
        let path = env.root.join(S.calls);
        let text = std::fs::read_to_string(&path).unwrap();
        let mut win = env.window();
        win.open(&env, &path, S.language, &text).await;
        env.wait_state(
            "jdtls",
            LspInstanceState::RuntimeMissing {
                tool: "java".into(),
                needed: ">=25".into(),
            },
        )
        .await;
        assert_eq!(env.manager.spawn_count(), 0);
        win.session.close();
    }

    /// 프로젝트 런타임이 JDK 17(`PW_TEST_JDK17` — 프로젝트 런타임 환경의 `JAVA_HOME`)이고 기기에 25 가 있으면 서버는 25 로 뜨고
    /// 프로젝트는 17 이다(결정 M4-D6 · 15-m4-plan.md M4-18): `java.configuration.runtimes` 로 JavaSE-17 이 넘어가고(서버 로그의
    /// initialize), 17 언어 수준에서 레코드 패턴(21)이 오류이고, JDK 17 라이브러리에서 `List.reversed()`(21 의 SequencedCollection)가
    /// 없다는 오류가 난다 — 둘째는 서버 JDK(25)가 아니라 넘긴 17 로 빌드할 때만 난다.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_jdtls_project_jdk_17_with_a_server_jdk_25() {
        use std::time::{Duration, Instant};

        use serde_json::json;

        use super::common::env::EnvOptions;

        let jdk17 = std::env::var_os("PW_TEST_JDK17")
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from)
            .expect("PW_TEST_JDK17 is not set — give it a JDK 17 home (CI: mise where java@17)");
        let jdk17 = std::fs::canonicalize(&jdk17).unwrap();
        assert!(jdk17.join("bin/java").is_file(), "{}", jdk17.display());
        let mut options = EnvOptions {
            isolated_shell: false,
            shell_prelude: Some(format!("JAVA_HOME='{}'; export JAVA_HOME", jdk17.display())),
            ..EnvOptions::default()
        };
        options.manager.trace = true;
        let env = live::env_for(&S, options);
        // 이 시험 몫의 임시 파일(픽스처 규칙 — 복사본에만).
        let file = "src/main/java/fixture/M418Level.java";
        std::fs::write(
            env.root.join(file),
            "package fixture;\n\nimport java.util.List;\n\n\
             final class M418Level {\n\
             \x20   private M418Level() {}\n\n\
             \x20   record Point(int x) {}\n\n\
             \x20   static Object api() {\n\
             \x20       return List.of(1, 2)./*@level.api*/reversed();\n\
             \x20   }\n\n\
             \x20   static int pattern(Object o) {\n\
             \x20       if (o instanceof /*@level.pattern*/Point(int x)) {\n\
             \x20           return x;\n\
             \x20       }\n\
             \x20       return 0;\n\
             \x20   }\n\
             }\n",
        )
        .unwrap();
        let mut o = live::open_in(&S, env, file).await;
        let lines = |name: &str| o.anchor(name).1;
        let (api, pattern) = (lines("level.api"), lines("level.pattern"));
        let deadline = Instant::now() + S.deadline();
        let errors = loop {
            let list = o
                .win
                .call(
                    "lsp.diagnostics.list",
                    json!({ "projectId": o.env.id, "path": o.path, "limit": 1000 }),
                )
                .await
                .unwrap();
            let errors: Vec<serde_json::Value> = list["files"][0]["diagnostics"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|d| d["severity"] == "error")
                .collect();
            let on = |line: u32| errors.iter().any(|d| d["range"]["start"]["line"] == line);
            if on(api) && on(pattern) {
                break errors;
            }
            assert!(
                Instant::now() < deadline,
                "no errors on lines {api} (List.reversed) and {pattern} (record pattern): {list}"
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        };
        eprintln!("jdtls at level 17: {errors:#?}");
        // 서버 프로세스는 프로젝트의 17 이 아니라 서버용 JDK(25 이상)의 java 다(런처가 그 `JAVA_HOME` 의 java 를 exec 한다).
        let pid = o.env.manager.pids_of(&o.env.id, Some("jdtls"))[0];
        let command = std::process::Command::new("ps")
            .args(["-o", "command=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let command = String::from_utf8_lossy(&command.stdout).into_owned();
        let java = command
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned();
        assert!(java.ends_with("/bin/java"), "{command}");
        assert!(!java.starts_with(&*jdk17.to_string_lossy()), "{command}");
        let version = std::process::Command::new(&java)
            .arg("-version")
            .output()
            .unwrap();
        let version = String::from_utf8_lossy(&version.stderr).into_owned();
        let major: u32 = version
            .split('"')
            .nth(1)
            .and_then(|v| v.split('.').next())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        assert!(major >= 25, "the server runs on {java}: {version}");
        // initialize 에 JavaSE-17 이 그 JDK 로 갔다(프로토콜 기록).
        let logs = o.env.paths.state.join("logs/lsp");
        let mut log = String::new();
        for entry in std::fs::read_dir(&logs).unwrap().flatten() {
            log.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
        }
        let runtimes = format!(
            "\"runtimes\":[{{\"default\":true,\"name\":\"JavaSE-17\",\"path\":\"{}\"}}]",
            jdk17.display()
        );
        assert!(
            log.contains(&runtimes),
            "{runtimes} is not in the protocol log"
        );
        o.win.session.close();
    }
}

// ---- kotlin-lsp -------------------------------------------------------------------------------------------------------------

#[cfg(pw_live_lsp_kotlin_lsp)]
mod kotlin_lsp {
    use pw_lsp::live::LiveServer;

    use super::live;
    use super::live::KOTLIN as S;

    /// kotlin-lsp 의 포맷 결과(v263.7084.0 — IntelliJ 의 Kotlin 공식 서식).
    const FORMATTED: &str = "// 흐트러진 서식(포맷).\n\
        package fixture\n\
        \n\
        fun /*@format.messy*/messy(a: Int, b: Int): Int {\n\
        \x20   val c = a + b\n\
        \x20   if (c > 10) {\n\
        \x20       return c - 10\n\
        \x20   } else {\n\
        \x20       return c\n\
        \x20   }\n\
        }\n";

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_kotlin_lsp_diagnostics() {
        live::diagnostics(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_kotlin_lsp_hover() {
        live::hover(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_kotlin_lsp_definition() {
        live::definition(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_kotlin_lsp_references() {
        live::references(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_kotlin_lsp_document_symbols() {
        live::document_symbols(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_kotlin_lsp_workspace_symbols() {
        live::workspace_symbols(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_kotlin_lsp_completion() {
        live::completion(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_kotlin_lsp_signature() {
        live::signature(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_kotlin_lsp_rename() {
        live::rename(&S).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_kotlin_lsp_format() {
        live::format(&S, LiveServer::KotlinLsp, FORMATTED).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn live_lsp_kotlin_lsp_code_action() {
        live::code_action(&S, "error.type-mismatch", "error", None).await;
    }
}
