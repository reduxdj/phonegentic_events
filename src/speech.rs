//! TTS speech normalizer — Rust mirror of the Dart `speech_normalizer.dart`.
//!
//! Runs on a *segmented sentence* right before it is handed to a TTS engine.
//! It must NOT run on streaming LLM deltas: a pattern that straddles two
//! deltas ("22" + "nd") would never match.
//!
//! Pass order matters. Each pass consumes what it recognises and leaves
//! everything else for the next one:
//!
//!   1. pronunciation fixes          Phonegentic → Phone-Jentic, yep → yes
//!   2. "#" / "No." before a number  → "number"
//!   3. ISO dates                    2026-09-22 → September twenty-second, …
//!   4. numeric dates                9/22, 09/22/2026
//!   5. month-name dates             Sep 22nd, September 22, 2026, Sep 22-24
//!      month + year                 Sept 2026 → September twenty twenty-six
//!   6. day-of-week abbreviations    Tues → Tuesday (context-guarded)
//!   7. clock times                  7:03 → seven oh three, 7pm → 7 p.m.
//!   8. street addresses             335 Main St → three thirty-five Main Street
//!   9. code-style labels            code 4821 → four eight two one
//!  10. identifier labels            Room 335 → Room three thirty-five
//!  11. ordinals                     22nd → twenty-second
//!  12. airline-style codes          UA278 → U A two seventy-eight
//!  13. common abbreviations         approx. → approximately, hrs → hours
//!  14. format_numbers_for_speech    phone runs / alphanumeric IDs
//!
//! The Dart side is regex-driven; this port hand-rolls each pattern as a
//! left-to-right scanner with the same leftmost / greedy / backtracking
//! semantics, so the two produce byte-identical output. The Dart test file
//! is the spec and is ported verbatim in `tests` below.

use crate::speech_numbers::format_numbers_for_speech;

/// Rewrite `text` for spoken TTS output. Idempotent for already-spoken text.
pub fn normalize_text_for_speech(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut s = apply_pronunciation_fixes(text);
    s = expand_number_sign(&s);
    s = expand_iso_dates(&s);
    s = expand_numeric_dates(&s);
    s = expand_month_name_dates(&s);
    s = expand_month_years(&s);
    s = expand_day_abbreviations(&s);
    s = expand_clock_times(&s);
    s = expand_street_addresses(&s);
    s = expand_code_labels(&s);
    s = expand_identifier_labels(&s);
    s = expand_ordinals(&s);
    s = expand_airline_codes(&s);
    s = expand_common_abbreviations(&s);
    format_numbers_for_speech(&s)
}

/// Remove markdown that an LLM leaks into spoken text so the markers are not
/// read aloud: `**bold**` / `*italic*` / `_italic_` (words kept), leading
/// `#` headings, `- ` / `* ` bullets at line start, backticks, and
/// `[label](url)` → `label`. A standalone `*` or `_` used as punctuation is
/// dropped. Apostrophes, hyphens and intra-word underscores are untouched.
pub fn strip_markdown_for_speech(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(text.len());
    for (idx, line) in text.split('\n').enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        strip_markdown_line(line, &mut out);
    }
    out
}

fn strip_markdown_line(line: &str, out: &mut String) {
    let c: Vec<char> = line.chars().collect();
    let indent_end = ws_run(&c, 0);
    let mut start = 0;
    // Heading: `#`+ then whitespace (or end of line).
    let mut j = indent_end;
    while at(&c, j) == Some('#') {
        j += 1;
    }
    if j > indent_end && (j == c.len() || c[j].is_whitespace()) {
        out.push_str(&slice(&c, 0, indent_end));
        start = ws_run(&c, j);
    } else if matches!(at(&c, indent_end), Some('-') | Some('*') | Some('+'))
        && c.get(indent_end + 1).is_some_and(|ch| ch.is_whitespace())
    {
        // List bullet.
        out.push_str(&slice(&c, 0, indent_end));
        start = ws_run(&c, indent_end + 1);
    }
    strip_markdown_inline(&c[start..], out);
}

fn strip_markdown_inline(c: &[char], out: &mut String) {
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        match ch {
            '`' => {
                i += 1;
            }
            '[' => {
                if let Some((label_a, label_b, end)) = markdown_link(c, i) {
                    strip_markdown_inline(&c[label_a..label_b], out);
                    i = end;
                } else {
                    out.push(ch);
                    i += 1;
                }
            }
            '!' if at(c, i + 1) == Some('[') => {
                if let Some((label_a, label_b, end)) = markdown_link(c, i + 1) {
                    strip_markdown_inline(&c[label_a..label_b], out);
                    i = end;
                } else {
                    out.push(ch);
                    i += 1;
                }
            }
            '*' => {
                // Keep a literal multiplication sign glued between digits.
                if i > 0 && c[i - 1].is_ascii_digit() && is_digit_at(c, i + 1) {
                    out.push(ch);
                    i += 1;
                } else {
                    i = drop_marker(c, i, out);
                }
            }
            '_' => {
                // Intra-word underscore (snake_case) is part of the word.
                if i > 0
                    && c[i - 1].is_alphanumeric()
                    && at(c, i + 1).is_some_and(|n| n.is_alphanumeric())
                {
                    out.push(ch);
                    i += 1;
                } else {
                    i = drop_marker(c, i, out);
                }
            }
            _ => {
                out.push(ch);
                i += 1;
            }
        }
    }
}

/// Drop the marker at `i`. If it sat between two spaces, also drop the
/// following space so "foo * bar" becomes "foo bar", not "foo  bar".
fn drop_marker(c: &[char], i: usize, out: &str) -> usize {
    let prev_space = out.chars().last().is_some_and(|p| p == ' ');
    let next_space = at(c, i + 1) == Some(' ');
    // Skip any run of the same marker first.
    let mut j = i;
    while at(c, j) == Some(c[i]) {
        j += 1;
    }
    if prev_space && next_space && j == i + 1 {
        return j + 1;
    }
    if prev_space && at(c, j) == Some(' ') {
        return j + 1;
    }
    j
}

/// `[label](url)` starting at `i` (the `[`). Returns (label_start,
/// label_end, end_after_closing_paren).
fn markdown_link(c: &[char], i: usize) -> Option<(usize, usize, usize)> {
    let label_a = i + 1;
    let mut j = label_a;
    while j < c.len() && c[j] != ']' && c[j] != '\n' && c[j] != '[' {
        j += 1;
    }
    if at(c, j) != Some(']') || at(c, j + 1) != Some('(') {
        return None;
    }
    let label_b = j;
    let mut k = j + 2;
    while k < c.len() && c[k] != ')' && c[k] != '\n' && !c[k].is_whitespace() {
        k += 1;
    }
    if at(c, k) != Some(')') {
        return None;
    }
    Some((label_a, label_b, k + 1))
}

// ───────────────────────── scanning helpers ─────────────────────────

/// Left-to-right, non-overlapping replacement driver. `f(chars, i)` either
/// matches at `i` and returns `(end, replacement)` or returns `None`, in
/// which case the char is copied and scanning resumes at `i + 1`. Lookbehind
/// checks inside `f` see the *original* text, exactly as a regex would.
fn scan<F>(s: &str, mut f: F) -> String
where
    F: FnMut(&[char], usize) -> Option<(usize, String)>,
{
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 16);
    let mut i = 0;
    while i < chars.len() {
        match f(&chars, i) {
            Some((end, rep)) if end > i => {
                out.push_str(&rep);
                i = end;
            }
            _ => {
                out.push(chars[i]);
                i += 1;
            }
        }
    }
    out
}

/// Regex `\w` (ASCII, as in Dart/JS without the unicode flag).
fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn at(c: &[char], i: usize) -> Option<char> {
    c.get(i).copied()
}

fn is_digit_at(c: &[char], i: usize) -> bool {
    at(c, i).is_some_and(|ch| ch.is_ascii_digit())
}

fn is_letter_at(c: &[char], i: usize) -> bool {
    at(c, i).is_some_and(|ch| ch.is_ascii_alphabetic())
}

fn is_word_at(c: &[char], i: usize) -> bool {
    at(c, i).is_some_and(is_word)
}

/// Regex `\b` at position `i`.
fn boundary(c: &[char], i: usize) -> bool {
    let before = i > 0 && is_word(c[i - 1]);
    let after = i < c.len() && is_word(c[i]);
    before != after
}

/// End of the whitespace run starting at `i` (`i` itself if none).
fn ws_run(c: &[char], i: usize) -> usize {
    let mut j = i;
    while j < c.len() && c[j].is_whitespace() {
        j += 1;
    }
    j
}

/// End of the ASCII digit run starting at `i` (`i` itself if none).
fn digit_run(c: &[char], i: usize) -> usize {
    let mut j = i;
    while j < c.len() && c[j].is_ascii_digit() {
        j += 1;
    }
    j
}

/// Literal match at `i`; returns the end index.
fn lit(c: &[char], i: usize, s: &str, ci: bool) -> Option<usize> {
    let mut j = i;
    for want in s.chars() {
        let got = *c.get(j)?;
        let eq = if ci {
            got.eq_ignore_ascii_case(&want)
        } else {
            got == want
        };
        if !eq {
            return None;
        }
        j += 1;
    }
    Some(j)
}

fn slice(c: &[char], a: usize, b: usize) -> String {
    c[a..b].iter().collect()
}

fn num(c: &[char], a: usize, b: usize) -> i64 {
    c[a..b].iter().fold(0i64, |acc, ch| {
        acc * 10 + ch.to_digit(10).unwrap_or(0) as i64
    })
}

/// `st|nd|rd|th` at `i`.
fn ordinal_suffix(c: &[char], i: usize, ci: bool) -> Option<usize> {
    let a = at(c, i)?;
    let b = at(c, i + 1)?;
    let (a, b) = if ci {
        (a.to_ascii_lowercase(), b.to_ascii_lowercase())
    } else {
        (a, b)
    };
    match (a, b) {
        ('s', 't') | ('n', 'd') | ('r', 'd') | ('t', 'h') => Some(i + 2),
        _ => None,
    }
}

/// `(?:19|20)\d{2}` at `i`.
fn year_1920(c: &[char], i: usize) -> Option<usize> {
    let c0 = at(c, i)?;
    let c1 = at(c, i + 1)?;
    if !((c0 == '1' && c1 == '9') || (c0 == '2' && c1 == '0')) {
        return None;
    }
    if !is_digit_at(c, i + 2) || !is_digit_at(c, i + 3) {
        return None;
    }
    Some(i + 4)
}

/// `,?\s+((?:19|20)\d{2})` at `i` → (end, raw year).
fn year_after(c: &[char], i: usize) -> Option<(usize, String)> {
    let mut j = i;
    if at(c, j) == Some(',') {
        j += 1;
    }
    let k = ws_run(c, j);
    if k == j {
        return None;
    }
    let e = year_1920(c, k)?;
    Some((e, slice(c, k, e)))
}

/// Drop an abbreviation dot unless it looks like a sentence end (followed by
/// end-of-text, a newline, or whitespace + capital letter). `after` is the
/// index just past the dot.
fn keep_dot_if_sentence_end(c: &[char], after: usize) -> &'static str {
    if after >= c.len() {
        return ".";
    }
    let rest = &c[after..];
    if rest.iter().all(|ch| ch.is_whitespace()) {
        return ".";
    }
    let k = ws_run(rest, 0);
    if k > 0 && rest.get(k).is_some_and(|ch| ch.is_ascii_uppercase()) {
        return ".";
    }
    if rest[0] == '\n' {
        return ".";
    }
    ""
}

// ───────────────────────── number → words ─────────────────────────

const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];

const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

const IRREGULAR_ORDINALS: [(&str, &str); 7] = [
    ("one", "first"),
    ("two", "second"),
    ("three", "third"),
    ("five", "fifth"),
    ("eight", "eighth"),
    ("nine", "ninth"),
    ("twelve", "twelfth"),
];

/// Cardinal words for 0 ≤ n < 1,000,000 ("three hundred thirty-five").
pub fn cardinal_words(n: i64) -> String {
    if n < 0 {
        return format!("minus {}", cardinal_words(-n));
    }
    if n < 20 {
        return ONES[n as usize].to_string();
    }
    if n < 100 {
        let t = (n / 10) as usize;
        let o = (n % 10) as usize;
        return if o == 0 {
            TENS[t].to_string()
        } else {
            format!("{}-{}", TENS[t], ONES[o])
        };
    }
    if n < 1000 {
        let h = (n / 100) as usize;
        let rem = n % 100;
        return if rem == 0 {
            format!("{} hundred", ONES[h])
        } else {
            format!("{} hundred {}", ONES[h], cardinal_words(rem))
        };
    }
    if n < 1_000_000 {
        let k = n / 1000;
        let rem = n % 1000;
        return if rem == 0 {
            format!("{} thousand", cardinal_words(k))
        } else {
            format!("{} thousand {}", cardinal_words(k), cardinal_words(rem))
        };
    }
    n.to_string()
}

/// Ordinal words for 0 ≤ n < 1,000,000 ("twenty-second", "hundredth").
pub fn ordinal_words(n: i64) -> String {
    let cardinal = cardinal_words(n);
    if n >= 1_000_000 {
        return cardinal;
    }
    // The ordinal marker attaches to the final word only.
    let cut = cardinal.rfind([' ', '-']);
    let (head, last) = match cut {
        Some(p) => (&cardinal[..=p], &cardinal[p + 1..]),
        None => ("", cardinal.as_str()),
    };
    let ordinal_last = if let Some((_, irr)) = IRREGULAR_ORDINALS.iter().find(|(w, _)| *w == last) {
        irr.to_string()
    } else if let Some(stem) = last.strip_suffix('y') {
        format!("{stem}ieth")
    } else {
        format!("{last}th")
    };
    format!("{head}{ordinal_last}")
}

/// Digit-by-digit ("four eight two one"). Zero is "oh", as in phone numbers.
pub fn spelled_digits(digits: &str) -> String {
    digits
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .map(|ch| {
            if ch == '0' {
                "oh"
            } else {
                ONES[ch.to_digit(10).unwrap() as usize]
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The way people say room numbers, flight numbers, house numbers and
/// highway numbers: split into pairs from the right.
///
/// - `335` → "three thirty-five", `305` → "three oh five", `300` → "three hundred"
/// - `1245` → "twelve forty-five", `1005` → "ten oh five", `1200` → "twelve hundred",
///   `1000` → "one thousand"
/// - `12345` → "twelve three forty-five"
/// - 1–2 digits → plain cardinal; leading zero → digit by digit
pub fn paired_number_words(digits: &str) -> String {
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return digits.to_string();
    }
    if digits.starts_with('0') {
        return spelled_digits(digits);
    }
    match digits.len() {
        1 | 2 => cardinal_words(digits.parse().unwrap_or(0)),
        3 => {
            let n: i64 = digits.parse().unwrap_or(0);
            let a = (n / 100) as usize;
            let bc = n % 100;
            if bc == 0 {
                format!("{} hundred", ONES[a])
            } else if bc < 10 {
                format!("{} oh {}", ONES[a], ONES[bc as usize])
            } else {
                format!("{} {}", ONES[a], cardinal_words(bc))
            }
        }
        4 => {
            let n: i64 = digits.parse().unwrap_or(0);
            let ab = n / 100;
            let cd = n % 100;
            if cd == 0 {
                if ab % 10 == 0 {
                    format!("{} thousand", ONES[(ab / 10) as usize])
                } else {
                    format!("{} hundred", cardinal_words(ab))
                }
            } else if cd < 10 {
                format!("{} oh {}", cardinal_words(ab), ONES[cd as usize])
            } else {
                format!("{} {}", cardinal_words(ab), cardinal_words(cd))
            }
        }
        5 => {
            let n: i64 = digits.parse().unwrap_or(0);
            format!(
                "{} {}",
                cardinal_words(n / 1000),
                paired_number_words(&digits[2..])
            )
        }
        _ => spelled_digits(digits),
    }
}

/// Years the way they are said: 1984 → "nineteen eighty-four",
/// 2007 → "two thousand seven", 2026 → "twenty twenty-six".
pub fn year_words(year: i64) -> String {
    if !(1000..=9999).contains(&year) {
        return cardinal_words(year);
    }
    let hi = year / 100;
    let lo = year % 100;
    if hi % 10 == 0 {
        // 2000–2099, 1000–1099 …
        if lo == 0 {
            return format!("{} thousand", ONES[(hi / 10) as usize]);
        }
        if lo < 10 {
            return format!(
                "{} thousand {}",
                ONES[(hi / 10) as usize],
                ONES[lo as usize]
            );
        }
        return format!("{} {}", cardinal_words(hi), cardinal_words(lo));
    }
    if lo == 0 {
        return format!("{} hundred", cardinal_words(hi));
    }
    if lo < 10 {
        return format!("{} oh {}", cardinal_words(hi), ONES[lo as usize]);
    }
    format!("{} {}", cardinal_words(hi), cardinal_words(lo))
}

fn month_name(m: i64) -> &'static str {
    if (1..=12).contains(&m) {
        MONTH_NAMES[(m - 1) as usize]
    } else {
        ""
    }
}

fn year_from_match(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    let mut y: i64 = raw.parse().unwrap_or(0);
    if raw.len() == 2 {
        y += 2000;
    }
    year_words(y)
}

// ───────────────────────── 1. pronunciation ─────────────────────────

fn apply_pronunciation_fixes(s: &str) -> String {
    let mut out = scan(s, |c, i| {
        lit(c, i, "Phonegentic", true).map(|e| (e, "Phone-Jentic".to_string()))
    });
    for (from, to) in SPEECH_PRONUNCIATION_WORDS {
        out = scan(&out, |c, i| {
            if !boundary(c, i) {
                return None;
            }
            let e = lit(c, i, from, false)?;
            if !boundary(c, e) {
                return None;
            }
            Some((e, to.to_string()))
        });
    }
    out
}

// ───────────────────────── 2. "#" / "No." ─────────────────────────

/// `(?:#|\bNo\.)\s*:?\s*(?=\d)` → "number ".
fn expand_number_sign(s: &str) -> String {
    scan(s, |c, i| {
        let j = if c[i] == '#' {
            i + 1
        } else if c[i] == 'N' && boundary(c, i) {
            lit(c, i, "No.", false)?
        } else {
            return None;
        };
        let mut k = ws_run(c, j);
        if at(c, k) == Some(':') {
            k += 1;
        }
        k = ws_run(c, k);
        if !is_digit_at(c, k) {
            return None;
        }
        let rep = if i > 0 && c[i - 1] != ' ' {
            " number "
        } else {
            "number "
        };
        Some((k, rep.to_string()))
    })
}

// ───────────────────────── 3–5. dates ─────────────────────────

/// `\b(\d{4})-(\d{2})-(\d{2})\b`.
fn expand_iso_dates(s: &str) -> String {
    scan(s, |c, i| {
        if !is_digit_at(c, i) || !boundary(c, i) {
            return None;
        }
        let y_end = digit_run(c, i);
        if y_end - i != 4 || at(c, y_end) != Some('-') {
            return None;
        }
        let m_start = y_end + 1;
        let m_end = digit_run(c, m_start);
        if m_end - m_start != 2 || at(c, m_end) != Some('-') {
            return None;
        }
        let d_start = m_end + 1;
        let d_end = digit_run(c, d_start);
        if d_end - d_start != 2 || !boundary(c, d_end) {
            return None;
        }
        let y = num(c, i, y_end);
        let mo = num(c, m_start, m_end);
        let d = num(c, d_start, d_end);
        if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
            return Some((d_end, slice(c, i, d_end)));
        }
        Some((
            d_end,
            format!("{} {}, {}", month_name(mo), ordinal_words(d), year_words(y)),
        ))
    })
}

const DATE_CONTEXT_WORDS: [&str; 15] = [
    "on",
    "by",
    "until",
    "till",
    "due",
    "date",
    "dated",
    "before",
    "after",
    "from",
    "through",
    "since",
    "starting",
    "deadline",
    "scheduled",
];

/// `\b(?:on|by|…)\s+$` against the text before `i`, case-insensitive.
fn date_context_before(c: &[char], i: usize) -> bool {
    let mut k = i;
    while k > 0 && c[k - 1].is_whitespace() {
        k -= 1;
    }
    if k == i {
        return false;
    }
    let mut w = k;
    while w > 0 && is_word(c[w - 1]) {
        w -= 1;
    }
    let word: String = slice(c, w, k).to_ascii_lowercase();
    DATE_CONTEXT_WORDS.iter().any(|kw| *kw == word)
}

/// `9/22`, `09/22/2026`, `9/22/26`. A bare `a/b` with both parts single
/// digit (`1/2`, `3/4`) is more likely a fraction and is left alone unless a
/// year follows or a date word precedes it.
fn expand_numeric_dates(s: &str) -> String {
    scan(s, |c, i| {
        if !is_digit_at(c, i) {
            return None;
        }
        if i > 0 && (c[i - 1].is_ascii_digit() || c[i - 1] == '/') {
            return None;
        }
        let a_run = digit_run(c, i);
        let a_end = a_run.min(i + 2);
        if a_run > a_end || at(c, a_end) != Some('/') {
            return None;
        }
        let b_start = a_end + 1;
        let b_run = digit_run(c, b_start);
        if b_run == b_start {
            return None;
        }
        let b_end = b_run.min(b_start + 2);
        if b_run > b_end {
            return None;
        }
        let mut end = b_end;
        let mut year: Option<String> = None;
        if at(c, b_end) == Some('/') {
            let y_start = b_end + 1;
            let y_run = digit_run(c, y_start);
            let ylen = y_run - y_start;
            if ylen == 4 || ylen == 2 {
                year = Some(slice(c, y_start, y_run));
                end = y_run;
            } else {
                return None;
            }
        }
        if at(c, end).is_some_and(|n| n.is_ascii_digit() || n == '/') {
            return None;
        }
        let a = slice(c, i, a_end);
        let b = slice(c, b_start, b_end);
        let has_context = date_context_before(c, i);
        if year.is_none() && a.len() == 1 && b.len() == 1 && !has_context {
            return Some((end, slice(c, i, end))); // probably a fraction
        }
        let mut mo: i64 = a.parse().unwrap_or(0);
        let mut d: i64 = b.parse().unwrap_or(0);
        if mo > 12 && d <= 12 {
            std::mem::swap(&mut mo, &mut d);
        }
        if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
            return Some((end, slice(c, i, end)));
        }
        let y = year.map(|r| year_from_match(&r)).unwrap_or_default();
        let mut b = format!("{} {}", month_name(mo), ordinal_words(d));
        if !y.is_empty() {
            b.push_str(", ");
            b.push_str(&y);
        }
        Some((end, b))
    })
}

/// Every month alternative (full names first, then abbreviations, in the
/// Dart alternation order) that matches at `i`, as (end, alternative).
fn month_alts(c: &[char], i: usize, ci: bool) -> Vec<(usize, &'static str)> {
    MONTH_NAMES
        .iter()
        .copied()
        .chain(MONTH_ABBREVIATIONS.iter().map(|(k, _)| *k))
        .filter_map(|alt| lit(c, i, alt, ci).map(|e| (e, alt)))
        .collect()
}

fn full_month(raw: &str) -> String {
    let key: String = raw.replace('.', "");
    let lower = key.to_lowercase();
    for full in MONTH_NAMES {
        if full.to_lowercase() == lower {
            return full.to_string();
        }
    }
    for (k, v) in MONTH_ABBREVIATIONS {
        if k.to_lowercase() == lower {
            return v.to_string();
        }
    }
    raw.to_string()
}

fn is_capitalized(w: &str) -> bool {
    w.chars().next().is_some_and(|ch| ch.is_uppercase())
}

/// Abbreviations must be capitalised ("Mar 5", not "mar 5") to avoid
/// rewriting ordinary words; full month names are accepted in any case.
fn month_token_ok(raw: &str) -> bool {
    let is_abbrev = !MONTH_NAMES.iter().any(|f| f.eq_ignore_ascii_case(raw));
    !is_abbrev || is_capitalized(raw)
}

/// `\s*[-–]\s*(\d{1,2})(?:st|nd|rd|th)?` → (end, day).
fn day_range(c: &[char], i: usize) -> Option<(usize, i64)> {
    let mut k = ws_run(c, i);
    if !matches!(at(c, k), Some('-') | Some('–')) {
        return None;
    }
    k = ws_run(c, k + 1);
    let run = digit_run(c, k);
    let len = run - k;
    if len == 0 || len > 2 {
        return None;
    }
    let d2 = num(c, k, run);
    let end = ordinal_suffix(c, run, true).unwrap_or(run);
    Some((end, d2))
}

/// `Sep 22`, `Sept. 22nd`, `September 22, 2026`, `Sep 22-24`, `Sep 22nd – 24th`.
/// Years are restricted to 19xx/20xx so "Sep 22 1000 people" is not a date.
fn match_month_day(c: &[char], i: usize) -> Option<(usize, String)> {
    if !is_letter_at(c, i) || !boundary(c, i) {
        return None;
    }
    for (m_end, _) in month_alts(c, i, true) {
        let month_raw = slice(c, i, m_end);
        let mut j = m_end;
        if at(c, j) == Some('.') {
            j += 1;
        }
        let k = ws_run(c, j);
        if k == j {
            continue;
        }
        let d_run = digit_run(c, k);
        let d_len = d_run - k;
        if d_len == 0 || d_len > 2 {
            continue;
        }
        let day = num(c, k, d_run);
        let after_day = ordinal_suffix(c, d_run, true).unwrap_or(d_run);
        // Greedy-then-backtrack order: range+year, range, year, neither.
        let mut cands: Vec<(usize, Option<i64>, Option<String>)> = Vec::new();
        if let Some((r_end, d2)) = day_range(c, after_day) {
            if let Some((y_end, y)) = year_after(c, r_end) {
                cands.push((y_end, Some(d2), Some(y)));
            }
            cands.push((r_end, Some(d2), None));
        }
        if let Some((y_end, y)) = year_after(c, after_day) {
            cands.push((y_end, None, Some(y)));
        }
        cands.push((after_day, None, None));
        for (end, d2, y) in cands {
            if !boundary(c, end) {
                continue;
            }
            if !month_token_ok(&month_raw) || !(1..=31).contains(&day) {
                return Some((end, slice(c, i, end)));
            }
            let mut b = format!("{} {}", full_month(&month_raw), ordinal_words(day));
            if let Some(d2) = d2 {
                if (1..=31).contains(&d2) {
                    b.push_str(&format!(" through {}", ordinal_words(d2)));
                }
            }
            if let Some(y) = y {
                let yw = year_from_match(&y);
                if !yw.is_empty() {
                    b.push_str(&format!(", {yw}"));
                }
            }
            return Some((end, b));
        }
    }
    None
}

/// `\bthe\s+$` against the text before `i`, case-insensitive.
fn the_before(c: &[char], i: usize) -> bool {
    let mut k = i;
    while k > 0 && c[k - 1].is_whitespace() {
        k -= 1;
    }
    if k == i || k < 3 {
        return false;
    }
    slice(c, k - 3, k).eq_ignore_ascii_case("the") && boundary(c, k - 3)
}

/// `22 Sep`, `22nd September 2026` → "the twenty-second of September …".
fn match_day_month(c: &[char], i: usize) -> Option<(usize, String)> {
    if !is_digit_at(c, i) || !boundary(c, i) {
        return None;
    }
    let d_run = digit_run(c, i);
    if d_run - i > 2 {
        return None;
    }
    let day = num(c, i, d_run);
    let j = ordinal_suffix(c, d_run, true).unwrap_or(d_run);
    let k = ws_run(c, j);
    if k == j {
        return None;
    }
    for (m_end, _) in month_alts(c, k, true) {
        if !boundary(c, m_end) {
            continue;
        }
        let month_raw = slice(c, k, m_end);
        let mut end = m_end;
        if at(c, end) == Some('.') {
            end += 1;
        }
        let mut year: Option<String> = None;
        if let Some((y_end, y)) = year_after(c, end) {
            end = y_end;
            year = Some(y);
        }
        if !month_token_ok(&month_raw) || !(1..=31).contains(&day) {
            return Some((end, slice(c, i, end)));
        }
        let has_the = the_before(c, i);
        let yw = year.map(|y| year_from_match(&y)).unwrap_or_default();
        let mut b = format!(
            "{}{} of {}",
            if has_the { "" } else { "the " },
            ordinal_words(day),
            full_month(&month_raw)
        );
        if !yw.is_empty() {
            b.push_str(&format!(", {yw}"));
        }
        return Some((end, b));
    }
    None
}

fn expand_month_name_dates(s: &str) -> String {
    let out = scan(s, match_month_day);
    scan(&out, match_day_month)
}

/// `Sept 2026`, `Sep. 2026` → "September twenty twenty-six".
fn expand_month_years(s: &str) -> String {
    scan(s, |c, i| {
        if !is_letter_at(c, i) || !boundary(c, i) {
            return None;
        }
        for (m_end, _) in month_alts(c, i, true) {
            let mut j = m_end;
            if at(c, j) == Some('.') {
                j += 1;
            }
            if at(c, j) == Some(',') {
                j += 1;
            }
            let k = ws_run(c, j);
            if k == j {
                continue;
            }
            let Some(y_end) = year_1920(c, k) else {
                continue;
            };
            if !boundary(c, y_end) {
                continue;
            }
            let raw = slice(c, i, m_end);
            if !month_token_ok(&raw) {
                return Some((y_end, slice(c, i, y_end)));
            }
            return Some((
                y_end,
                format!("{} {}", full_month(&raw), year_words(num(c, k, y_end))),
            ));
        }
        None
    })
}

// ───────────────────────── 6. day-of-week abbreviations ─────────────────────────

const DAY_FOLLOWED_WORDS: [&str; 4] = ["morning", "afternoon", "evening", "night"];

/// Lookahead `(?=\s*(?:,|\d|the\s+\d|MONTH|morning|afternoon|evening|night|at\b))`.
fn day_followed_lookahead(c: &[char], j: usize) -> bool {
    let k = ws_run(c, j);
    let Some(ch) = at(c, k) else { return false };
    if ch == ',' || ch.is_ascii_digit() {
        return true;
    }
    if let Some(e) = lit(c, k, "the", false) {
        let w = ws_run(c, e);
        if w > e && is_digit_at(c, w) {
            return true;
        }
    }
    if !month_alts(c, k, false).is_empty() {
        return true;
    }
    if DAY_FOLLOWED_WORDS
        .iter()
        .any(|w| lit(c, k, w, false).is_some())
    {
        return true;
    }
    if let Some(e) = lit(c, k, "at", false) {
        if boundary(c, e) {
            return true;
        }
    }
    false
}

const DAY_PRECEDED_WORDS: [&str; 16] = [
    "on", "this", "next", "last", "every", "by", "until", "till", "through", "thru", "from", "for",
    "and", "or", "see you", "starting",
];

/// Lookbehind `(?<=\b(?:on|this|next|…)\s)`.
fn day_preceded_lookbehind(c: &[char], i: usize) -> bool {
    if i == 0 || !c[i - 1].is_whitespace() {
        return false;
    }
    let word_end = i - 1;
    DAY_PRECEDED_WORDS.iter().any(|kw| {
        let len = kw.chars().count();
        if word_end < len {
            return false;
        }
        let start = word_end - len;
        lit(c, start, kw, false) == Some(word_end) && boundary(c, start)
    })
}

fn expand_day_abbreviations(s: &str) -> String {
    // Abbreviation followed by something date-like: `Tues 9/22`, `Sat, Sep 22`,
    // `Mon. the 22nd`. The trailing dot is an abbreviation dot and is dropped.
    let out = scan(s, |c, i| {
        if !boundary(c, i) {
            return None;
        }
        for (abbr, full) in DAY_ABBREVIATIONS {
            let Some(mut j) = lit(c, i, abbr, false) else {
                continue;
            };
            if at(c, j) == Some('.') {
                j += 1;
            }
            if day_followed_lookahead(c, j) {
                return Some((j, full.to_string()));
            }
        }
        None
    });
    // Abbreviation preceded by a scheduling word: `on Tues`, `next Wed.`,
    // `every Fri`. A trailing dot is kept — with nothing date-like after it,
    // it is probably the sentence end.
    scan(&out, |c, i| {
        if !day_preceded_lookbehind(c, i) {
            return None;
        }
        for (abbr, full) in DAY_ABBREVIATIONS {
            if let Some(j) = lit(c, i, abbr, false) {
                if boundary(c, j) {
                    return Some((j, full.to_string()));
                }
            }
        }
        None
    })
}

// ───────────────────────── 7. clock times ─────────────────────────

/// `\s*([AaPp])\.?[Mm]\.?(?![A-Za-z])` at `j` → (end, 'a' | 'p').
fn match_meridiem(c: &[char], j: usize) -> Option<(usize, char)> {
    let k = ws_run(c, j);
    let ap = at(c, k)?;
    if !matches!(ap, 'A' | 'a' | 'P' | 'p') {
        return None;
    }
    let mut k = k + 1;
    if at(c, k) == Some('.') {
        k += 1;
    }
    if !matches!(at(c, k), Some('M') | Some('m')) {
        return None;
    }
    k += 1;
    let ap = ap.to_ascii_lowercase();
    if at(c, k) == Some('.') {
        if is_letter_at(c, k + 1) {
            return Some((k, ap)); // dot not consumed; lookahead sees '.'
        }
        return Some((k + 1, ap));
    }
    if is_letter_at(c, k) {
        return None;
    }
    Some((k, ap))
}

fn minute_words(mm: i64) -> String {
    if mm == 0 {
        return String::new();
    }
    if mm < 10 {
        return format!("oh {}", ONES[mm as usize]);
    }
    cardinal_words(mm)
}

fn expand_clock_times(s: &str) -> String {
    // `7:03`, `10:30`, `7:03 PM`, `7:03pm`, `19:00`, `12:00 p.m.`. Times with
    // seconds (`7:03:45`) are left alone.
    let out = scan(s, |c, i| {
        if !is_digit_at(c, i) {
            return None;
        }
        if i > 0 && (c[i - 1].is_ascii_digit() || c[i - 1] == ':') {
            return None;
        }
        let h_run = digit_run(c, i);
        let h_end = h_run.min(i + 2);
        if h_run > h_end || at(c, h_end) != Some(':') {
            return None;
        }
        let m_start = h_end + 1;
        let m_end = digit_run(c, m_start);
        if m_end - m_start != 2 || at(c, m_end) == Some(':') {
            return None;
        }
        let (end, mer) = match match_meridiem(c, m_end) {
            Some((e, ap)) => (e, Some(ap)),
            None => (m_end, None),
        };
        let mut h = num(c, i, h_end);
        let mm = num(c, m_start, m_end);
        if h > 23 || mm > 59 {
            return Some((end, slice(c, i, end)));
        }
        let mut meridiem = match mer {
            None => "",
            Some('a') => "a.m.",
            Some(_) => "p.m.",
        };
        if h == 0 {
            h = 12;
            if meridiem.is_empty() {
                meridiem = "a.m.";
            }
        } else if h > 12 {
            h -= 12;
            if meridiem.is_empty() {
                meridiem = "p.m.";
            }
        }
        let minutes = minute_words(mm);
        let mut b = ONES[h as usize].to_string();
        if !minutes.is_empty() {
            b.push(' ');
            b.push_str(&minutes);
        } else if meridiem.is_empty() {
            b.push_str(" o'clock");
        }
        if !meridiem.is_empty() {
            b.push(' ');
            b.push_str(meridiem);
        }
        Some((end, b))
    });
    // Bare `7pm`, `7 PM`, `11 a.m.` — normalise the meridiem only; the hour
    // is a small count TTS already says correctly.
    scan(&out, |c, i| {
        if !is_digit_at(c, i) || !boundary(c, i) {
            return None;
        }
        let h_run = digit_run(c, i);
        let h_end = h_run.min(i + 2);
        if h_run > h_end {
            return None;
        }
        let (end, ap) = match_meridiem(c, h_end)?;
        let h = num(c, i, h_end);
        if !(1..=12).contains(&h) {
            return Some((end, slice(c, i, end)));
        }
        let meridiem = if ap == 'a' { "a.m." } else { "p.m." };
        Some((end, format!("{} {}", slice(c, i, h_end), meridiem)))
    })
}

// ───────────────────────── 8. street addresses ─────────────────────────

const DIRECTIONAL_WORDS: [&str; 4] = ["North", "South", "East", "West"];

fn directional_full(letter: &str) -> Option<&'static str> {
    match letter {
        "N" => Some("North"),
        "S" => Some("South"),
        "E" => Some("East"),
        "W" => Some("West"),
        _ => None,
    }
}

/// `(?:[NSEW]|North|South|East|West)\.?\s+` at `p` → end.
fn directional(c: &[char], p: usize) -> Option<usize> {
    if matches!(at(c, p), Some('N') | Some('S') | Some('E') | Some('W')) {
        let mut j = p + 1;
        if at(c, j) == Some('.') {
            j += 1;
        }
        let w = ws_run(c, j);
        if w > j {
            return Some(w);
        }
    }
    for word in DIRECTIONAL_WORDS {
        if let Some(mut j) = lit(c, p, word, false) {
            if at(c, j) == Some('.') {
                j += 1;
            }
            let w = ws_run(c, j);
            if w > j {
                return Some(w);
            }
        }
    }
    None
}

/// One street-name token: `[A-Z][A-Za-z']*` or `\d{1,3}(?:st|nd|rd|th)`.
fn street_name_token(c: &[char], pos: usize) -> Option<usize> {
    let ch = at(c, pos)?;
    if ch.is_ascii_uppercase() {
        let mut j = pos + 1;
        while at(c, j).is_some_and(|n| n.is_ascii_alphabetic() || n == '\'') {
            j += 1;
        }
        return Some(j);
    }
    if ch.is_ascii_digit() {
        let run = digit_run(c, pos);
        if run - pos > 3 {
            return None;
        }
        return ordinal_suffix(c, run, false);
    }
    None
}

/// Exactly `k` name tokens each followed by `\s+`, starting at `start`.
fn street_name_tokens(c: &[char], start: usize, k: usize) -> Option<usize> {
    let mut pos = start;
    for _ in 0..k {
        let e = street_name_token(c, pos)?;
        let w = ws_run(c, e);
        if w == e {
            return None;
        }
        pos = w;
    }
    Some(pos)
}

/// Street suffix alternation (keys then values, deduped) followed by `\b`.
fn street_suffix(c: &[char], pos: usize) -> Option<(usize, &'static str)> {
    let mut seen: Vec<&'static str> = Vec::new();
    for alt in STREET_SUFFIXES
        .iter()
        .map(|(k, _)| *k)
        .chain(STREET_SUFFIXES.iter().map(|(_, v)| *v))
    {
        if seen.contains(&alt) {
            continue;
        }
        seen.push(alt);
        if let Some(e) = lit(c, pos, alt, false) {
            if boundary(c, e) {
                return Some((e, alt));
            }
        }
    }
    None
}

/// `335 Main St`, `1245 W 5th Ave.`, `10 Downing Street`.
fn expand_street_addresses(s: &str) -> String {
    scan(s, |c, i| {
        if !is_digit_at(c, i) || !boundary(c, i) {
            return None;
        }
        let n_run = digit_run(c, i);
        let n_end = n_run.min(i + 5);
        if n_run > n_end {
            return None;
        }
        let p = ws_run(c, n_end);
        if p == n_end {
            return None;
        }
        let mut starts: Vec<usize> = Vec::new();
        if let Some(de) = directional(c, p) {
            starts.push(de);
        }
        starts.push(p);
        for st in starts {
            for k in (1..=3).rev() {
                let Some(t_end) = street_name_tokens(c, st, k) else {
                    continue;
                };
                let Some((s_end, suffix_raw)) = street_suffix(c, t_end) else {
                    continue;
                };
                let had_dot = at(c, s_end) == Some('.');
                let end = if had_dot { s_end + 1 } else { s_end };
                let number = slice(c, i, n_end);
                let name_raw = slice(c, p, t_end);
                let name_trim = name_raw.trim_end();
                let mut tokens: Vec<String> =
                    name_trim.split_whitespace().map(str::to_string).collect();
                let first = tokens[0].replace('.', "");
                let name = if let Some(d) = directional_full(&first) {
                    tokens[0] = d.to_string();
                    tokens.join(" ")
                } else {
                    name_trim.to_string()
                };
                let suffix = STREET_SUFFIXES
                    .iter()
                    .find(|(k, _)| *k == suffix_raw)
                    .map(|(_, v)| *v)
                    .unwrap_or(suffix_raw);
                let spoken = if number.len() >= 3 {
                    paired_number_words(&number)
                } else {
                    number
                };
                let dot = if had_dot {
                    keep_dot_if_sentence_end(c, end)
                } else {
                    ""
                };
                return Some((end, format!("{spoken} {name} {suffix}{dot}")));
            }
        }
        None
    })
}

// ───────────────────────── 9–10. labelled numbers ─────────────────────────

struct LabelMatch {
    end: usize,
    label: String,
    tail: String,
    digits: String,
}

/// `\s*:?\s*(\d{3,4})\b` at `pos` → (digits_start, digits_end).
fn label_digits(c: &[char], pos: usize) -> Option<(usize, usize)> {
    let mut k = ws_run(c, pos);
    if at(c, k) == Some(':') {
        k += 1;
    }
    let d = ws_run(c, k);
    let run = digit_run(c, d);
    let len = run - d;
    if (len == 3 || len == 4) && !is_word_at(c, run) {
        Some((d, run))
    } else {
        None
    }
}

/// `no\.?` style tail alternative (a trailing dot is greedy).
fn label_tail(c: &[char], pos: usize, tail: &str) -> Option<usize> {
    if let Some(stem) = tail.strip_suffix('.') {
        let e = lit(c, pos, stem, true)?;
        Some(if at(c, e) == Some('.') { e + 1 } else { e })
    } else {
        lit(c, pos, tail, true)
    }
}

/// `\b(LABEL)(\s+(?:TAIL))?\s*:?\s*(\d{3,4})\b`, case-insensitive.
fn match_label_number(c: &[char], i: usize, labels: &[&str], tails: &[&str]) -> Option<LabelMatch> {
    if !is_letter_at(c, i) || !boundary(c, i) {
        return None;
    }
    for label in labels {
        let Some(l_end) = lit(c, i, label, true) else {
            continue;
        };
        let w = ws_run(c, l_end);
        if w > l_end {
            for tail in tails {
                let Some(t_end) = label_tail(c, w, tail) else {
                    continue;
                };
                if let Some((ds, de)) = label_digits(c, t_end) {
                    return Some(LabelMatch {
                        end: de,
                        label: slice(c, i, l_end),
                        tail: slice(c, l_end, t_end),
                        digits: slice(c, ds, de),
                    });
                }
            }
        }
        if let Some((ds, de)) = label_digits(c, l_end) {
            return Some(LabelMatch {
                end: de,
                label: slice(c, i, l_end),
                tail: String::new(),
                digits: slice(c, ds, de),
            });
        }
    }
    None
}

const CODE_LABEL_TAILS: [&str; 5] = ["number", "no.", "num", "code", "id"];
const IDENTIFIER_LABEL_TAILS: [&str; 2] = ["number", "no."];

/// `code 4821`, `PIN 335`, `order number 4821`, `confirmation #: 48213`.
fn expand_code_labels(s: &str) -> String {
    scan(s, |c, i| {
        let m = match_label_number(c, i, &CODE_LABELS, &CODE_LABEL_TAILS)?;
        Some((
            m.end,
            format!("{}{} {}", m.label, m.tail, spelled_digits(&m.digits)),
        ))
    })
}

const LETTERED_SLOT_LABELS: [&str; 10] = [
    "gate", "seat", "row", "terminal", "pier", "dock", "zone", "section", "door", "lot",
];

/// `Gate B12`, `seat 12A`, `row 3B` → "Gate B twelve", "seat twelve A".
fn match_lettered_slot(c: &[char], i: usize) -> Option<(usize, String)> {
    if !is_letter_at(c, i) || !boundary(c, i) {
        return None;
    }
    for label in LETTERED_SLOT_LABELS {
        let Some(l_end) = lit(c, i, label, true) else {
            continue;
        };
        let p = ws_run(c, l_end);
        if p == l_end {
            continue;
        }
        let label_raw = slice(c, i, l_end);
        // ([A-Z])(\d{1,3})\b
        if is_letter_at(c, p) {
            let run = digit_run(c, p + 1);
            let len = run - (p + 1);
            if (1..=3).contains(&len) && !is_word_at(c, run) {
                return Some((
                    run,
                    format!(
                        "{} {} {}",
                        label_raw,
                        c[p].to_ascii_uppercase(),
                        cardinal_words(num(c, p + 1, run))
                    ),
                ));
            }
            continue;
        }
        // (\d{1,3})([A-Z])\b
        if is_digit_at(c, p) {
            let run = digit_run(c, p);
            let len = run - p;
            if (1..=3).contains(&len) && is_letter_at(c, run) && boundary(c, run + 1) {
                return Some((
                    run + 1,
                    format!(
                        "{} {} {}",
                        label_raw,
                        cardinal_words(num(c, p, run)),
                        c[run].to_ascii_uppercase()
                    ),
                ));
            }
        }
    }
    None
}

/// `I-405`, `I-5`, `US-101`, `US 101`. The hyphen is mandatory after `I` so
/// the pronoun ("I 100 percent agree") is left alone.
fn match_highway(c: &[char], i: usize) -> Option<(usize, String)> {
    if !boundary(c, i) {
        return None;
    }
    let (prefix, pos) = if c[i] == 'I' && at(c, i + 1) == Some('-') {
        ("I", i + 2)
    } else if let Some(e) = lit(c, i, "US", false) {
        (
            "US",
            if matches!(at(c, e), Some('-') | Some(' ')) {
                e + 1
            } else {
                e
            },
        )
    } else if let Some(e) = lit(c, i, "SR", false) {
        (
            "SR",
            if matches!(at(c, e), Some('-') | Some(' ')) {
                e + 1
            } else {
                e
            },
        )
    } else {
        return None;
    };
    let run = digit_run(c, pos);
    let len = run - pos;
    if !(1..=3).contains(&len) || is_word_at(c, run) {
        return None;
    }
    let letters: Vec<String> = prefix.chars().map(|ch| ch.to_string()).collect();
    Some((
        run,
        format!(
            "{} {}",
            letters.join(" "),
            paired_number_words(&slice(c, pos, run))
        ),
    ))
}

/// `Room 335`, `flight 1245`, `Gate 12`, `exit 405`, `number 335`.
fn expand_identifier_labels(s: &str) -> String {
    let out = scan(s, |c, i| {
        let m = match_label_number(c, i, &IDENTIFIER_LABELS, &IDENTIFIER_LABEL_TAILS)?;
        Some((
            m.end,
            format!("{}{} {}", m.label, m.tail, paired_number_words(&m.digits)),
        ))
    });
    let out = scan(&out, match_lettered_slot);
    scan(&out, match_highway)
}

// ───────────────────────── 11. ordinals ─────────────────────────

/// `22nd`, `33RD`, `101st`. Mismatched suffixes (`22th`) are common LLM
/// typos and are read from the number, not the suffix.
fn expand_ordinals(s: &str) -> String {
    scan(s, |c, i| {
        if !is_digit_at(c, i) || !boundary(c, i) {
            return None;
        }
        let run = digit_run(c, i);
        if run - i > 6 {
            return None;
        }
        let e = ordinal_suffix(c, run, true)?;
        if !boundary(c, e) {
            return None;
        }
        Some((e, ordinal_words(num(c, i, run))))
    })
}

// ───────────────────────── 12. airline-style codes ─────────────────────────

/// `UA278`, `UA-278`, `DL1245`, `AA100` → letters spelled, digits paired the
/// way gate agents say flight numbers. Five or more digits (order IDs) are
/// left for `format_numbers_for_speech`.
fn expand_airline_codes(s: &str) -> String {
    scan(s, |c, i| {
        if !boundary(c, i) {
            return None;
        }
        if !c[i].is_ascii_uppercase() || !at(c, i + 1).is_some_and(|ch| ch.is_ascii_uppercase()) {
            return None;
        }
        let mut j = i + 2;
        if at(c, j) == Some('-') {
            j += 1;
        }
        let run = digit_run(c, j);
        let len = run - j;
        if !(3..=4).contains(&len) || is_word_at(c, run) {
            return None;
        }
        Some((
            run,
            format!(
                "{} {} {}",
                c[i],
                c[i + 1],
                paired_number_words(&slice(c, j, run))
            ),
        ))
    })
}

// ───────────────────────── 13. common abbreviations ─────────────────────────

#[derive(Clone, Copy, Debug)]
pub enum AbbrevPattern {
    /// `\bWORD\.?(?![A-Za-z])`
    WordOptionalDot { word: &'static str, ci: bool },
    /// `\bWORD\b`
    Word { word: &'static str },
    /// `\bX\.Y\.,?` — `e.g.` / `i.e.`
    Dotted { word: &'static str },
    /// `\bw/o(?![A-Za-z])`
    WithoutSlash,
    /// `\bw/\s*(?=\w)`
    WithSlash,
    /// `\bMt\.(?=\s+[A-Z])`
    MountDot,
    /// `\s*&\s*`
    Ampersand,
}

#[derive(Clone, Copy, Debug)]
pub struct CommonAbbreviation {
    pub pattern: AbbrevPattern,
    pub spoken: &'static str,
}

fn match_common_abbreviation(
    c: &[char],
    i: usize,
    entry: &CommonAbbreviation,
) -> Option<(usize, String)> {
    use AbbrevPattern::*;
    let (end, had_dot): (usize, bool) = match entry.pattern {
        WordOptionalDot { word, ci } => {
            if !boundary(c, i) {
                return None;
            }
            let j = lit(c, i, word, ci)?;
            if at(c, j) == Some('.') {
                if is_letter_at(c, j + 1) {
                    (j, false)
                } else {
                    (j + 1, true)
                }
            } else if is_letter_at(c, j) {
                return None;
            } else {
                (j, false)
            }
        }
        Word { word } => {
            if !boundary(c, i) {
                return None;
            }
            let j = lit(c, i, word, false)?;
            if !boundary(c, j) {
                return None;
            }
            (j, false)
        }
        Dotted { word } => {
            if !boundary(c, i) {
                return None;
            }
            let j = lit(c, i, word, false)?;
            if at(c, j) == Some(',') {
                (j + 1, false)
            } else {
                (j, true)
            }
        }
        WithoutSlash => {
            if !boundary(c, i) {
                return None;
            }
            let j = lit(c, i, "w/o", false)?;
            if is_letter_at(c, j) {
                return None;
            }
            (j, false)
        }
        WithSlash => {
            if !boundary(c, i) {
                return None;
            }
            let j = lit(c, i, "w/", false)?;
            let k = ws_run(c, j);
            if !is_word_at(c, k) {
                return None;
            }
            (k, false)
        }
        MountDot => {
            if !boundary(c, i) {
                return None;
            }
            let j = lit(c, i, "Mt.", false)?;
            let k = ws_run(c, j);
            if k == j || !at(c, k).is_some_and(|ch| ch.is_ascii_uppercase()) {
                return None;
            }
            (j, true)
        }
        Ampersand => {
            let k = ws_run(c, i);
            if at(c, k) != Some('&') {
                return None;
            }
            (ws_run(c, k + 1), false)
        }
    };
    let dot = if had_dot {
        keep_dot_if_sentence_end(c, end)
    } else {
        ""
    };
    Some((end, format!("{}{}", entry.spoken, dot)))
}

fn expand_common_abbreviations(s: &str) -> String {
    let mut out = s.to_string();
    for entry in COMMON_ABBREVIATIONS {
        out = scan(&out, |c, i| match_common_abbreviation(c, i, &entry));
    }
    // Unit abbreviations only when a number precedes them ("15 min", "2 hrs").
    scan(&out, |c, i| {
        if !is_digit_at(c, i) || !boundary(c, i) {
            return None;
        }
        let run = digit_run(c, i);
        let mut j = run;
        if at(c, j) == Some('.') && is_digit_at(c, j + 1) {
            j = digit_run(c, j + 1);
        }
        let k = ws_run(c, j);
        for (key, word) in UNIT_ABBREVIATIONS {
            let Some(e) = lit(c, k, key, true) else {
                continue;
            };
            if is_letter_at(c, e) {
                continue;
            }
            let number = slice(c, i, j);
            let mut spoken = word.to_string();
            if number == "1" {
                if word == "feet" {
                    spoken = "foot".to_string();
                } else if let Some(stem) = word.strip_suffix('s') {
                    spoken = stem.to_string();
                }
            }
            return Some((e, format!("{number} {spoken}")));
        }
        None
    })
}

// =============================================================================
// Tables
// =============================================================================

/// Word-bounded, case-preserving "yep" fixes (PocketTTS mispronounces it).
/// The brand fix (`Phonegentic` → `Phone-Jentic`, case-insensitive, not
/// word-bounded) is applied first in `apply_pronunciation_fixes`.
pub const SPEECH_PRONUNCIATION_WORDS: [(&str, &str); 3] =
    [("yep", "yes"), ("Yep", "Yes"), ("YEP", "YES")];

pub const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Month abbreviations, expanded only inside a date (adjacent to a day
/// number) because several are ordinary words or names (Mar, Jan, Dec).
pub const MONTH_ABBREVIATIONS: [(&str, &str); 12] = [
    ("Jan", "January"),
    ("Feb", "February"),
    ("Mar", "March"),
    ("Apr", "April"),
    ("Jun", "June"),
    ("Jul", "July"),
    ("Aug", "August"),
    ("Sep", "September"),
    ("Sept", "September"),
    ("Oct", "October"),
    ("Nov", "November"),
    ("Dec", "December"),
];

/// Day-of-week abbreviations. Capitalised only, and guarded by context
/// (followed by a date / preceded by a scheduling word) because Sat, Sun,
/// Wed and Mon are also ordinary words.
pub const DAY_ABBREVIATIONS: [(&str, &str); 11] = [
    ("Mon", "Monday"),
    ("Tue", "Tuesday"),
    ("Tues", "Tuesday"),
    ("Wed", "Wednesday"),
    ("Weds", "Wednesday"),
    ("Thu", "Thursday"),
    ("Thur", "Thursday"),
    ("Thurs", "Thursday"),
    ("Fri", "Friday"),
    ("Sat", "Saturday"),
    ("Sun", "Sunday"),
];

/// Street suffixes. `St`, `Dr` and `Pl` are expanded only inside an address
/// (`<number> <Name> St`) because they also mean Saint, Doctor and plural.
pub const STREET_SUFFIXES: [(&str, &str); 30] = [
    ("St", "Street"),
    ("Ave", "Avenue"),
    ("Rd", "Road"),
    ("Blvd", "Boulevard"),
    ("Dr", "Drive"),
    ("Ln", "Lane"),
    ("Ct", "Court"),
    ("Pl", "Place"),
    ("Hwy", "Highway"),
    ("Pkwy", "Parkway"),
    ("Ter", "Terrace"),
    ("Terr", "Terrace"),
    ("Cir", "Circle"),
    ("Trl", "Trail"),
    ("Sq", "Square"),
    ("Way", "Way"),
    ("Street", "Street"),
    ("Avenue", "Avenue"),
    ("Road", "Road"),
    ("Boulevard", "Boulevard"),
    ("Drive", "Drive"),
    ("Lane", "Lane"),
    ("Court", "Court"),
    ("Place", "Place"),
    ("Highway", "Highway"),
    ("Parkway", "Parkway"),
    ("Terrace", "Terrace"),
    ("Circle", "Circle"),
    ("Trail", "Trail"),
    ("Square", "Square"),
];

/// Labels whose 3–4 digit number is read digit by digit ("four eight two
/// one"). Longer runs already go through `format_numbers_for_speech`.
pub const CODE_LABELS: [&str; 49] = [
    "code",
    "pin",
    "passcode",
    "password",
    "otp",
    "verification",
    "confirmation",
    "conf",
    "order",
    "ticket",
    "invoice",
    "account",
    "acct",
    "reference",
    "ref",
    "case",
    "claim",
    "policy",
    "id",
    "badge",
    "tracking",
    "zip",
    "postal",
    "cvv",
    "cvc",
    "ssn",
    "member",
    "membership",
    "customer",
    "loyalty",
    "rewards",
    "card",
    "license",
    "licence",
    "plate",
    "serial",
    "sku",
    "transaction",
    "receipt",
    "booking",
    "reservation",
    "locator",
    "itinerary",
    "pnr",
    "voucher",
    "coupon",
    "promo",
    "extension",
    "ext",
];

/// Labels whose 3–4 digit number is read in pairs ("three thirty-five",
/// "twelve forty-five") — rooms, flights, roads, gates, pages.
pub const IDENTIFIER_LABELS: [&str; 49] = [
    "room",
    "rm",
    "suite",
    "ste",
    "flight",
    "gate",
    "route",
    "rte",
    "rt",
    "highway",
    "hwy",
    "interstate",
    "freeway",
    "unit",
    "apt",
    "apartment",
    "channel",
    "ch",
    "bus",
    "train",
    "exit",
    "terminal",
    "platform",
    "floor",
    "level",
    "hangar",
    "hall",
    "building",
    "bldg",
    "section",
    "row",
    "seat",
    "table",
    "booth",
    "bay",
    "pier",
    "dock",
    "box",
    "lot",
    "zone",
    "precinct",
    "ward",
    "district",
    "line",
    "track",
    "page",
    "pg",
    "chapter",
    "number",
];

/// Common written abbreviations → spoken form. Word-bounded; an optional
/// trailing dot is consumed and re-added only when it ends the sentence.
/// Applied in order, each as a full pass over the text.
pub const COMMON_ABBREVIATIONS: [CommonAbbreviation; 30] = {
    use AbbrevPattern::*;
    const fn wd(word: &'static str, ci: bool, spoken: &'static str) -> CommonAbbreviation {
        CommonAbbreviation {
            pattern: WordOptionalDot { word, ci },
            spoken,
        }
    }
    const fn wb(word: &'static str, spoken: &'static str) -> CommonAbbreviation {
        CommonAbbreviation {
            pattern: Word { word },
            spoken,
        }
    }
    [
        wd("approx", true, "approximately"),
        wd("etc", false, "et cetera"),
        wd("vs", true, "versus"),
        CommonAbbreviation {
            pattern: Dotted { word: "e.g." },
            spoken: "for example,",
        },
        CommonAbbreviation {
            pattern: Dotted { word: "i.e." },
            spoken: "that is,",
        },
        CommonAbbreviation {
            pattern: WithoutSlash,
            spoken: "without",
        },
        CommonAbbreviation {
            pattern: WithSlash,
            spoken: "with ",
        },
        wd("appt", true, "appointment"),
        wd("appts", true, "appointments"),
        wd("msg", true, "message"),
        wd("msgs", true, "messages"),
        wd("dept", true, "department"),
        wd("Bros", false, "Brothers"),
        wb("ASAP", "as soon as possible"),
        wb("ETA", "E T A"),
        wb("FYI", "F Y I"),
        wb("TBD", "to be determined"),
        wb("DOB", "date of birth"),
        wb("SSN", "social security number"),
        wd("Ave", false, "Avenue"),
        wd("Blvd", false, "Boulevard"),
        wd("Rd", false, "Road"),
        wd("Hwy", false, "Highway"),
        wd("Pkwy", false, "Parkway"),
        wd("Ln", false, "Lane"),
        wd("Apt", false, "Apartment"),
        wd("Ste", false, "Suite"),
        CommonAbbreviation {
            pattern: MountDot,
            spoken: "Mount",
        },
        wd("ext", true, "extension"),
        CommonAbbreviation {
            pattern: Ampersand,
            spoken: " and ",
        },
    ]
};

/// Unit abbreviations, expanded only after a number ("15 min" → "15
/// minutes"; "1 hr" → "1 hour"). Keys lower-case, no dot.
pub const UNIT_ABBREVIATIONS: [(&str, &str); 19] = [
    ("hr", "hours"),
    ("hrs", "hours"),
    ("min", "minutes"),
    ("mins", "minutes"),
    ("sec", "seconds"),
    ("secs", "seconds"),
    ("lb", "pounds"),
    ("lbs", "pounds"),
    ("oz", "ounces"),
    ("mph", "miles per hour"),
    ("km", "kilometers"),
    ("mi", "miles"),
    ("ft", "feet"),
    ("wk", "weeks"),
    ("wks", "weeks"),
    ("yr", "years"),
    ("yrs", "years"),
    ("mo", "months"),
    ("mos", "months"),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> String {
        normalize_text_for_speech(s)
    }

    // ───────────── ordinals ─────────────

    #[test]
    fn ordinals_user_reported_suffixes() {
        assert_eq!(n("the 22nd"), "the twenty-second");
        assert_eq!(n("the 23rd"), "the twenty-third");
        assert_eq!(n("the 33rd"), "the thirty-third");
        assert_eq!(n("the 44th"), "the forty-fourth");
        assert_eq!(n("the 25th"), "the twenty-fifth");
    }

    #[test]
    fn ordinals_irregulars_and_large_values() {
        assert_eq!(n("1st, 2nd, 3rd, 4th"), "first, second, third, fourth");
        assert_eq!(
            n("11th 12th 13th 20th 21st"),
            "eleventh twelfth thirteenth twentieth twenty-first"
        );
        assert_eq!(
            n("100th and 101st and 1000th"),
            "one hundredth and one hundred first and one thousandth"
        );
        assert_eq!(n("33RD floor"), "thirty-third floor");
    }

    #[test]
    fn ordinals_mismatched_suffix_is_read_from_the_number() {
        assert_eq!(n("22th"), "twenty-second");
    }

    // ───────────── cardinal / paired helpers ─────────────

    #[test]
    fn cardinal_words_cases() {
        assert_eq!(cardinal_words(0), "zero");
        assert_eq!(cardinal_words(15), "fifteen");
        assert_eq!(cardinal_words(42), "forty-two");
        assert_eq!(cardinal_words(335), "three hundred thirty-five");
        assert_eq!(cardinal_words(1245), "one thousand two hundred forty-five");
    }

    #[test]
    fn paired_number_words_cases() {
        assert_eq!(paired_number_words("335"), "three thirty-five");
        assert_eq!(paired_number_words("305"), "three oh five");
        assert_eq!(paired_number_words("300"), "three hundred");
        assert_eq!(paired_number_words("101"), "one oh one");
        assert_eq!(paired_number_words("1245"), "twelve forty-five");
        assert_eq!(paired_number_words("1005"), "ten oh five");
        assert_eq!(paired_number_words("1200"), "twelve hundred");
        assert_eq!(paired_number_words("1000"), "one thousand");
        assert_eq!(paired_number_words("2025"), "twenty twenty-five");
        assert_eq!(paired_number_words("12345"), "twelve three forty-five");
        assert_eq!(paired_number_words("042"), "oh four two");
    }

    #[test]
    fn year_words_cases() {
        assert_eq!(year_words(2026), "twenty twenty-six");
        assert_eq!(year_words(2007), "two thousand seven");
        assert_eq!(year_words(2000), "two thousand");
        assert_eq!(year_words(1984), "nineteen eighty-four");
        assert_eq!(year_words(1905), "nineteen oh five");
    }

    #[test]
    fn ordinal_and_spelled_helpers() {
        assert_eq!(ordinal_words(22), "twenty-second");
        assert_eq!(ordinal_words(100), "one hundredth");
        assert_eq!(ordinal_words(20), "twentieth");
        assert_eq!(spelled_digits("4821"), "four eight two one");
        assert_eq!(spelled_digits("2025"), "two oh two five");
    }

    // ───────────── three- and four-digit identifiers ─────────────

    #[test]
    fn identifiers_labels_read_in_pairs() {
        assert_eq!(n("Room 335"), "Room three thirty-five");
        assert_eq!(
            n("your flight 335 departs"),
            "your flight three thirty-five departs"
        );
        assert_eq!(n("flight 1245"), "flight twelve forty-five");
        assert_eq!(n("take exit 405"), "take exit four oh five");
        assert_eq!(n("Suite 300"), "Suite three hundred");
        assert_eq!(n("Room #335"), "Room number three thirty-five");
        assert_eq!(n("#335"), "number three thirty-five");
        assert_eq!(n("Gate B12"), "Gate B twelve");
        assert_eq!(n("seat 12A"), "seat twelve A");
        assert_eq!(n("I-405 and US 101"), "I four oh five and U S one oh one");
    }

    #[test]
    fn identifiers_plain_counts_are_untouched() {
        assert_eq!(n("I have 335 messages."), "I have 335 messages.");
        assert_eq!(n("I have 25 messages."), "I have 25 messages.");
        assert_eq!(n("I 100 percent agree"), "I 100 percent agree");
    }

    #[test]
    fn identifiers_code_labels_read_digit_by_digit() {
        assert_eq!(n("your code is 4821"), "your code is four eight two one");
        assert_eq!(n("PIN 335"), "PIN three three five");
        assert_eq!(n("order number 4821"), "order number four eight two one");
        // The colon is dropped so TTS does not pause before the digits.
        assert_eq!(
            n("confirmation code: 2025"),
            "confirmation code two oh two five"
        );
        assert_eq!(n("extension 335"), "extension three three five");
    }

    // ───────────── street addresses ─────────────

    #[test]
    fn street_house_number_paired_and_suffix_expanded() {
        assert_eq!(n("335 Main St"), "three thirty-five Main Street");
        assert_eq!(
            n("1245 W 5th Ave. tomorrow"),
            "twelve forty-five West fifth Avenue tomorrow"
        );
        // End of text counts as sentence end, so the dot is kept.
        assert_eq!(n("1245 W 5th Ave."), "twelve forty-five West fifth Avenue.");
        assert_eq!(n("at 7 Elm Street"), "at 7 Elm Street");
        assert_eq!(
            n("2025 Oak Blvd, Apt 4"),
            "twenty twenty-five Oak Boulevard, Apartment 4"
        );
    }

    #[test]
    fn street_sentence_final_dot_survives() {
        assert_eq!(
            n("It is at 335 Main St. Then turn left."),
            "It is at three thirty-five Main Street. Then turn left."
        );
    }

    // ───────────── day and month abbreviations ─────────────

    #[test]
    fn day_abbreviations_user_examples() {
        assert_eq!(n("on Tues"), "on Tuesday");
        assert_eq!(n("this Mon"), "this Monday");
        assert_eq!(n("Tues 9/22"), "Tuesday September twenty-second");
        assert_eq!(n("Sat, Sep 22"), "Saturday, September twenty-second");
        assert_eq!(n("next Wed."), "next Wednesday.");
        assert_eq!(n("Thurs at 7:03"), "Thursday at seven oh three");
    }

    #[test]
    fn day_abbreviations_ambiguous_words_left_alone_without_context() {
        assert_eq!(n("He sat down"), "He sat down");
        assert_eq!(n("The Sun is bright"), "The Sun is bright");
        assert_eq!(n("Monday"), "Monday");
    }

    #[test]
    fn month_abbreviations_only_inside_dates() {
        assert_eq!(n("Sept 22"), "September twenty-second");
        assert_eq!(n("Sep. 22nd"), "September twenty-second");
        assert_eq!(n("Dec 3, 2026"), "December third, twenty twenty-six");
        assert_eq!(n("in Sept 2026"), "in September twenty twenty-six");
        assert_eq!(
            n("Sep 22-24"),
            "September twenty-second through twenty-fourth"
        );
        assert_eq!(n("I spoke with Jan."), "I spoke with Jan.");
        assert_eq!(n("mar 5"), "mar 5");
    }

    // ───────────── dates ─────────────

    #[test]
    fn dates_month_name() {
        assert_eq!(n("September 22"), "September twenty-second");
        assert_eq!(
            n("Tuesday, September 22, 2026"),
            "Tuesday, September twenty-second, twenty twenty-six"
        );
        assert_eq!(n("on 22 September"), "on the twenty-second of September");
        assert_eq!(n("the 22nd of September"), "the twenty-second of September");
        assert_eq!(n("in September 2026"), "in September twenty twenty-six");
    }

    #[test]
    fn dates_numeric() {
        assert_eq!(n("9/22"), "September twenty-second");
        assert_eq!(
            n("09/22/2026"),
            "September twenty-second, twenty twenty-six"
        );
        assert_eq!(n("9/22/26"), "September twenty-second, twenty twenty-six");
        assert_eq!(n("22/9"), "September twenty-second");
        assert_eq!(n("1/2 cup"), "1/2 cup");
        assert_eq!(n("due 1/2"), "due January second");
    }

    #[test]
    fn dates_iso() {
        assert_eq!(
            n("2026-09-22"),
            "September twenty-second, twenty twenty-six"
        );
    }

    // ───────────── times ─────────────

    #[test]
    fn times_user_example_and_common_forms() {
        assert_eq!(n("7:03"), "seven oh three");
        assert_eq!(n("at 7:03 PM"), "at seven oh three p.m.");
        assert_eq!(n("7:03pm"), "seven oh three p.m.");
        assert_eq!(n("10:30"), "ten thirty");
        assert_eq!(n("12:15 a.m."), "twelve fifteen a.m.");
        assert_eq!(n("7:00"), "seven o'clock");
        assert_eq!(n("7:00 PM"), "seven p.m.");
        assert_eq!(n("19:00"), "seven p.m.");
        assert_eq!(n("00:05"), "twelve oh five a.m.");
        assert_eq!(n("7pm"), "7 p.m.");
        assert_eq!(n("7 AM"), "7 a.m.");
    }

    #[test]
    fn times_non_times_are_left_alone() {
        assert_eq!(n("7:03:45"), "7:03:45");
        assert_eq!(n("45:30"), "45:30");
        assert_eq!(n("a 3:1 ratio"), "a 3:1 ratio");
    }

    // ───────────── airline codes ─────────────

    #[test]
    fn airline_letters_spelled_digits_paired() {
        assert_eq!(
            n("You're on flight UA278."),
            "You're on flight U A two seventy-eight."
        );
        assert_eq!(n("DL1245"), "D L twelve forty-five");
        assert_eq!(n("AA-100 departs soon"), "A A one hundred departs soon");
    }

    #[test]
    fn airline_longer_ids_still_go_through_format_numbers_for_speech() {
        assert_eq!(
            n("Order ORD-94821 is ready"),
            "Order O R D nine four eight, two one is ready"
        );
        assert_eq!(n("code X7K2"), "code X seven K two");
        assert_eq!(
            n("Confirmation AB12CD34"),
            "Confirmation A B one two C D three four"
        );
    }

    // ───────────── common abbreviations ─────────────

    #[test]
    fn common_abbreviations_table_entries() {
        assert_eq!(n("approx. 5"), "approximately 5");
        assert_eq!(n("apples, pears, etc."), "apples, pears, et cetera.");
        assert_eq!(n("cats vs. dogs"), "cats versus dogs");
        assert_eq!(n("fruit, e.g. apples"), "fruit, for example, apples");
        assert_eq!(
            n("w/ cheese and w/o onions"),
            "with cheese and without onions"
        );
        assert_eq!(n("your appt is"), "your appointment is");
        assert_eq!(n("Warner Bros. called"), "Warner Brothers called");
        assert_eq!(n("reply ASAP"), "reply as soon as possible");
        assert_eq!(n("AT&T"), "AT and T");
        assert_eq!(n("Ste 200"), "Suite two hundred");
    }

    #[test]
    fn common_abbreviations_units_after_a_number() {
        assert_eq!(n("15 min"), "15 minutes");
        assert_eq!(n("1 hr"), "1 hour");
        assert_eq!(n("2 hrs."), "2 hours.");
        assert_eq!(n("1 ft"), "1 foot");
        assert_eq!(n("the min is"), "the min is");
    }

    // ───────────── pronunciation fixes and phone parity ─────────────

    #[test]
    fn pronunciation_brand_and_yep() {
        assert_eq!(n("Phonegentic says yep"), "Phone-Jentic says yes");
        assert_eq!(n("Yep!"), "Yes!");
    }

    #[test]
    fn phone_numbers_still_spelled() {
        assert_eq!(
            n("Call me at 1-800-221-1212 today."),
            "Call me at one, eight hundred, two two one, one two one two today."
        );
    }

    #[test]
    fn years_standing_alone_are_untouched() {
        assert_eq!(n("in 2025 we grew"), "in 2025 we grew");
    }

    #[test]
    fn idempotent_on_spoken_output() {
        let once = "Tuesday, September twenty-second at seven oh three p.m.";
        assert_eq!(n(once), once);
        // Every golden output above must survive a second pass unchanged.
        for s in [
            "three thirty-five Main Street. Then turn left.",
            "September twenty-second, twenty twenty-six",
            "Room number three thirty-five",
            "with cheese and without onions",
            "twelve forty-five West fifth Avenue.",
            "Call me at one, eight hundred, two two one, one two one two today.",
            "Order O R D nine four eight, two one is ready",
        ] {
            assert_eq!(n(s), s, "not idempotent: {s:?}");
        }
    }

    #[test]
    fn empty_input() {
        assert_eq!(n(""), "");
        assert_eq!(strip_markdown_for_speech(""), "");
    }

    // ───────────── strip_markdown_for_speech ─────────────

    #[test]
    fn markdown_bold_markers_removed_words_kept() {
        assert_eq!(
            strip_markdown_for_speech("so the street is **North of Riggs**, not \"North Rage.\""),
            "so the street is North of Riggs, not \"North Rage.\""
        );
        assert_eq!(
            strip_markdown_for_speech("it's *really* good, __truly__ and _yes_"),
            "it's really good, truly and yes"
        );
    }

    #[test]
    fn markdown_then_normalize() {
        let s = normalize_text_for_speech(&strip_markdown_for_speech("around **October 6th**"));
        assert_eq!(s, "around October sixth");
    }

    #[test]
    fn markdown_headings_bullets_backticks_links() {
        assert_eq!(
            strip_markdown_for_speech("# Heading\n- item one\n* item two\n  - nested"),
            "Heading\nitem one\nitem two\n  nested"
        );
        assert_eq!(
            strip_markdown_for_speech(
                "run `cargo test` then read [the docs](https://example.com/x)."
            ),
            "run cargo test then read the docs."
        );
        assert_eq!(
            strip_markdown_for_speech("![alt text](img.png) and [**bold link**](u)"),
            "alt text and bold link"
        );
    }

    #[test]
    fn markdown_leaves_apostrophes_hyphens_and_snake_case() {
        assert_eq!(
            strip_markdown_for_speech("it's a well-known snake_case name, -5 degrees"),
            "it's a well-known snake_case name, -5 degrees"
        );
        assert_eq!(strip_markdown_for_speech("2*3 is six"), "2*3 is six");
        assert_eq!(
            strip_markdown_for_speech("#hashtag stays"),
            "#hashtag stays"
        );
    }

    #[test]
    fn markdown_standalone_punctuation_markers_dropped() {
        assert_eq!(strip_markdown_for_speech("just * alone"), "just alone");
        assert_eq!(strip_markdown_for_speech("ends with *"), "ends with ");
        assert_eq!(strip_markdown_for_speech("a _ b"), "a b");
        assert_eq!(
            strip_markdown_for_speech("[not a link] here"),
            "[not a link] here"
        );
    }

    #[test]
    fn markdown_idempotent() {
        let once = strip_markdown_for_speech("**Bold** and `code` and [x](y)");
        assert_eq!(strip_markdown_for_speech(&once), once);
    }
}
