use chrono::Utc;

use crate::audit::{AuditEntry, AuditError, AuditLog};
use crate::config::{Config, OverrideRole};

/// Resolves the effective role for `user` on `repository`: the local
/// override in `config.toml` wins if present, otherwise the provider's role
/// mapping (passed in already-resolved by the caller -- this crate doesn't
/// depend on `provider_core`, to avoid a dependency cycle). Never caches
/// across calls: every call re-reads `config` and writes a fresh audit row
/// so a stale role can't linger past a permission change.
pub fn resolve_role(
    user: &str,
    repository: &str,
    provider_role: OverrideRole,
    config: &Config,
    audit: &AuditLog,
) -> Result<OverrideRole, AuditError> {
    let (role, source) = match config.find_override(user, repository) {
        Some(override_role) => (override_role, "local_override"),
        None => (provider_role, "provider"),
    };

    audit.log(&AuditEntry {
        timestamp: Utc::now(),
        user: user.to_string(),
        provider: "n/a".to_string(),
        repository: repository.to_string(),
        branch: None,
        mr_pr: None,
        action: "resolve_role".to_string(),
        result: format!("{}:{source}", role_str(role)),
        error: None,
    })?;

    Ok(role)
}

fn role_str(role: OverrideRole) -> &'static str {
    match role {
        OverrideRole::Owner => "owner",
        OverrideRole::Member => "member",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RoleOverride;
    use tempfile::tempdir;

    fn audit_log() -> (tempfile::TempDir, AuditLog) {
        let dir = tempdir().unwrap();
        let log = AuditLog::open(dir.path().join("audit.sqlite3")).unwrap();
        (dir, log)
    }

    #[test]
    fn no_override_uses_provider_role_and_tags_source() {
        let (_dir, audit) = audit_log();
        let config = Config::default();

        let role = resolve_role("alice", "group/repo", OverrideRole::Member, &config, &audit).unwrap();
        assert_eq!(role, OverrideRole::Member);

        let rows = audit.recent(1).unwrap();
        assert_eq!(rows[0].action, "resolve_role");
        assert_eq!(rows[0].result, "member:provider");
    }

    #[test]
    fn override_present_wins_and_tags_source() {
        let (_dir, audit) = audit_log();
        let mut config = Config::default();
        config.overrides.push(RoleOverride {
            user: "alice".into(),
            repository: "group/repo".into(),
            role: OverrideRole::Owner,
        });

        // Provider says Member, but the local override says Owner -- override wins.
        let role = resolve_role("alice", "group/repo", OverrideRole::Member, &config, &audit).unwrap();
        assert_eq!(role, OverrideRole::Owner);

        let rows = audit.recent(1).unwrap();
        assert_eq!(rows[0].result, "owner:local_override");
    }
}
