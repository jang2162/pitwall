//! 받기(HTTPS 만 · 리다이렉트 · 크기 · 시한) · zip 풀기(폴더 밖 이름 · 링크 · 폭탄) · `checksums.txt` · 태그 꼴.

use std::time::{Duration, Instant};

use super::*;
use crate::fetch::{FetchError, MAX_REDIRECT_HOPS};
use crate::test_http::{FakeServer, Reply, zip_of};

#[tokio::test]
async fn fetch_refuses_non_https_urls_and_redirects() {
    let server = FakeServer::start().await;
    server.route("/ok", Reply::Body(b"hello".to_vec()));
    // 실제 받기(시험용이 아닌 것)는 루프백 http 도 받지 않는다.
    let real = Fetcher::new().unwrap();
    assert!(matches!(
        real.get(&server.url("/ok"), 1024, None).await,
        Err(FetchError::Insecure(_))
    ));
    assert_eq!(server.total(), 0, "nothing is sent for a refused URL");
    for bad in [
        "http://example.com/x",
        "ftp://example.com/x",
        "file:///etc/passwd",
        "not a url",
    ] {
        assert!(
            matches!(
                real.get(bad, 1024, None).await,
                Err(FetchError::Insecure(_))
            ),
            "{bad}"
        );
    }

    let f = Fetcher::for_loopback_tests();
    assert_eq!(
        f.get(&server.url("/ok"), 1024, None).await.unwrap(),
        b"hello"
    );
    // https 가 아닌 곳으로의 리다이렉트는 따라가지 않는다.
    server.route(
        "/to-http",
        Reply::Redirect("http://example.com/registry.json.zip".into()),
    );
    assert!(matches!(
        f.get(&server.url("/to-http"), 1024, None).await,
        Err(FetchError::Insecure(u)) if u.starts_with("http://example.com/")
    ));
    // 같은 서버 안의 리다이렉트는 상한까지 따라간다.
    server.route("/hop", Reply::Redirect(server.url("/ok")));
    assert_eq!(
        f.get(&server.url("/hop"), 1024, None).await.unwrap(),
        b"hello"
    );
    server.route("/loop", Reply::Redirect(server.url("/loop")));
    assert_eq!(
        f.get(&server.url("/loop"), 1024, None).await,
        Err(FetchError::TooManyRedirects)
    );
    assert_eq!(server.hits("/loop"), MAX_REDIRECT_HOPS + 1);
    server.route("/missing", Reply::Status(404));
    assert_eq!(
        f.get(&server.url("/missing"), 1024, None).await,
        Err(FetchError::Status(404))
    );
}

#[tokio::test]
async fn fetch_stops_at_the_size_limit_and_the_timeout() {
    let server = FakeServer::start().await;
    server.route("/sized", Reply::Body(vec![b'x'; 2048]));
    server.route("/unsized", Reply::Unsized(vec![b'x'; 2048]));
    server.route("/exact", Reply::Unsized(vec![b'x'; 1024]));
    let f = Fetcher::for_loopback_tests();
    for path in ["/sized", "/unsized"] {
        assert_eq!(
            f.get(&server.url(path), 1024, None).await,
            Err(FetchError::TooLarge { limit: 1024 }),
            "{path}"
        );
    }
    assert_eq!(
        f.get(&server.url("/exact"), 1024, None)
            .await
            .unwrap()
            .len(),
        1024
    );

    server.route("/hang", Reply::Hang);
    let f = Fetcher::for_loopback_tests_with_timeout(Duration::from_millis(500));
    let start = Instant::now();
    assert!(matches!(
        f.get(&server.url("/hang"), 1024, None).await,
        Err(FetchError::Network(_))
    ));
    // 시한(0.5 s)으로 끝난다 — 넉넉한 감시 상한은 기기 부하를 견디게 둔다.
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn unzip_takes_registry_json_and_rejects_unsafe_archives() {
    let good = zip_of(&[("registry.json", b"[]")]);
    assert_eq!(unzip_registry(&good).unwrap(), b"[]");

    let reject = |bytes: &[u8], what: &str| {
        let e = unzip_with(bytes, 1024, 4).unwrap_err();
        assert!(e.contains(what), "{e}");
    };
    reject(
        &zip_of(&[("registry.json", b"[]"), ("../evil", b"x")]),
        "outside",
    );
    reject(
        &zip_of(&[("registry.json", b"[]"), ("a/../../evil", b"x")]),
        "outside",
    );
    reject(
        &zip_of(&[("registry.json", b"[]"), ("/etc/evil", b"x")]),
        "outside",
    );
    reject(
        &zip_of(&[("registry.json", b"[]"), ("C:/evil", b"x")]),
        "outside",
    );
    reject(
        &zip_of(&[("registry.json", b"[]"), ("a\\..\\evil", b"x")]),
        "outside",
    );
    reject(
        &zip_of(&[
            ("registry.json", b"[]"),
            ("a", b""),
            ("b", b""),
            ("c", b""),
            ("d", b""),
        ]),
        "entries",
    );
    reject(&zip_of(&[("other.json", b"[]")]), "no registry.json");
    // 압축 폭탄: 1 MiB 의 같은 글자는 1 KiB 남짓으로 줄지만 풀면 상한(1 KiB)을 넘는다.
    let bomb = zip_of(&[("registry.json", &vec![b' '; 1 << 20])]);
    assert!(bomb.len() < 8 * 1024, "{}", bomb.len());
    reject(&bomb, "unpacks to");
    // 두 항목의 합으로 넘는 것도.
    reject(
        &zip_of(&[("registry.json", &[b' '; 600]), ("pad", &[b' '; 600])]),
        "unpacks to",
    );

    let mut link = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut link);
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file("registry.json", opts).unwrap();
        std::io::Write::write_all(&mut w, b"[]").unwrap();
        w.add_symlink("link", "/etc/passwd", opts).unwrap();
        w.finish().unwrap();
    }
    reject(&link.into_inner(), "symbolic link");
    assert!(unzip_registry(b"not a zip").is_err());
}

#[test]
fn checksums_and_tags() {
    let sums = "B083DF38b9f548ac40a9341c5f573c5f1bb228b9f97cc166fa8350e4bf973b94  registry.json\n\
                2ef62c6eda948f975d80d2b0841ea944d1a538ae3c6b6d04ed7a55037cdeba7a *registry.json.zip\n\
                zz  other\n";
    assert_eq!(
        checksum_for(sums, "registry.json").as_deref(),
        Some("b083df38b9f548ac40a9341c5f573c5f1bb228b9f97cc166fa8350e4bf973b94")
    );
    assert_eq!(
        checksum_for(sums, "registry.json.zip").as_deref(),
        Some("2ef62c6eda948f975d80d2b0841ea944d1a538ae3c6b6d04ed7a55037cdeba7a")
    );
    assert_eq!(checksum_for(sums, "other"), None);
    assert_eq!(checksum_for(sums, "missing"), None);

    for ok in ["2026-11-01-batty-dew", "2025-01-02-a-b2-c"] {
        assert!(valid_version(ok), "{ok}");
    }
    for bad in [
        "",
        "2026-11-01",
        "2026-11-01-",
        "2026-11-01-Batty-dew",
        "2026-11-01-a/../b",
        "../2026-11-01-a",
        "2026_11_01-a",
        "2026-11-01-a--b",
        &format!("2026-11-01-{}", "a".repeat(64)),
    ] {
        assert!(!valid_version(bad), "{bad}");
    }
}
