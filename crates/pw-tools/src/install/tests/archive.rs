//! ① 압축 풀기: 폴더 밖 경로 · 링크 · 같은 이름 두 번은 설치 전체 실패, 권한 규칙.

use super::*;

const BIN: &str = "demo/bin/demo";

fn ok_entries() -> Vec<Entry> {
    vec![
        Entry::Dir("demo/"),
        Entry::Dir("demo/bin/"),
        Entry::File(BIN, b"#!/bin/sh\necho demo\n".to_vec(), 0o644),
    ]
}

async fn install_archive(fx: &Fx, file_name: &str, body: Vec<u8>) -> Result<(), RpcError> {
    fx.server
        .route("/demo-archive", crate::test_http::Reply::Body(body.clone()));
    let reg = registry(
        vec![files_pkg(
            "demo",
            "1.0.0",
            &[(file_name, "/demo-archive")],
            &[("demo", BIN)],
            &["MIT"],
        )],
        &[Row::file(
            "demo",
            "1.0.0",
            file_name,
            "/demo-archive",
            &body,
        )],
    );
    let installer = fx.installer(reg, Limits::default());
    fx.consent(&installer, "demo");
    installer
        .install(&settings_grant(), "demo", None, None, false)
        .await
        .map(|_| ())
}

/// 표: `../x` · 절대 경로 · 폴더 밖 링크 · 링크를 지나는 항목 · 같은 이름 두 번 · 하드 링크가 모두 설치 실패이고, 설치 폴더는 지워지고,
/// 대상 밖에 파일이 생기지 않는다(`pwned` 라는 이름의 파일이 tmp 어디에도 없다).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsafe_archives_fail_the_whole_install_and_leave_nothing_outside() {
    let outside = "/tmp/pitwall-zip-slip-absolute/pwned";
    let cases: Vec<(&str, Vec<Entry>)> = vec![
        (
            "dotdot",
            vec![Entry::File("../pwned", b"x".to_vec(), 0o644)],
        ),
        (
            "dotdot-deep",
            vec![Entry::File("demo/../../pwned", b"x".to_vec(), 0o644)],
        ),
        ("absolute", vec![Entry::File(outside, b"x".to_vec(), 0o644)]),
        (
            "link-outside",
            vec![Entry::Symlink("demo/out", "../../pwned-dir")],
        ),
        ("link-absolute", vec![Entry::Symlink("demo/out", "/tmp")]),
        (
            "through-link",
            vec![
                Entry::Dir("sub/"),
                Entry::Symlink("l", "sub"),
                Entry::File("l/pwned", b"x".to_vec(), 0o644),
            ],
        ),
        (
            "same-name-twice",
            vec![
                Entry::File("demo/a", b"1".to_vec(), 0o644),
                Entry::File("demo/a", b"2".to_vec(), 0o644),
            ],
        ),
        (
            "file-over-link",
            vec![
                Entry::Symlink("demo/x", "y"),
                Entry::File("demo/x", b"pwned".to_vec(), 0o644),
            ],
        ),
        ("hard-link", vec![Entry::Hard("demo/h", "../../pwned")]),
    ];
    for (label, bad) in cases {
        for format in ["demo.tar.gz", "demo.zip"] {
            if format == "demo.zip" && bad.iter().any(|e| matches!(e, Entry::Hard(..))) {
                continue;
            }
            let fx = Fx::new().await;
            let mut entries = ok_entries();
            entries.extend(bad.clone());
            let body = if format == "demo.zip" {
                zip(&entries)
            } else {
                targz(&entries)
            };
            let err = install_archive(&fx, format, body).await.expect_err(label);
            assert_eq!(
                code_and_stage(&err),
                (code::INSTALL_FAILED, Some("unpack".into())),
                "{label} {format}: {err:?}"
            );
            assert!(
                !fx.layout().version_dir("demo", "1.0.0").exists(),
                "{label} {format}: the version folder is removed"
            );
            assert!(
                fx.layout().read_record("demo").unwrap().is_none(),
                "{label} {format}: no install record"
            );
            let leaked: Vec<_> = fx
                .all_files()
                .into_iter()
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().contains("pwned"))
                })
                .collect();
            assert!(leaked.is_empty(), "{label} {format}: {leaked:?}");
            assert!(!Path::new(outside).exists(), "{label} {format}");
        }
    }
}

/// 같은 보관 파일의 안전한 판은 설치된다(위 표가 「무엇이든 실패」 로 통과하지 않게). 안쪽 링크는 남는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_safe_archive_with_an_inside_link_installs() {
    for format in ["demo.tar.gz", "demo.zip"] {
        let fx = Fx::new().await;
        let mut entries = ok_entries();
        entries.push(Entry::Dir("demo/lib/"));
        entries.push(Entry::File("demo/lib/a.txt", b"a".to_vec(), 0o644));
        entries.push(Entry::Symlink("demo/lib/b.txt", "a.txt"));
        let body = if format == "demo.zip" {
            zip(&entries)
        } else {
            targz(&entries)
        };
        install_archive(&fx, format, body).await.unwrap();
        let dir = fx.layout().version_dir("demo", "1.0.0");
        assert_eq!(
            std::fs::read(dir.join("demo/lib/b.txt")).unwrap(),
            b"a",
            "{format}"
        );
        assert!(dir.join(BIN).is_file());
    }
}

/// 권한: `bin` 은 늘 0755, 보관 파일이 실행 비트를 적은 일반 파일은 0755(kotlin-lsp 의 `jbr/bin/java` 꼴), setuid · setgid · sticky
/// · group/other 쓰기 비트는 지워지고, 실행 비트가 없는 파일은 0644.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn modes_keep_exec_bits_drop_special_and_write_bits_and_bins_are_executable() {
    for format in ["demo.tar.gz", "demo.zip"] {
        let fx = Fx::new().await;
        let entries = vec![
            Entry::Dir("demo/"),
            Entry::Dir("demo/bin/"),
            // bin 은 보관 파일에 실행 비트가 없어도 0755.
            Entry::File(BIN, b"#!/bin/sh\n".to_vec(), 0o600),
            Entry::Dir("demo/jbr/"),
            Entry::Dir("demo/jbr/bin/"),
            Entry::File("demo/jbr/bin/java", b"elf".to_vec(), 0o755),
            Entry::File("demo/jbr/lib/suid", b"s".to_vec(), 0o4755),
            Entry::File("demo/jbr/lib/sgid-sticky", b"s".to_vec(), 0o3775),
            Entry::File("demo/jbr/lib/world-write", b"w".to_vec(), 0o666),
            Entry::File("demo/jbr/lib/world-write-exec", b"w".to_vec(), 0o777),
            Entry::File("demo/jbr/lib/plain", b"p".to_vec(), 0o640),
        ];
        let body = if format == "demo.zip" {
            zip(&entries)
        } else {
            targz(&entries)
        };
        install_archive(&fx, format, body).await.unwrap();
        let dir = fx.layout().version_dir("demo", "1.0.0");
        let expect = [
            (BIN, 0o755),
            ("demo/jbr/bin/java", 0o755),
            ("demo/jbr/lib/suid", 0o755),
            ("demo/jbr/lib/sgid-sticky", 0o755),
            ("demo/jbr/lib/world-write", 0o644),
            ("demo/jbr/lib/world-write-exec", 0o755),
            ("demo/jbr/lib/plain", 0o644),
            ("demo/jbr", 0o755),
        ];
        for (rel, mode) in expect {
            assert_eq!(mode_of(&dir.join(rel)), mode, "{format} {rel}");
        }
    }
}

/// `.gz` 단일 파일(rust-analyzer 꼴)은 저장 이름에서 `.gz` 를 뗀 이름으로 풀리고 `bin` 이 실행 파일이 된다. `.tar.xz` 는 지원하지 않음.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_single_gz_unpacks_to_the_bin_and_tar_xz_is_unsupported() {
    let fx = Fx::new().await;
    let body = gz(b"\x7fELF fake rust-analyzer");
    fx.server
        .route("/ra.gz", crate::test_http::Reply::Body(body.clone()));
    let reg = registry(
        vec![files_pkg(
            "ra",
            "2026-10-26",
            &[("rust-analyzer-test.gz", "/ra.gz")],
            &[("rust-analyzer", "rust-analyzer-test")],
            &["MIT", "Apache-2.0"],
        )],
        &[Row::file(
            "ra",
            "2026-10-26",
            "rust-analyzer-test.gz",
            "/ra.gz",
            &body,
        )],
    );
    let installer = fx.installer(reg, Limits::default());
    fx.consent(&installer, "ra");
    installer
        .install(&settings_grant(), "ra", None, None, false)
        .await
        .unwrap();
    let bin = fx
        .layout()
        .version_dir("ra", "2026-10-26")
        .join("rust-analyzer-test");
    assert_eq!(std::fs::read(&bin).unwrap(), b"\x7fELF fake rust-analyzer");
    #[cfg(unix)]
    assert_eq!(mode_of(&bin), 0o755);

    let tmp = tempfile::tempdir().unwrap();
    let archive = tmp.path().join("a.tar.xz");
    std::fs::write(&archive, b"x").unwrap();
    let root = tmp.path().join("root");
    std::fs::create_dir(&root).unwrap();
    let mut u = extract::Unpacker::new(root, 1024);
    assert_eq!(
        u.unpack(
            &archive,
            "a.tar.xz",
            crate::package::ArchiveFormat::TarXz,
            None
        ),
        Err(extract::ExtractError::Unsupported {
            format: crate::package::ArchiveFormat::TarXz
        })
    );
}
