use std::fs;
use std::io;
use std::path::Path;

use crate::version::Version;

const HEADER: &str = "# Changelog\n\n";

/// Prepends a release section to `<workdir>/CHANGELOG.md`, creating the file
/// with a `# Changelog` header when it does not exist. `date` is an
/// already-formatted `YYYY-MM-DD` string -- `git_core` has no clock, the
/// command layer passes it. `body` is the (markdown) section content, typically
/// the user-edited seed from `release_scope::changelog_seed`.
pub fn prepend_section(
    workdir: &Path,
    version: &Version,
    date: &str,
    body: &str,
) -> Result<(), io::Error> {
    let path = workdir.join("CHANGELOG.md");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let rest = existing.strip_prefix(HEADER).unwrap_or(&existing);
    let section = format!("## {version} — {date}\n\n{}\n\n", body.trim());
    fs::write(&path, format!("{HEADER}{section}{}", rest.trim_start()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn v() -> Version {
        Version { major: 1, minor: 4, patch: 0 }
    }

    #[test]
    fn creates_file_with_header_when_absent() {
        let dir = tempdir().unwrap();
        prepend_section(dir.path(), &v(), "2026-08-31", "### Features\n- thing").unwrap();
        let out = fs::read_to_string(dir.path().join("CHANGELOG.md")).unwrap();
        assert_eq!(
            out,
            "# Changelog\n\n## 1.4.0 — 2026-08-31\n\n### Features\n- thing\n\n"
        );
    }

    #[test]
    fn prepends_section_preserving_existing() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("CHANGELOG.md"),
            "# Changelog\n\n## 1.3.0 — 2026-01-01\n\n- old\n\n",
        )
        .unwrap();
        prepend_section(dir.path(), &v(), "2026-08-31", "- new").unwrap();
        let out = fs::read_to_string(dir.path().join("CHANGELOG.md")).unwrap();
        assert_eq!(
            out,
            "# Changelog\n\n## 1.4.0 — 2026-08-31\n\n- new\n\n## 1.3.0 — 2026-01-01\n\n- old\n\n"
        );
    }
}
