//! 가짜 `git` 실행 파일(unix sh 스크립트). 실행기의 버전 판정 · 자식 환경 · 취소를 진짜 git 없이 본다.
//!
//! - `git --version` → `git version <버전>`.
//! - `git sleep` → 자기 pid 를 `git.pid` 에, 30초 자는 자식의 pid 를 `child.pid` 에 적고 그 자식을 기다린다.
//! - `git stubborn` → `sleep` 과 같되 자식이 SIGTERM 을 무시한다(훅이 띄운 고집 센 프로세스).
//! - 그 밖의 인자 → 받은 인자(NUL 구분)를 `argv` 에, 환경(`env` 출력)을 `env` 에, 실제 cwd 를 `cwd` 에 적는다.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub struct FakeGit {
    path: PathBuf,
    out: PathBuf,
}

impl FakeGit {
    /// `bin/<name>` 에 만든다. 기록은 `bin/<name>.out/` 에 남는다.
    pub fn new(bin: &Path, name: &str, version: &str) -> Self {
        let path = bin.join(name);
        let out = bin.join(format!("{name}.out"));
        fs::create_dir_all(&out).unwrap();
        fs::write(
            &path,
            format!(
                "#!/bin/sh\nout='{out}'\ncase \"$1\" in\n  --version) printf 'git version %s\\n' '{version}'; exit 0 ;;\n  sleep) echo $$ > \"$out/git.pid.tmp\"; sleep 30 & echo $! > \"$out/child.pid.tmp\"; mv \"$out/child.pid.tmp\" \"$out/child.pid\"; mv \"$out/git.pid.tmp\" \"$out/git.pid\"; wait; exit 0 ;;\n  stubborn) echo $$ > \"$out/git.pid.tmp\"; (trap '' TERM; exec sleep 30) & echo $! > \"$out/child.pid.tmp\"; mv \"$out/child.pid.tmp\" \"$out/child.pid\"; mv \"$out/git.pid.tmp\" \"$out/git.pid\"; wait; exit 0 ;;\nesac\nfor a in \"$@\"; do printf '%s\\0' \"$a\"; done > \"$out/argv\"\nenv > \"$out/env\"\npwd -P > \"$out/cwd\"\n",
                out = out.display(),
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        Self { path, out }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 기록 파일 하나(`argv` · `env` · `cwd` · `git.pid` · `child.pid`). 없으면 `None`.
    pub fn record(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.out.join(name)).ok()
    }

    /// 마지막으로 받은 환경. 값에 줄바꿈이 없는 변수만 바르게 읽힌다.
    pub fn env_var(&self, name: &str) -> Option<String> {
        let env = self.record("env")?;
        env.lines()
            .find_map(|l| l.strip_prefix(&format!("{name}=")).map(str::to_owned))
    }

    pub fn received_args(&self) -> Option<Vec<String>> {
        let raw = self.record("argv")?;
        Some(
            raw.split('\0')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
        )
    }
}
