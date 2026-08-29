use std::fmt;
use std::fs;
use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VersionError {
    #[error("invalid semver '{0}': expected major.minor.patch")]
    Parse(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BumpKind {
    Major,
    Minor,
    Patch,
}

impl Version {
    pub fn parse(s: &str) -> Result<Self, VersionError> {
        let s = s.trim();
        // Tolerate a leading tag prefix like `v` / `ver` / `release-` (common on git tags).
        let s = s.trim_start_matches(|c: char| !c.is_ascii_digit());
        let parts: Vec<&str> = s.split('.').collect();
        let [major, minor, patch] = parts.as_slice() else {
            return Err(VersionError::Parse(s.to_string()));
        };
        let parse_part = |p: &str| p.parse::<u64>().map_err(|_| VersionError::Parse(s.to_string()));
        Ok(Version {
            major: parse_part(major)?,
            minor: parse_part(minor)?,
            patch: parse_part(patch)?,
        })
    }

    pub fn bump(&self, kind: BumpKind) -> Self {
        match kind {
            BumpKind::Major => Version { major: self.major + 1, minor: 0, patch: 0 },
            BumpKind::Minor => Version { major: self.major, minor: self.minor + 1, patch: 0 },
            BumpKind::Patch => Version { major: self.major, minor: self.minor, patch: self.patch + 1 },
        }
    }

    /// Hotfixes always bump patch, no user confirm needed (unlike release).
    pub fn bump_patch(&self) -> Self {
        self.bump(BumpKind::Patch)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Reads and parses `<workdir>/VERSION`. This is the sole version source of
/// truth -- `CHANGELOG.md` is always manual and never read here.
pub fn read_version_file(workdir: &Path) -> Result<Version, VersionError> {
    let content = fs::read_to_string(workdir.join("VERSION"))?;
    Version::parse(&content)
}

/// Writes only `VERSION` -- never touches any other manifest.
pub fn write_version_file(workdir: &Path, version: &Version) -> Result<(), VersionError> {
    fs::write(workdir.join("VERSION"), format!("{version}\n"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_valid_semver() {
        assert_eq!(Version::parse("1.2.3").unwrap(), Version { major: 1, minor: 2, patch: 3 });
        assert_eq!(Version::parse(" 1.2.3 \n").unwrap(), Version { major: 1, minor: 2, patch: 3 });
        assert_eq!(Version::parse("v2.32.7").unwrap(), Version { major: 2, minor: 32, patch: 7 });
        assert_eq!(Version::parse("ver2.32.7").unwrap(), Version { major: 2, minor: 32, patch: 7 });
    }

    #[test]
    fn rejects_malformed_semver() {
        assert!(Version::parse("1.2").is_err());
        assert!(Version::parse("1.2.3.4").is_err());
        assert!(Version::parse("a.b.c").is_err());
        assert!(Version::parse("").is_err());
    }

    #[test]
    fn bumps_each_component_and_resets_lower() {
        let v = Version { major: 1, minor: 2, patch: 3 };
        assert_eq!(v.bump(BumpKind::Patch), Version { major: 1, minor: 2, patch: 4 });
        assert_eq!(v.bump(BumpKind::Minor), Version { major: 1, minor: 3, patch: 0 });
        assert_eq!(v.bump(BumpKind::Major), Version { major: 2, minor: 0, patch: 0 });
        assert_eq!(v.bump_patch(), Version { major: 1, minor: 2, patch: 4 });
    }

    #[test]
    fn round_trips_through_file() {
        let dir = tempdir().unwrap();
        let v = Version { major: 0, minor: 17, patch: 0 };
        write_version_file(dir.path(), &v).unwrap();
        assert_eq!(read_version_file(dir.path()).unwrap(), v);
    }
}
