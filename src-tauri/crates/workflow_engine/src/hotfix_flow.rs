/// Outcome of one target branch's MR, tracked independently per target --
/// a hotfix's master-MR and develop-MR can be at different points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MrOutcome {
    NotOpened,
    Open,
    Merged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HotfixState {
    pub master_mr: Option<MrOutcome>,
    pub develop_mr: Option<MrOutcome>,
}

impl HotfixState {
    pub fn both_merged(&self) -> bool {
        self.master_mr == Some(MrOutcome::Merged) && self.develop_mr == Some(MrOutcome::Merged)
    }
}

/// Hotfix creation/finish is not role-gated -- only Conflict Resolution is
/// Owner-only. Two MRs, tracked independently: `hotfix/* -> master`, then a
/// `master -> develop` sync MR (`OpenDevelopMr`). Neither is ever
/// auto-merged; a stuck MR is a human/provider matter, not this engine's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotfixAction {
    OpenMasterMr,
    /// The `master -> develop` sync MR.
    OpenDevelopMr,
}

pub fn apply(state: HotfixState, action: HotfixAction) -> HotfixState {
    match action {
        HotfixAction::OpenMasterMr => HotfixState { master_mr: Some(MrOutcome::Open), ..state },
        HotfixAction::OpenDevelopMr => HotfixState { develop_mr: Some(MrOutcome::Open), ..state },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_both_mrs_independently_through_open_and_merged() {
        let state = HotfixState::default();
        let state = apply(state, HotfixAction::OpenMasterMr);
        let state = apply(state, HotfixAction::OpenDevelopMr);
        assert_eq!(state.master_mr, Some(MrOutcome::Open));
        assert_eq!(state.develop_mr, Some(MrOutcome::Open));
        assert!(!state.both_merged());

        let one_merged = HotfixState { master_mr: Some(MrOutcome::Merged), develop_mr: Some(MrOutcome::Open) };
        assert!(!one_merged.both_merged());

        let both_merged = HotfixState { master_mr: Some(MrOutcome::Merged), develop_mr: Some(MrOutcome::Merged) };
        assert!(both_merged.both_merged());
    }

    /// Compile-time proof, not a runtime assertion: `HotfixAction` has
    /// exactly the two provider-facing variants below and nothing else --
    /// an exhaustive match with no wildcard arm fails to compile the moment
    /// a third variant (e.g. an auto-merge action) is added.
    #[test]
    fn exactly_two_mr_actions_exist() {
        fn assert_exhaustive(action: HotfixAction) -> &'static str {
            match action {
                HotfixAction::OpenMasterMr => "master",
                HotfixAction::OpenDevelopMr => "develop",
            }
        }
        assert_eq!(assert_exhaustive(HotfixAction::OpenMasterMr), "master");
    }
}
