//! Spelling suggestions for unknown names in commands and options.

/// Returns the candidate closest to `name` when it is near enough to be a typo.
pub(crate) fn closest_match<'a>(
    name: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    candidates
        .into_iter()
        .map(|candidate| (edit_distance(name, candidate), candidate))
        .filter(|(distance, candidate)| *distance <= 2.max(candidate.len() / 3))
        .min_by_key(|(distance, candidate)| (*distance, *candidate))
        .map(|(_, candidate)| candidate)
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right = right.chars().collect::<Vec<_>>();
    let mut row = (0..=right.len()).collect::<Vec<_>>();
    for (left_index, left_char) in left.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = left_index + 1;
        for (right_index, right_char) in right.iter().enumerate() {
            let above = row[right_index + 1];
            let cost = usize::from(left_char != *right_char);
            row[right_index + 1] = (above + 1).min(row[right_index] + 1).min(diagonal + cost);
            diagonal = above;
        }
    }
    row[right.len()]
}

#[cfg(test)]
mod tests {
    use super::closest_match;

    #[test]
    fn suggests_near_typos_only() {
        assert_eq!(
            closest_match("trakc", ["track", "ch", "key"]),
            Some("track")
        );
        assert_eq!(closest_match("zzzzzzzz", ["track", "ch"]), None);
    }
}
