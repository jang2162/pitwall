//! 셸 시작 파일이 다 돈 뒤 런타임 제공자의 PATH 를 다시 앞에 둔다.
//!
//! 왜: macOS 의 로그인 셸은 시작 파일(`/etc/zprofile` · `/etc/profile`)에서 `path_helper` 를 돌려 `/etc/paths` ·
//! `/etc/paths.d`(`/opt/homebrew/bin` 등)를 PATH 앞으로 올리고, 우리가 넘긴 PATH 의 나머지를 뒤로 민다. 그러면 런타임 제공자
//! (mise 등)가 앞에 둔 폴더가 Homebrew 뒤로 가서, 신뢰한 프로젝트의 새 터미널에서 `node -v` 가 mise 판이 아니게 된다(M1-16).
//!
//! 어떻게: 제공자가 로그인 셸의 PATH 에 **더한** 항목([`provider_front`])만 골라, 셸이 사용자의 시작 파일을 모두 읽은 뒤 그
//! 항목을 PATH 맨 앞으로 옮긴다. 사용자의 시작 파일은 고치지 않고, 그 파일들이 PATH 에 넣은 것(앞에 둔 `~/bin` 등)은 제공자
//! 항목 바로 뒤에 그 순서대로 남는다. 제공자 항목이 없으면(로그인 셸 환경뿐) 아무것도 바꾸지 않는다.
//!
//! - zsh: `ZDOTDIR` 을 이 터미널의 임시 폴더로 바꿔 띄운다. 그 폴더의 `.zshenv` 는 사용자의 `.zshenv`(사용자의 `ZDOTDIR`, 없으면
//!   홈)를 읽은 뒤 `ZDOTDIR` 을 사용자의 값으로 두고, 첫 프롬프트 직전(`precmd`)에 한 번 PATH 를 고치는 훅을 건다. 그 뒤의 시작
//!   파일(`/etc/zprofile` · `/etc/zshrc` · 사용자의 `.zprofile` · `.zshrc` · `.zlogin`)은 감싸지 않은 셸과 같은 `ZDOTDIR` 을 본다 —
//!   macOS `/etc/zshrc` 가 `${ZDOTDIR:-$HOME}` 으로 정하는 `HISTFILE` 이 임시 폴더로 가면 명령 기록이 터미널마다 지워진다. 훅은
//!   `emulate -L zsh` 아래에서 돈다(사용자가 켠 `KSH_ARRAYS` · `SH_WORD_SPLIT` 등에 PATH 가 망가지지 않게). 사용자 rc 가
//!   `precmd_functions` 를 통째로 바꾸면 훅이 빠져 PATH 를 고치지 않는다.
//! - bash: 로그인 셸의 시작 파일은 `--rcfile` 을 받지 않으므로, 대화형 셸에 `--rcfile` 로 우리 파일을 주고 그 파일이 로그인 셸의
//!   순서(`/etc/profile`, 그다음 `~/.bash_profile` · `~/.bash_login` · `~/.profile` 중 처음 있는 것)대로 읽은 뒤 PATH 를 고친다.
//!   차이: 셸이 로그인 셸로 표시되지 않는다(`shopt login_shell` 이 꺼짐, `~/.bash_logout` 을 읽지 않음).
//! - fish: `--init-command`(설정 파일을 다 읽은 뒤 돈다)로 고친다.
//! - 그 밖의 셸, 설정 `[terminal] shell` 로 직접 고른 셸: 바꾸지 않는다.
//!
//! 임시 폴더는 0700 으로 만들고, 만든 뒤 이 사용자의 것 · 링크 아님 · 0700 인지 확인한다(아니면 통합하지 않는다) — 남이 그 폴더의
//! 시작 파일을 바꾸면 이 사용자의 셸에서 코드가 돈다.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// 제공자가 앞에 둔 PATH 항목을 셸에 넘기는 변수. 시작이 끝나면 셸이 지운다.
pub const FRONT_VAR: &str = "PITWALL_PATH_FRONT";
/// zsh: 사용자의 `ZDOTDIR`(없으면 변수도 없다).
pub const USER_ZDOTDIR_VAR: &str = "PITWALL_USER_ZDOTDIR";

/// 런타임 환경의 PATH 에서 로그인 셸의 PATH(`base`)에 없는 항목, 순서대로. 제공자(mise 등)가 더한 폴더다. `:` 가 든 항목은 PATH
/// 문법으로 넘길 수 없어 뺀다.
pub fn provider_front(path: &OsStr, base: &OsStr) -> Vec<PathBuf> {
    let base: Vec<PathBuf> = std::env::split_paths(base).collect();
    let mut front: Vec<PathBuf> = Vec::new();
    for dir in std::env::split_paths(path) {
        if dir.as_os_str().is_empty() || base.contains(&dir) || front.contains(&dir) {
            continue;
        }
        if dir.to_string_lossy().contains(':') {
            continue;
        }
        front.push(dir);
    }
    front
}

/// 셸에 붙인 것: 바뀐 argv, 더할 환경, 셸이 시작 파일을 읽는 동안 있어야 하는 임시 폴더(터미널이 끝날 때 지운다).
#[derive(Debug)]
pub struct Integration {
    pub argv: Vec<String>,
    pub env: Vec<(OsString, OsString)>,
    pub dir: Option<tempfile::TempDir>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shell {
    Zsh,
    Bash,
    Fish,
}

fn shell_of(program: &str) -> Option<Shell> {
    let name = Path::new(program).file_name()?.to_str()?;
    match name.trim_start_matches('-') {
        "zsh" => Some(Shell::Zsh),
        "bash" => Some(Shell::Bash),
        "fish" => Some(Shell::Fish),
        _ => None,
    }
}

/// 기본 셸(`$SHELL -l`)의 argv 에 붙인다. 붙일 것이 없으면(제공자 항목이 없음 · 모르는 셸) `None` — argv 와 환경을 그대로 쓴다.
/// `user_zdotdir` 는 런타임 환경의 `ZDOTDIR`.
pub fn integrate(
    argv: &[String],
    front: &[PathBuf],
    user_zdotdir: Option<&OsStr>,
) -> io::Result<Option<Integration>> {
    if front.is_empty() {
        return Ok(None);
    }
    let Some(program) = argv.first() else {
        return Ok(None);
    };
    let Some(shell) = shell_of(program) else {
        return Ok(None);
    };
    let joined = std::env::join_paths(front).map_err(io::Error::other)?;
    let mut env = vec![(OsString::from(FRONT_VAR), joined)];
    let integration = match shell {
        Shell::Zsh => {
            let dir = private_dir("pitwall-zsh-")?;
            std::fs::write(dir.path().join(".zshenv"), ZSH_ENV)?;
            env.push(("ZDOTDIR".into(), dir.path().into()));
            if let Some(user) = user_zdotdir {
                env.push((USER_ZDOTDIR_VAR.into(), user.to_owned()));
            }
            Integration {
                argv: argv.to_vec(),
                env,
                dir: Some(dir),
            }
        }
        Shell::Bash => {
            let dir = private_dir("pitwall-bash-")?;
            let rcfile = dir.path().join("bashrc");
            std::fs::write(&rcfile, BASH_RC)?;
            // 로그인 셸의 `-l` 은 빼고 대화형으로 띄운다(머리 주석). 긴 옵션은 짧은 옵션보다 앞이어야 한다.
            let mut next = vec![
                program.clone(),
                "--rcfile".to_owned(),
                rcfile.display().to_string(),
            ];
            next.extend(
                argv[1..]
                    .iter()
                    .filter(|a| *a != "-l" && *a != "--login")
                    .cloned(),
            );
            if !next.iter().any(|a| a == "-i") {
                next.push("-i".to_owned());
            }
            Integration {
                argv: next,
                env,
                dir: Some(dir),
            }
        }
        Shell::Fish => {
            let mut next = argv.to_vec();
            next.extend(["--init-command".to_owned(), FISH_INIT.to_owned()]);
            Integration {
                argv: next,
                env,
                dir: None,
            }
        }
    };
    Ok(Some(integration))
}

/// 이 사용자만 들어갈 수 있는 임시 폴더(0700). 만든 뒤 다시 확인한다 — umask · 파일 시스템이 권한을 바꿨거나 그 자리가 이미 남의
/// 것이면 쓰지 않는다.
fn private_dir(prefix: &str) -> io::Result<tempfile::TempDir> {
    let dir = tempfile::Builder::new()
        .prefix(prefix)
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()?;
    check_private(dir.path())?;
    Ok(dir)
}

fn check_private(path: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    // SAFETY: geteuid 는 인자가 없고 실패하지 않는다.
    let uid = unsafe { libc::geteuid() };
    let mode = meta.mode() & 0o7777;
    if !meta.file_type().is_dir() || meta.uid() != uid || mode != 0o700 {
        return Err(io::Error::other(format!(
            "`{}` is not a private directory (mode {mode:o}, owner {})",
            path.display(),
            meta.uid()
        )));
    }
    Ok(())
}

/// zsh 의 래퍼 `.zshenv`. 사용자의 `.zshenv` 는 바깥(함수 밖)에서 읽는다 — 함수 안에서 읽으면 그 파일의 `typeset` 이 지역 변수가
/// 된다. 뒤의 코드는 사용자의 옵션과 떨어지게 `emulate -L zsh` 아래에서 돈다.
const ZSH_ENV: &str = r#"# Pitwall 터미널(crates/pw-run/src/integration.rs). 사용자의 .zshenv 를 읽고, 첫 프롬프트 직전에 PATH 를 고친다.
if [[ -n ${PITWALL_USER_ZDOTDIR+x} ]]; then ZDOTDIR=$PITWALL_USER_ZDOTDIR; else unset ZDOTDIR; fi
__pitwall_front=$PITWALL_PATH_FRONT
unset PITWALL_USER_ZDOTDIR PITWALL_PATH_FRONT
[[ -r ${ZDOTDIR:-$HOME}/.zshenv ]] && builtin source -- "${ZDOTDIR:-$HOME}/.zshenv"
() {
  emulate -L zsh
  __pitwall_finish() {
    emulate -L zsh
    precmd_functions=(${precmd_functions:#__pitwall_finish})
    local -a front
    front=(${(s.:.)__pitwall_front})
    path=($front ${path:|front})
    unset __pitwall_front
    unfunction __pitwall_finish
  }
  typeset -ga precmd_functions
  precmd_functions=(__pitwall_finish $precmd_functions)
}
"#;

/// bash: 로그인 셸의 순서로 읽고 PATH 를 고친다. macOS 의 bash 3.2 에서도 돈다.
const BASH_RC: &str = r#"# Pitwall 터미널(crates/pw-run/src/integration.rs). 로그인 셸처럼 시작 파일을 읽는다.
if [ -r /etc/profile ]; then . /etc/profile; fi
if [ -r ~/.bash_profile ]; then . ~/.bash_profile
elif [ -r ~/.bash_login ]; then . ~/.bash_login
elif [ -r ~/.profile ]; then . ~/.profile
fi
__pitwall_rest=
IFS=: read -r -a __pitwall_front <<< "$PITWALL_PATH_FRONT"
IFS=: read -r -a __pitwall_parts <<< "$PATH"
for __pitwall_p in "${__pitwall_parts[@]}"; do
  __pitwall_skip=
  for __pitwall_f in "${__pitwall_front[@]}"; do
    if [ "$__pitwall_p" = "$__pitwall_f" ]; then __pitwall_skip=1; break; fi
  done
  if [ -z "$__pitwall_skip" ]; then __pitwall_rest="$__pitwall_rest:$__pitwall_p"; fi
done
PATH="$PITWALL_PATH_FRONT$__pitwall_rest"
unset PITWALL_PATH_FRONT __pitwall_rest __pitwall_front __pitwall_parts __pitwall_p __pitwall_f __pitwall_skip
"#;

/// fish: 설정을 다 읽은 뒤 돈다.
const FISH_INIT: &str = "set -l __pitwall_front (string split : -- $PITWALL_PATH_FRONT); set -l __pitwall_rest; for p in $PATH; contains -- $p $__pitwall_front; or set -a __pitwall_rest $p; end; set -gx PATH $__pitwall_front $__pitwall_rest; set -e PITWALL_PATH_FRONT";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_front_is_what_the_provider_added_to_the_login_shell_path() {
        let front = provider_front(
            OsStr::new("/mise/node/bin:/mise/python/bin:/usr/bin:/opt/homebrew/bin"),
            OsStr::new("/usr/bin:/opt/homebrew/bin"),
        );
        assert_eq!(
            front,
            [
                PathBuf::from("/mise/node/bin"),
                PathBuf::from("/mise/python/bin")
            ]
        );
        // 로그인 셸 환경뿐이면 없다.
        assert!(
            provider_front(OsStr::new("/usr/bin:/bin"), OsStr::new("/usr/bin:/bin")).is_empty()
        );
    }

    #[test]
    fn only_known_shells_with_a_provider_front_are_changed() {
        let argv = |s: &[&str]| s.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
        let front = [PathBuf::from("/mise/bin")];
        assert!(
            integrate(&argv(&["/bin/zsh", "-l"]), &[], None)
                .unwrap()
                .is_none()
        );
        assert!(
            integrate(&argv(&["/bin/sh", "-l"]), &front, None)
                .unwrap()
                .is_none()
        );
        let bash = integrate(&argv(&["/bin/bash", "-l"]), &front, None)
            .unwrap()
            .unwrap();
        assert_eq!(bash.argv[1], "--rcfile");
        assert_eq!(&bash.argv[3..], ["-i"]);
        let fish = integrate(&argv(&["/usr/local/bin/fish", "-l"]), &front, None)
            .unwrap()
            .unwrap();
        assert_eq!(
            &fish.argv[..3],
            ["/usr/local/bin/fish", "-l", "--init-command"]
        );
        let zsh = integrate(&argv(&["/bin/zsh", "-l"]), &front, Some(OsStr::new("/cfg")))
            .unwrap()
            .unwrap();
        let dir = zsh.dir.as_ref().unwrap().path();
        assert!(dir.join(".zshenv").is_file());
        assert!(zsh.env.contains(&("ZDOTDIR".into(), dir.into())));
        assert!(zsh.env.contains(&(USER_ZDOTDIR_VAR.into(), "/cfg".into())));
        assert!(zsh.env.contains(&(FRONT_VAR.into(), "/mise/bin".into())));
    }

    #[test]
    fn the_startup_files_are_in_a_directory_only_this_user_can_enter() {
        let front = [PathBuf::from("/mise/bin")];
        for shell in ["/bin/zsh", "/bin/bash"] {
            let found = integrate(&[shell.to_owned(), "-l".to_owned()], &front, None)
                .unwrap()
                .unwrap();
            let meta = std::fs::symlink_metadata(found.dir.as_ref().unwrap().path()).unwrap();
            assert_eq!(meta.mode() & 0o7777, 0o700, "{shell}");
            // SAFETY: geteuid 는 인자가 없고 실패하지 않는다.
            assert_eq!(meta.uid(), unsafe { libc::geteuid() }, "{shell}");
        }
    }

    #[test]
    fn a_directory_others_can_enter_or_a_link_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let private = tmp.path().join("private");
        std::fs::create_dir(&private).unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
        check_private(&private).unwrap();
        let open = tmp.path().join("open");
        std::fs::create_dir(&open).unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(check_private(&open).is_err());
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&private, &link).unwrap();
        assert!(check_private(&link).is_err());
        let file = tmp.path().join("file");
        std::fs::write(&file, "").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(check_private(&file).is_err());
    }
}
