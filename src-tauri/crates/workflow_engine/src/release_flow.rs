use crate::hotfix_flow::MrOutcome;

/// A release candidate's two MRs, tracked independently -- the
/// `release/x.y.z -> production` MR opened at finish, and the later
/// `production -> develop` sync MR opened only once a candidate has merged.
/// Decoupled on purpose: superseded candidates never owe a sync, and the sync
/// is a separate explicit Owner action, not part of `finish_release`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReleaseState {
    pub production_mr: Option<MrOutcome>,
    pub develop_sync_mr: Option<MrOutcome>,
}

impl ReleaseState {
    /// The production MR merged but no sync MR is open yet -- the Owner still
    /// owes a `production -> develop` sync.
    pub fn sync_owed(&self) -> bool {
        self.production_mr == Some(MrOutcome::Merged) && self.develop_sync_mr.is_none()
    }

    /// Both MRs merged -- the release shipped and develop is caught up.
    pub fn complete(&self) -> bool {
        self.production_mr == Some(MrOutcome::Merged)
            && self.develop_sync_mr == Some(MrOutcome::Merged)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseAction {
    OpenProductionMr,
    /// The `production -> develop` sync MR.
    OpenDevelopSyncMr,
}

pub fn apply(state: ReleaseState, action: ReleaseAction) -> ReleaseState {
    match action {
        ReleaseAction::OpenProductionMr => {
            ReleaseState { production_mr: Some(MrOutcome::Open), ..state }
        }
        ReleaseAction::OpenDevelopSyncMr => {
            ReleaseState { develop_sync_mr: Some(MrOutcome::Open), ..state }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_two_mrs_independently() {
        let s = ReleaseState::default();
        let s = apply(s, ReleaseAction::OpenProductionMr);
        assert_eq!(s.production_mr, Some(MrOutcome::Open));
        assert!(s.develop_sync_mr.is_none());
        assert!(!s.sync_owed());

        let merged = ReleaseState { production_mr: Some(MrOutcome::Merged), develop_sync_mr: None };
        assert!(merged.sync_owed());
        assert!(!merged.complete());
    }

    #[test]
    fn complete_only_when_both_merged() {
        let s = ReleaseState {
            production_mr: Some(MrOutcome::Merged),
            develop_sync_mr: Some(MrOutcome::Merged),
        };
        assert!(s.complete());
        assert!(!s.sync_owed());
    }
}
