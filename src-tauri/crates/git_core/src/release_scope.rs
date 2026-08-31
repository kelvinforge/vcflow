use git2::Repository;

use crate::version::{BumpKind, Version};

/// One non-merge commit in the release range, split into subject + body so the
/// Conventional Commit parser can see both `feat!:` and a `BREAKING CHANGE:`
/// footer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub summary: String,
    pub body: String,
}

/// SemVer impact of the release range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bump {
    Major,
    Minor,
    Patch,
}

impl Bump {
    pub fn as_str(self) -> &'static str {
        match self {
            Bump::Major => "major",
            Bump::Minor => "minor",
            Bump::Patch => "patch",
        }
    }
}

/// Commits reachable from `develop` but not from `production_ref` (e.g.
/// `"refs/remotes/origin/master"`), newest first. Merge commits are skipped --
/// the range is "what develop added since the last release".
pub fn commits_to_release(
    repo: &Repository,
    production_ref: &str,
    develop: &str,
) -> Result<Vec<CommitInfo>, git2::Error> {
    let dev_oid = repo.revparse_single(develop)?.peel_to_commit()?.id();
    let prod_oid = repo.revparse_single(production_ref)?.peel_to_commit()?.id();

    let mut walk = repo.revwalk()?;
    walk.push(dev_oid)?;
    walk.hide(prod_oid)?;

    let mut out = Vec::new();
    for oid in walk {
        let commit = repo.find_commit(oid?)?;
        if commit.parent_count() > 1 {
            continue;
        }
        let msg = commit.message().unwrap_or_default();
        let mut parts = msg.splitn(2, '\n');
        let summary = parts.next().unwrap_or("").trim().to_string();
        let body = parts.next().unwrap_or("").trim().to_string();
        if summary.starts_with("Merge ") {
            continue;
        }
        out.push(CommitInfo { summary, body });
    }
    Ok(out)
}

struct Subject<'a> {
    type_: &'a str,
    breaking: bool,
}

/// Parses `type(scope)!: description` -- returns `None` when the subject is not
/// Conventional Commit shaped.
fn parse_subject(summary: &str) -> Option<Subject<'_>> {
    let colon = summary.find(':')?;
    let head = &summary[..colon];
    let breaking = head.ends_with('!');
    let head = head.trim_end_matches('!');
    let type_ = match head.find('(') {
        Some(i) => &head[..i],
        None => head,
    };
    if type_.is_empty() || !type_.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    Some(Subject { type_, breaking })
}

fn body_is_breaking(body: &str) -> bool {
    body.contains("BREAKING CHANGE:") || body.contains("BREAKING-CHANGE:")
}

/// Highest SemVer impact across the range. `feat!` / `BREAKING CHANGE:` ->
/// Major, `feat:` -> Minor, everything else (including an all-`chore`/`refactor`
/// range or an empty range) -> Patch. The release always ships (D1).
pub fn conventional_bump(commits: &[CommitInfo]) -> Bump {
    let mut bump = Bump::Patch;
    for c in commits {
        if body_is_breaking(&c.body) {
            return Bump::Major;
        }
        let Some(s) = parse_subject(&c.summary) else { continue };
        if s.breaking {
            return Bump::Major;
        }
        if s.type_ == "feat" {
            bump = Bump::Minor;
        }
    }
    bump
}

/// Applies `bump` to `current`.
pub fn suggest_version(current: &Version, bump: Bump) -> Version {
    current.bump(match bump {
        Bump::Major => BumpKind::Major,
        Bump::Minor => BumpKind::Minor,
        Bump::Patch => BumpKind::Patch,
    })
}

/// Markdown seed for the CHANGELOG textarea: one `- <subject>` line per commit,
/// grouped `### Features` / `### Fixes` / `### Other`. Reuses the range from
/// `commits_to_release` (D5).
pub fn changelog_seed(commits: &[CommitInfo]) -> Vec<String> {
    let mut feats = Vec::new();
    let mut fixes = Vec::new();
    let mut others = Vec::new();
    for c in commits {
        let line = format!("- {}", c.summary);
        match parse_subject(&c.summary) {
            Some(s) if s.type_ == "feat" => feats.push(line),
            Some(s) if s.type_ == "fix" || s.type_ == "perf" => fixes.push(line),
            _ => others.push(line),
        }
    }
    let mut out: Vec<String> = Vec::new();
    for (title, items) in [("Features", feats), ("Fixes", fixes), ("Other", others)] {
        if items.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(String::new());
        }
        out.push(format!("### {title}"));
        out.extend(items);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ci(summary: &str) -> CommitInfo {
        CommitInfo { summary: summary.into(), body: String::new() }
    }

    #[test]
    fn all_chore_and_refactor_yields_patch() {
        let commits = [ci("chore: bump deps"), ci("refactor: tidy module")];
        assert_eq!(conventional_bump(&commits), Bump::Patch);
    }

    #[test]
    fn empty_range_yields_patch() {
        assert_eq!(conventional_bump(&[]), Bump::Patch);
    }

    #[test]
    fn two_fixes_yield_patch_one_feat_yields_minor() {
        assert_eq!(conventional_bump(&[ci("fix: a"), ci("fix: b")]), Bump::Patch);
        assert_eq!(conventional_bump(&[ci("fix: a"), ci("feat: b")]), Bump::Minor);
        assert_eq!(conventional_bump(&[ci("feat(ui): scoped")]), Bump::Minor);
    }

    #[test]
    fn bang_or_body_breaking_yields_major() {
        assert_eq!(conventional_bump(&[ci("feat!: drop v1 api")]), Bump::Major);
        assert_eq!(conventional_bump(&[ci("feat(api)!: drop")]), Bump::Major);
        let breaking = CommitInfo {
            summary: "fix: tweak".into(),
            body: "BREAKING CHANGE: config renamed".into(),
        };
        assert_eq!(conventional_bump(&[breaking]), Bump::Major);
    }

    #[test]
    fn suggest_version_resets_lower_components() {
        let v = Version { major: 1, minor: 3, patch: 7 };
        assert_eq!(suggest_version(&v, Bump::Minor), Version { major: 1, minor: 4, patch: 0 });
        assert_eq!(suggest_version(&v, Bump::Major), Version { major: 2, minor: 0, patch: 0 });
        assert_eq!(suggest_version(&v, Bump::Patch), Version { major: 1, minor: 3, patch: 8 });
    }

    #[test]
    fn changelog_seed_groups_by_type() {
        let commits = [
            ci("feat: add release workflow"),
            ci("fix: off-by-one"),
            ci("chore: bump deps"),
        ];
        let seed = changelog_seed(&commits);
        assert_eq!(
            seed,
            vec![
                "### Features",
                "- feat: add release workflow",
                "",
                "### Fixes",
                "- fix: off-by-one",
                "",
                "### Other",
                "- chore: bump deps",
            ]
        );
    }

    #[test]
    fn commits_to_release_excludes_merges_and_prior_history() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let git = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(p)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        let commit = |file: &str, msg: &str| {
            std::fs::write(p.join(file), msg).unwrap();
            git(&["add", "."]);
            git(&["commit", "-m", msg]);
        };
        git(&["init", "-b", "master"]);
        git(&["config", "user.email", "t@e.com"]);
        git(&["config", "user.name", "T"]);
        commit("a", "chore: seed");
        git(&["update-ref", "refs/remotes/origin/master", "HEAD"]);
        git(&["checkout", "-b", "develop"]);
        commit("b", "feat: b");
        commit("c", "fix: c");

        let repo = Repository::open(p).unwrap();
        let commits =
            commits_to_release(&repo, "refs/remotes/origin/master", "develop").unwrap();
        let summaries: Vec<_> = commits.iter().map(|c| c.summary.as_str()).collect();
        assert_eq!(summaries, vec!["fix: c", "feat: b"]);
        assert_eq!(conventional_bump(&commits), Bump::Minor);
    }
}
