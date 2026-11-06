//! npm package specs, read the same way by discovery and by the writers.

/// Split `name@version` / `@scope/name@version`. Scoped or bare name with no
/// `@version` yields an empty version string so callers can refuse it.
pub(crate) fn split_name_version(spec: &str) -> Option<(&str, &str)> {
    if let Some(without_at) = spec.strip_prefix('@') {
        if let Some(at) = without_at.find('@') {
            let name_end = at + 1; // include leading `@`
            Some((&spec[..name_end], &without_at[at + 1..]))
        } else if without_at.is_empty() {
            None
        } else {
            Some((spec, ""))
        }
    } else if let Some(at) = spec.find('@') {
        Some((&spec[..at], &spec[at + 1..]))
    } else if spec.is_empty() {
        None
    } else {
        Some((spec, ""))
    }
}
