//! 실행 파일 찾기: 런타임 환경의 PATH 에서 이름으로 찾는다. 런타임 환경으로 프로세스를 띄우는 크레이트(외부 도구 `pw-open-with`,
//! git CLI `pw-git`)가 같은 규칙을 쓴다.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// `program` 을 실행할 절대 경로. 이름이면 `path`(PATH 형식)에서 찾는다. 상대 경로(`./x`, `bin/x`)는 받지 않는다 — 무엇의
/// 상대인지 사람이 정할 수 없다.
pub fn find_program(program: &str, path: &OsStr) -> Option<PathBuf> {
    let as_path = Path::new(program);
    if as_path.is_absolute() {
        return is_executable(as_path).then(|| as_path.to_path_buf());
    }
    if program.contains('/') || (cfg!(windows) && program.contains('\\')) {
        return None;
    }
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .flat_map(|dir| candidates(&dir, program))
        .find(|p| is_executable(p))
}

#[cfg(windows)]
fn candidates(dir: &Path, program: &str) -> Vec<PathBuf> {
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| DEFAULT_PATHEXT.to_owned());
    windows_names(program, &exts)
        .into_iter()
        .map(|name| dir.join(name))
        .collect()
}

#[cfg(not(windows))]
fn candidates(dir: &Path, program: &str) -> Vec<PathBuf> {
    vec![dir.join(program)]
}

#[cfg(windows)]
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// Windows 에서 `program` 이름으로 찾아볼 파일 이름(cmd.exe 의 찾기 규칙). 이름이 PATHEXT 확장자로 끝나면 그 이름만, 아니면
/// PATHEXT 를 붙인 이름만 본다 — 확장자 없는 이름 그대로는 보지 않는다. VS Code · Cursor 의 `bin` 에는 WSL · Git Bash 용
/// 확장자 없는 셸 스크립트 `code` 와 `code.cmd` 가 함께 있어, 그대로 보면 셸 스크립트가 잡혀 띄우기에 실패한다.
/// 플랫폼 무관한 순수 함수라 macOS 에서도 시험한다.
pub fn windows_names(program: &str, pathext: &str) -> Vec<String> {
    let exts: Vec<&str> = pathext.split(';').filter(|e| !e.is_empty()).collect();
    let lower = program.to_ascii_lowercase();
    if exts
        .iter()
        .any(|e| lower.ends_with(&e.to_ascii_lowercase()))
    {
        return vec![program.to_owned()];
    }
    exts.iter().map(|e| format!("{program}{e}")).collect()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_tries_only_pathext_names() {
        let ext = ".COM;.EXE;.BAT;.CMD";
        assert_eq!(
            windows_names("code", ext),
            ["code.COM", "code.EXE", "code.BAT", "code.CMD"]
        );
        assert_eq!(windows_names("code.cmd", ext), ["code.cmd"]);
        assert_eq!(windows_names("Explorer.EXE", ext), ["Explorer.EXE"]);
        // PATHEXT 에 없는 확장자는 확장자가 아니다.
        assert_eq!(windows_names("my.tool", ".EXE"), ["my.tool.EXE"]);
    }
}

#[cfg(all(test, unix))]
mod unix_tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn finds_programs_on_the_given_path_only() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("my bin");
        std::fs::create_dir(&bin).unwrap();
        let tool = script(&bin, "tool", "true");
        std::fs::write(bin.join("not-exec"), "x").unwrap();
        let path = std::env::join_paths(["relative/dir".into(), bin.clone()]).unwrap();
        assert_eq!(find_program("tool", &path), Some(tool.clone()));
        assert_eq!(find_program("not-exec", &path), None);
        assert_eq!(find_program("tool", OsStr::new("/usr/bin:/bin")), None);
        assert_eq!(
            find_program(tool.to_str().unwrap(), OsStr::new("")),
            Some(tool)
        );
        assert_eq!(find_program("my bin/tool", &path), None);
        // PATH 의 상대 폴더는 보지 않는다 — 무엇의 상대인지(앱의 cwd)에 따라 다른 실행 파일이 잡힌다. 이 상대 경로는 지금 cwd 에서
        // 보면 `bin` 을 가리킨다.
        let cwd = std::env::current_dir().unwrap();
        let up = "../".repeat(cwd.components().count() - 1);
        let relative = PathBuf::from(format!(
            "{up}{}",
            bin.display().to_string().trim_start_matches('/')
        ));
        assert!(
            relative.is_relative() && relative.join("tool").is_file(),
            "{}",
            relative.display()
        );
        assert_eq!(find_program("tool", relative.as_os_str()), None);
        assert_eq!(find_program("./tool", &path), None);
    }
}
