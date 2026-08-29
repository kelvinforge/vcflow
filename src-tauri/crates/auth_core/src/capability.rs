/// Result of a non-mutating capability probe: whether the current
/// credential can do something, or whether that couldn't be determined
/// (e.g. a transient network error) -- distinct from a definite "no".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Yes,
    No,
    Unknown,
}
