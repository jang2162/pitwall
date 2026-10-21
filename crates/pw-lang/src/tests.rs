use std::fs;
use std::path::Path;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
use pw_core::config::{AppPaths, ConfigSource, ConfigStore};
use pw_core::registry::{NewProject, Registry as Projects};
use serde_json::json;

use super::*;
use crate::methods::{HostParts, Lang, LangDetectFile, LangDetectParams, LangInfo, MAX_FILES};

fn plain(path: &str, head: Option<&str>) -> Detection {
    detect(
        Input {
            repo_path: path,
            project_path: path,
            head,
        },
        &Attrs::default(),
        &Mapping::default(),
        &Contributed::default(),
    )
}

fn lang(path: &str, head: Option<&str>) -> Option<String> {
    plain(path, head).language
}

// ── Linguist 단계 ──────────────────────────────────────────────

#[test]
fn env_shebang_picks_the_interpreter() {
    let d = plain("bin/tool", Some("#!/usr/bin/env python3\nprint('hi')\n"));
    assert_eq!(d.language.as_deref(), Some("Python"));
    assert_eq!(d.source, LangSource::Shebang);
    assert_eq!(
        lang("run", Some("#!/usr/bin/env -S node --no-warnings\n")).as_deref(),
        Some("JavaScript")
    );
    assert_eq!(
        lang("x", Some("#!/usr/bin/python2.7\n")).as_deref(),
        Some("Python")
    );
    assert_eq!(lang("x", Some("#! /bin/bash\n")).as_deref(), Some("Shell"));
}

/// Linguist 순서: shebang 이 확장자보다 먼저다(`lib/linguist.rb` 의 STRATEGIES).
#[test]
fn shebang_wins_over_the_extension() {
    let d = plain("script.rb", Some("#!/bin/bash\necho hi\n"));
    assert_eq!(d.language.as_deref(), Some("Shell"));
    assert_eq!(d.source, LangSource::Shebang);
}

#[test]
fn sh_exec_trick_names_the_real_interpreter() {
    let head = "#!/bin/sh\nexec scala \"$0\" \"$@\"\n!#\nobject X\n";
    assert_eq!(lang("x", Some(head)).as_deref(), Some("Scala"));
}

#[test]
fn dot_h_heuristics_pick_c_cpp_and_objective_c() {
    let objc = "#import <Foundation/Foundation.h>\n@interface Foo : NSObject\n@end\n";
    let cpp = "#include <string>\nnamespace app {\nclass Widget {\n  std::string name;\n};\n}\n";
    let c = "#include <stdio.h>\nint add(int a, int b);\n";
    for (head, want, source) in [
        (objc, "Objective-C", LangSource::Heuristics),
        (cpp, "C++", LangSource::Heuristics),
        // `.h` 의 마지막 규칙은 패턴 없는 C 다.
        (c, "C", LangSource::Heuristics),
    ] {
        let d = plain("src/a.h", Some(head));
        assert_eq!(d.language.as_deref(), Some(want), "{head}");
        assert_eq!(d.source, source, "{head}");
    }
}

#[test]
fn vim_modelines() {
    for head in [
        "# vim: set ft=ruby:\nputs 1\n",
        "x = 1\n# vim: ts=4 sw=4 filetype=python\n",
        "/* vi: ft=perl */\n",
        "# ex: syntax=ruby\n",
        "# vim:noai:ft=ruby:sw=2\n",
    ] {
        let d = plain("notes.txt", Some(head));
        assert_eq!(d.source, LangSource::Modeline, "{head:?}");
        assert!(
            matches!(d.language.as_deref(), Some("Ruby" | "Python" | "Perl")),
            "{head:?} → {:?}",
            d.language
        );
    }
    assert_eq!(
        lang("notes.txt", Some("# vim: ts=4 sw=4 filetype=python\n")).as_deref(),
        Some("Python")
    );
    // 모드라인이 아니면 `.txt` 는 heuristics 의 마지막 규칙(Text)으로 간다.
    // 「set」 꼴은 콜론으로 끝나야 한다.
    assert_eq!(
        lang("a.txt", Some("# vim: set ft=ruby\n")).as_deref(),
        Some("Text")
    );
    // `lex:` 의 `ex:` 는 모드라인이 아니다.
    assert_eq!(
        lang("a.txt", Some("lex: ft=ruby\n")).as_deref(),
        Some("Text")
    );
}

#[test]
fn emacs_modelines() {
    for (head, want) in [
        ("# -*- ruby -*-\n", "Ruby"),
        (";; -*- mode: lisp; coding: utf-8 -*-\n", "Common Lisp"),
        ("/* -*-mode:C++-*- */\n", "C++"),
        ("# -*- coding: utf-8; Mode: python -*-\n", "Python"),
    ] {
        let d = plain("foo.txt", Some(head));
        assert_eq!(d.language.as_deref(), Some(want), "{head:?}");
        assert_eq!(d.source, LangSource::Modeline, "{head:?}");
    }
    // `mode:` 앞이 공백 · `;` 가 아니면 변수 이름의 일부다. 이름이 `-` 로 끝나며 닫는 표시에 붙으면 무효다.
    assert_eq!(
        lang("a.txt", Some("# -*- foomode: ruby -*-\n")).as_deref(),
        Some("Text")
    );
    assert_eq!(
        lang("a.txt", Some("# -*- ruby--*-\n")).as_deref(),
        Some("Text")
    );
    // `F*` 는 아는 별칭이라 이 규칙만이 가른다 — 띄우면 맞고, 닫는 표시에 붙이면(`F*-*-`) 무효다.
    assert_eq!(
        lang("a.txt", Some("// -*- F* -*-\n")).as_deref(),
        Some("F*")
    );
    assert_eq!(
        lang("a.txt", Some("// -*- F*-*-\n")).as_deref(),
        Some("Text")
    );
    // 끝 5줄도 본다.
    let tail = format!("{}# -*- ruby -*-\n", "x\n".repeat(20));
    assert_eq!(lang("a.txt", Some(&tail)).as_deref(), Some("Ruby"));
}

/// 조작된 긴 한 줄(50 KiB)도 한 파일 판별이 시간 안에 끝난다. 모드라인 파서가 줄 길이에 대해 2제곱이면 debug 에서 수십 초다.
/// 상한은 느린 CI 를 위해 넉넉히 잡았다(이 기기의 debug 실측은 줄마다 10 ms 아래).
#[test]
fn long_single_lines_detect_in_time() {
    for unit in [
        " ex: ft=a,",
        " vi: ft=a,",
        " vim:",
        "vim:ft=a ",
        "-*- ",
        "-*- mode: ",
    ] {
        let line = unit.repeat(50 * 1024 / unit.len());
        let started = std::time::Instant::now();
        let _ = plain("a.txt", Some(&line));
        let took = started.elapsed();
        assert!(
            took < std::time::Duration::from_millis(500),
            "{unit:?}: {took:?}"
        );
    }
    // 상한 안의 평범한 모드라인은 그대로 맞는다.
    assert_eq!(
        lang("a.txt", Some("x = 1 # vim: set ts=4 sw=4 ft=python:\n")).as_deref(),
        Some("Python")
    );
}

#[test]
fn filenames_extensions_and_generic_extensions() {
    assert_eq!(plain("Makefile", None).source, LangSource::Filename);
    assert_eq!(lang("Makefile", None).as_deref(), Some("Makefile"));
    assert_eq!(
        lang("app/views/x.html.erb", None).as_deref(),
        Some("HTML+ERB")
    );
    // 내용이 없으면 `.rs` heuristics 규칙이 하나도 맞지 않아 후보(RenderScript · Rust)에서 popular 인 Rust 를 고른다.
    assert_eq!(lang("SRC/MAIN.RS", None).as_deref(), Some("Rust"));
    assert_eq!(plain("main.rs", None).source, LangSource::Ambiguous);
    assert_eq!(
        plain("main.rs", Some("fn main() {}\n")).source,
        LangSource::Heuristics
    );
    assert_eq!(lang("README", None), None);
    // `.alg` 는 generic(흔한 확장자)이다 — 후보가 ALGOL 하나뿐이어도 확장자만으로 정하지 않고, heuristics 가 맞아야 한다.
    assert_eq!(plain("prog.alg", None).source, LangSource::None);
    let d = plain("prog.alg", Some("comment hello;\nbegin end\n"));
    assert_eq!(
        (d.language.as_deref(), d.source),
        (Some("ALGOL"), LangSource::Heuristics)
    );
}

/// 앞부분이 없거나 빈 파일(새 파일 · 앞부분 없이 부르는 파일 트리)도 흔한 확장자는 대표 언어다 — heuristics 를 빈 내용으로
/// 돌려 묶음의 기본 규칙이 고른다.
#[test]
fn empty_or_missing_heads_pick_the_usual_language() {
    for head in [None, Some("")] {
        for (path, want) in [
            ("README.md", "Markdown"),
            ("config/a.yaml", "YAML"),
            ("pnpm-lock.yaml", "YAML"),
            ("notes.txt", "Text"),
            ("include/a.h", "C"),
            ("src/main.rs", "Rust"),
        ] {
            assert_eq!(lang(path, head).as_deref(), Some(want), "{path} {head:?}");
        }
    }
}

#[test]
fn xml_header_names_unknown_extensions() {
    let d = plain("thing.unknownext", Some("<?xml version=\"1.0\"?>\n<a/>\n"));
    assert_eq!(d.language.as_deref(), Some("XML"));
    assert_eq!(d.source, LangSource::Xml);
}

/// 빌드 스크립트가 옮기지 못한 heuristics 정규식. Linguist 데이터를 올리면 이 목록이 바뀔 수 있다 — 바뀌면 이유를 보고
/// 목록을 고친다(`CONTRIBUTING.md` 「Linguist 데이터」 절).
#[test]
fn skipped_heuristics_are_exactly_these() {
    let skipped: Vec<(&str, &str)> = SKIPPED_PATTERNS
        .iter()
        .filter(|s| !s.pattern.is_empty())
        .map(|s| (s.context, s.reason))
        .collect();
    assert_eq!(
        skipped,
        [
            (".tpl rule 2 (Smarty)", "lookbehind"),
            (".txt rule 1 (Vim Help File)", "lookahead"),
            (".txt rule 2 (Hosts File)", "lookahead"),
            (".txt rule 3 (Adblock Filter List)", "possessive quantifier"),
        ]
    );
    assert!(SKIPPED_PATTERNS.iter().all(|s| s.file == "heuristics.yml"));
}

#[test]
fn every_table_language_resolves_by_its_name() {
    for (i, l) in linguist::languages().iter().enumerate() {
        assert_eq!(linguist::find_by_name(l.name), Some(i as u16), "{}", l.name);
        assert_eq!(
            linguist::find_by_alias(l.aliases[0]),
            Some(i as u16),
            "{}",
            l.name
        );
    }
}

// ── 루비 정규식 옮기기 ────────────────────────────────────────

#[test]
fn ruby_regex_translation() {
    use crate::ruby_regex::translate;
    assert_eq!(translate(r"^\h+$").unwrap(), r"(?m)^[0-9a-fA-F]+$");
    assert_eq!(translate(r"[\h_]").unwrap(), r"(?m)[0-9a-fA-F_]");
    // Ruby 의 m 은 dotall 이다.
    assert_eq!(translate(r"(?m:a.b)").unwrap(), r"(?m)(?s:a.b)");
    assert_eq!(translate(r"(?im)x").unwrap(), r"(?m)(?is)x");
    assert_eq!(translate(r"a{,3}").unwrap(), r"(?m)a{0,3}");
    assert_eq!(translate(r"x{ y}").unwrap(), r"(?m)x\{ y\}");
    assert_eq!(translate(r"\<tag\>").unwrap(), r"(?m)<tag>");
    assert_eq!(translate(r"end\Z").unwrap(), r"(?m)end(?:\n?\z)");
    assert_eq!(translate(r"[[:alpha:]]+").unwrap(), r"(?m)[[:alpha:]]+");
    for (bad, why) in [
        (r"(?<!x)y", "lookbehind"),
        (r"a(?=b)", "lookahead"),
        (r"(?>ab)", "atomic group"),
        (r"a++", "possessive quantifier"),
        (r"(a)\1", "backreference `\\1`"),
    ] {
        assert_eq!(translate(bad).unwrap_err(), why, "{bad}");
    }
    // `\w` · `\s` · `\d` · `\b` 는 Ruby 처럼 ASCII 다 — `é` 는 단어 글자가 아니다.
    assert_eq!(
        translate(r"\w+\s\d").unwrap(),
        r"(?m)[[:word:]]+[[:space:]][[:digit:]]"
    );
    assert_eq!(
        translate(r"[^\s@#\w-]").unwrap(),
        r"(?m)[^[:space:]@\#[:word:]-]"
    );
    assert_eq!(translate(r"\bx\B").unwrap(), r"(?m)(?-u:\b)x(?-u:\B)");
    let bytes = |p: &str| regex::bytes::Regex::new(&translate(p).unwrap()).unwrap();
    assert!(!bytes(r"^\w+$").is_match("é".as_bytes()));
    assert!(bytes(r"^\s*[^\s@#\w-]").is_match("é".as_bytes()));
    assert!(bytes(r"^\W$").is_match("é".as_bytes()));
    // 문자 클래스 안의 공백 · `#` 은 x 모드에서도 글자다(Ruby). Rust x 모드는 지우므로 이스케이프한다.
    assert_eq!(
        translate("(?x) a [ #] b").unwrap(),
        r"(?m)(?x) a [\x20\#] b"
    );
    assert!(bytes("(?x) a [ #] b").is_match(b"a b"));
    assert!(bytes("(?x) a [ #] b").is_match(b"a#b"));
    // `Regexp.union` 처럼 묶으면 한쪽의 인라인 플래그가 다른 쪽으로 새지 않는다.
    let joined = crate::ruby_regex::union([
        translate("(?i)abc").unwrap().as_str(),
        translate("XYZ").unwrap().as_str(),
    ]);
    let re = regex::Regex::new(&joined).unwrap();
    assert!(re.is_match("ABC") && re.is_match("XYZ") && !re.is_match("xyz"));
    // 옮긴 것은 regex 로 컴파일되고 Ruby 와 같이 맞는다.
    let re = regex::Regex::new(&translate(r"^\s*#import").unwrap()).unwrap();
    assert!(re.is_match("x\n  #import <a>"));
}

// ── 벤더 · 생성 · 문서 ──────────────────────────────────────

#[test]
fn vendored_documentation_and_generated_paths() {
    assert!(plain("node_modules/react/index.js", None).vendored);
    assert!(plain("third_party/zlib/zlib.h", None).vendored);
    assert!(!plain("src/main.rs", None).vendored);
    assert!(plain("docs/guide.md", None).documentation);
    assert!(plain("CHANGELOG.md", None).documentation);
    assert!(!plain("src/docsy.rs", None).documentation);
    for path in [
        "Cargo.lock",
        "web/package-lock.json",
        "pnpm-lock.yaml",
        "a/__generated__/q.ts",
    ] {
        assert!(plain(path, None).generated, "{path}");
    }
    assert!(!plain("src/lib.rs", None).generated);
}

#[test]
fn generated_first_line_rules() {
    assert_eq!(
        generated::generated(
            "api/x.pb.go",
            Some("// Code generated by protoc-gen-go. DO NOT EDIT.\npackage x\n")
        ),
        Some("generated_go?")
    );
    let minified = format!("{}\n", "a".repeat(400));
    assert_eq!(
        generated::generated("app.min.js", Some(&minified)),
        Some("minified_files?")
    );
    assert_eq!(
        generated::generated("app.js", Some("const a = 1;\nconst b = 2;\n")),
        None
    );
    assert_eq!(
        generated::generated("x.go", Some("package x\n// Code generated later\n")),
        Some("generated_go?")
    );
    assert_eq!(
        generated::generated("x.go", Some("package x\nfunc f() {}\n")),
        None
    );
    assert_eq!(generated::extname(".bashrc"), "");
    assert_eq!(generated::extname("a/b.tar.gz"), ".gz");
}

// ── .gitattributes ─────────────────────────────────────────

fn repo_with(attrs: &[(&str, &str)]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join(".git/info")).unwrap();
    for (rel, text) in attrs {
        let p = tmp.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
    tmp
}

fn with_attrs(top: &Path, rel: &str, head: Option<&str>) -> Detection {
    let dir = rel.rsplit_once('/').map_or("", |(d, _)| d);
    let repo = attributes::RepoAttributes::load(top, Some(&top.join(".git")), [dir]);
    detect(
        Input {
            repo_path: rel,
            project_path: rel,
            head,
        },
        &repo.get(rel),
        &Mapping::default(),
        &Contributed::default(),
    )
}

#[test]
fn gitattributes_language_beats_the_extension() {
    let tmp = repo_with(&[(".gitattributes", "*.rb linguist-language=Python\n")]);
    let d = with_attrs(tmp.path(), "lib/a.rb", Some("#!/bin/bash\n"));
    assert_eq!(d.language.as_deref(), Some("Python"));
    assert_eq!(d.source, LangSource::Gitattributes);
    // 별칭으로도 찾는다(Linguist 의 `find_by_alias`). 모르는 이름이면 Linguist 판별로 넘어간다.
    let tmp = repo_with(&[(
        ".gitattributes",
        "*.inc linguist-language=cpp\n*.rs linguist-language=nope\n",
    )]);
    assert_eq!(
        with_attrs(tmp.path(), "a.inc", None).language.as_deref(),
        Some("C++")
    );
    assert_eq!(
        with_attrs(tmp.path(), "a.rs", None).language.as_deref(),
        Some("Rust")
    );
    assert_ne!(
        with_attrs(tmp.path(), "a.rs", None).source,
        LangSource::Gitattributes
    );
}

#[test]
fn gitattributes_vendored_generated_documentation_override_the_rules() {
    let tmp = repo_with(&[
        (
            ".gitattributes",
            "src/gen/** linguist-generated\nnode_modules/** -linguist-vendored\nlib/ext/* linguist-vendored\n",
        ),
        ("docs/.gitattributes", "*.md linguist-documentation=false\n"),
    ]);
    let top = tmp.path();
    let gen_file = with_attrs(top, "src/gen/api.rs", None);
    assert!(gen_file.generated);
    assert!(!with_attrs(top, "src/api.rs", None).generated);
    assert!(!with_attrs(top, "node_modules/x/index.js", None).vendored);
    assert!(with_attrs(top, "lib/ext/a.c", None).vendored);
    assert!(!with_attrs(top, "docs/guide.md", None).documentation);
    // `linguist-generated=false` 는 경로 규칙(Cargo.lock)을 끈다.
    let tmp = repo_with(&[(".gitattributes", "Cargo.lock linguist-generated=false\n")]);
    assert!(!with_attrs(tmp.path(), "Cargo.lock", None).generated);
}

#[test]
fn deeper_gitattributes_and_info_attributes_take_precedence() {
    let tmp = repo_with(&[
        (
            ".gitattributes",
            "*.x linguist-language=Ruby\n*.y linguist-language=Ruby\n",
        ),
        ("sub/.gitattributes", "*.x linguist-language=Python\n"),
        (".git/info/attributes", "*.y linguist-language=Perl\n"),
    ]);
    assert_eq!(
        with_attrs(tmp.path(), "sub/a.x", None).language.as_deref(),
        Some("Python")
    );
    assert_eq!(
        with_attrs(tmp.path(), "a.x", None).language.as_deref(),
        Some("Ruby")
    );
    assert_eq!(
        with_attrs(tmp.path(), "sub/a.y", None).language.as_deref(),
        Some("Perl")
    );
}

/// 시간 안에 끝나지 않으면 실패한다(FIFO 에서 영구히 막히는 회귀가 시험 전체를 멈추지 않게 스레드에서 돈다).
#[cfg(unix)]
fn within<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(std::time::Duration::from_secs(secs))
        .expect("did not finish in time")
}

#[cfg(unix)]
fn mkfifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0, "mkfifo");
}

/// git 처럼 `.gitattributes` 심링크를 따라가지 않는다 — 루트 밖 파일을 읽지 않는다(결정 M1-D8).
#[cfg(unix)]
#[test]
fn gitattributes_symlinks_are_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tmp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(
        outside.join("attrs"),
        "*.txt linguist-language=Rust linguist-vendored\n",
    )
    .unwrap();
    let repo = tmp.path().join("repo");
    fs::create_dir_all(repo.join(".git/info")).unwrap();
    std::os::unix::fs::symlink("../outside/attrs", repo.join(".gitattributes")).unwrap();
    std::os::unix::fs::symlink("../../../outside/attrs", repo.join(".git/info/attributes"))
        .unwrap();
    let d = with_attrs(&repo, "a.txt", None);
    assert_ne!(d.source, LangSource::Gitattributes);
    assert!(!d.vendored);
}

/// FIFO · 장치 파일은 읽지 않는다 — 읽으면 영구히 막히거나 끝이 없다. `.git` 이 FIFO 여도 막히지 않는다.
#[cfg(unix)]
#[test]
fn gitattributes_fifos_and_devices_return_in_time() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    fs::create_dir_all(repo.join(".git/info")).unwrap();
    mkfifo(&repo.join(".gitattributes"));
    mkfifo(&repo.join(".git/info/attributes"));
    let attrs = within(10, move || {
        attributes::RepoAttributes::load(&repo, Some(&repo.join(".git")), [""]).get("a.txt")
    });
    assert_eq!(attrs, Attrs::default());
    // 장치 파일: 일반 파일이 아니라 읽지 않는다.
    assert!(attributes::read_small(Path::new("/dev/zero"), 1024).is_err());
    // 비어 보이는 장치(`/dev/null`)도 일반 파일이 아니라 읽지 않는다.
    assert!(attributes::read_small(Path::new("/dev/null"), 1024).is_err());

    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    mkfifo(&repo.join(".git"));
    let found = within(10, move || attributes::find_repo(&repo));
    assert_eq!(found.map(|r| r.gitdir), Some(None));

    // 연결 워크트리의 `.git` 이 가리키는 `commondir` 이 FIFO 여도 막히지 않는다.
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    fs::create_dir_all(repo.join("admin")).unwrap();
    fs::write(repo.join(".git"), "gitdir: admin\n").unwrap();
    mkfifo(&repo.join("admin/commondir"));
    let admin = repo.join("admin");
    let found = within(10, move || attributes::find_repo(&repo));
    assert_eq!(found.map(|r| r.gitdir), Some(Some(admin)));
}

#[test]
fn oversized_gitattributes_are_ignored() {
    let big = format!(
        "*.txt linguist-language=Rust\n{}",
        // 크기는 글자로 둔다 — 상한 상수로 계산하면 상수를 키워도 파일이 같이 커져 시험이 지키지 못한다.
        "# padding\n".repeat(110_000)
    );
    assert!(big.len() as u64 > attributes::MAX_ATTRIBUTES_BYTES);
    let tmp = repo_with(&[(".gitattributes", big.as_str())]);
    assert_ne!(
        with_attrs(tmp.path(), "a.txt", None).source,
        LangSource::Gitattributes
    );
    // 상한 안이면 읽는다.
    let tmp = repo_with(&[(".gitattributes", "*.txt linguist-language=Rust\n")]);
    assert_eq!(
        with_attrs(tmp.path(), "a.txt", None).source,
        LangSource::Gitattributes
    );
}

// ── 설정 매핑 · 확장 기여 ────────────────────────────────────

#[test]
fn mapping_beats_gitattributes_and_project_beats_global() {
    let contributed = Contributed::default();
    let mut warnings = Vec::new();
    let mapping = Mapping::from_settings(
        Some(&json!({ "*.conf": "nginx" })),
        Some(&json!({ "*.conf": "INI", "etc/*.conf": "Shell", "*.zzz": "No Such", "*.n": 3 })),
        &contributed,
        &mut warnings,
    );
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert_eq!(mapping.get("site.conf"), Some("Nginx"));
    // 프로젝트 설정이 이긴다 — 전역의 더 긴 패턴보다도.
    assert_eq!(mapping.get("etc/site.conf"), Some("Nginx"));
    let attrs = Attrs {
        language: Some("Ruby".into()),
        ..Attrs::default()
    };
    let d = detect(
        Input {
            repo_path: "site.conf",
            project_path: "site.conf",
            head: None,
        },
        &attrs,
        &mapping,
        &contributed,
    );
    assert_eq!(
        (d.language.as_deref(), d.source),
        (Some("Nginx"), LangSource::Mapping)
    );
}

#[test]
fn longer_mapping_patterns_win_within_one_settings_file() {
    let mut warnings = Vec::new();
    let mapping = Mapping::from_settings(
        None,
        Some(&json!({ "*.conf": "INI", "nginx/*.conf": "Nginx" })),
        &Contributed::default(),
        &mut warnings,
    );
    assert!(warnings.is_empty());
    assert_eq!(mapping.get("nginx/a.conf"), Some("Nginx"));
    assert_eq!(mapping.get("other/a.conf"), Some("INI"));
}

#[test]
fn contributed_extensions_apply_only_when_linguist_finds_nothing() {
    let contributed = Contributed::from_pairs(&[("pwx", "Pitwall Thing"), ("rs", "Not Rust")]);
    let run = |path: &str| {
        detect(
            Input {
                repo_path: path,
                project_path: path,
                head: None,
            },
            &Attrs::default(),
            &Mapping::default(),
            &contributed,
        )
    };
    let d = run("a.PWX");
    assert_eq!(
        (d.language.as_deref(), d.source),
        (Some("Pitwall Thing"), LangSource::Contributed)
    );
    assert_eq!(run("a.rs").language.as_deref(), Some("Rust"));
    assert_eq!(run("a.unknown").source, LangSource::None);
}

// ── RPC ────────────────────────────────────────────────────

struct Fixture {
    _tmp: tempfile::TempDir,
    lang: Lang,
    projects: Projects,
}

impl Fixture {
    fn new(settings: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(&tmp.path().join("app"));
        fs::create_dir_all(&paths.config).unwrap();
        fs::write(paths.config.join("settings.toml"), settings).unwrap();
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let global = ConfigStore::open(ConfigSource::global(&paths), bus.clone(), Scope::Host);
        assert_eq!(global.fault(), None);
        let projects = Projects::new(paths.clone(), None, bus);
        let lang = Lang::new(&pw_ext::Registry::default());
        lang.attach(HostParts {
            paths,
            global,
            projects: projects.clone(),
        });
        Self {
            _tmp: tmp,
            lang,
            projects,
        }
    }

    /// `rel` 아래 프로젝트(그 위 `repo` 가 git 저장소).
    fn project(&self, repo: &Path, rel: &str) -> String {
        let root = repo.join(rel);
        fs::create_dir_all(&root).unwrap();
        self.projects
            .create(NewProject {
                root,
                ..Default::default()
            })
            .unwrap()
            .id
    }
}

fn file(path: &Path, head: Option<&str>) -> LangDetectFile {
    LangDetectFile {
        path: path.display().to_string(),
        head: head.map(str::to_owned),
    }
}

#[test]
fn rpc_detects_a_batch_in_order_with_repo_attributes_and_project_mapping() {
    let fx = Fixture::new("schema = 1\n[languages.mapping]\n\"*.tmpl\" = \"HTML\"\n");
    let repo = repo_with(&[
        (
            ".gitattributes",
            "*.inc linguist-language=PHP\nweb/vendor/** linguist-vendored=false\n",
        ),
        (
            "web/.pitwall/project.toml",
            "schema = 1\n[languages.mapping]\n\"*.cfg\" = \"Nginx\"\n",
        ),
    ]);
    let repo_top = fs::canonicalize(repo.path()).unwrap();
    let id = fx.project(&repo_top, "web");
    let web = repo_top.join("web");
    let out = fx
        .lang
        .detect(&LangDetectParams {
            project_id: Some(id),
            worktree: None,
            files: vec![
                file(&web.join("x.inc"), None),
                file(&web.join("site.cfg"), None),
                file(&web.join("page.tmpl"), None),
                file(&web.join("vendor/lib.js"), None),
                file(&web.join("tool"), Some("#!/usr/bin/env python3\n")),
                // 프로젝트 밖: `.gitattributes`(PHP)를 보지 않고, 경로 규칙은 파일 이름에만 맞춘다.
                file(Path::new("/elsewhere/vendor/a.inc"), None),
            ],
        })
        .unwrap()
        .files;
    let langs: Vec<_> = out.iter().map(|d| d.language.as_deref()).collect();
    assert_eq!(
        langs,
        [
            Some("PHP"),
            Some("Nginx"),
            Some("HTML"),
            Some("JavaScript"),
            Some("Python"),
            Some("C++")
        ]
    );
    assert_eq!(out[0].source, LangSource::Gitattributes);
    assert_eq!(out[1].source, LangSource::Mapping);
    assert_eq!(out[2].source, LangSource::Mapping);
    assert!(
        !out[3].vendored,
        "linguist-vendored=false 가 vendor.yml 을 이긴다"
    );
    assert_ne!(out[5].source, LangSource::Gitattributes);
    assert!(!out[5].vendored);
}

/// 활성 워크트리의 `.gitattributes` 를 본다. 창이 넘긴 워크트리 경로가 정규화되지 않은 꼴(심링크 별칭)이어도 파일 경로와 같은
/// 함수로 정규화해 비교한다(`pw_fs` 의 워크트리 루트 계산과 같다).
#[cfg(unix)]
#[test]
fn rpc_uses_the_active_worktree_given_through_a_symlinked_path() {
    let fx = Fixture::new("schema = 1\n");
    let tmp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let main = base.join("main");
    let wt = base.join("wt");
    let admin = main.join(".git/worktrees/wt");
    fs::create_dir_all(&admin).unwrap();
    fs::create_dir_all(&wt).unwrap();
    fs::write(admin.join("commondir"), "../..\n").unwrap();
    fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
    fs::write(
        admin.join("gitdir"),
        format!("{}\n", wt.join(".git").display()),
    )
    .unwrap();
    fs::write(wt.join(".gitattributes"), "*.x linguist-language=Ruby\n").unwrap();
    let id = fx.project(&main, "");
    std::os::unix::fs::symlink(&base, base.join("alias")).unwrap();
    let out = fx
        .lang
        .detect(&LangDetectParams {
            project_id: Some(id),
            worktree: Some(base.join("alias/wt").display().to_string()),
            files: vec![file(&wt.join("a.x"), None)],
        })
        .unwrap();
    assert_eq!(
        (out.files[0].language.as_deref(), out.files[0].source),
        (Some("Ruby"), LangSource::Gitattributes)
    );
}

#[test]
fn rpc_rejects_relative_paths_too_many_files_and_unknown_projects() {
    let fx = Fixture::new("schema = 1\n");
    let one = |path: &str| LangDetectParams {
        files: vec![LangDetectFile {
            path: path.into(),
            head: None,
        }],
        ..Default::default()
    };
    let abs = if cfg!(windows) { r"C:\a.rs" } else { "/a.rs" };
    // invalid params 의 사람용 설명은 `data` 에 있다.
    let err = |params: &LangDetectParams| {
        let e = fx.lang.detect(params).unwrap_err();
        assert_eq!(e.code, pw_rpc::code::INVALID_PARAMS);
        e.data.and_then(|d| d.as_str().map(str::to_owned)).unwrap()
    };
    assert!(err(&one("src/a.rs")).contains("must be absolute"));
    let ok = fx.lang.detect(&one(abs)).unwrap();
    assert_eq!(ok.files[0].language.as_deref(), Some("Rust"));
    // 상한까지는 받고, 하나 넘으면 거절한다(경로는 모두 절대 경로라 다른 이유로 거절되지 않는다).
    let file = LangDetectFile {
        path: abs.into(),
        head: None,
    };
    let at_limit = LangDetectParams {
        files: vec![file.clone(); MAX_FILES],
        ..Default::default()
    };
    assert_eq!(fx.lang.detect(&at_limit).unwrap().files.len(), MAX_FILES);
    let many = LangDetectParams {
        files: vec![file; MAX_FILES + 1],
        ..Default::default()
    };
    assert!(err(&many).contains("too many files"));
    let unknown = LangDetectParams {
        project_id: Some("no-such-project".into()),
        ..Default::default()
    };
    assert!(fx.lang.detect(&unknown).is_err());
}

/// 상태바의 언어 고르기(M1-18)가 쓰는 목록: Linguist 언어 전부(플레인 텍스트 `Text` 포함)와, 내장 확장이 적은 줄 주석.
#[test]
fn lang_list_has_every_linguist_language_and_builtin_line_comments() {
    let registry =
        pw_ext::Registry::from_roots(&semver::Version::new(0, 0, 0), pw_ext::builtin::roots());
    let list = Lang::new(&registry).list();
    let get = |name: &str| -> &LangInfo {
        list.languages
            .iter()
            .find(|l| l.name == name)
            .unwrap_or_else(|| panic!("{name} not listed"))
    };
    assert_eq!(get("Rust").line_comment.as_deref(), Some("//"));
    assert_eq!(get("Python").line_comment.as_deref(), Some("#"));
    assert_eq!(get("SQL").line_comment.as_deref(), Some("--"));
    assert!(get("Rust").popular);
    // 확장이 줄 주석을 적지 않은 언어 · 플레인 텍스트는 없다.
    assert_eq!(get("Text").line_comment, None);
    assert_eq!(get("COBOL").line_comment, None);
    assert_eq!(list.languages.len(), linguist::languages().len());
    let mut names: Vec<&str> = list.languages.iter().map(|l| l.name.as_str()).collect();
    names.dedup();
    assert_eq!(names.len(), list.languages.len());
}

/// Linguist 에 없는 언어를 기여한 확장은 목록 뒤에 붙고, 같은 언어의 줄 주석은 먼저 등록된 것이 이긴다.
#[test]
fn lang_list_appends_contributed_languages_and_keeps_the_first_line_comment() {
    let tmp = tempfile::tempdir().unwrap();
    for (dir, body) in [
        (
            "a",
            "[contributes.languages.zz]\nlinguist = \"Zzlang\"\nextensions = [\".zz\"]\nline_comment = \";;\"\n",
        ),
        (
            "b",
            "[contributes.languages.zz2]\nlinguist = \"Zzlang\"\nextensions = [\".zz2\"]\nline_comment = \"%\"\n",
        ),
    ] {
        let root = tmp.path().join(dir);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("extension.toml"),
            format!("id = \"test.{dir}\"\nname = \"{dir}\"\nversion = \"0.1.0\"\nengines = {{ pitwall = \"*\" }}\n{body}"),
        )
        .unwrap();
    }
    let registry = pw_ext::Registry::from_roots(
        &semver::Version::new(0, 0, 0),
        ["a", "b"].map(|d| pw_ext::ExtensionRoot::Dir(tmp.path().join(d))),
    );
    assert!(
        registry.diagnostics().is_empty(),
        "{:?}",
        registry.diagnostics()
    );
    let list = Lang::new(&registry).list();
    let last = list.languages.last().unwrap();
    assert_eq!(last.name, "Zzlang");
    assert_eq!(last.line_comment.as_deref(), Some(";;"));
    assert_eq!(list.languages.len(), linguist::languages().len() + 1);
}
