//! 리비전 입력: 비었거나 `-` 로 시작하면 git 에 넘기지 않는다(옵션으로 읽힌다).

use pw_rpc::{RpcError, code};

use super::super::check_rev;

#[test]
fn revisions_that_git_would_read_as_options_are_refused() {
    for bad in ["", "-", "--hard", "--upload-pack=touch /tmp/x", "-n1"] {
        let e = RpcError::from(check_rev("rev", bad).unwrap_err());
        assert_eq!(e.code, code::INVALID_PARAMS, "{bad:?}");
    }
    for good in ["main", "HEAD~1", "a1b2c3d", "feature/-x", "@{-1}"] {
        assert_eq!(check_rev("rev", good), Ok(()), "{good:?}");
    }
}
