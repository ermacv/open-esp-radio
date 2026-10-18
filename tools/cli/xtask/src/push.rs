//! Push this checkout's branch and open its pull request as a draft.
//!
//! `cargo xtask push` runs no checks and no review: it pushes `HEAD` to the
//! branch of the same name with `git push`, replacing a rebased branch's
//! remote copy with `--force-with-lease` bound to the copy it saw, and opens
//! the branch's pull request into `main` as a draft when it has none. A
//! person marks it ready and merges it. `main` changes only through pull
//! requests, so the push refuses `main` and a detached `HEAD`.

use oer_process::{self as process, Checkout, git};

use crate::Result;

const MAIN: &str = "main";

/// Why `branch` cannot be pushed, if it cannot.
pub fn unpushable(branch: &str) -> Option<String> {
    match branch {
        "HEAD" => Some(String::from(
            "HEAD is detached; create a branch for the change first",
        )),
        MAIN => Some(String::from(
            "main changes only through pull requests; create a branch for the change first",
        )),
        _ => None,
    }
}

/// How a push updates the branch's remote copy.
#[derive(Debug, Eq, PartialEq)]
pub enum Update {
    /// The branch has no remote copy yet, or `HEAD` descends from it.
    FastForward,
    /// `HEAD` rewrote the branch, a rebase: replace the remote copy only
    /// while it is still `seen`.
    Rewrite { seen: String },
}

/// The update for a branch whose remote copy is `remote`, when `HEAD`
/// `descends` from it or not.
pub fn update(remote: Option<&str>, descends: bool) -> Update {
    match remote {
        Some(seen) if !descends => Update::Rewrite {
            seen: seen.to_owned(),
        },
        _ => Update::FastForward,
    }
}

/// The `gh` arguments that open `branch`'s pull request: always a draft.
fn create_pull_request(branch: &str) -> [&str; 8] {
    [
        "pr",
        "create",
        "--draft",
        "--fill-first",
        "--base",
        MAIN,
        "--head",
        branch,
    ]
}

fn gh(ctx: &Checkout, arguments: &[&str]) -> Result<String> {
    let output = process::capture(ctx.command("gh").args(arguments))?;
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

/// Push, then open the pull request; see the module documentation.
pub fn run(ctx: &Checkout) -> Result<()> {
    let branch = git::text(&ctx.root, ["rev-parse", "--abbrev-ref", "HEAD"])?;
    if let Some(reason) = unpushable(&branch) {
        return Err(format!("push: {reason}").into());
    }
    let head = git::text(&ctx.root, ["rev-parse", "HEAD"])?;
    let reference = format!("refs/heads/{branch}");
    let remote = git::text(&ctx.root, ["ls-remote", "origin", &reference])?
        .split_whitespace()
        .next()
        .map(str::to_owned);
    let descends = match &remote {
        Some(seen) => {
            git::output(&ctx.root, ["fetch", "--quiet", "origin", &reference])?;
            git::output(&ctx.root, ["merge-base", "--is-ancestor", seen, &head]).is_ok()
        }
        None => true,
    };
    let update = update(remote.as_deref(), descends);
    let mut command = git::command(&ctx.root);
    command.args(["push", "--quiet"]);
    if let Update::Rewrite { seen } = &update {
        command.arg(format!("--force-with-lease={reference}:{seen}"));
    }
    process::capture(command.args(["origin", &format!("{head}:{reference}")]))
        .map_err(|error| format!("push: git push of {branch} failed: {error}"))?;
    // A commit, not a branch, was pushed, so Git set no upstream.
    git::output(
        &ctx.root,
        [
            "branch",
            &format!("--set-upstream-to=origin/{branch}"),
            &branch,
        ],
    )?;
    match &update {
        Update::FastForward => println!("push: pushed {} to {branch}", &head[..12]),
        Update::Rewrite { seen } => println!(
            "push: rewrote {branch} from {} to {}",
            &seen[..seen.len().min(12)],
            &head[..12]
        ),
    }
    match gh(
        ctx,
        &["pr", "view", &branch, "--json", "url", "--jq", ".url"],
    )
    .ok()
    .filter(|url| !url.is_empty())
    {
        Some(url) => println!("push: updated pull request {url}"),
        None => {
            let url = gh(ctx, &create_pull_request(&branch))
                .map_err(|error| format!("push: gh pr create failed: {error}"))?;
            println!(
                "push: opened draft pull request {url}; a person marks it ready and merges it"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_changes_only_through_pull_requests() {
        assert!(unpushable("main").unwrap().contains("pull requests"));
        assert!(unpushable("HEAD").unwrap().contains("detached"));
        assert_eq!(unpushable("fix/a"), None);
    }

    #[test]
    fn a_push_opens_a_draft() {
        let create = create_pull_request("fix/a");
        assert_eq!(&create[..3], ["pr", "create", "--draft"]);
        assert!(create.contains(&"fix/a"));
    }

    #[test]
    fn only_a_rebased_branch_is_rewritten_and_only_from_what_was_seen() {
        assert_eq!(update(None, true), Update::FastForward);
        assert_eq!(update(Some("abc"), true), Update::FastForward);
        assert_eq!(
            update(Some("abc"), false),
            Update::Rewrite {
                seen: String::from("abc")
            }
        );
    }
}
