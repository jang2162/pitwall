//! 파일 목록 인덱스의 성능 픽스처(M1-12): 목록에 드는 파일이 정확히 `<개수>` 개인 합성 저장소를 만든다.
//!
//! ```bash
//! cargo run --release -p pw-fs --example synth_repo -- <빈 폴더> [개수, 기본 100000] [시드, 기본 1]
//! ```
//!
//! 같은 개수 · 시드면 같은 트리다. 모양은 모노레포를 흉내 낸다: `crates/` · `packages/` · `apps/` · `docs/` · `vendor/` 아래 2~6 단계
//! 폴더에 폴더당 10~40 개 파일, 여러 확장자. `.git/`(빈 폴더 — 저장소로 보이게)과 `.gitignore`(`target/` · `node_modules/` ·
//! `*.log`)를 두고 그 규칙에 걸리는 파일을 개수의 5% 더 만든다 — 목록 빌드가 ignore 판정을 지나게. 파일 내용은 비어 있다(목록은
//! 이름만 본다). 저장소에는 넣지 않는다 — scratchpad 나 임시 폴더에 만들고 쓰고 나면 지운다.

use std::fs;
use std::path::{Path, PathBuf};

const TOPS: [&str; 5] = ["crates", "packages", "apps", "docs", "vendor"];
const WORDS: [&str; 48] = [
    "core", "util", "config", "handler", "service", "client", "server", "router", "parser",
    "lexer", "render", "view", "model", "store", "cache", "index", "search", "query", "schema",
    "types", "editor", "buffer", "syntax", "theme", "layout", "panel", "dialog", "menu", "status",
    "task", "event", "bus", "watch", "fs", "path", "git", "lsp", "terminal", "runtime", "shell",
    "auth", "session", "token", "api", "http", "codec", "stream", "worker",
];
const EXTS: [&str; 12] = [
    "rs", "ts", "tsx", "js", "json", "md", "toml", "py", "go", "css", "yaml", "txt",
];

/// 시드로 정해지는 수열(xorshift64*). 의존성을 더하지 않는다.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn word(&mut self) -> &'static str {
        WORDS[self.below(WORDS.len())]
    }
}

fn touch(path: &Path) {
    fs::write(path, b"").unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("an empty folder"));
    let count: usize = args.next().map_or(100_000, |n| n.parse().expect("count"));
    let seed: u64 = args.next().map_or(1, |n| n.parse().expect("seed"));
    if root.exists() && fs::read_dir(&root).unwrap().next().is_some() {
        panic!("{} is not empty", root.display());
    }
    fs::create_dir_all(root.join(".git")).unwrap();
    fs::write(root.join(".gitignore"), "target/\nnode_modules/\n*.log\n").unwrap();
    let mut rng = Rng(seed.max(1));

    // 목록에 드는 파일. `.gitignore` 도 하나로 센다.
    let mut made = 1;
    let mut dirs = 0;
    while made < count {
        let depth = 2 + rng.below(5);
        let mut dir = root.join(TOPS[rng.below(TOPS.len())]);
        for level in 0..depth {
            dir.push(if level == depth - 1 && rng.below(2) == 0 {
                "src".to_owned()
            } else {
                format!("{}-{}", rng.word(), rng.below(8))
            });
        }
        fs::create_dir_all(&dir).unwrap();
        dirs += 1;
        let files = (10 + rng.below(31)).min(count - made);
        for i in 0..files {
            let name = match rng.below(4) {
                0 => format!(
                    "{}_{}.{}",
                    rng.word(),
                    rng.word(),
                    EXTS[rng.below(EXTS.len())]
                ),
                1 => format!("{}{}.{}", rng.word(), i, EXTS[rng.below(EXTS.len())]),
                2 => format!(
                    "{}-{}-{}.{}",
                    rng.word(),
                    rng.word(),
                    i,
                    EXTS[rng.below(EXTS.len())]
                ),
                _ => format!("{}.{}", rng.word(), EXTS[rng.below(EXTS.len())]),
            };
            let path = dir.join(name);
            // 같은 이름이 나오면 건너뛴다(개수는 만든 것으로 센다).
            if path.exists() {
                continue;
            }
            touch(&path);
            made += 1;
        }
    }

    // ignore 규칙에 걸리는 파일(목록에 들지 않는다).
    let ignored = count / 20;
    for i in 0..ignored {
        let dir = match i % 3 {
            0 => root.join("target/debug/deps"),
            1 => root.join(format!("node_modules/{}-{}", rng.word(), i % 50)),
            _ => root.join(format!("crates/{}-{}/logs", rng.word(), i % 10)),
        };
        fs::create_dir_all(&dir).unwrap();
        let name = if i % 3 == 2 {
            format!("run{i}.log")
        } else {
            format!("{}{i}.{}", rng.word(), EXTS[rng.below(EXTS.len())])
        };
        touch(&dir.join(name));
    }
    println!(
        "{}: {made} listed files in {dirs} leaf folders, {ignored} ignored files",
        root.display()
    );
}
