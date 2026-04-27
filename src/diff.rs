// src/diff.rs – Shared tag diff logic (v2 – forced-remove wins, dedup)
//
// Because hamsters know you shouldn't touch another rodent's tag stash
// unless it's on the "managed" list.
//
// Renovation: forced_remove now unconditionally wins over desired_add.
// If a rule says +inbox and another says -inbox, the removal takes
// precedence. The hamster believes in negative authority.
//
// Also: to_add and to_remove are now deduplicated, so a tag never
// appears in both lists simultaneously (which would confuse callers
// and potentially cause double-indexing issues).

use std::collections::HashSet;

/// Compute which tags should be added and which removed.
///
/// `desired_add` – tags the rules want to see.
/// `forced_remove` – tags the rules explicitly ban.
/// `actual` – tags currently on the message.
/// `managed` – tags the hamster is allowed to alter.
///
/// The hamster will:
/// * Remove any forced tag that is present.
/// * Remove any managed tag that isn't desired.
/// * Add any desired tag that is missing and not forced-removed.
///
/// User tags (not in `managed`) stay untouched – we're polite rodents.
/// Forced removals take precedence over desired additions: if a tag is
/// in both sets, it is removed.
pub fn compute_tag_diff(
    desired_add: &HashSet<String>,
    forced_remove: &HashSet<String>,
    actual: &HashSet<String>,
    managed: &HashSet<String>,
) -> (Vec<String>, Vec<String>) {
    // Forced remove always wins. Strip forced-removed tags from desired_add
    // before computing the diff, so a tag never appears in both outputs.
    let effective_desired: HashSet<String> = desired_add
        .difference(forced_remove)
        .cloned()
        .collect();

    // +++++++++++++++
    // Add tags that are desired but missing.
    let to_add: Vec<String> = effective_desired.difference(actual).cloned().collect();

    // ---------------
    // Remove tags that are forced-removed and present.
    let mut to_remove: Vec<String> = forced_remove.intersection(actual).cloned().collect();

    // Also remove managed tags that are no longer desired.
    let extra_managed: Vec<String> = actual
        .difference(&effective_desired)
        .filter(|t| managed.contains(*t) && !forced_remove.contains(*t))
        .cloned()
        .collect();
    to_remove.extend(extra_managed);

    // Sort for deterministic output (helps tests and explain panels).
    let mut to_add = to_add;
    let mut to_remove = to_remove;
    to_add.sort();
    to_remove.sort();

    (to_add, to_remove)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_basic_add() {
        let desired = set(&["inbox", "unread"]);
        let forced = set(&[]);
        let actual = set(&["inbox"]);
        let managed = set(&["inbox", "unread", "archive"]);

        let (add, remove) = compute_tag_diff(&desired, &forced, &actual, &managed);
        assert_eq!(add, vec!["unread"]);
        assert!(remove.is_empty());
    }

    #[test]
    fn test_basic_remove() {
        let desired = set(&["inbox"]);
        let forced = set(&[]);
        let actual = set(&["inbox", "spam"]);
        let managed = set(&["inbox", "spam"]);

        let (add, remove) = compute_tag_diff(&desired, &forced, &actual, &managed);
        assert!(add.is_empty());
        assert_eq!(remove, vec!["spam"]);
    }

    #[test]
    fn test_forced_remove_wins() {
        // The critical fix: -inbox beats +inbox.
        let desired = set(&["inbox"]);
        let forced = set(&["inbox"]);
        let actual = set(&[]);
        let managed = set(&["inbox"]);

        let (add, remove) = compute_tag_diff(&desired, &forced, &actual, &managed);
        assert!(add.is_empty(), "inbox should NOT be added when forced-removed");
        assert!(remove.is_empty(), "inbox is not present, so nothing to remove");
    }

    #[test]
    fn test_forced_remove_present() {
        let desired = set(&["inbox"]);
        let forced = set(&["inbox"]);
        let actual = set(&["inbox"]);
        let managed = set(&["inbox"]);

        let (add, remove) = compute_tag_diff(&desired, &forced, &actual, &managed);
        assert!(add.is_empty());
        assert_eq!(remove, vec!["inbox"]);
    }

    #[test]
    fn test_unmanaged_tag_preserved() {
        let desired = set(&["inbox"]);
        let forced = set(&[]);
        let actual = set(&["inbox", "user-custom"]);
        let managed = set(&["inbox"]);

        let (add, remove) = compute_tag_diff(&desired, &forced, &actual, &managed);
        assert!(add.is_empty());
        assert!(remove.is_empty(), "user-custom is unmanaged and should stay");
    }

    #[test]
    fn test_no_overlap_between_add_and_remove() {
        // A tag should never appear in both lists.
        let desired = set(&["inbox", "unread"]);
        let forced = set(&["unread"]);
        let actual = set(&["inbox", "unread", "archive"]);
        let managed = set(&["inbox", "unread", "archive"]);

        let (add, remove) = compute_tag_diff(&desired, &forced, &actual, &managed);

        let add_set: HashSet<String> = add.into_iter().collect();
        let remove_set: HashSet<String> = remove.into_iter().collect();
        let intersection: Vec<&String> = add_set.intersection(&remove_set).collect();
        assert!(
            intersection.is_empty(),
            "No tag should be in both add and remove: {:?}",
            intersection
        );
    }

    #[test]
    fn test_conflict_with_actual_present() {
        // Rule A says +inbox, Rule B says -inbox, inbox is already present.
        // Result: remove it.
        let desired = set(&["inbox"]);
        let forced = set(&["inbox"]);
        let actual = set(&["inbox"]);
        let managed = set(&["inbox"]);

        let (add, remove) = compute_tag_diff(&desired, &forced, &actual, &managed);
        assert!(add.is_empty());
        assert_eq!(remove, vec!["inbox"]);
    }

    #[test]
    fn test_empty_inputs() {
        let desired = set(&[]);
        let forced = set(&[]);
        let actual = set(&["inbox"]);
        let managed = set(&["inbox"]);

        let (add, remove) = compute_tag_diff(&desired, &forced, &actual, &managed);
        assert!(add.is_empty());
        assert_eq!(remove, vec!["inbox"]);
    }
}
