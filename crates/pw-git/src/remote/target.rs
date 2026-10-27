//! push 대상(13-m2-plan.md M2-09 「push 대상 결정 규칙」): 그 브랜치를 체크아웃하고 터미널에서 `git push [<원격>]` 하면 git 이 보낼
//! 곳을 git 의 규칙(`builtin/push.c` · `remote.c`)대로 셈하고, 그 대상을 `refs/heads/<브랜치>:<대상>` 으로 명시해 보낸다 — lease 의
//! ref 가 실제로 바뀌는 ref 와 같게. git 이 거절할 상황이면 아무것도 보내지 않고 그 까닭([`GitPushRefusal`])으로 거절한다. 사람이
//! 대상을 입력으로 주면 그것이다. `git.push.plan` 과 `git.push` 는 이 [`plan`] 하나로 셈한다.
//!
//! - 원격: 입력 → `branch.<b>.pushRemote` → `remote.pushDefault` → `branch.<b>.remote` → 원격이 하나뿐이면 그것 → `origin`.
//! - `remote.<원격>.mirror` 면 거절한다 — git 이 명시한 refspec 과 함께 받지 않는다.
//! - `remote.<원격>.push` 가 있으면 그것(`push.default` 는 쓰이지 않는다). 정확한 이름(`HEAD` 는 그 브랜치)을 먼저 맞추고, 그다음
//!   로컬 ref 마다 처음 맞는 패턴, 패턴이 없으면 `:`(matching — 원격에 같은 이름이 있을 때만)이다. 이미 다른 것을 보내는 원격 ref
//!   에는 보내지 않고, `^` 가 고른 것은 뺀다. 이 브랜치를 보내는 것이 여럿이면(정확한 이름과 패턴이 둘 다 맞으면 git 은 둘 다
//!   보낸다) 처음 것 — 정확한 이름이 먼저다. 정확한 이름 둘이 한 원격 ref 로 보내면 git 처럼 거절한다. 정확한 이름의 대상이
//!   짧으면(`refs/` 로 시작하지 않으면) 거절한다 — git 은 그 이름을 원격이 광고하는 ref 에서 찾아(`refs/heads/` 와 `refs/tags/`
//!   에 다 있으면 거절) 정하는데, 이 계산은 원격의 ref 를 보지 않는다.
//! - 없으면 `push.default`(없으면 `simple`): `nothing` · `matching`(맞는 브랜치를 모두 보내는 push 는 하지 않는다)은 거절,
//!   `current` 는 같은 이름, `upstream`(`tracking`)은 upstream 브랜치(원격이 upstream 원격일 때만), `simple` 은 원격이 upstream
//!   원격이면 upstream 의 이름이 같을 때만 같은 이름(다르면 거절)이고 다른 원격이면 `current` 와 같다. upstream 이 없으면
//!   거절하되 `push.autoSetupRemote` 면 같은 이름이다. `upstream` 의 `merge` 가 짧으면 짧은 대상과 같은 까닭으로 거절한다.
//!
//! 「원격이 하나뿐이면」 의 개수에는 설정의 원격 이름에 더해 push 원격(입력 · 설정의 주소나 설정에 없는 이름)도 센다 — git 은 그
//! 원격도 원격 목록에 넣고 나서 upstream 원격을 다시 셈한다(`remote.c` 의 `make_remote` · `remote_for_branch`).
//!
//! 지금 브랜치가 아닌 브랜치도 그 브랜치가 지금 브랜치인 것처럼 셈한다. 네트워크를 쓰지 않으므로 원격에 무엇이 있는지(`:` 의
//! 「원격에 같은 이름」)는 마지막 fetch · push 가 남긴 원격 추적 브랜치로 본다. 신뢰와 상관없이 저장소 설정을 읽는다 — 값은
//! 이름일 뿐 아무것도 띄우지 않는다(`crate::status::refs` 와 같다).

use std::collections::BTreeSet;

use gix::bstr::ByteSlice;
use serde::{Deserialize, Serialize};

use super::{PushPlan, redact};

/// git 이 그 push 를 거절할 까닭.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum GitPushRefusalReason {
    /// `push.default = nothing`.
    Nothing,
    /// `push.default = matching` — 맞는 브랜치를 모두 보내는 push 는 하지 않는다.
    Matching,
    /// 원격이 upstream 원격인데(`simple` · `upstream`) upstream 이 없다.
    NoUpstream,
    /// `simple`: upstream 브랜치의 이름이 이 브랜치 이름과 다르다.
    UpstreamNameDiffers,
    /// `upstream`: 그 원격이 upstream 원격이 아니다.
    NotUpstreamRemote,
    /// `branch.<b>.merge` 가 여럿이다.
    MultipleUpstreams,
    /// `remote.<원격>.push` 가 이 브랜치를 보내지 않는다.
    NotPushedByConfig,
    /// `remote.<원격>.mirror` 가 켜져 있다.
    Mirror,
    /// git 이 받지 않는 설정이다(모르는 `push.default`, 잘못됐거나 맞는 로컬 ref 가 없는 push refspec, 한 원격 ref 로 보내는
    /// 정확한 refspec 둘).
    InvalidConfig,
    /// 대상이 짧다(`refs/` 로 시작하지 않는다) — `remote.<원격>.push` 의 정확한 refspec 이나 `upstream` 모드의
    /// `branch.<b>.merge`. git 은 원격이 광고하는 ref 로 그 이름을 정하므로(브랜치일 수도 태그일 수도, 둘 다면 거절) 원격의 ref
    /// 를 보지 않고는 git 이 보낼 곳을 알 수 없다.
    UnqualifiedDestination,
}

/// 거절. 창은 `candidates` 중 하나를 대상(`remoteRef`)으로 명시해 다시 부를 수 있다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitPushRefusal {
    pub reason: GitPushRefusalReason,
    /// 영어 설명(git 의 거절 문구와 같은 뜻). 원격 주소의 사용자 정보는 지웠다.
    pub message: String,
    /// 고를 만한 대상(원격의 전체 ref 이름) — upstream 브랜치(있고 이름이 다르면), 같은 이름 차례. `mirror` 면 비었다.
    /// `unqualifiedDestination` 이고 그 짧은 대상이 이 브랜치의 것이면 원격에 그 이름이 없을 때 git 이 고를 것이 맨 앞이다.
    pub candidates: Vec<String>,
}

/// 대상을 정한 것.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum GitPushTargetSource {
    /// 입력의 `remoteRef`.
    Input,
    /// `remote.<원격>.push`.
    PushRefspec,
    /// `push.default`.
    PushDefault,
}

/// 터미널의 `git push [<원격>]` 이 이 push 와 다르게 할 일 — 푸시 다이얼로그(M2-20)가 경고로 보인다. 보통은 없다(`null`).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitPushTerminal {
    /// `remote.<원격>.push` 원문(차례대로). 비었으면 `pushDefault` 를 따른다.
    pub configured: Vec<String>,
    /// `push.default`(없으면 `simple`).
    pub push_default: String,
    /// git 이 보낼 것(`[+]<로컬 ref>:<원격 ref>`, 로컬 ref 가 비었으면 그 원격 ref 를 지운다), 많아야 [`MAX_SENDS`] 개. 원격 ref
    /// 가 짧으면(`refs/` 로 시작하지 않으면) 설정에 적힌 그대로다 — git 이 원격의 ref 로 정한다.
    pub sends: Vec<String>,
    /// `sends` 가 잘렸다.
    pub truncated: bool,
    /// git 이 거절할 까닭(그때 `sends` 는 비었다).
    pub refusal: Option<GitPushRefusal>,
}

/// force 의 기대값을 읽을 곳: 대상의 원격 추적 ref 와 지금 가리키는 커밋. 창은 이 `commit` 을 `forceWithLease` 로 준다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitPushLease {
    /// `refs/remotes/…`.
    pub tracking_ref: String,
    /// `null` 이면 그 원격 추적 ref 가 없다(원격에 아직 없는 브랜치).
    pub commit: Option<String>,
}

/// [`GitPushTerminal::sends`] 의 상한.
pub const MAX_SENDS: usize = 100;

/// git 이 보낼 ref 하나. `src` 가 비었으면 원격 `dst` 를 지운다. `dst` 가 `refs/` 로 시작하지 않으면 설정의 짧은 대상 그대로다.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Mapping {
    pub src: String,
    pub dst: String,
    pub force: bool,
}

impl Mapping {
    fn display(&self) -> String {
        format!(
            "{}{}:{}",
            if self.force { "+" } else { "" },
            self.src,
            self.dst
        )
    }
}

/// 셈한 계획.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// 로컬 브랜치의 짧은 이름.
    pub branch: String,
    /// 원격 이름(또는 설정 · 입력의 주소 원문 — 응답에는 [`redact`] 를 거쳐 싣는다).
    pub remote: String,
    /// 보낼 원격 ref 와 그것을 정한 것, 또는 거절.
    pub target: Result<(String, GitPushTargetSource), GitPushRefusal>,
    /// 터미널의 `git push [<원격>]` 이 보낼 것, 또는 그 거절.
    pub terminal: Result<Vec<Mapping>, GitPushRefusal>,
    pub configured: Vec<String>,
    pub push_default: String,
    pub lease: Option<GitPushLease>,
}

impl Planned {
    /// 보낼 것. 거절이면 그 까닭.
    pub fn ready(&self) -> Result<PushPlan, GitPushRefusal> {
        let (remote_ref, _) = self.target.clone()?;
        Ok(PushPlan {
            branch: self.branch.clone(),
            remote: self.remote.clone(),
            remote_ref,
        })
    }

    /// 터미널과 다르면 그 내용. 둘 다 거절하거나 터미널이 이 브랜치 하나만 같은 곳으로(강제 없이) 보내면 `None`.
    pub fn terminal_difference(&self) -> Option<GitPushTerminal> {
        let same = match (&self.target, &self.terminal) {
            (Err(_), Err(_)) => true,
            (Ok((dst, _)), Ok(sends)) => {
                sends.len() == 1
                    && sends[0]
                        == Mapping {
                            src: format!("refs/heads/{}", self.branch),
                            dst: dst.clone(),
                            force: false,
                        }
            }
            _ => false,
        };
        if same {
            return None;
        }
        let sends = self.terminal.as_ref().map(Vec::as_slice).unwrap_or(&[]);
        Some(GitPushTerminal {
            configured: self.configured.clone(),
            push_default: self.push_default.clone(),
            sends: sends.iter().take(MAX_SENDS).map(Mapping::display).collect(),
            truncated: sends.len() > MAX_SENDS,
            refusal: self.terminal.clone().err(),
        })
    }
}

/// 설정 읽기(`remote.<r>.push` 처럼 여러 값인 키 포함). 빈 값은 없는 것으로 본다.
struct Config<'a> {
    file: &'a gix::config::File,
}

impl Config<'_> {
    fn values(&self, section: &str, subsection: Option<&str>, key: &str) -> Vec<String> {
        self.file
            .strings_by(section, subsection.map(|s| s.as_bytes().as_bstr()), key)
            .unwrap_or_default()
            .iter()
            .map(|v| v.to_str_lossy().into_owned())
            .filter(|v| !v.is_empty())
            .collect()
    }

    fn value(&self, section: &str, subsection: Option<&str>, key: &str) -> Option<String> {
        self.file
            .string_by(section, subsection.map(|s| s.as_bytes().as_bstr()), key)
            .map(|v| v.to_str_lossy().into_owned())
            .filter(|v| !v.is_empty())
    }

    fn flag(&self, section: &str, subsection: Option<&str>, key: &str) -> bool {
        matches!(
            self.file
                .boolean_by(section, subsection.map(|s| s.as_bytes().as_bstr()), key),
            Ok(Some(true))
        )
    }
}

/// 사람이 준 대상: `refs/` 로 시작하면 그 전체 이름, 아니면 브랜치 이름(`refs/heads/<이름>`). git 의 ref 이름 규칙에 맞아야 한다.
pub fn remote_ref_input(value: &str) -> Result<String, String> {
    let full = if value.starts_with("refs/") {
        value.to_owned()
    } else {
        format!("refs/heads/{value}")
    };
    if value.starts_with('-') || gix::validate::reference::name(full.as_bytes().as_bstr()).is_err()
    {
        return Err(format!("the push target `{value}` is not a valid ref name"));
    }
    Ok(full)
}

/// 보낼 곳을 셈한다([모듈 주석](self)). 브랜치는 `branch`(없으면 HEAD 의 브랜치), 로컬에 있어야 한다. 입력 오류(분리된 HEAD,
/// 없는 브랜치, 잘못된 대상 이름)는 `Err`.
pub fn plan(
    repo: &gix::Repository,
    branch: Option<&str>,
    remote: Option<&str>,
    remote_ref: Option<&str>,
) -> Result<Planned, String> {
    let branch = match branch {
        Some(branch) => branch.to_owned(),
        None => {
            let head = repo
                .head_name()
                .map_err(|e| e.to_string())?
                .ok_or("HEAD is detached; choose a branch to push")?;
            head.as_bstr()
                .strip_prefix(b"refs/heads/")
                .ok_or("HEAD is not on a local branch; choose a branch to push")?
                .to_str_lossy()
                .into_owned()
        }
    };
    let local = format!("refs/heads/{branch}");
    if !matches!(repo.try_find_reference(local.as_str()), Ok(Some(_))) {
        return Err(format!("there is no local branch `{branch}` to push"));
    }
    let input = remote_ref.map(remote_ref_input).transpose()?;
    let snapshot = repo.config_snapshot();
    let config = Config {
        file: snapshot.plumbing(),
    };

    let upstream_remote = config.value("branch", Some(&branch), "remote");
    // git 의 `remote_for_branch`: upstream 원격, 원격이 하나뿐이면 그것, 아니면 `origin`.
    let remote_for_branch = |remotes: &BTreeSet<String>| {
        upstream_remote
            .clone()
            .or_else(|| match remotes.iter().collect::<Vec<_>>().as_slice() {
                [one] => Some((*one).clone()),
                _ => None,
            })
            .unwrap_or_else(|| "origin".to_owned())
    };
    let mut remotes: BTreeSet<String> = repo
        .remote_names()
        .iter()
        .map(|r| r.to_str_lossy().into_owned())
        .collect();
    let remote = remote
        .map(str::to_owned)
        .or_else(|| config.value("branch", Some(&branch), "pushRemote"))
        .or_else(|| config.value("remote", None, "pushDefault"))
        .unwrap_or_else(|| remote_for_branch(&remotes));
    // git 은 push 원격(주소 · 설정에 없는 이름도)을 원격 목록에 넣은 뒤 upstream 원격을 다시 셈한다.
    remotes.insert(remote.clone());
    let default_remote = remote_for_branch(&remotes);
    let shown = redact(&remote);

    let merges: Vec<String> = config.values("branch", Some(&branch), "merge");
    let candidates = {
        let mut out = Vec::new();
        if let [merge] = merges.as_slice() {
            let merge = qualify_branch(merge);
            if merge != local {
                out.push(merge);
            }
        }
        out.push(local.clone());
        out
    };
    let refuse = |reason: GitPushRefusalReason, message: String| GitPushRefusal {
        reason,
        message,
        candidates: if reason == GitPushRefusalReason::Mirror {
            Vec::new()
        } else {
            candidates.clone()
        },
    };

    let configured = config.values("remote", Some(&remote), "push");
    let push_default = config
        .value("push", None, "default")
        .unwrap_or_else(|| "simple".to_owned());
    let mirror = config.flag("remote", Some(&remote), "mirror");

    // 터미널의 `git push [<원격>]` 이 보낼 것과, 입력이 없을 때 이 push 의 대상.
    let (terminal, default_target) = if mirror {
        let refusal = refuse(
            GitPushRefusalReason::Mirror,
            format!(
                "remote `{shown}` is a mirror (remote.{shown}.mirror); git pushes every ref there and cannot push one branch"
            ),
        );
        (Err(refusal.clone()), Err(refusal))
    } else if !configured.is_empty() {
        match configured_mappings(repo, &remote, &local, &configured) {
            Ok(sends) => {
                let target = if let Some(short) = sends.iter().find(|m| !m.dst.starts_with("refs/"))
                {
                    let mut refusal = refuse(
                        GitPushRefusalReason::UnqualifiedDestination,
                        format!(
                            "remote.{shown}.push names the destination `{}` without refs/; git resolves it against the refs on `{shown}` (a branch or a tag), which this cannot see; choose where to push `{branch}`",
                            short.dst
                        ),
                    );
                    if let Some(guess) = sends
                        .iter()
                        .find(|m| m.src == local && !m.dst.starts_with("refs/"))
                        .and_then(|m| guess_dst(&m.dst, &m.src))
                    {
                        refusal.candidates.retain(|c| *c != guess);
                        refusal.candidates.insert(0, guess);
                    }
                    Err(refusal)
                } else {
                    sends
                        .iter()
                        .find(|m| m.src == local)
                        .map(|m| (m.dst.clone(), GitPushTargetSource::PushRefspec))
                        .ok_or_else(|| {
                            refuse(
                                GitPushRefusalReason::NotPushedByConfig,
                                format!(
                                    "remote.{shown}.push does not push `{branch}`; choose where to push it"
                                ),
                            )
                        })
                };
                (Ok(sends), target)
            }
            Err(message) => {
                let refusal = refuse(GitPushRefusalReason::InvalidConfig, message);
                (Err(refusal.clone()), Err(refusal))
            }
        }
    } else {
        let same_remote = remote == default_remote;
        let upstream_ref = || -> Result<String, GitPushRefusal> {
            if merges.is_empty() && config.flag("push", None, "autoSetupRemote") {
                return Ok(local.clone());
            }
            if merges.is_empty() || upstream_remote.is_none() {
                return Err(refuse(
                    GitPushRefusalReason::NoUpstream,
                    format!(
                        "the branch `{branch}` has no upstream branch; choose where to push it on `{shown}`"
                    ),
                ));
            }
            if merges.len() != 1 {
                return Err(refuse(
                    GitPushRefusalReason::MultipleUpstreams,
                    format!("the branch `{branch}` has multiple upstream branches"),
                ));
            }
            Ok(merges[0].clone())
        };
        let dst: Result<String, GitPushRefusal> = match push_default.as_str() {
            "nothing" => Err(refuse(
                GitPushRefusalReason::Nothing,
                format!("push.default is \"nothing\"; choose where to push `{branch}`"),
            )),
            // 터미널은 원격에 있는 브랜치를 모두 보낸다 — 그것을 보이되 이 push 는 하지 않는다.
            "matching" => Err(refuse(
                GitPushRefusalReason::Matching,
                format!(
                    "push.default is \"matching\", which pushes every branch that exists on the remote; choose where to push `{branch}`"
                ),
            )),
            "current" => Ok(local.clone()),
            "upstream" | "tracking" if !same_remote => Err(refuse(
                GitPushRefusalReason::NotUpstreamRemote,
                format!(
                    "`{shown}` is not the upstream remote of `{branch}` (push.default is upstream); choose where to push it"
                ),
            )),
            // git 은 `merge` 값을 그대로 대상으로 쓴다 — 짧으면 원격의 ref 로 정한다(아래에서 거절).
            "upstream" | "tracking" => upstream_ref(),
            "simple" if !same_remote => Ok(local.clone()),
            "simple" => upstream_ref().and_then(|up| {
                if up == local {
                    Ok(local.clone())
                } else {
                    Err(refuse(
                        GitPushRefusalReason::UpstreamNameDiffers,
                        format!(
                            "the upstream of `{branch}` is `{up}`, whose name differs from the branch (push.default is simple); choose where to push it"
                        ),
                    ))
                }
            }),
            other => Err(refuse(
                GitPushRefusalReason::InvalidConfig,
                format!(
                    "push.default `{other}` is not one of nothing, matching, simple, upstream or current"
                ),
            )),
        };
        let terminal = if push_default == "matching" {
            Ok(matching_mappings(repo, &remote, false))
        } else {
            dst.clone().map(|dst| {
                vec![Mapping {
                    src: local.clone(),
                    dst,
                    force: false,
                }]
            })
        };
        let target = dst.and_then(|dst| {
            if dst.starts_with("refs/") {
                Ok((dst, GitPushTargetSource::PushDefault))
            } else {
                Err(refuse(
                    GitPushRefusalReason::UnqualifiedDestination,
                    format!(
                        "the upstream of `{branch}` is the short name `{dst}`; git resolves it against the refs on `{shown}` (a branch or a tag), which this cannot see; choose where to push it"
                    ),
                ))
            }
        });
        (terminal, target)
    };
    // 사람이 준 대상이 이긴다 — mirror 만은 git 이 명시한 refspec 을 받지 않는다.
    let target = match input {
        Some(input) if !mirror => Ok((input, GitPushTargetSource::Input)),
        _ => default_target,
    };
    let lease = target.as_ref().ok().and_then(|(dst, _)| {
        let tracking = crate::status::refs::remote_tracking(
            repo,
            remote.as_bytes().as_bstr(),
            dst.as_bytes().as_bstr(),
        )?;
        let tracking = tracking.to_str_lossy().into_owned();
        Some(GitPushLease {
            commit: ref_target(repo, &tracking),
            tracking_ref: tracking,
        })
    });
    Ok(Planned {
        branch,
        remote,
        target,
        terminal,
        configured,
        push_default,
        lease,
    })
}

fn ref_target(repo: &gix::Repository, name: &str) -> Option<String> {
    repo.try_find_reference(name)
        .ok()
        .flatten()
        .and_then(|mut r| r.peel_to_id().ok().map(|id| id.to_string()))
}

/// `branch.<b>.merge` 처럼 짧을 수 있는 브랜치 이름의 전체 이름.
fn qualify_branch(name: &str) -> String {
    if name.starts_with("refs/") {
        name.to_owned()
    } else {
        format!("refs/heads/{name}")
    }
}

/// 로컬 ref 의 전체 이름(차례대로).
fn local_refs(repo: &gix::Repository) -> Vec<String> {
    let Ok(platform) = repo.references() else {
        return Vec::new();
    };
    let Ok(all) = platform.all() else {
        return Vec::new();
    };
    let mut names: Vec<String> = all
        .filter_map(Result::ok)
        .map(|r| r.name().as_bstr().to_str_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// 원격에 `name`(로컬과 같은 전체 이름)이 있는가 — 브랜치만, 그 원격 추적 ref 가 있으면 있다고 본다.
fn remote_has(repo: &gix::Repository, remote: &str, name: &str) -> bool {
    name.starts_with("refs/heads/")
        && crate::status::refs::remote_tracking(
            repo,
            remote.as_bytes().as_bstr(),
            name.as_bytes().as_bstr(),
        )
        .is_some_and(|tracking| ref_target(repo, &tracking.to_str_lossy()).is_some())
}

/// `:`(matching): 원격에 같은 이름이 있는 로컬 ref 전부.
fn matching_mappings(repo: &gix::Repository, remote: &str, force: bool) -> Vec<Mapping> {
    local_refs(repo)
        .into_iter()
        .filter(|name| remote_has(repo, remote, name))
        .map(|name| Mapping {
            src: name.clone(),
            dst: name,
            force,
        })
        .collect()
}

/// push refspec 한 줄(`[+|^]<src>[:<dst>]`).
struct Item<'a> {
    src: &'a str,
    dst: Option<&'a str>,
    force: bool,
    negative: bool,
}

impl<'a> Item<'a> {
    fn parse(spec: &'a str) -> Self {
        let (force, rest) = match spec.strip_prefix('+') {
            Some(rest) => (true, rest),
            None => (false, spec),
        };
        let (negative, rest) = match rest.strip_prefix('^') {
            Some(rest) if !force => (true, rest),
            _ => (false, rest),
        };
        let (src, dst) = match rest.split_once(':') {
            Some((src, dst)) => (src, Some(dst)),
            None => (rest, None),
        };
        Item {
            src,
            dst,
            force,
            negative,
        }
    }

    /// `:` — 원격에 같은 이름이 있는 것을 보낸다.
    fn matching(&self) -> bool {
        !self.negative && self.src.is_empty() && self.dst.is_none_or(str::is_empty)
    }

    fn pattern(&self) -> bool {
        self.src.contains('*')
    }
}

/// git 의 `match_name_with_pattern`: `key` 의 `*` 하나가 `name` 의 가운데에 맞으면 `value` 의 `*` 를 그것으로 바꾼 것.
fn glob(key: &str, value: &str, name: &str) -> Option<String> {
    let (prefix, suffix) = key.split_once('*')?;
    if name.len() < prefix.len() + suffix.len()
        || !name.starts_with(prefix)
        || !name.ends_with(suffix)
    {
        return None;
    }
    let middle = &name[prefix.len()..name.len() - suffix.len()];
    Some(value.replacen('*', middle, 1))
}

/// git 의 `count_refspec_match`: 짧은 이름 `pattern` 에 맞는 로컬 ref 하나. `refs/heads/` · `refs/tags/` 밖의 것은 이름을 다 쓰지
/// 않았으면 약하게 맞는다 — 강한 것이 하나면 그것, 없고 약한 것이 하나면 그것이다.
fn dwim(refs: &[String], pattern: &str) -> Result<String, String> {
    let rules = [
        pattern.to_owned(),
        format!("refs/{pattern}"),
        format!("refs/tags/{pattern}"),
        format!("refs/heads/{pattern}"),
        format!("refs/remotes/{pattern}"),
        format!("refs/remotes/{pattern}/HEAD"),
    ];
    let (mut strong, mut weak) = (Vec::new(), Vec::new());
    for name in refs.iter().filter(|name| rules.contains(name)) {
        let full = name.len() == pattern.len() || name.len() == pattern.len() + 5;
        if full || name.starts_with("refs/heads/") || name.starts_with("refs/tags/") {
            strong.push(name);
        } else {
            weak.push(name);
        }
    }
    match (strong.as_slice(), weak.as_slice()) {
        ([one], _) | ([], [one]) => Ok((*one).clone()),
        ([], []) => Err(format!(
            "the configured push refspec source `{pattern}` does not match any local ref"
        )),
        _ => Err(format!(
            "the configured push refspec source `{pattern}` matches more than one local ref"
        )),
    }
}

/// 짧은 대상 이름에 원격의 어느 ref 도 맞지 않을 때 git 이 고르는 것(`guess_ref`): 보내는 쪽의 이름 공간(`refs/heads/` ·
/// `refs/tags/`). 지우기거나 그 밖의 이름 공간이면 git 은 거절한다(`None`).
fn guess_dst(dst: &str, src: &str) -> Option<String> {
    ["refs/heads/", "refs/tags/"]
        .into_iter()
        .find(|space| src.starts_with(space))
        .map(|space| format!("{space}{dst}"))
}

/// `remote.<원격>.push` 가 보낼 것(git 의 `match_push_refs`). `local` 은 이 브랜치(`HEAD` 가 가리키는 것으로 본다).
fn configured_mappings(
    repo: &gix::Repository,
    remote: &str,
    local: &str,
    specs: &[String],
) -> Result<Vec<Mapping>, String> {
    for spec in specs {
        gix::refspec::parse(
            spec.as_bytes().as_bstr(),
            gix::refspec::parse::Operation::Push,
        )
        .map_err(|e| format!("invalid push refspec `{spec}`: {e}"))?;
    }
    let items: Vec<Item> = specs.iter().map(|s| Item::parse(s)).collect();
    let refs = local_refs(repo);
    let mut out = Vec::new();
    let mut dsts = BTreeSet::new();
    for item in items
        .iter()
        .filter(|i| !i.negative && !i.matching() && !i.pattern())
    {
        // 짧은 대상은 설정 그대로 둔다(원격의 ref 로 정해진다 — [`plan`] 이 거절한다).
        let (src, dst) = if item.src.is_empty() {
            // `:<dst>` 는 원격의 그 ref 를 지운다.
            (String::new(), item.dst.unwrap_or_default().to_owned())
        } else {
            let src = if item.src == "HEAD" {
                local.to_owned()
            } else {
                dwim(&refs, item.src)?
            };
            let dst = match item.dst.filter(|d| !d.is_empty()) {
                Some(dst) => dst.to_owned(),
                None => src.clone(),
            };
            (src, dst)
        };
        // git 의 `match_explicit`: 한 원격 ref 가 둘에게서 받으면 push 전체를 거절한다. 짧은 대상은 원격에 그 이름이 없을 때 git
        // 이 고를 것으로 견준다.
        let key = guess_dst(&dst, &src)
            .filter(|_| !dst.starts_with("refs/"))
            .unwrap_or_else(|| dst.clone());
        if !dsts.insert(key.clone()) {
            return Err(format!(
                "the configured push refspecs send more than one source to `{key}` (dst ref receives from more than one src)"
            ));
        }
        out.push(Mapping {
            src,
            dst,
            force: item.force,
        });
    }
    for name in &refs {
        let mut matching = None;
        let mut found = None;
        for item in &items {
            if item.negative {
                continue;
            }
            if item.matching() {
                if matching.is_none() || item.force {
                    matching = Some(item);
                }
                continue;
            }
            if item.pattern()
                && let Some(dst) = glob(item.src, item.dst.unwrap_or(item.src), name)
            {
                found = Some((item, dst));
                break;
            }
        }
        let (dst, force) = match (found, matching) {
            (Some((item, dst)), _) => (dst, item.force),
            (None, Some(item)) if remote_has(repo, remote, name) => (name.clone(), item.force),
            _ => continue,
        };
        if !dsts.insert(dst.clone()) {
            // 그 원격 ref 에는 이미 다른 것을 보낸다.
            continue;
        }
        out.push(Mapping {
            src: name.clone(),
            dst,
            force,
        });
    }
    out.retain(|m| {
        m.src.is_empty()
            || !items.iter().filter(|i| i.negative).any(|i| {
                if i.pattern() {
                    glob(i.src, i.src, &m.src).is_some()
                } else {
                    i.src == m.src
                }
            })
    });
    Ok(out)
}
