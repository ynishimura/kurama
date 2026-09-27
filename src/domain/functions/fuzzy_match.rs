//! Fuzzy matching for the command palette: every character of the query in
//! order, case-insensitively; a run of adjacent characters and a match at
//! the start of a word score higher, an early first match a little more.

/// The score of `query` against `text` (already lower case), `None` when
/// the query's characters are not all there in order. An empty query
/// matches everything with score 0.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i64> {
    let mut score = 0i64;
    let mut previous: Option<usize> = None;
    let mut first: Option<usize> = None;
    let mut chars = text.char_indices();
    let mut before = ' ';
    'query: for wanted in query.chars().flat_map(char::to_lowercase) {
        if wanted == ' ' {
            continue;
        }
        for (index, found) in chars.by_ref() {
            let word_start = matches!(before, ' ' | '/' | '_' | '-' | '.' | ':' | '[' | '{');
            before = found;
            if found != wanted {
                continue;
            }
            score += 1;
            if previous.is_some_and(|at| at + 1 == index) {
                score += 5;
            }
            if word_start {
                score += 2;
            }
            first.get_or_insert(index);
            previous = Some(index + found.len_utf8() - 1);
            continue 'query;
        }
        return None;
    }
    Some(score - first.map_or(0, |at| (at as i64).min(10)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_in_order_match_and_a_word_or_a_run_scores_higher() {
        assert_eq!(fuzzy_score("", "anything"), Some(0));
        assert!(fuzzy_score("pg", "pets/get").is_some());
        assert!(fuzzy_score("gp", "pets/get").is_none(), "order matters");
        assert!(fuzzy_score("PETS", "pets/get").is_some(), "case does not");
        let run = fuzzy_score("get", "pets/get").unwrap();
        let scattered = fuzzy_score("get", "g-e-t").unwrap();
        assert!(run > scattered, "{run} > {scattered}");
        let word = fuzzy_score("pg", "pets/get").unwrap();
        let inside = fuzzy_score("pg", "apogee").unwrap();
        assert!(word > inside, "{word} > {inside}");
        assert!(fuzzy_score("opz", "op pets/get").is_none());
        assert!(
            fuzzy_score("op get", "op pets/get").is_some(),
            "spaces are skipped"
        );
    }
}
