//! Format numbers / IDs in text for human-like TTS.
//!
//! The reverse of [`crate::phone_number::parse_spoken_phone`]: take prose that
//! contains phones, flight numbers, order IDs, confirmation codes, etc. and
//! rewrite the numeric bits so TTS says them the way a person would —
//! digit-by-digit with "oh", "eight hundred" for toll-free 800, spelled
//! letters for airline codes — instead of giant cardinals.
//!
//! Mirrored in the Dart app (`formatNumbersForSpeech`). Golden cases:
//! `test-vectors/speech_numbers.json`.

/// Rewrite phones, digit runs, and alphanumeric IDs in [text] for spoken TTS.
pub fn format_numbers_for_speech(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() * 2);
    let mut i = 0;
    while i < chars.len() {
        // 1. Alphanumeric ID / flight / order token (letters + digits).
        if let Some((end, spoken)) = match_alnum_id(&chars, i) {
            push_spoken(&mut out, &spoken);
            i = end;
            continue;
        }
        // 2. Phone-like or long digit run (optional leading + / parens).
        if let Some((end, spoken)) = match_digit_run(&chars, i) {
            push_spoken(&mut out, &spoken);
            i = end;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn push_spoken(out: &mut String, spoken: &str) {
    // Keep surrounding whitespace natural: if we glued onto a word, insert a
    // space; if the previous char is already whitespace/punctuation, fine.
    if let Some(prev) = out.chars().last() {
        if prev.is_ascii_alphanumeric() {
            out.push(' ');
        }
    }
    out.push_str(spoken);
}

const DIGIT_WORDS: [&str; 10] = [
    "oh", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
];

fn digit_word(d: u32) -> &'static str {
    DIGIT_WORDS[d as usize]
}

fn speak_digits(digits: &[u32]) -> String {
    digits
        .iter()
        .map(|&d| digit_word(d).to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Speak a digit group with human flourishes:
/// - all-same (≥2): "triple five" / "double oh"
/// - N00 (100–900): "one hundred", "eight hundred"
/// - otherwise digit-by-digit with "oh"
fn speak_group(g: &[u32]) -> String {
    if g.is_empty() {
        return String::new();
    }
    if g.len() >= 2 && g.iter().all(|&d| d == g[0]) {
        let word = digit_word(g[0]);
        return match g.len() {
            2 => format!("double {word}"),
            3 => format!("triple {word}"),
            4 => format!("quadruple {word}"),
            _ => speak_digits(g),
        };
    }
    if g.len() == 3 && g[1] == 0 && g[2] == 0 && g[0] != 0 {
        return format!("{} hundred", digit_word(g[0]));
    }
    speak_digits(g)
}

fn speak_digit_buffer(buf: &[u32]) -> String {
    if buf.is_empty() {
        return String::new();
    }
    if buf.len() <= 4 {
        return speak_group(buf);
    }
    group_phone_digits(buf)
        .iter()
        .map(|g| speak_group(g))
        .collect::<Vec<_>>()
        .join(", ")
}

fn group_phone_digits(d: &[u32]) -> Vec<Vec<u32>> {
    let seg = |a: usize, b: usize| d[a..b].to_vec();
    match d.len() {
        11 if d[0] == 1 => vec![vec![1], seg(1, 4), seg(4, 7), seg(7, 11)],
        10 => vec![seg(0, 3), seg(3, 6), seg(6, 10)],
        7 => vec![seg(0, 3), seg(3, 7)],
        _ => {
            let mut groups: Vec<Vec<u32>> = d.chunks(3).map(|g| g.to_vec()).collect();
            if groups.len() >= 2 && groups.last().map(|g| g.len()) == Some(1) {
                let last = groups.pop().unwrap();
                groups.last_mut().unwrap().extend(last);
            }
            groups
        }
    }
}

fn speak_phone(digits: &[u32]) -> String {
    group_phone_digits(digits)
        .iter()
        .map(|g| speak_group(g))
        .collect::<Vec<_>>()
        .join(", ")
}

fn is_year(digits: &[u32]) -> bool {
    if digits.len() != 4 {
        return false;
    }
    let n = digits[0] * 1000 + digits[1] * 100 + digits[2] * 10 + digits[3];
    (1900..=2099).contains(&n)
}

fn is_phone_sep(c: char) -> bool {
    matches!(c, '+' | '(' | ')' | '-' | '.' | ' ')
}

/// Match a phone-like / long digit run starting at `i`.
/// Returns (end_index, spoken).
fn match_digit_run(chars: &[char], i: usize) -> Option<(usize, String)> {
    let start_ok = chars[i].is_ascii_digit()
        || (matches!(chars[i], '+' | '(') && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit()));
    if !start_ok {
        return None;
    }
    let mut j = i;
    while j < chars.len() && (chars[j].is_ascii_digit() || is_phone_sep(chars[j])) {
        j += 1;
    }
    // Don't swallow a trailing letter into a digit run (that belongs to alnum).
    // Trim trailing separators.
    let mut end = j;
    while end > i && is_phone_sep(chars[end - 1]) && chars[end - 1] != '+' {
        end -= 1;
    }
    let digits: Vec<u32> = chars[i..end]
        .iter()
        .filter_map(|c| c.to_digit(10))
        .collect();
    if digits.is_empty() {
        return None;
    }
    // Phone-length: always spell.
    if digits.len() >= 7 {
        return Some((end, speak_phone(&digits)));
    }
    // Years stay as cardinals ("in 2025").
    if is_year(&digits) {
        return None;
    }
    // 4–6 digit codes / PINs / last-fours: digit-by-digit.
    // Require the run to be "code-like": either has separators (5-5-5-1),
    // or is a contiguous ≥4 digit token not adjacent to more letters
    // (those would have been caught as alnum).
    if digits.len() >= 4 {
        let had_sep = chars[i..end].iter().any(|&c| is_phone_sep(c) && c != '+');
        let contiguous = chars[i..end].iter().all(|c| c.is_ascii_digit());
        if had_sep || contiguous {
            // Group as 3-3 / 2-2 / etc. for cadence on longer shorts.
            let spoken = if digits.len() >= 5 {
                group_phone_digits(&digits)
                    .iter()
                    .map(|g| speak_group(g))
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                speak_digits(&digits)
            };
            return Some((end, spoken));
        }
    }
    None
}

/// Match an alphanumeric ID: flight (UA278), order (ORD-94821), conf code
/// (X7K2M9). Requires both a letter and a digit in the token.
fn match_alnum_id(chars: &[char], i: usize) -> Option<(usize, String)> {
    if !chars[i].is_ascii_alphanumeric() {
        return None;
    }
    // Must start with a letter for flight/order-style, OR be mixed later.
    // Scan a token of letters/digits/hyphens.
    let mut j = i;
    let mut has_letter = false;
    let mut has_digit = false;
    while j < chars.len() {
        let c = chars[j];
        if c.is_ascii_alphabetic() {
            has_letter = true;
            j += 1;
        } else if c.is_ascii_digit() {
            has_digit = true;
            j += 1;
        } else if c == '-' || c == '_' {
            // Hyphen only if something alphanumeric follows.
            if chars.get(j + 1).is_some_and(|n| n.is_ascii_alphanumeric()) {
                j += 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    if !has_letter || !has_digit {
        return None;
    }
    // Avoid rewriting ordinary words that happen to contain a digit mid-prose
    // only when the token is very word-like and long without enough digits.
    // Require: at least one digit AND (starts with letter OR is short code).
    let token: String = chars[i..j].iter().collect();
    let letter_count = token.chars().filter(|c| c.is_ascii_alphabetic()).count();
    let digit_count = token.chars().filter(|c| c.is_ascii_digit()).count();
    // Skip things like "i18n" / "h2o" style (few digits buried in letters) unless
    // digit-heavy (flight/order). Heuristic: digit_count >= 2 OR letter_count <= 3.
    if digit_count < 2 && letter_count > 3 {
        return None;
    }
    // Also skip if it looks like a normal English word with a trailing year —
    // handled separately. Here we're good.
    Some((j, speak_alnum_token(&token)))
}

fn speak_alnum_token(token: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut digit_buf: Vec<u32> = Vec::new();
    let flush_digits = |buf: &mut Vec<u32>, parts: &mut Vec<String>| {
        if buf.is_empty() {
            return;
        }
        parts.push(speak_digit_buffer(buf));
        buf.clear();
    };
    for c in token.chars() {
        if c.is_ascii_alphabetic() {
            flush_digits(&mut digit_buf, &mut parts);
            parts.push(c.to_ascii_uppercase().to_string());
        } else if let Some(d) = c.to_digit(10) {
            digit_buf.push(d);
        } else {
            // hyphen / underscore → flush; pause comes from spaces between parts
            flush_digits(&mut digit_buf, &mut parts);
        }
    }
    flush_digits(&mut digit_buf, &mut parts);
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn phone_toll_free_uses_eight_hundred() {
        let s = format_numbers_for_speech("Call me at 1-800-221-1212 today.");
        assert_eq!(
            s,
            "Call me at one, eight hundred, two two one, one two one two today."
        );
    }

    #[test]
    fn phone_e164() {
        assert_eq!(
            format_numbers_for_speech("dial +15034472755"),
            "dial one, five oh three, four four seven, two seven five five"
        );
    }

    #[test]
    fn phone_triple_area_code() {
        let s = format_numbers_for_speech("reach them at 555-121-1212");
        assert!(s.contains("triple five"), "{s}");
        assert!(s.contains("one two one"), "{s}");
    }

    #[test]
    fn leaves_years_and_small_counts() {
        assert_eq!(
            format_numbers_for_speech("I have 25 messages."),
            "I have 25 messages."
        );
        assert_eq!(
            format_numbers_for_speech("in 2025 we grew"),
            "in 2025 we grew"
        );
        assert_eq!(
            format_numbers_for_speech("press 1 for sales"),
            "press 1 for sales"
        );
    }

    #[test]
    fn flight_number() {
        assert_eq!(
            format_numbers_for_speech("You're on flight UA278."),
            "You're on flight U A two seven eight."
        );
        assert_eq!(
            format_numbers_for_speech("AA-100 departs soon"),
            "A A one hundred departs soon"
        );
    }

    #[test]
    fn order_and_confirmation() {
        assert_eq!(
            format_numbers_for_speech("Order ORD-94821 is ready"),
            "Order O R D nine four eight, two one is ready"
        );
        // 94821 → groups 3+2 = "nine four eight, two one"
        assert_eq!(
            format_numbers_for_speech("code X7K2"),
            "code X seven K two"
        );
    }

    #[test]
    fn short_pin_like() {
        assert_eq!(
            format_numbers_for_speech("your code is 4821"),
            "your code is four eight two one"
        );
    }

    #[test]
    fn golden_vector_file() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/test-vectors/speech_numbers.json"
        );
        let raw = fs::read_to_string(path).expect("speech_numbers.json");
        let cases: serde_json::Value = serde_json::from_str(&raw).unwrap();
        for c in cases.as_array().unwrap() {
            let input = c["input"].as_str().unwrap();
            let expect = c["speech"].as_str().unwrap();
            let got = format_numbers_for_speech(input);
            assert_eq!(got, expect, "input={input:?}");
        }
    }
}
