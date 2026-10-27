//! 커밋 로그 · 그래프 레인 · 필터(`docs/design/13-m2-plan.md` M2-03 — 결정 M2-D1 · M2-D2 · M2-D7).
//!
//! - [`walk`]: 시작 ref 에서 닿는 커밋을 gix 로 모으고(commit-graph 가 있으면 쓴다) git 의 `--topo-order` · `--date-order` 와 같은
//!   순서를 낸다. 경로 필터의 이력 단순화도 여기서 한다.
//! - [`graph`]: 행 순서대로 레인을 배치한다(노드 레인 · 지나가는 간선 · 모이고 갈라지는 간선).
//! - [`filter`]: 브랜치 밖의 필터. 하나라도 켜지면 그래프 없이 맞는 커밋만 낸다(결정 M2-D2).
//! - [`methods`]: 스트림 `git.log` 와 쪽 단위 `git.logPage`.
//!
//! 순서를 정하려면 시작 ref 에서 닿는 커밋을 모두 모아야 한다(자식이 모두 나온 뒤에야 부모가 나온다 — commit-graph 의 세대 번호가
//! 없으면 미리 알 길이 없다). 모은 뒤의 배치 · 메시지 읽기는 행마다 하므로, 첫 청크([`FIRST_CHUNK`] 행)를 배치하자마자 보낸다.
//!
//! 저장소의 프로그램을 띄우지 않는다: 읽기는 모두 gix 의 객체 · ref 읽기이고(필터 드라이버 · textconv 를 고르지 않는다), 저장소는
//! [`crate::Git::open`] 으로 열린 것만 받는다(미신뢰 열기는 gix 가 보안 키를 읽을 때 저장소 설정을 거른다 — `config_snapshot()`
//! 으로 읽는 값(`log.mailmap`)은 거르지 않는다, [`crate::repo::open_options`]). git CLI 는 부르지 않는다 — 미신뢰 프로젝트에서도
//! 같은 경로다. 요청 하나 동안만 저장소 핸들을 쥐고 캐시를 남기지 않으므로 신뢰가 바뀌어도 다음 요청이 새로 연다.

pub mod commit;
pub mod filter;
pub mod graph;
pub mod methods;
pub mod walk;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gix::ObjectId;
use gix::bstr::ByteSlice;
use gix::objs::FindExt;
use pw_rpc::RpcError;
use serde::{Deserialize, Serialize};

pub use self::filter::Filter;
pub use self::graph::{GraphRow, Layout};

/// 첫 청크의 행 수(화면 한 쪽).
pub const FIRST_CHUNK: usize = 200;
/// 그 뒤 청크의 행 수.
pub const CHUNK: usize = 500;
/// 걸러진 목록에서 맞는 행이 드물 때, 모인 행을 이만큼 기다린 뒤 보낸다.
const FLUSH_AFTER: Duration = Duration::from_millis(100);
/// `git.logPage` 의 `limit` 상한.
pub const MAX_PAGE: u32 = 500;

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum LogOrder {
    /// `git log --topo-order`: 자식이 모두 나온 뒤 부모, 한 줄의 이력을 섞지 않는다(머지 다음에는 머지된 쪽 줄이 먼저).
    #[default]
    Topo,
    /// `git log --date-order`: 자식이 모두 나온 뒤 부모, 그 안에서 커미터 시각이 늦은 것부터.
    Date,
}

/// 브랜치 밖의 필터(F5-2). 하나라도 있으면 그래프 없이 맞는 커밋만 온다(결정 M2-D2). 판정은 [`filter`] 머리 주석의 git 옵션과 같다.
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LogFilter {
    /// 작성자 「이름 <메일>」 에 맞출 패턴(`--author`).
    #[serde(default)]
    pub author: Option<String>,
    /// 메시지의 한 줄에 맞출 패턴(`--grep`).
    #[serde(default)]
    pub message: Option<String>,
    /// `author` · `message` 가 정규식인가(`-E`). 아니면 글자 그대로(`-F`).
    #[serde(default)]
    pub regex: bool,
    /// 대소문자를 가리지 않는다(`-i`).
    #[serde(default)]
    pub ignore_case: bool,
    /// 커미터 시각(유닉스 초)이 이 값 이상(`--since`).
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub since: Option<i64>,
    /// 커미터 시각(유닉스 초)이 이 값 이하(`--until`).
    #[serde(default)]
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub until: Option<i64>,
    /// 저장소 작업 트리 기준 상대 경로(구분자 `/` — Windows 는 `\` 도 받는다), 파일이나 폴더. 하나라도 바꾼 커밋만(`-- <경로>…`,
    /// git 의 기본 이력 단순화).
    #[serde(default)]
    pub paths: Vec<String>,
    /// 해시 접두(16진).
    #[serde(default)]
    pub hash: Option<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum LogRefKind {
    /// 분리된 HEAD(브랜치에 있으면 그 브랜치의 `current` 가 켜진다).
    Head,
    Branch,
    Remote,
    Tag,
}

/// 커밋에 붙은 ref 장식.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LogRef {
    /// 짧은 이름(`main`, `origin/main`, `v1.0`, 분리된 HEAD 는 `HEAD`).
    pub name: String,
    pub kind: LogRefKind,
    /// HEAD 가 가리키는 브랜치인가.
    pub current: bool,
}

/// 로그의 한 행.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LogRow {
    /// 전체 해시(16진).
    pub hash: String,
    /// 부모 해시(커밋에 적힌 순서).
    pub parents: Vec<String>,
    /// 제목(메시지의 첫 문단을 한 줄로 — git 의 `%s`).
    pub subject: String,
    pub author_name: String,
    pub author_email: String,
    /// 작성 시각(유닉스 초).
    #[specta(type = pw_rpc::JsNumber)]
    pub author_time: i64,
    /// 이 커밋을 가리키는 ref(분리된 HEAD · 지금 브랜치가 앞, 다음은 브랜치 · 원격 · 태그의 이름순).
    pub refs: Vec<LogRef>,
    /// 레인 배치. 필터가 켜진 목록이면 `null`(결정 M2-D2).
    pub graph: Option<GraphRow>,
}

/// `git.log` 스트림의 항목 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct LogChunk {
    pub rows: Vec<LogRow>,
    /// 전체 행 수. 그래프 목록의 첫 청크에만 있다(걸러진 목록은 끝까지 걸러 봐야 알므로 `null`).
    #[specta(type = Option<pw_rpc::JsNumber>)]
    pub total: Option<u32>,
}

/// 판정을 마친 요청.
#[derive(Debug, Clone)]
pub struct Query {
    /// 시작 ref(브랜치 필터). `None` 이면 모든 브랜치 · 원격 · 태그 · HEAD.
    refs: Option<Vec<String>>,
    filter: Filter,
    order: LogOrder,
}

/// 시작 ref 수 상한.
const MAX_REFS: usize = 1024;

impl Query {
    pub fn new(
        refs: Option<Vec<String>>,
        filter: &LogFilter,
        order: LogOrder,
    ) -> Result<Self, RpcError> {
        if let Some(refs) = &refs {
            if refs.is_empty() {
                return Err(RpcError::invalid_params("`refs` is empty"));
            }
            if refs.len() > MAX_REFS {
                return Err(RpcError::invalid_params(format!(
                    "`refs` has more than {MAX_REFS} entries"
                )));
            }
        }
        Ok(Self {
            refs,
            filter: Filter::new(filter)?,
            order,
        })
    }

    /// 그래프(레인)를 내는가.
    pub fn keeps_graph(&self) -> bool {
        self.filter.keeps_graph()
    }
}

/// 내보낼 행의 범위: 커밋 `after` 의 다음 행부터(없으면 처음부터) 많아야 `limit` 행.
#[derive(Debug, Clone, Copy, Default)]
pub struct Window {
    pub after: Option<ObjectId>,
    pub limit: Option<usize>,
}

/// [`run`] 의 결과.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outcome {
    /// `limit` 뒤에 행이 더 있다.
    pub more: bool,
    /// `emit` 이 거절해(받는 쪽이 사라짐) 멈췄다.
    pub abandoned: bool,
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::internal(e.to_string())
}

/// 로그를 만들어 청크로 `emit` 한다(첫 청크 [`FIRST_CHUNK`] 행, 다음은 [`CHUNK`] 행). 그래프 목록은 청크를 적어도 하나 보낸다(빈
/// 저장소면 빈 청크 하나 — `total` 을 알린다). 청크의 첫 행에는 열린 레인 전체([`GraphRow::top`])가 실린다. `emit` 이 `false` 를
/// 돌려주거나 `stop` 이 켜지면 멈춘다(멈춤은 `CANCELLED` 오류). `trusted` 는 연 저장소의 신뢰다(mailmap 설정을 볼지,
/// [`commit::Mailmap::of`]). `window.after` 의 커밋이 로그에 없으면 invalid params(`stale cursor`).
pub fn run(
    repo: &gix::Repository,
    trusted: bool,
    query: &Query,
    window: Window,
    stop: &AtomicBool,
    emit: &mut dyn FnMut(LogChunk) -> bool,
) -> Result<Outcome, RpcError> {
    // ref 목록은 한 번만 읽는다(낱개 ref 파일이 수천 개인 저장소에서 파일 열기가 첫 청크 시간의 대부분이다).
    let refs = branch_refs(repo)?;
    let head = head(repo)?;
    let decorations = decorations(&refs, &head);
    let tips = tips(repo, query.refs.as_deref(), &refs, head.0)?;
    let dag = walk::collect(repo, &tips, &query.filter.paths, stop)?;
    let order = walk::order(&dag, query.order, stop)?;
    let filter = &query.filter;
    let old = filter
        .since()
        .map(|since| walk::before(&dag, &order, since));
    // 첫 커밋을 읽을 때 연다(빈 로그 · 넘기는 행만 있는 쪽은 열지 않는다).
    let mut mailmap: Option<commit::Mailmap> = None;
    let graph = filter.keeps_graph();
    let mut layout =
        graph.then(
            || match head.0.and_then(|h| dag.ids.iter().position(|&id| id == h)) {
                Some(at) => Layout::with_head(&head_chain(&dag, at as u32), dag.len()),
                None => Layout::new(),
            },
        );
    let mut out = Chunker {
        rows: Vec::new(),
        sent: 0,
        total: graph.then_some(order.len() as u32),
        since_flush: Instant::now(),
        timed: !graph,
    };
    let mut passed = window.after.is_none();
    let mut matched = 0usize;
    let mut buf = Vec::new();
    let mut more = false;
    for (n, &i) in order.iter().enumerate() {
        if n % 256 == 0 && stop.load(Ordering::Relaxed) {
            return Err(RpcError::cancelled());
        }
        let id = dag.ids[i as usize];
        // 그래프 목록은 거르는 행이 없다 — 내보낼 행이면 청크의 첫 행인지 지금 안다.
        let top = layout
            .as_ref()
            .filter(|_| passed && out.rows.is_empty())
            .map(Layout::top);
        let row_graph = layout.as_mut().map(|l| GraphRow {
            top,
            ..l.row(i, dag.parents(i))
        });
        if !passed {
            passed = window.after == Some(id);
            continue;
        }
        if dag.hidden(i)
            || old.as_ref().is_some_and(|old| old[i as usize])
            || !filter.matches_cheaply(&id, dag.time[i as usize])
        {
            continue;
        }
        if window.limit.is_some_and(|limit| matched >= limit) {
            // 한 행 더 맞는지만 본다.
            let mailmap = mailmap.get_or_insert_with(|| commit::Mailmap::of(repo, trusted));
            if !filter.needs_commit() || commit_matches(repo, filter, mailmap, &id, &mut buf)? {
                more = true;
                break;
            }
            continue;
        }
        let commit = repo.objects.find_commit(&id, &mut buf).map_err(internal)?;
        let mailmap = mailmap.get_or_insert_with(|| commit::Mailmap::of(repo, trusted));
        let text = commit::Text::read(&commit, mailmap)?;
        if filter.needs_commit() && !filter.matches_commit(&text.ident(), &text.message) {
            continue;
        }
        matched += 1;
        let row = LogRow {
            hash: id.to_string(),
            parents: if dag.is_shallow(&id) {
                Vec::new()
            } else {
                commit.parents().map(|p| p.to_string()).collect()
            },
            subject: commit::subject(&commit),
            author_name: commit::clip(&text.author_name, commit::MAX_IDENT),
            author_email: commit::clip(&text.author_email, commit::MAX_IDENT),
            author_time: text.author_time,
            refs: decorations.get(&id).cloned().unwrap_or_default(),
            graph: row_graph,
        };
        if !out.push(row, emit) {
            return Ok(Outcome {
                more: false,
                abandoned: true,
            });
        }
    }
    if !passed {
        return Err(RpcError::invalid_params(
            "stale cursor: its commit is no longer in the log",
        ));
    }
    if !out.finish(emit) {
        return Ok(Outcome {
            more,
            abandoned: true,
        });
    }
    Ok(Outcome {
        more,
        abandoned: false,
    })
}

/// `at` 에서 첫 부모를 따라 뿌리(또는 얕은 경계)까지 — HEAD 의 줄([`graph`] 머리 주석). 대체 객체(`git replace`)로 고리가 생겨도
/// 끝나게 이미 지난 커밋에서 멈춘다.
fn head_chain(dag: &walk::Dag, at: u32) -> Vec<u32> {
    let mut seen = vec![false; dag.len()];
    let mut chain = Vec::new();
    let mut next = Some(at);
    while let Some(i) = next.filter(|&i| !std::mem::replace(&mut seen[i as usize], true)) {
        chain.push(i);
        next = dag.parents(i).first().copied();
    }
    chain
}

fn commit_matches(
    repo: &gix::Repository,
    filter: &Filter,
    mailmap: &commit::Mailmap,
    id: &ObjectId,
    buf: &mut Vec<u8>,
) -> Result<bool, RpcError> {
    let commit = repo.objects.find_commit(id, buf).map_err(internal)?;
    let text = commit::Text::read(&commit, mailmap)?;
    Ok(filter.matches_commit(&text.ident(), &text.message))
}

/// 행을 청크로 묶는다.
struct Chunker {
    rows: Vec<LogRow>,
    sent: usize,
    /// 첫 청크에 실을 전체 행 수.
    total: Option<u32>,
    since_flush: Instant,
    /// 시간으로도 보낸다(걸러진 목록).
    timed: bool,
}

impl Chunker {
    fn push(&mut self, row: LogRow, emit: &mut dyn FnMut(LogChunk) -> bool) -> bool {
        self.rows.push(row);
        let size = if self.sent == 0 { FIRST_CHUNK } else { CHUNK };
        if self.rows.len() >= size || (self.timed && self.since_flush.elapsed() >= FLUSH_AFTER) {
            return self.flush(emit);
        }
        true
    }

    fn flush(&mut self, emit: &mut dyn FnMut(LogChunk) -> bool) -> bool {
        let rows = std::mem::take(&mut self.rows);
        let total = if self.sent == 0 { self.total } else { None };
        self.sent += 1;
        self.since_flush = Instant::now();
        emit(LogChunk { rows, total })
    }

    /// 남은 행을 보낸다. 아무것도 보내지 않았으면 빈 청크 하나.
    fn finish(&mut self, emit: &mut dyn FnMut(LogChunk) -> bool) -> bool {
        if self.rows.is_empty() && self.sent > 0 {
            return true;
        }
        self.flush(emit)
    }
}

/// ref 하나: 전체 이름, 가리키는 커밋(커밋이 아니면 `None`), 심볼릭 ref 인가.
struct RefTip {
    name: gix::refs::FullName,
    commit: Option<ObjectId>,
    symbolic: bool,
}

/// `refs/heads` · `refs/remotes` · `refs/tags` 의 ref(이 차례, 각각 이름순).
fn branch_refs(repo: &gix::Repository) -> Result<Vec<RefTip>, RpcError> {
    let platform = repo.references().map_err(internal)?;
    let mut all = Vec::new();
    for reference in platform.all().map_err(internal)? {
        let mut reference = reference.map_err(internal)?;
        let name = reference.name().to_owned();
        let category = rank(name.as_bstr());
        if category == usize::MAX {
            continue;
        }
        let symbolic = matches!(reference.target(), gix::refs::TargetRef::Symbolic(_));
        let commit = match reference.peel_to_id() {
            Ok(id) => is_commit(repo, id.detach()).then(|| id.detach()),
            // 깨진 심볼릭 ref(가리키는 ref 가 없다) — git 도 건너뛴다.
            Err(_) => None,
        };
        all.push((
            category,
            RefTip {
                name,
                commit,
                symbolic,
            },
        ));
    }
    all.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.name.as_bstr().cmp(b.1.name.as_bstr()))
    });
    Ok(all.into_iter().map(|(_, r)| r).collect())
}

/// `refs/heads` 0, `refs/remotes` 1, `refs/tags` 2, 나머지 `usize::MAX`.
fn rank(name: &gix::bstr::BStr) -> usize {
    if name.starts_with(b"refs/heads/") {
        0
    } else if name.starts_with(b"refs/remotes/") {
        1
    } else if name.starts_with(b"refs/tags/") {
        2
    } else {
        usize::MAX
    }
}

fn is_commit(repo: &gix::Repository, id: ObjectId) -> bool {
    repo.find_header(id)
        .is_ok_and(|h| h.kind() == gix::object::Kind::Commit)
}

/// HEAD 가 가리키는 커밋과 브랜치(분리돼 있으면 `None`). 태어나지 않았으면 커밋도 `None`.
fn head(
    repo: &gix::Repository,
) -> Result<(Option<ObjectId>, Option<gix::refs::FullName>), RpcError> {
    let head = repo.head().map_err(internal)?;
    let branch = head.referent_name().map(ToOwned::to_owned);
    let commit = head
        .id()
        .map(gix::Id::detach)
        .filter(|&id| is_commit(repo, id));
    Ok((commit, branch))
}

/// 시작 커밋(받은 순서). `refs` 가 없으면 `git log --branches --remotes --tags HEAD` 와 같은 차례(`all` 은 [`branch_refs`],
/// `head` 는 HEAD 의 커밋).
fn tips(
    repo: &gix::Repository,
    refs: Option<&[String]>,
    all: &[RefTip],
    head: Option<ObjectId>,
) -> Result<Vec<ObjectId>, RpcError> {
    let Some(refs) = refs else {
        let mut tips: Vec<ObjectId> = all.iter().filter_map(|r| r.commit).collect();
        tips.extend(head);
        return Ok(tips);
    };
    let mut tips = Vec::with_capacity(refs.len());
    for name in refs {
        let unknown = || RpcError::invalid_params(format!("unknown ref `{name}`"));
        let id = if name == "HEAD" {
            head.ok_or_else(unknown)?
        } else {
            let mut reference = repo
                .try_find_reference(name.as_str())
                .map_err(|e| RpcError::invalid_params(format!("bad ref `{name}`: {e}")))?
                .ok_or_else(unknown)?;
            reference.peel_to_id().map_err(internal)?.detach()
        };
        if !is_commit(repo, id) {
            return Err(RpcError::invalid_params(format!(
                "ref `{name}` does not point to a commit"
            )));
        }
        tips.push(id);
    }
    Ok(tips)
}

/// 커밋마다 붙일 ref 장식(`refs` 는 [`branch_refs`], `head` 는 [`head`]). 원격의 심볼릭 ref(`origin/HEAD`)는 뺀다.
fn decorations(
    refs: &[RefTip],
    head: &(Option<ObjectId>, Option<gix::refs::FullName>),
) -> HashMap<ObjectId, Vec<LogRef>> {
    let (head_commit, head_branch) = head;
    let mut map: HashMap<ObjectId, Vec<LogRef>> = HashMap::new();
    if let (Some(commit), None) = (*head_commit, head_branch) {
        map.entry(commit).or_default().push(LogRef {
            name: "HEAD".to_owned(),
            kind: LogRefKind::Head,
            current: false,
        });
    }
    for r in refs {
        let Some(commit) = r.commit else { continue };
        let kind = match rank(r.name.as_bstr()) {
            0 => LogRefKind::Branch,
            1 if r.symbolic => continue,
            1 => LogRefKind::Remote,
            _ => LogRefKind::Tag,
        };
        let current = head_branch.as_ref() == Some(&r.name);
        let entry = LogRef {
            name: r.name.shorten().to_str_lossy().into_owned(),
            kind,
            current,
        };
        let list = map.entry(commit).or_default();
        if current {
            list.insert(0, entry);
        } else {
            list.push(entry);
        }
    }
    map
}
