use std::io::{Cursor, Read, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use super::*;

const SERVER: &str = "com/microsoft/java/debug/plugin/internal/JavaDebugServer.class";

/// 상류 jar 꼴의 작은 jar: 바꿀 세 클래스(원래 내용)와 바꾸지 않는 항목들(deflate · stored · 폴더 · 다른 시각).
pub(crate) fn upstream_like_jar() -> Vec<u8> {
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    let at = DateTime::from_date_and_time(2026, 4, 16, 4, 30, 0).unwrap();
    let deflated = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(at);
    w.add_directory("META-INF/", deflated).unwrap();
    w.start_file("META-INF/MANIFEST.MF", deflated).unwrap();
    w.write_all(b"Manifest-Version: 1.0\nBundle-SymbolicName: com.microsoft.java.debug.plugin;singleton:=true\n")
        .unwrap();
    for (name, _) in JAVA_DEBUG_LOOPBACK.entries {
        w.start_file(*name, deflated).unwrap();
        w.write_all(format!("upstream bytes of {name}").repeat(20).as_bytes())
            .unwrap();
    }
    w.start_file(
        "com/microsoft/java/debug/plugin/internal/JavaDebugDelegateCommandHandler.class",
        deflated,
    )
    .unwrap();
    w.write_all(&[0xCA, 0xFE, 0xBA, 0xBE].repeat(64)).unwrap();
    w.start_file(
        "lib/com.microsoft.java.debug.core-0.53.2.jar",
        SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .last_modified_time(DateTime::from_date_and_time(2025, 1, 2, 3, 4, 6).unwrap()),
    )
    .unwrap();
    w.write_all(b"PK nested jar").unwrap();
    w.finish().unwrap().into_inner()
}

/// 항목마다 (이름, 압축 방식, CRC, 시각, 압축된 바이트).
type RawEntry = (String, CompressionMethod, u32, Option<DateTime>, Vec<u8>);

fn raw_entries(jar: &[u8]) -> Vec<RawEntry> {
    let mut a = ZipArchive::new(Cursor::new(jar)).unwrap();
    (0..a.len())
        .map(|i| {
            let mut f = a.by_index_raw(i).unwrap();
            let name = f.name().unwrap().into_owned();
            let (method, crc, time) = (f.compression(), f.crc32(), f.last_modified());
            let mut bytes = Vec::new();
            f.read_to_end(&mut bytes).unwrap();
            (name, method, crc, time, bytes)
        })
        .collect()
}

#[test]
fn only_the_three_classes_change_and_every_other_entry_keeps_its_bytes() {
    let input = upstream_like_jar();
    let output = patch_jar(&input, &JAVA_DEBUG_LOOPBACK).unwrap();
    let before = raw_entries(&input);
    let after = raw_entries(&output);
    assert_eq!(
        before.iter().map(|e| &e.0).collect::<Vec<_>>(),
        after.iter().map(|e| &e.0).collect::<Vec<_>>(),
        "same entries in the same order"
    );
    let mut changed = Vec::new();
    for (b, a) in before.iter().zip(&after) {
        match JAVA_DEBUG_LOOPBACK.entries.iter().find(|(n, _)| *n == b.0) {
            Some((_, bytes)) => {
                changed.push(b.0.clone());
                assert_eq!(read_entry(&output, &b.0).unwrap(), *bytes, "{}", b.0);
                assert_eq!(a.1, CompressionMethod::Stored, "{}", b.0);
                assert_eq!(a.3, b.3, "{} keeps its time", b.0);
            }
            None => assert_eq!(a, b, "{} is copied byte for byte", b.0),
        }
    }
    assert_eq!(changed.len(), 3);
}

#[test]
fn patching_is_deterministic() {
    let input = upstream_like_jar();
    let one = patch_jar(&input, &JAVA_DEBUG_LOOPBACK).unwrap();
    let two = patch_jar(&input, &JAVA_DEBUG_LOOPBACK).unwrap();
    assert_eq!(sha256_hex(&one), sha256_hex(&two));
    assert_ne!(sha256_hex(&one), sha256_hex(&input));
}

#[test]
fn a_jar_whose_classes_differ_from_the_patch_is_refused() {
    // 바꿀 클래스가 없다.
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    w.start_file("META-INF/MANIFEST.MF", SimpleFileOptions::default())
        .unwrap();
    w.write_all(b"x").unwrap();
    let missing = w.finish().unwrap().into_inner();
    let e = patch_jar(&missing, &JAVA_DEBUG_LOOPBACK).unwrap_err();
    assert!(e.contains("is not in the jar"), "{e}");

    // 상류에 새 내부 클래스가 생겼다(바꾸지 않으면 고친 클래스와 짝이 맞지 않는다).
    let mut a = ZipArchive::new(Cursor::new(upstream_like_jar())).unwrap();
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..a.len() {
        w.raw_copy_file(a.by_index_raw(i).unwrap()).unwrap();
    }
    w.start_file(
        "com/microsoft/java/debug/plugin/internal/JavaDebugServer$3.class",
        SimpleFileOptions::default(),
    )
    .unwrap();
    w.write_all(b"new").unwrap();
    let extra = w.finish().unwrap().into_inner();
    let e = patch_jar(&extra, &JAVA_DEBUG_LOOPBACK).unwrap_err();
    assert!(e.contains("JavaDebugServer$3.class"), "{e}");
}

/// 묶인 클래스가 고친 것인가 — 상수 풀에 loopback 주소 호출과 세 인자 생성자가 있고 두 인자 생성자 호출만 남지 않았다.
#[test]
fn the_bundled_server_class_binds_the_loopback_address() {
    let (_, class) = JAVA_DEBUG_LOOPBACK
        .entries
        .iter()
        .find(|(n, _)| *n == SERVER)
        .unwrap();
    let has = |needle: &[u8]| class.windows(needle.len()).any(|w| w == needle);
    assert_eq!(&class[..4], &[0xCA, 0xFE, 0xBA, 0xBE]);
    assert_eq!(
        u16::from_be_bytes([class[6], class[7]]),
        65,
        "Java 21 like upstream"
    );
    assert!(has(b"getLoopbackAddress"));
    assert!(has(b"(IILjava/net/InetAddress;)V"));
}

#[test]
fn the_notice_names_the_changed_file_and_why() {
    let n = JAVA_DEBUG_LOOPBACK.notice;
    assert!(n.contains("Eclipse Public License v1.0"));
    assert!(n.contains("JavaDebugServer.java"));
    assert!(n.contains("getLoopbackAddress"));
    assert!(n.contains("54058b9eb466576a7a77c9303963ca88967ec58b"));
    assert_eq!(
        for_package("java-debug-adapter").map(|p| p.name),
        Some("java-debug-loopback")
    );
    assert!(for_package("codelldb").is_none());
}

/// 내장 고정 표가 java-debug 의 번들 판에 패치 결과를 고정한다 — 이 행이 없으면 설치기가 거절한다.
#[test]
fn the_builtin_pins_pin_the_java_debug_patch() {
    let pinned = crate::pinned::Pinned::builtin();
    assert!(pinned.is_bundled("java-debug-adapter"));
    let row = pinned
        .patched
        .iter()
        .find(|p| p.package == "java-debug-adapter")
        .expect("a [[patched]] row for java-debug-adapter");
    assert_eq!(row.patch, "java-debug-loopback");
    assert_eq!(
        row.file,
        "extension/server/com.microsoft.java.debug.plugin-0.53.2.jar"
    );
    assert!(
        pinned
            .patched_for("java-debug-adapter", &row.version)
            .is_some()
    );
}
