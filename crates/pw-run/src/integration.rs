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
//! - zsh: `ZDOTDIR` 을 이 터미널의 임시 폴더로 바꿔 띄운다. 그 폴더의 `.zshenv` · `.zprofile` · `.zshrc` · `.zlogin` 은 사용자의
//!   같은 파일(사용자의 `ZDOTDIR`, 없으면 홈)을 읽고 다음 파일도 이 폴더를 거치게 한다. 마지막 파일(로그인 셸이면 `.zlogin`)이
//!   PATH 를 고치고 `ZDOTDIR` 을 사용자의 값으로 되돌린다 — 그 셸에서 띄운 zsh 는 사용자의 파일을 읽는다.
//! - bash: 로그인 셸의 시작 파일은 `--rcfile` 을 받지 않으므로, 대화형 셸에 `--rcfile` 로 우리 파일을 주고 그 파일이 로그인 셸의
//!   순서(`/etc/profile`, 그다음 `~/.bash_profile` · `~/.bash_login` · `~/.profile` 중 처음 있는 것)대로 읽은 뒤 PATH 를 고친다.
//!   차이: 셸이 로그인 셸로 표시되지 않는다(`shopt login_shell` 이 꺼짐, `~/.bash_logout` 을 읽지 않음).
//! - fish: `--init-command`(설정 파일을 다 읽은 뒤 돈다)로 고친다.
//! - 그 밖의 셸, 설정 `[terminal] shell` 로 직접 고른 셸: 바꾸지 않는다.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

/// 제공자가 앞에 둔 PATH 항목을 셸에 넘기는 변수. 시작이 끝나면 셸이 지운다.
pub const FRONT_VAR: &str = "PITWALL_PATH_FRONT";
/// zsh: 이 터미널의 래퍼 폴더.
pub const ZDOTDIR_VAR: &str = "PITWALL_ZDOTDIR";
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
            let dir = tempfile::Builder::new().prefix("pitwall-zsh-").tempdir()?;
            for (name, body) in ZSH_FILES {
                std::fs::write(dir.path().join(name), body)?;
            }
            std::fs::write(dir.path().join(ZSH_FINISH_FILE), ZSH_FINISH)?;
            env.push(("ZDOTDIR".into(), dir.path().into()));
            env.push((ZDOTDIR_VAR.into(), dir.path().into()));
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
            let dir = tempfile::Builder::new().prefix("pitwall-bash-").tempdir()?;
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

/// zsh 래퍼 파일의 앞부분: 사용자의 `ZDOTDIR` 로 돌려 같은 이름의 파일을 읽고, 그 파일이 바꾼 `ZDOTDIR` 을 기억한 뒤 래퍼 폴더로
/// 돌아온다.
macro_rules! zsh_wrapper {
    ($file:literal) => {
        concat!(
            "# Pitwall 터미널(crates/pw-run/src/integration.rs). 사용자의 ",
            $file,
            " 을 읽는다.\n",
            "if [[ -n ${PITWALL_USER_ZDOTDIR+x} ]]; then ZDOTDIR=$PITWALL_USER_ZDOTDIR; else unset ZDOTDIR; fi\n",
            "[[ -r ${ZDOTDIR:-$HOME}/",
            $file,
            " ]] && builtin source -- \"${ZDOTDIR:-$HOME}/",
            $file,
            "\"\n",
            "if [[ -n ${ZDOTDIR+x} ]]; then PITWALL_USER_ZDOTDIR=$ZDOTDIR; else unset PITWALL_USER_ZDOTDIR; fi\n",
            "ZDOTDIR=$PITWALL_ZDOTDIR\n",
        )
    };
}

/// 시작이 끝났을 때: `ZDOTDIR` 을 사용자의 값으로 되돌리고 제공자 항목을 PATH 맨 앞으로 옮긴다.
const ZSH_FINISH: &str = r#"if [[ -n ${PITWALL_USER_ZDOTDIR+x} ]]; then ZDOTDIR=$PITWALL_USER_ZDOTDIR; else unset ZDOTDIR; fi
() {
  local -a front
  front=(${(s.:.)PITWALL_PATH_FRONT})
  path=($front ${path:|front})
}
unset PITWALL_PATH_FRONT PITWALL_USER_ZDOTDIR PITWALL_ZDOTDIR
"#;

const ZSH_FILES: [(&str, &str); 4] = [
    (".zshenv", zsh_wrapper!(".zshenv")),
    (".zprofile", zsh_wrapper!(".zprofile")),
    (
        ".zshrc",
        concat!(
            zsh_wrapper!(".zshrc"),
            "# 로그인 셸이 아니면 .zlogin 이 없다 — 여기서 끝낸다.\n",
            "if [[ ! -o login ]]; then\n",
            "  builtin source -- \"$PITWALL_ZDOTDIR/.pitwall-finish\"\n",
            "fi\n",
        ),
    ),
    (
        ".zlogin",
        concat!(
            zsh_wrapper!(".zlogin"),
            "builtin source -- \"$PITWALL_ZDOTDIR/.pitwall-finish\"\n",
        ),
    ),
];

/// zsh 의 끝내기 파일 이름. `ZSH_FILES` 와 함께 쓴다.
const ZSH_FINISH_FILE: &str = ".pitwall-finish";

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
        assert!(dir.join(".zlogin").is_file());
        assert!(zsh.env.contains(&("ZDOTDIR".into(), dir.into())));
        assert!(zsh.env.contains(&(USER_ZDOTDIR_VAR.into(), "/cfg".into())));
        assert!(zsh.env.contains(&(FRONT_VAR.into(), "/mise/bin".into())));
    }
}
