//! 원격 픽스처: 시험 저장소 옆의 로컬 bare 저장소(`file://`)와, 그 원격을 따로 움직이는 다른 클론. 진짜 원격 · 사람의 credential
//! 을 쓰지 않는다.

use std::path::{Path, PathBuf};

use super::repo::TestRepo;

/// 로컬 bare 원격.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    /// bare 저장소 폴더.
    pub dir: PathBuf,
    /// `file://…` 주소.
    pub url: String,
}

impl TestRepo {
    /// [`TestRepo::base`] 아래 `<name>.git` 에 bare 저장소를 만들어 `origin` 으로 더하고, 지금 브랜치를 보내 upstream 으로 정한다.
    /// 커밋이 하나는 있어야 한다.
    pub fn add_bare_remote(&self, name: &str) -> Remote {
        let dir = self.base().join(format!("{name}.git"));
        let path = dir.to_str().expect("utf-8 temp path");
        self.git_in(self.base(), &["init", "--bare", "--quiet", path]);
        let url = format!("file://{path}");
        self.git(&["remote", "add", "origin", &url]);
        self.git(&["push", "--quiet", "--set-upstream", "origin", "HEAD"]);
        Remote { dir, url }
    }

    /// `remote` 를 [`TestRepo::base`] 아래 `name` 에 클론한다(원격을 이 저장소 몰래 움직일 때 쓴다). 그 꼭대기를 돌려준다.
    pub fn clone_remote(&self, remote: &Remote, name: &str) -> PathBuf {
        let path = self.base().join(name);
        self.git_in(
            self.base(),
            &[
                "clone",
                "--quiet",
                &remote.url,
                path.to_str().expect("utf-8 temp path"),
            ],
        );
        path
    }

    /// 다른 클론 `dir` 에 파일 하나를 커밋하고 그 브랜치 `branch` 를 원격에 보낸다. 새 커밋의 해시를 돌려준다.
    pub fn push_from(&self, dir: &Path, branch: &str, rel: &str, contents: &str) -> String {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        let commit = self.commit_all_in(dir, &format!("{branch}: {rel}"));
        self.git_in(
            dir,
            &[
                "push",
                "--quiet",
                "origin",
                &format!("HEAD:refs/heads/{branch}"),
            ],
        );
        commit
    }

    /// 원격의 `reference`(전체 이름)가 가리키는 커밋. 없으면 `None`.
    pub fn remote_ref(&self, remote: &Remote, reference: &str) -> Option<String> {
        let out = self.try_git_in(
            self.base(),
            &[
                "--git-dir",
                remote.dir.to_str().expect("utf-8 temp path"),
                "rev-parse",
                "--verify",
                "--quiet",
                reference,
            ],
        );
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    /// 로컬 브랜치 `branch` 의 upstream 짧은 이름(`origin/feat`). 없으면 `None`.
    pub fn upstream_of(&self, branch: &str) -> Option<String> {
        let out = self.try_git_in(
            self.root(),
            &[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                &format!("{branch}@{{upstream}}"),
            ],
        );
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }
}
