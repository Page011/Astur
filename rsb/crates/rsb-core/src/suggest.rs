//! "Did you mean" for misspelt keys, properties and names.

/// Edit distance (insert, delete, replace) between two strings, by chars.
pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb);
            cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The closest candidate within `max` edits, if any. A candidate must share
/// at least one char with the word so very short words do not match anything.
pub(crate) fn closest<'a>(word: &str, candidates: impl IntoIterator<Item = &'a str>, max: usize) -> Option<&'a str> {
    let mut best: Option<(usize, &str)> = None;
    for c in candidates {
        if c == word {
            continue;
        }
        let d = edit_distance(word, c);
        if d <= max && d < word.chars().count().max(2) && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, c));
        }
    }
    best.map(|(_, c)| c)
}

/// `; did you mean 'x'?` or an empty string.
pub fn hint<'a>(word: &str, candidates: impl IntoIterator<Item = &'a str>) -> String {
    match closest(word, candidates, 2) {
        Some(c) => format!("; did you mean '{c}'?"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distances() {
        assert_eq!(edit_distance("colr", "color"), 1);
        assert_eq!(edit_distance("heigth", "height"), 2);
        assert_eq!(edit_distance("", "ab"), 2);
        assert_eq!(closest("colr", ["color", "gap"], 2), Some("color"));
        assert_eq!(closest("zzzzz", ["color", "gap"], 2), None);
        assert_eq!(hint("paddin", ["padding"]), "; did you mean 'padding'?");
    }
}
