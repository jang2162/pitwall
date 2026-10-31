//! 언어 설정 쓰기: 전역 `$CONFIG/settings.toml` 의 `ui.language` 한 키만 고친다. 파일을 다루는 규칙(주석 · 서식 보존, 없으면
//! 만들기, 깨진 파일 · 새 `schema` 거절, 심링크, 경합 재시도)은 `pw_core::config::write` 에 있다 — 설정 창의 DB 페이지(M3-20)도
//! 같은 함수로 쓴다.

#[cfg(test)]
use std::fs;
use std::path::Path;

#[cfg(test)]
use pw_core::config::write::ATTEMPTS;
pub use pw_core::config::write::{SETTINGS_FILE, WriteError};

use crate::language::LANGUAGE_KEY;

/// `config_dir/settings.toml` 의 `ui.language` 를 `value` 로 둔다. 바꿨으면 참, 이미 그 값이면 거짓.
pub fn write_language(config_dir: &Path, value: &str) -> Result<bool, WriteError> {
    write_language_with(config_dir, value, &mut |_| {})
}

/// [`write_language`] 본체. `before_recheck` 는 새 내용을 만든 뒤 · 다시 읽기 직전에 불린다 — 시험이 그 틈에 다른 쓰기를
/// 끼워 경합을 만든다.
fn write_language_with(
    config_dir: &Path,
    value: &str,
    before_recheck: &mut dyn FnMut(&Path),
) -> Result<bool, WriteError> {
    pw_core::config::write::write_setting_with(
        config_dir,
        LANGUAGE_KEY,
        Some(toml_edit::Value::from(value)),
        before_recheck,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_the_file_with_a_schema_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        assert!(write_language(&config, "ko").unwrap());
        assert_eq!(
            fs::read_to_string(config.join(SETTINGS_FILE)).unwrap(),
            "schema = 1\n\n[ui]\nlanguage = \"ko\"\n"
        );
    }

    #[test]
    fn changes_only_the_language_key_and_keeps_comments() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(SETTINGS_FILE);
        fs::write(
            &file,
            "schema = 1\n# 내 설정\n[ui]\nlanguage = \"en\" # 언어\nfont = 13\n\n[diagnostics]\ncrash_reports = true\n",
        )
        .unwrap();
        assert!(write_language(dir.path(), "ko").unwrap());
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "schema = 1\n# 내 설정\n[ui]\nlanguage = \"ko\" # 언어\nfont = 13\n\n[diagnostics]\ncrash_reports = true\n"
        );
        assert!(
            !write_language(dir.path(), "ko").unwrap(),
            "같은 값은 쓰지 않는다"
        );
    }

    #[test]
    fn refuses_a_file_it_cannot_read_as_toml() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(SETTINGS_FILE);
        for text in ["schema = = 1\n", "schema = 1\nui = 3\n"] {
            fs::write(&file, text).unwrap();
            let e = write_language(dir.path(), "ko").unwrap_err();
            assert!(matches!(e, WriteError::Invalid { .. }), "{e}");
            assert_eq!(fs::read_to_string(&file).unwrap(), text);
        }
    }

    /// 설정 계층이 손대지 않는 파일(앱보다 새 `schema`, 정수가 아닌 `schema`)은 고치지 않는다. 옛 파일(`schema` 없음)은 고친다.
    #[test]
    fn refuses_a_file_from_a_newer_schema_and_keeps_an_old_one_writable() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(SETTINGS_FILE);
        for text in ["schema = 9\nx = 1\n", "schema = \"one\"\n", "schema = -1\n"] {
            fs::write(&file, text).unwrap();
            let e = write_language(dir.path(), "ko").unwrap_err();
            assert!(matches!(e, WriteError::Invalid { .. }), "{e}");
            assert_eq!(fs::read_to_string(&file).unwrap(), text);
        }
        fs::write(&file, "x = 1\n").unwrap();
        assert!(write_language(dir.path(), "ko").unwrap());
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "x = 1\n\n[ui]\nlanguage = \"ko\"\n"
        );
    }

    /// 새 내용을 만든 뒤 다른 쪽이 파일을 고치면 다시 읽어 그 변경 위에 언어를 쓴다 — 다른 쪽의 키를 덮지 않는다.
    #[test]
    fn rebuilds_on_top_of_a_change_made_while_writing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(SETTINGS_FILE);
        fs::write(&file, "schema = 1\n").unwrap();
        let mut edits = 0;
        let changed = write_language_with(dir.path(), "ko", &mut |path| {
            if edits == 0 {
                fs::write(path, "schema = 1\n[diagnostics]\ncrash_reports = true\n").unwrap();
            }
            edits += 1;
        })
        .unwrap();
        assert!(changed);
        assert_eq!(edits, 2, "한 번 다시 만들었다");
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "schema = 1\n[diagnostics]\ncrash_reports = true\n\n[ui]\nlanguage = \"ko\"\n"
        );
    }

    /// 쓸 때마다 파일이 바뀌면 몇 번 다시 해 보고 쓰지 않은 채 `Busy` 로 끝난다.
    #[test]
    fn gives_up_when_the_file_keeps_changing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(SETTINGS_FILE);
        fs::write(&file, "schema = 1\n").unwrap();
        let mut edits = 0;
        let e = write_language_with(dir.path(), "ko", &mut |path| {
            edits += 1;
            fs::write(path, format!("schema = 1\nedit = {edits}\n")).unwrap();
        })
        .unwrap_err();
        assert!(matches!(e, WriteError::Busy { .. }), "{e}");
        assert_eq!(edits, ATTEMPTS);
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            format!("schema = 1\nedit = {ATTEMPTS}\n")
        );
    }

    #[cfg(unix)]
    #[test]
    fn writes_through_a_symlink_and_refuses_a_dangling_one() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("dotfiles/settings.toml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, "schema = 1\n").unwrap();
        let config = dir.path().join("config");
        fs::create_dir_all(&config).unwrap();
        std::os::unix::fs::symlink(&target, config.join(SETTINGS_FILE)).unwrap();
        write_language(&config, "ko").unwrap();
        assert!(
            fs::symlink_metadata(config.join(SETTINGS_FILE))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            fs::read_to_string(&target)
                .unwrap()
                .contains("language = \"ko\"")
        );

        fs::remove_file(&target).unwrap();
        let e = write_language(&config, "en").unwrap_err();
        assert!(matches!(e, WriteError::Io { .. }), "{e}");
        assert!(!target.exists(), "끊긴 링크 자리에 새 파일을 만들지 않는다");
    }
}
