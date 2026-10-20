//! 텍스트 검색 `search.text`(`docs/design/12-m1-plan.md` M1-07, 결정 M1-D6): 인덱스 없이 질의마다 병렬로 걷고 찾는다.
//!
//! - 걷기는 `ignore`(ripgrep 의 규칙), 찾기는 `grep-searcher` · `grep-regex`(ripgrep 의 라이브러리)다. 옵션이 같으면 결과도
//!   `rg` 와 같게 둔다 — 다른 것: `.git` 은 숨김 파일을 켜도 찾지 않는다(VS Code 의 기본 제외와 같다), 루트 안을 가리키는
//!   심링크는 따라간다(`rg -L` 과 같다, 아래).
//! - **심링크**(결정 M1-D8): 실제 대상이 프로젝트 루트 안인 링크만 따라 찾는다(pnpm 의 `node_modules` → `.pnpm`). 밖을
//!   가리키거나 풀리지 않는 링크는 걷기에서 잘라 낸다. 연 파일은 핸들의 실제 경로로 다시 판정한다([`crate::scope`] 의 「읽기를 연
//!   뒤 다시 판정」 과 같다). FIFO · 장치 파일은 걷기가 건너뛰고, 그사이 바뀐 것은 막히지 않게 연 뒤(unix `O_NONBLOCK`) 핸들의
//!   종류로 거른다.
//! - **건너뛰는 파일**: 바이너리(앞 8 KiB 에 NUL — [`crate::judge`] 와 같은 기준. 그 뒤에서 NUL 을 만나면 ripgrep 처럼 그 파일
//!   검색을 멈춘다)와 50 MiB([`READ_ONLY_MAX`], 결정 M1-D9 의 검색 상한) 넘는 파일.
//! - **벤더 · 생성 제외**(M1-02): `pw_lang` 의 경로 규칙(`vendor.yml`, generated 의 경로 규칙)과 `.gitattributes` 의
//!   `linguist-vendored` · `linguist-generated` 로 판정한다. 파일 내용은 보지 않는다 — generated 의 앞 줄 규칙(「Code generated
//!   … DO NOT EDIT」 · minified 등)은 파일마다 앞부분을 한 번 더 판정해야 해서 쓰지 않는다. 속성 파일은 폴더마다 한 번 읽는다.
//! - **결과**는 파일별 묶음이다. 한 결과는 일치한 줄 하나(줄 번호 · 미리보기 · 일치 범위)이고 상한([`DEFAULT_MAX_RESULTS`])은 이
//!   줄 수로 센다. 상한을 넘는 줄이 있으면 마지막 청크(`done`)의 `truncated` 가 켜진다. 파일 순서는 정하지 않는다(병렬).
//! - **취소**: 스트림이 끝나면(취소 · 세션 종료) 핸들러의 future 가 버려지고, 그 때 걷기 · 읽기가 멈춘다 — 읽기 한 번(버퍼 하나)
//!   안에 멈추므로 50 MiB 파일 한가운데서도 바로 끝난다.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use grep_matcher::Matcher as _;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::overrides::{Override, OverrideBuilder};
use ignore::{WalkBuilder, WalkState};
use pw_core::registry::path::{normalize, simplify};
use pw_lang::attributes::{RepoAttributes, find_repo};
use pw_rpc::{Router, RpcError, StreamSink, rpc_method};
use serde::{Deserialize, Serialize};

use crate::judge::{Bom, READ_ONLY_MAX, SNIFF_BYTES};
use crate::scope::Roots;
use crate::{Fs, denied, io_error};

/// 결과(일치한 줄) 상한의 기본값.
pub const DEFAULT_MAX_RESULTS: u32 = 10_000;
/// `maxResults` 로 줄 수 있는 가장 큰 값.
pub const MAX_RESULTS_LIMIT: u32 = 100_000;
/// 미리보기의 최대 길이(UTF-16 코드 단위).
pub const PREVIEW_MAX: usize = 200;
/// 미리보기를 자를 때 첫 일치 앞에 남기는 길이(UTF-16 코드 단위).
pub const PREVIEW_BEFORE: usize = 40;
/// 한 청크에 싣는 줄 수의 목표. 넘으면 다음 청크로 넘긴다(파일 하나는 나누지 않는다).
const CHUNK_LINES: usize = 500;
/// 검색 스레드가 보낸 뒤 아직 청크로 나가지 않은 파일 묶음의 상한. 클라이언트가 credit 을 주지 않으면 검색 스레드가 여기서 멈춘다.
const PENDING_FILES: usize = 256;

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SearchTextParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 찾을 폴더 · 파일(절대 경로, 하나 이상). 프로젝트 전체는 프로젝트 루트를 넘긴다. 루트 밖이면 invalid params.
    pub paths: Vec<String>,
    /// 찾을 것. 비어 있으면 invalid params.
    pub query: String,
    /// `query` 가 정규식(Rust `regex` 문법)이다. 끄면 글자 그대로 찾는다.
    #[serde(default)]
    pub regex: bool,
    /// 대소문자를 가린다. 끄면 가리지 않는다(유니코드 대소문자 접기).
    #[serde(default)]
    pub match_case: bool,
    /// 단어 단위로만 맞춘다(`rg -w`: 앞뒤가 단어 글자가 아니어야 한다).
    #[serde(default)]
    pub whole_word: bool,
    /// 이 글롭에 맞는 파일만 찾는다(`.gitignore` 문법, `rg -g`). `/` 가 없으면 어느 깊이의 이름에든, 있으면 찾는 폴더 기준 경로에
    /// 맞춘다. 비어 있으면 모든 파일.
    #[serde(default)]
    pub include: Vec<String>,
    /// 이 글롭에 맞는 파일 · 폴더는 찾지 않는다(`rg -g '!글롭'`). `include` 보다 우선한다.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// `.gitignore` · `.git/info/exclude` · 전역 gitignore · `.ignore` 로 무시되는 파일을 건너뛴다(기본 켬). `.gitignore` 는
    /// 저장소(`.git`) 안에서만 적용된다(ripgrep 과 같다).
    #[serde(default = "yes")]
    pub respect_ignore: bool,
    /// 벤더 파일(`vendor.yml` 의 경로 · `linguist-vendored`)을 건너뛴다.
    #[serde(default)]
    pub exclude_vendored: bool,
    /// 생성 파일(generated 의 경로 규칙 · `linguist-generated`)을 건너뛴다.
    #[serde(default)]
    pub exclude_generated: bool,
    /// 숨김 파일 · 폴더(`.` 으로 시작)도 찾는다. 켜도 `.git` 은 찾지 않는다.
    #[serde(default)]
    pub hidden: bool,
    /// 결과(일치한 줄) 상한. 기본 [`DEFAULT_MAX_RESULTS`], 1 ~ [`MAX_RESULTS_LIMIT`].
    #[serde(default)]
    pub max_results: Option<u32>,
}

fn yes() -> bool {
    true
}

/// 일치 범위. 줄 안의 UTF-16 코드 단위 위치(JS 문자열 · CodeMirror 의 위치 단위)이고 `end` 는 들지 않는다. UTF-8 이 아닌
/// 바이트는 대체 문자 하나로 센다.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SearchRange {
    pub start: u32,
    pub end: u32,
}

/// 일치한 줄 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SearchLine {
    /// 1 부터.
    #[specta(type = pw_rpc::JsNumber)]
    pub line: u64,
    /// 줄의 일부(줄바꿈 제외). 줄이 [`PREVIEW_MAX`] 보다 길면 첫 일치 앞 [`PREVIEW_BEFORE`] 부터 잘라 낸다.
    pub preview: String,
    /// `preview` 가 줄의 어디서 시작하는가(UTF-16 코드 단위). 미리보기 안의 범위는 `range.start - previewStart` 다.
    pub preview_start: u32,
    /// 일치 범위(줄 기준, 앞에서부터). 미리보기에 걸친 것만 싣는다 — 첫 일치는 늘 든다.
    pub ranges: Vec<SearchRange>,
}

/// 파일 하나의 결과.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct SearchFile {
    /// 걸어 간 경로(절대 경로). 루트 안 심링크를 지났으면 링크 쪽 경로다 — 실제 경로는 `fs.stat` 이 준다.
    pub path: String,
    /// 줄 번호순.
    pub lines: Vec<SearchLine>,
}

/// `search.text` 의 청크.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SearchTextChunk {
    /// 결과가 있는 파일들. 한 파일은 한 청크 안에만 있다.
    Files { files: Vec<SearchFile> },
    /// 마지막 청크. 상한 · 취소 없이 끝까지 찾았으면 `truncated` 가 꺼져 있다.
    Done {
        /// 결과가 있는 파일 수.
        #[specta(type = pw_rpc::JsNumber)]
        matched_files: u64,
        /// 보낸 결과(일치한 줄) 수.
        #[specta(type = pw_rpc::JsNumber)]
        lines: u64,
        /// 상한을 넘는 결과가 더 있다(「더 있음」).
        truncated: bool,
        /// 내용을 찾아본 파일 수.
        #[specta(type = pw_rpc::JsNumber)]
        searched_files: u64,
        /// 바이너리라 건너뛴 파일 수.
        #[specta(type = pw_rpc::JsNumber)]
        binary_files: u64,
        /// 50 MiB 를 넘어 건너뛴 파일 수.
        #[specta(type = pw_rpc::JsNumber)]
        large_files: u64,
    },
}

rpc_method! {
    /// 프로젝트 안의 텍스트 검색. 결과를 파일별 묶음 청크로 보내고 마지막에 `done` 청크 하나를 보낸다. 규칙(ignore · 심링크 ·
    /// 바이너리 · 50 MiB · 벤더/생성 제외 · 결과 상한)은 `pw_fs::search` 머리 주석. 루트 밖 경로 · 빈 질의 · 잘못된 정규식 ·
    /// 글롭은 invalid params 로, 없는 경로는 `NOT_FOUND` 로 스트림이 `failed` 로 끝난다(청크 없이).
    pub SearchTextMethod: "search.text" {
        params: SearchTextParams,
        stream: SearchTextChunk,
        tier: Read,
    }
}

/// 핸들러를 붙인다. 요청 판정(레지스트리 · 경로)과 걷기 · 찾기는 블로킹 스레드에서 돈다.
pub fn register(router: &mut Router, fs: Arc<Fs>) {
    router.stream(SearchTextMethod, move |_ctx, params, sink| {
        let fs = Arc::clone(&fs);
        async move { stream(fs, params, &sink, |_| {}).await }
    });
}

/// 스트림 하나. `tweak` 은 시험이 계획을 바꾸는 자리다.
async fn stream(
    fs: Arc<Fs>,
    params: SearchTextParams,
    sink: &StreamSink<SearchTextChunk>,
    tweak: impl FnOnce(&mut Plan) + Send + 'static,
) -> Result<(), RpcError> {
    let stop = Arc::new(AtomicBool::new(false));
    // 스트림이 끝나 이 future 가 버려지면 검색 스레드도 멈춘다.
    let _stop_on_drop = StopOnDrop(Arc::clone(&stop));
    let (tx, mut rx) = tokio::sync::mpsc::channel::<SearchFile>(PENDING_FILES);
    let worker = {
        let stop = Arc::clone(&stop);
        tokio::task::spawn_blocking(move || {
            let mut plan = fs.search_plan(&params)?;
            tweak(&mut plan);
            // 받는 쪽이 없으면(스트림이 끝남) 더 찾지 않는다.
            let summary = plan.run(&stop, &mut |file| tx.blocking_send(file).is_ok());
            #[cfg(test)]
            if let Some(finish) = plan.on_finish {
                finish();
            }
            Ok::<_, RpcError>(summary)
        })
    };
    while let Some(first) = rx.recv().await {
        let mut lines = first.lines.len();
        let mut files = vec![first];
        while lines < CHUNK_LINES {
            let Ok(next) = rx.try_recv() else { break };
            lines += next.lines.len();
            files.push(next);
        }
        sink.send(SearchTextChunk::Files { files }).await?;
    }
    let summary = worker
        .await
        .map_err(|e| RpcError::internal(e.to_string()))??;
    sink.send(summary.chunk()).await
}

struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl Fs {
    /// 요청을 판정해 검색 계획으로. 디스크는 찾는 경로의 상태만 본다.
    fn search_plan(&self, params: &SearchTextParams) -> Result<Plan, RpcError> {
        if params.query.is_empty() {
            return Err(RpcError::invalid_params("`query` is empty"));
        }
        if params.paths.is_empty() {
            return Err(RpcError::invalid_params("`paths` is empty"));
        }
        let max_results = params.max_results.unwrap_or(DEFAULT_MAX_RESULTS);
        if !(1..=MAX_RESULTS_LIMIT).contains(&max_results) {
            return Err(RpcError::invalid_params(format!(
                "`maxResults` must be 1 to {MAX_RESULTS_LIMIT}"
            )));
        }
        let roots = self.roots(&params.project_id, params.worktree.as_deref())?;
        let mut targets = Vec::with_capacity(params.paths.len());
        for path in &params.paths {
            let target = roots.readable(Path::new(path)).map_err(denied)?;
            std::fs::metadata(&target.real).map_err(|e| io_error(&target.real, e))?;
            let overrides = overrides(&target.real, &params.include, &params.exclude)?;
            targets.push(Target {
                path: target.real,
                root: target.root,
                overrides,
            });
        }
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(!params.match_case)
            .word(params.whole_word)
            .fixed_strings(!params.regex)
            .line_terminator(Some(b'\n'))
            .build(&params.query)
            .map_err(|e| RpcError::invalid_params(format!("invalid `query`: {e}")))?;
        Ok(Plan {
            roots,
            targets,
            matcher,
            respect_ignore: params.respect_ignore,
            hidden: params.hidden,
            exclude: Exclude {
                vendored: params.exclude_vendored,
                generated: params.exclude_generated,
            },
            max_results: u64::from(max_results),
            #[cfg(test)]
            read_delay: None,
            #[cfg(test)]
            on_finish: None,
        })
    }
}

fn overrides(dir: &Path, include: &[String], exclude: &[String]) -> Result<Override, RpcError> {
    let bad = |glob: &str, e: ignore::Error| {
        RpcError::invalid_params(format!("invalid glob `{glob}`: {e}"))
    };
    let mut builder = OverrideBuilder::new(dir);
    for glob in include {
        builder.add(glob).map_err(|e| bad(glob, e))?;
    }
    for glob in exclude {
        builder.add(&format!("!{glob}")).map_err(|e| bad(glob, e))?;
    }
    builder
        .build()
        .map_err(|e| RpcError::invalid_params(e.to_string()))
}

/// 찾는 경로 하나.
struct Target {
    /// 정규화한 실제 경로.
    path: PathBuf,
    /// 그 경로를 담은 가장 깊은 루트. 저장소 밖 파일의 벤더 · 생성 판정 기준이다.
    root: PathBuf,
    overrides: Override,
}

#[derive(Clone, Copy)]
struct Exclude {
    vendored: bool,
    generated: bool,
}

impl Exclude {
    fn any(self) -> bool {
        self.vendored || self.generated
    }
}

/// 판정을 마친 검색 하나.
struct Plan {
    roots: Roots,
    targets: Vec<Target>,
    matcher: RegexMatcher,
    respect_ignore: bool,
    hidden: bool,
    exclude: Exclude,
    max_results: u64,
    /// 시험: 읽기마다 이만큼 쉰다(오래 걸리는 검색을 재현해 취소를 본다).
    #[cfg(test)]
    read_delay: Option<std::time::Duration>,
    /// 시험: 검색 스레드가 [`Plan::run`] 을 마칠 때 부른다.
    #[cfg(test)]
    on_finish: Option<Box<dyn FnOnce() + Send + Sync>>,
}

/// 검색 전체에서 나누는 상태.
struct Shared<'a> {
    plan: &'a Plan,
    stop: &'a AtomicBool,
    lines: AtomicU64,
    truncated: AtomicBool,
    matched_files: AtomicU64,
    searched_files: AtomicU64,
    binary_files: AtomicU64,
    large_files: AtomicU64,
    labels: Labels,
}

/// 끝난 검색의 수치.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Summary {
    matched_files: u64,
    lines: u64,
    truncated: bool,
    searched_files: u64,
    binary_files: u64,
    large_files: u64,
}

impl Summary {
    fn chunk(self) -> SearchTextChunk {
        SearchTextChunk::Done {
            matched_files: self.matched_files,
            lines: self.lines,
            truncated: self.truncated,
            searched_files: self.searched_files,
            binary_files: self.binary_files,
            large_files: self.large_files,
        }
    }
}

impl Plan {
    /// 찾는다. 결과가 있는 파일마다 `emit` 을 부르고, `emit` 이 `false` 를 돌려주거나 `stop` 이 켜지면 멈춘다.
    fn run(&self, stop: &AtomicBool, emit: &mut (dyn FnMut(SearchFile) -> bool + Send)) -> Summary {
        let shared = Shared {
            plan: self,
            stop,
            lines: AtomicU64::new(0),
            truncated: AtomicBool::new(false),
            matched_files: AtomicU64::new(0),
            searched_files: AtomicU64::new(0),
            binary_files: AtomicU64::new(0),
            large_files: AtomicU64::new(0),
            labels: Labels::default(),
        };
        let emit = Mutex::new(emit);
        for target in &self.targets {
            if stop.load(Ordering::Relaxed) || shared.truncated.load(Ordering::Relaxed) {
                break;
            }
            self.walker(target).run(|| {
                let shared = &shared;
                let emit = &emit;
                let mut searcher = SearcherBuilder::new()
                    .line_number(true)
                    .binary_detection(BinaryDetection::quit(0))
                    .build();
                Box::new(move |entry| {
                    if shared.done() {
                        return WalkState::Quit;
                    }
                    let Ok(entry) = entry else {
                        return WalkState::Continue;
                    };
                    if !entry.file_type().is_some_and(|t| t.is_file()) {
                        return WalkState::Continue;
                    }
                    if self.exclude.any()
                        && shared.labels.excluded(entry.path(), target, self.exclude)
                    {
                        return WalkState::Continue;
                    }
                    if let Some(file) = shared.search(&mut searcher, entry.path()) {
                        shared.matched_files.fetch_add(1, Ordering::Relaxed);
                        let mut emit = emit.lock().unwrap_or_else(PoisonError::into_inner);
                        let sent = (*emit)(file);
                        if !sent {
                            shared.stop.store(true, Ordering::Relaxed);
                        }
                    }
                    if shared.done() {
                        WalkState::Quit
                    } else {
                        WalkState::Continue
                    }
                })
            });
        }
        Summary {
            matched_files: shared.matched_files.load(Ordering::Relaxed),
            lines: shared.lines.load(Ordering::Relaxed).min(self.max_results),
            truncated: shared.truncated.load(Ordering::Relaxed),
            searched_files: shared.searched_files.load(Ordering::Relaxed),
            binary_files: shared.binary_files.load(Ordering::Relaxed),
            large_files: shared.large_files.load(Ordering::Relaxed),
        }
    }

    fn walker(&self, target: &Target) -> ignore::WalkParallel {
        let ignore = self.respect_ignore;
        let roots = self.roots.clone();
        let mut walk = WalkBuilder::new(&target.path);
        walk.hidden(!self.hidden)
            .ignore(ignore)
            .git_ignore(ignore)
            .git_global(ignore)
            .git_exclude(ignore)
            .parents(ignore)
            .follow_links(true)
            .overrides(target.overrides.clone())
            .filter_entry(move |entry| {
                if entry.depth() == 0 {
                    return true;
                }
                if entry.file_name() == ".git" {
                    return false;
                }
                // 루트 안을 가리키는 링크만 따라간다. 풀리지 않는 링크는 링크 경로 그대로 돌아와 아래 판정을 지나지만, 걷기가
                // 대상을 읽지 못해 오류 항목이 된다.
                !entry.path_is_symlink()
                    || normalize(entry.path()).is_ok_and(|real| roots.contains(&real))
            });
        walk.build_parallel()
    }
}

impl Shared<'_> {
    /// 멈출 때다(취소 · 받는 쪽 없음 · 상한).
    fn done(&self) -> bool {
        self.stop.load(Ordering::Relaxed) || self.truncated.load(Ordering::Relaxed)
    }

    /// 파일 하나를 찾는다. 결과가 없거나 건너뛰었으면 `None`.
    fn search(&self, searcher: &mut Searcher, path: &Path) -> Option<SearchFile> {
        let Ok(mut file) = open(&self.plan.roots, path) else {
            return None;
        };
        let meta = file.metadata().ok()?;
        if !meta.is_file() {
            return None;
        }
        if meta.len() > READ_ONLY_MAX {
            self.large_files.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let mut head = Vec::with_capacity(SNIFF_BYTES);
        (&mut file)
            .take(SNIFF_BYTES as u64)
            .read_to_end(&mut head)
            .ok()?;
        let utf16 = matches!(Bom::detect(&head), Some(Bom::Utf16Le | Bom::Utf16Be));
        if !utf16 && head.contains(&0) {
            self.binary_files.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        self.searched_files.fetch_add(1, Ordering::Relaxed);
        let reader = io::Cursor::new(head).chain(Stoppable {
            inner: file,
            stop: self.stop,
            #[cfg(test)]
            delay: self.plan.read_delay,
        });
        let mut sink = Lines {
            shared: self,
            lines: Vec::new(),
        };
        // 읽기 오류 · 멈춤도 그때까지 찾은 줄은 보낸다.
        let _ = searcher.search_reader(&self.plan.matcher, reader, &mut sink);
        (!sink.lines.is_empty()).then(|| SearchFile {
            path: path.display().to_string(),
            lines: sink.lines,
        })
    }
}

/// 읽기로 연다. FIFO 에서 막히지 않게 열고(unix), 연 핸들의 실제 경로가 루트 밖이면(판정 뒤 링크로 바뀜) 버린다.
fn open(roots: &Roots, path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt as _;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?
    };
    #[cfg(not(unix))]
    let file = File::open(path)?;
    if let Some(opened) = crate::handle::path_of(&file)
        && !roots.contains(&simplify(opened))
    {
        return Err(io::Error::from(io::ErrorKind::PermissionDenied));
    }
    Ok(file)
}

/// 멈추라면 읽기를 끝낸다(검색기는 끝까지 읽은 것으로 보고 그 파일을 마친다).
struct Stoppable<'a> {
    inner: File,
    stop: &'a AtomicBool,
    #[cfg(test)]
    delay: Option<std::time::Duration>,
}

impl Read for Stoppable<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        #[cfg(test)]
        if let Some(delay) = self.delay {
            std::thread::sleep(delay);
        }
        if self.stop.load(Ordering::Relaxed) {
            return Ok(0);
        }
        self.inner.read(buf)
    }
}

/// 한 파일의 일치한 줄을 모은다.
struct Lines<'a, 'b> {
    shared: &'a Shared<'b>,
    lines: Vec<SearchLine>,
}

impl Sink for Lines<'_, '_> {
    type Error = io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, io::Error> {
        let shared = self.shared;
        if shared.stop.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let first = mat.line_number().unwrap_or(1);
        for (i, line) in mat.lines().enumerate() {
            if shared.lines.fetch_add(1, Ordering::Relaxed) >= shared.plan.max_results {
                shared.truncated.store(true, Ordering::Relaxed);
                return Ok(false);
            }
            self.lines
                .push(line_result(&shared.plan.matcher, first + i as u64, line));
        }
        Ok(true)
    }
}

/// 줄 하나의 결과. `raw` 는 줄바꿈까지 든 바이트다.
fn line_result(matcher: &RegexMatcher, line: u64, raw: &[u8]) -> SearchLine {
    let raw = raw.strip_suffix(b"\n").unwrap_or(raw);
    let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
    let mut found: Vec<(usize, usize)> = Vec::new();
    let _ = matcher.find_iter(raw, |m| {
        found.push((m.start(), m.end()));
        true
    });
    // 바이트 위치 → UTF-16 위치. 앞에서부터 이어 세어 줄 길이에 비례하게 한다.
    let mut cursor = Utf16Cursor::default();
    let mut ranges: Vec<SearchRange> = found
        .iter()
        .map(|&(s, e)| {
            let start = cursor.at(raw, s);
            let end = cursor.at(raw, e);
            SearchRange { start, end }
        })
        .collect();
    let units: Vec<u16> = String::from_utf8_lossy(raw).encode_utf16().collect();
    let (from, to) = window(&units, ranges.first().map_or(0, |r| r.start as usize));
    ranges.retain(|r| overlaps(*r, from, to));
    SearchLine {
        line,
        preview: String::from_utf16_lossy(&units[from..to]),
        preview_start: u32::try_from(from).unwrap_or(u32::MAX),
        ranges,
    }
}

/// 범위가 미리보기 구간 `[from, to)` 에 걸친다. 빈 범위(`^` 등)는 구간 안(끝 포함)에 있으면 걸친 것이다.
fn overlaps(r: SearchRange, from: usize, to: usize) -> bool {
    let (start, end) = (r.start as usize, r.end as usize);
    if start == end {
        (from..=to).contains(&start)
    } else {
        start < to && end > from
    }
}

/// 미리보기 구간 `[from, to)`. 서로게이트 쌍을 가르지 않는다.
fn window(units: &[u16], first: usize) -> (usize, usize) {
    if units.len() <= PREVIEW_MAX {
        return (0, units.len());
    }
    let mut from = first
        .saturating_sub(PREVIEW_BEFORE)
        .min(units.len() - PREVIEW_MAX);
    if from > 0 && is_low_surrogate(units[from]) {
        from -= 1;
    }
    let mut to = (from + PREVIEW_MAX).min(units.len());
    if to < units.len() && is_low_surrogate(units[to]) {
        to -= 1;
    }
    (from, to)
}

fn is_low_surrogate(unit: u16) -> bool {
    (0xDC00..=0xDFFF).contains(&unit)
}

/// 바이트 위치를 앞에서부터 차례로 UTF-16 위치로 바꾼다.
#[derive(Default)]
struct Utf16Cursor {
    byte: usize,
    unit: u32,
}

impl Utf16Cursor {
    /// `byte` 는 이전에 받은 것보다 작지 않아야 한다(작으면 처음부터 센다).
    fn at(&mut self, raw: &[u8], byte: usize) -> u32 {
        if byte < self.byte {
            *self = Self::default();
        }
        let part = String::from_utf8_lossy(&raw[self.byte..byte]);
        self.unit += u32::try_from(part.encode_utf16().count()).unwrap_or(u32::MAX);
        self.byte = byte;
        self.unit
    }
}

/// 폴더마다 한 번 읽는 벤더 · 생성 판정 재료.
#[derive(Default)]
struct Labels {
    dirs: Mutex<HashMap<PathBuf, Arc<DirLabels>>>,
}

struct DirLabels {
    /// 저장소 꼭대기와 그 폴더까지의 속성 파일. 저장소 밖이면 `None`.
    repo: Option<(PathBuf, RepoAttributes)>,
}

impl DirLabels {
    fn load(dir: &Path) -> Self {
        let repo = find_repo(dir).map(|repo| {
            let rel = relative(dir, &repo.top).unwrap_or_default();
            let attrs = RepoAttributes::load(&repo.top, repo.gitdir.as_deref(), [rel.as_str()]);
            (repo.top, attrs)
        });
        Self { repo }
    }
}

impl Labels {
    /// 건너뛸 파일인가.
    fn excluded(&self, path: &Path, target: &Target, exclude: Exclude) -> bool {
        let Some(dir) = path.parent() else {
            return false;
        };
        let labels = self.dir(dir);
        let (base, attrs) = match &labels.repo {
            Some((top, attrs)) => (top.as_path(), Some(attrs)),
            None => (target.root.as_path(), None),
        };
        let Some(rel) = relative(path, base) else {
            return false;
        };
        let attrs = attrs.map(|a| a.get(&rel)).unwrap_or_default();
        (exclude.vendored
            && attrs
                .vendored
                .unwrap_or_else(|| pw_lang::linguist::vendored_path(&rel)))
            || (exclude.generated
                && attrs
                    .generated
                    .unwrap_or_else(|| pw_lang::generated::generated(&rel, None).is_some()))
    }

    /// 그 폴더의 판정 재료. 속성 파일이 없는 폴더는 부모의 것을 그대로 쓴다 — 읽는 파일(꼭대기부터 그 폴더까지의
    /// `.gitattributes` 와 `info/attributes`)이 같다. 그래서 속성 파일은 저장소마다, 그리고 `.gitattributes` 가 있는 폴더마다 한
    /// 번만 읽고, 나머지 폴더는 `lstat` 두 번(`.git` · `.gitattributes`)이다.
    fn dir(&self, dir: &Path) -> Arc<DirLabels> {
        if let Some(found) = self.lock().get(dir) {
            return Arc::clone(found);
        }
        // 잠금 밖에서 읽는다. 두 스레드가 같은 폴더를 함께 읽어도 결과가 같다.
        let exists = |name: &str| std::fs::symlink_metadata(dir.join(name)).is_ok();
        let labels = if exists(".git") {
            Arc::new(DirLabels::load(dir))
        } else {
            match dir.parent() {
                None => Arc::new(DirLabels { repo: None }),
                Some(parent) => {
                    let parent = self.dir(parent);
                    if parent.repo.is_some() && exists(".gitattributes") {
                        Arc::new(DirLabels::load(dir))
                    } else {
                        parent
                    }
                }
            }
        };
        Arc::clone(self.lock().entry(dir.to_path_buf()).or_insert(labels))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, Arc<DirLabels>>> {
        self.dirs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// `path` 가 `base` 안이면 `/` 로 가른 상대 경로(같으면 빈 문자열).
fn relative(path: &Path, base: &Path) -> Option<String> {
    let rel = path.strip_prefix(base).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Option<_>>()?;
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests;
