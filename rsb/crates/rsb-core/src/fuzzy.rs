//! Typo-tolerant ranking for search lists (the app launcher, the Bluetooth
//! device filter, `rsb.fuzzy`). Pure and allocation-light: a few hundred
//! candidates are ranked per keystroke on the script thread.
//!
//! Matching is case-insensitive (Unicode lower case) and ignores leading and
//! trailing spaces. Tiers, best first, each scored above every lower tier:
//! the whole candidate; a prefix of the candidate; a prefix of a word
//! (words split on spaces, `-`, `_`, `.`, and lower-to-upper case changes);
//! a substring; a subsequence (every query character in order; fewer and
//! shorter gaps score higher); a typo (Damerau-Levenshtein distance 1 for a
//! 3-7 character query, 2 for 8 or more, to the nearest word prefix of the
//! query's length minus one, equal or plus one, so a dropped or an extra
//! letter counts as one typo: `clcul` finds Calculator). Within a tier an
//! earlier position and a shorter candidate score higher. An empty query
//! matches everything with score 0.

/// Room each tier owns in the score; a tier's bonus is always below it.
const TIER: u32 = 1_000_000;
const WHOLE: u32 = 6 * TIER;
const PREFIX: u32 = 5 * TIER;
const WORD_PREFIX: u32 = 4 * TIER;
const SUBSTRING: u32 = 3 * TIER;
const SUBSEQUENCE: u32 = 2 * TIER;
const TYPO: u32 = TIER;

/// A query is lower-cased and trimmed once, then used against many candidates.
struct Query {
    chars: Vec<char>,
}

impl Query {
    fn new(text: &str) -> Query {
        Query { chars: lower_chars(text.trim()) }
    }

    /// Typos allowed: none under 3 characters, one up to 7, two from 8.
    fn max_typos(&self) -> usize {
        match self.chars.len() {
            0..=2 => 0,
            3..=7 => 1,
            _ => 2,
        }
    }
}

/// One char to one char, so a position in the lower-cased text is a position
/// in the original (`'İ'` lower-cases to two chars; its first one is kept).
fn lower_one(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn lower_chars(text: &str) -> Vec<char> {
    text.chars().map(lower_one).collect()
}

/// A candidate prepared for matching; buffers are reused between candidates.
#[derive(Default)]
struct Cand {
    /// Lower-cased chars, one per original char.
    low: Vec<char>,
    /// Which chars start a word.
    starts: Vec<bool>,
    /// First and one past the last char left after trimming.
    lo: usize,
    hi: usize,
}

fn is_separator(c: char) -> bool {
    matches!(c, ' ' | '-' | '_' | '.')
}

impl Cand {
    fn load(&mut self, text: &str) {
        self.low.clear();
        self.starts.clear();
        let mut prev: Option<char> = None;
        for c in text.chars() {
            let start = match prev {
                None => !is_separator(c),
                Some(p) => (is_separator(p) && !is_separator(c)) || (p.is_lowercase() && c.is_uppercase()),
            };
            self.low.push(lower_one(c));
            self.starts.push(start);
            prev = Some(c);
        }
        self.lo = self.low.iter().position(|c| !c.is_whitespace()).unwrap_or(self.low.len());
        self.hi = self.low.iter().rposition(|c| !c.is_whitespace()).map_or(self.lo, |i| i + 1);
        // The first char after leading spaces starts a word.
        if self.lo < self.low.len() {
            self.starts[self.lo] = true;
        }
    }

    fn len(&self) -> usize {
        self.hi - self.lo
    }

    fn text(&self) -> &[char] {
        &self.low[self.lo..self.hi]
    }
}

/// A match: the score and the char indexes that matched.
struct Found {
    score: u32,
    /// Char index runs `[start, end)` that matched, for `positions`.
    runs: Vec<(usize, usize)>,
}

/// Position and length parts of a bonus: earlier and shorter score higher.
/// Always below one tier.
fn bonus(pos: usize, len: usize) -> u32 {
    (999 - pos.min(999) as u32) * 1000 + (999 - len.min(999) as u32)
}

fn find_from(hay: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == *needle)
}

/// Fewest-gap subsequence: from each place the first query char occurs, match
/// the rest greedily; keep the narrowest window.
fn subsequence(hay: &[char], q: &[char]) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    let mut from = 0;
    while let Some(start) = hay[from..].iter().position(|&c| c == q[0]).map(|p| p + from) {
        let mut at = start + 1;
        let mut end = start;
        let mut ok = true;
        for &qc in &q[1..] {
            match hay[at..].iter().position(|&c| c == qc) {
                Some(p) => {
                    end = at + p;
                    at = end + 1;
                }
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            break;
        }
        if best.is_none_or(|(s, e)| end - start < e - s) {
            best = Some((start, end));
        }
        from = start + 1;
    }
    best
}

/// Damerau-Levenshtein (optimal string alignment) distance, stopping early
/// once every cell passes `limit`.
fn distance(a: &[char], b: &[char], limit: usize, rows: &mut [Vec<usize>; 3]) -> Option<usize> {
    let (n, m) = (a.len(), b.len());
    if n.abs_diff(m) > limit {
        return None;
    }
    for r in rows.iter_mut() {
        r.clear();
        r.resize(m + 1, 0);
    }
    for (j, cell) in rows[1].iter_mut().enumerate() {
        *cell = j;
    }
    // rows[0] = two rows back, rows[1] = previous, rows[2] = current.
    for i in 1..=n {
        rows[2][0] = i;
        let mut row_min = i;
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (rows[1][j] + 1).min(rows[2][j - 1] + 1).min(rows[1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(rows[0][j - 2] + 1);
            }
            rows[2][j] = v;
            row_min = row_min.min(v);
        }
        if row_min > limit {
            return None;
        }
        rows.rotate_left(1);
        // After the rotation the finished row is rows[1] and rows[2] is free.
    }
    let d = rows[1][m];
    (d <= limit).then_some(d)
}

fn evaluate(q: &Query, cand: &Cand, rows: &mut [Vec<usize>; 3], runs: bool) -> Option<Found> {
    let qc = &q.chars;
    if qc.is_empty() {
        return Some(Found { score: 0, runs: Vec::new() });
    }
    let text = cand.text();
    let len = cand.len();
    let run = |start: usize, n: usize| if runs { vec![(cand.lo + start, cand.lo + start + n)] } else { Vec::new() };

    if text == qc.as_slice() {
        return Some(Found { score: WHOLE + bonus(0, len), runs: run(0, len) });
    }
    if text.starts_with(qc) {
        return Some(Found { score: PREFIX + bonus(0, len), runs: run(0, qc.len()) });
    }
    // A prefix of a later word.
    let mut from = 1;
    while let Some(p) = find_from(text, qc, from) {
        if cand.starts[cand.lo + p] {
            return Some(Found { score: WORD_PREFIX + bonus(p, len), runs: run(p, qc.len()) });
        }
        from = p + 1;
    }
    if let Some(p) = find_from(text, qc, 0) {
        return Some(Found { score: SUBSTRING + bonus(p, len), runs: run(p, qc.len()) });
    }
    if qc.len() > 1
        && let Some((s, e)) = subsequence(text, qc)
    {
        let gaps = (e - s + 1 - qc.len()) as u32;
        let score = SUBSEQUENCE + (999 - gaps.min(999)) * 1000 + (999 - (len as u32).min(999));
        let runs = if runs { subsequence_runs(text, qc, s, cand.lo) } else { Vec::new() };
        return Some(Found { score, runs });
    }
    typo(q, cand, rows, runs)
}

/// The runs of consecutive matched chars of a subsequence match that starts
/// at `start`.
fn subsequence_runs(text: &[char], q: &[char], start: usize, lo: usize) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut at = start;
    for &qc in q {
        let i = at + text[at..].iter().position(|&c| c == qc).unwrap_or(0);
        match runs.last_mut() {
            Some(last) if last.1 == lo + i => last.1 += 1,
            _ => runs.push((lo + i, lo + i + 1)),
        }
        at = i + 1;
    }
    runs
}

fn typo(q: &Query, cand: &Cand, rows: &mut [Vec<usize>; 3], runs: bool) -> Option<Found> {
    let limit = q.max_typos();
    if limit == 0 {
        return None;
    }
    let n = q.chars.len();
    let text = cand.text();
    let mut best: Option<(usize, usize, usize)> = None; // (distance, start, length)
    for start in 0..text.len() {
        if !cand.starts[cand.lo + start] {
            continue;
        }
        let word_end =
            (start + 1..text.len()).find(|&i| cand.starts[cand.lo + i] || is_separator(text[i])).unwrap_or(text.len());
        let word = &text[start..word_end];
        for l in n - 1..=n + 1 {
            if l == 0 || l > word.len() {
                continue;
            }
            if let Some(d) = distance(&q.chars, &word[..l], limit, rows)
                && best.is_none_or(|b| (d, start, l) < b)
            {
                best = Some((d, start, l));
            }
        }
    }
    let (d, start, l) = best?;
    let closeness = 3 - d.min(2) as u32;
    let score =
        TYPO + closeness * 300_000 + (99 - start.min(99) as u32) * 500 + (99 - (cand.len() as u32).min(99)) * 400;
    let runs = if runs { vec![(cand.lo + start, cand.lo + start + l)] } else { Vec::new() };
    Some(Found { score, runs })
}

fn new_rows() -> [Vec<usize>; 3] {
    [Vec::new(), Vec::new(), Vec::new()]
}

/// The candidate's score for `query`, or None when it does not match.
pub fn score(query: &str, candidate: &str) -> Option<u32> {
    let q = Query::new(query);
    let mut cand = Cand::default();
    cand.load(candidate);
    evaluate(&q, &cand, &mut new_rows(), false).map(|f| f.score)
}

/// Indexes of the matching `candidates`, best first; equal scores keep
/// their original order. At most `limit` (0 = all).
pub fn rank(query: &str, candidates: &[&str], limit: usize) -> Vec<usize> {
    let q = Query::new(query);
    let mut cand = Cand::default();
    let mut rows = new_rows();
    let mut scored: Vec<(u32, usize)> = Vec::new();
    for (i, text) in candidates.iter().enumerate() {
        cand.load(text);
        if let Some(f) = evaluate(&q, &cand, &mut rows, false) {
            scored.push((f.score, i));
        }
    }
    // Stable sort: equal scores keep their order.
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    let take = if limit == 0 { scored.len() } else { limit.min(scored.len()) };
    scored.into_iter().take(take).map(|(_, i)| i).collect()
}

/// The character indexes (`char` positions, not bytes) of `candidate` that
/// matched `query`, for highlighting; None when it does not match. Tiers as
/// `score`; a typo match reports the word prefix it matched.
pub fn positions(query: &str, candidate: &str) -> Option<Vec<usize>> {
    let q = Query::new(query);
    let mut cand = Cand::default();
    cand.load(candidate);
    let found = evaluate(&q, &cand, &mut new_rows(), true)?;
    Some(found.runs.into_iter().flat_map(|(s, e)| s..e).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(query: &str, list: &[&str]) -> Option<String> {
        rank(query, list, 1).first().map(|&i| list[i].to_string())
    }

    #[test]
    fn tiers_rank_in_order() {
        let list = ["Notepad calc", "calculator", "Calc", "my calc", "c_a_l_c", "kcalc"];
        let order: Vec<&str> = rank("calc", &list, 0).into_iter().map(|i| list[i]).collect();
        // whole, prefix (shorter first), word prefix, substring, subsequence.
        assert_eq!(order, vec!["Calc", "calculator", "my calc", "Notepad calc", "kcalc", "c_a_l_c"]);
    }

    #[test]
    fn every_tier_beats_the_next() {
        let q = "abc";
        let tiers = ["abc", "abcd", "x abc", "xabc", "a1b2c", "abd"];
        let scores: Vec<u32> = tiers.iter().map(|c| score(q, c).unwrap()).collect();
        assert!(scores.windows(2).all(|w| w[0] > w[1]), "{scores:?}");
    }

    #[test]
    fn typos_find_what_was_meant() {
        assert_eq!(top("blutooth", &["Brightness", "Bluetooth", "Network"]).as_deref(), Some("Bluetooth"));
        assert_eq!(top("clcul", &["Camera", "Calculator", "Clock"]).as_deref(), Some("Calculator"));
        assert_eq!(top("calculater", &["Calendar", "Calculator"]).as_deref(), Some("Calculator"));
        assert!(score("zzzzz", "Calculator").is_none());
    }

    #[test]
    fn short_queries_take_no_typos() {
        assert!(score("cx", "Calculator").is_none());
        assert!(score("cl", "Calculator").is_some());
    }

    #[test]
    fn a_swap_is_one_typo() {
        assert!(score("calcualtor", "Calculator").is_some());
    }

    #[test]
    fn case_and_edge_spaces_are_ignored() {
        assert_eq!(score("  CALC ", "calc"), score("calc", "  Calc  "));
        assert!(score("calc", "CALCULATOR").is_some());
    }

    #[test]
    fn empty_query_matches_everything_with_zero() {
        assert_eq!(score("", "anything"), Some(0));
        assert_eq!(score("   ", ""), Some(0));
        assert_eq!(rank("", &["b", "a"], 0), vec![0, 1]);
    }

    #[test]
    fn ties_keep_their_order_and_limit_cuts() {
        let list = ["ab1", "ab2", "ab3", "ab4"];
        assert_eq!(rank("ab", &list, 0), vec![0, 1, 2, 3]);
        assert_eq!(rank("ab", &list, 2), vec![0, 1]);
    }

    #[test]
    fn words_split_on_separators_and_case_changes() {
        assert!(score("pad", "Note-pad").unwrap() >= WORD_PREFIX);
        assert!(score("pad", "note_pad").unwrap() >= WORD_PREFIX);
        assert!(score("pad", "NotePad").unwrap() >= WORD_PREFIX);
        assert!(score("pad", "Notepad").unwrap() < WORD_PREFIX);
    }

    #[test]
    fn positions_are_char_indexes() {
        assert_eq!(positions("calc", "Calculator"), Some(vec![0, 1, 2, 3]));
        assert_eq!(positions("pad", "Note pad"), Some(vec![5, 6, 7]));
        assert_eq!(positions("ncr", "Note Calc Rest"), Some(vec![0, 5, 10]));
        assert_eq!(positions("zzz", "abc"), None);
        assert_eq!(positions("", "abc"), Some(vec![]));
    }

    #[test]
    fn subsequence_positions_come_from_the_narrowest_window() {
        // "ac" in "a b ac": the window at the end has no gap.
        assert_eq!(positions("ac", "a b a_c"), Some(vec![4, 6]));
        assert_eq!(positions("abc", "xaxbxc"), Some(vec![1, 3, 5]));
    }

    #[test]
    fn typo_positions_name_the_word_prefix() {
        assert_eq!(positions("calculater", "Calculator"), Some((0..10).collect::<Vec<_>>()));
        // A subsequence match highlights the letters that matched.
        assert_eq!(positions("blutooth", "My Bluetooth"), Some(vec![3, 4, 5, 7, 8, 9, 10, 11]));
        // Two typos (a missing letter and a swap) report the word prefix.
        let hit = positions("blutooht", "My Bluetooth").unwrap();
        assert_eq!(hit.first(), Some(&3));
        assert!(hit.windows(2).all(|w| w[1] == w[0] + 1), "{hit:?}");
    }

    #[test]
    fn positions_count_chars_not_bytes() {
        assert_eq!(positions("ber", "Ärger Berlin"), Some(vec![6, 7, 8]));
    }

    #[test]
    fn three_hundred_candidates_rank_fast() {
        let words = ["Calculator", "Bluetooth settings", "Windows Terminal", "Visual Studio Code", "File Explorer"];
        // 64 characters each: the longest a candidate is expected to be.
        let list: Vec<String> = (0..300)
            .map(|i| {
                let mut c = format!("{} {} ", words[i % 5], i);
                while c.chars().count() < 64 {
                    c.push_str(words[(i + 1) % 5]);
                    c.push(' ');
                }
                c.chars().take(64).collect()
            })
            .collect();
        let refs: Vec<&str> = list.iter().map(String::as_str).collect();
        // The first matches high up, the second only through typos, the third nowhere
        // (every word prefix is measured).
        let mut worst = std::time::Duration::ZERO;
        for q in ["blutooth", "clcul", "qzxjkw"] {
            let _ = rank(q, &refs, 50);
            let t = std::time::Instant::now();
            let runs = 20;
            for _ in 0..runs {
                let _ = rank(q, &refs, 50);
            }
            worst = worst.max(t.elapsed() / runs);
        }
        eprintln!("ranked 300 candidates of 64 characters in {worst:?} (worst of three queries)");
        // The real budget (2 ms) is for release builds.
        let budget = if cfg!(debug_assertions) { 60 } else { 2 };
        assert!(worst.as_millis() < budget, "{worst:?} per ranking");
    }
}
