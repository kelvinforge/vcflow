use std::path::Path;
use std::process::Command;

use git2::Repository;
use thiserror::Error;

/// One offending finding, always naming the file (and line, when known) so
/// the Owner's UI can point at the exact problem instead of a generic
/// "still broken" message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub file: String,
    pub line: Option<usize>,
    pub detail: String,
}

#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("repository has no working directory")]
    NoWorkdir,
    #[error(transparent)]
    Git(#[from] git2::Error),
}

const CONFLICT_MARKERS: [&str; 3] = ["<<<<<<<", "=======", ">>>>>>>"];

/// Checks the worktree at `path` for leftover conflict markers in tracked
/// files, then runs `git diff --check` for whitespace-conflict artifacts.
/// Fails loudly with every offending file (not just the first) so the Owner
/// fixes everything in one pass instead of discovering issues one at a time.
pub fn verify_resolved(repo: &Repository, path: &Path) -> Result<(), Vec<Issue>> {
    let mut issues = marker_issues(repo, path).map_err(|e| {
        vec![Issue {
            file: String::new(),
            line: None,
            detail: e.to_string(),
        }]
    })?;
    issues.extend(diff_check_issues(path));

    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

fn marker_issues(repo: &Repository, path: &Path) -> Result<Vec<Issue>, VerifyError> {
    let index = repo.index()?;
    let mut issues = Vec::new();

    for entry in index.iter() {
        let rel_path = String::from_utf8_lossy(&entry.path).to_string();
        let full_path = path.join(&rel_path);
        let Ok(content) = std::fs::read_to_string(&full_path) else {
            continue; // binary or unreadable -- markers can't live there anyway
        };
        for (i, line) in content.lines().enumerate() {
            if CONFLICT_MARKERS.iter().any(|m| line.starts_with(m)) {
                issues.push(Issue {
                    file: rel_path.clone(),
                    line: Some(i + 1),
                    detail: format!("unresolved conflict marker: {}", line.trim()),
                });
            }
        }
    }
    Ok(issues)
}

fn diff_check_issues(path: &Path) -> Vec<Issue> {
    let output = Command::new("git")
        .args(["diff", "--check"])
        .current_dir(path)
        .output();

    let Ok(output) = output else { return Vec::new() };
    if output.status.success() {
        return Vec::new();
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| Issue {
            file: l.split(':').next().unwrap_or("").to_string(),
            line: None,
            detail: l.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("failed to run git");
        assert!(status.success(), "git {:?} failed", args);
    }

    fn init_repo(dir: &Path) -> Repository {
        run(dir, &["init", "-b", "develop"]);
        run(dir, &["config", "user.email", "test@example.com"]);
        run(dir, &["config", "user.name", "Test"]);
        Repository::open(dir).unwrap()
    }

    #[test]
    fn unresolved_markers_block() {
        let dir = tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("f.txt"), "before\n").unwrap();
        run(dir.path(), &["add", "f.txt"]);
        run(dir.path(), &["commit", "-m", "init"]);

        std::fs::write(
            dir.path().join("f.txt"),
            "<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> other\n",
        )
        .unwrap();

        let issues = verify_resolved(&repo, dir.path()).unwrap_err();
        assert!(issues.iter().any(|i| i.file == "f.txt" && i.detail.contains("<<<<<<<")));
    }

    #[test]
    fn clean_resolution_passes() {
        let dir = tempdir().unwrap();
        let repo = init_repo(dir.path());
        std::fs::write(dir.path().join("f.txt"), "before\n").unwrap();
        run(dir.path(), &["add", "f.txt"]);
        run(dir.path(), &["commit", "-m", "init"]);

        std::fs::write(dir.path().join("f.txt"), "resolved\n").unwrap();

        assert!(verify_resolved(&repo, dir.path()).is_ok());
    }
}
