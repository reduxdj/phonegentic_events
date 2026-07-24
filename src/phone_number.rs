//! Spoken / formatted phone-number parsing.
//!
//! Converts transcript fragments and vanity strings into E.164. Shared by the
//! server agent (`parse_phone_number` tool) and mirrored in the Dart app
//! (`phone_numbers.dart`). Golden cases live in
//! `test-vectors/spoken_phone.json`.

use serde_json::{json, Value};

/// Result of parsing spoken or formatted phone-number text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPhone {
    /// Best-effort E.164 (`+` + digits). Empty when no digits were recovered.
    pub e164: String,
    /// Digits only (no `+`).
    pub digits: String,
    pub digit_count: usize,
    /// True when the digit count matches national length for [default_cc]
    /// (with or without the country-code prefix).
    pub complete: bool,
    /// Compact display form for read-back (e.g. `1-800-221-1212`).
    pub display: String,
}

impl ParsedPhone {
    /// JSON tool / API payload the model can read.
    pub fn to_json(&self) -> Value {
        json!({
            "e164": self.e164,
            "digits": self.digits,
            "digit_count": self.digit_count,
            "complete": self.complete,
            "display": self.display,
        })
    }
}

/// National significant-number length by country calling code.
fn national_len(cc: &str) -> usize {
    match cc {
        "1" => 10,   // US/CA NANP
        "44" => 10,  // GB
        "33" => 9,   // FR
        "49" => 11,  // DE
        "61" => 9,   // AU
        "81" => 10,  // JP
        "86" => 11,  // CN
        "91" => 10,  // IN
        "52" => 10,  // MX
        "55" => 11,  // BR
        "82" => 10,  // KR
        "7" => 10,   // RU/KZ
        "39" => 10,  // IT
        "34" => 9,   // ES
        "65" => 8,   // SG
        "27" => 9,   // ZA
        _ => 10,
    }
}

/// Parse spoken words, digit runs, and vanity letters into E.164.
///
/// Handles common speech patterns:
/// - Digit words: "oh"/"zero"→0 … "nine"→9; homophones to/too→2, for→4, ate→8
/// - Multipliers: "double five"→55, "triple oh"→000
/// - "hundred"→00 (so "eight hundred"→800)
/// - Teens / tens: "twelve"→12, "twenty"→20 (useful for "twelve twelve"→1212)
/// - Formatted numerals: "1-800-221-1212"
/// - Vanity keypad letters: A/B/C→2 … W/X/Y/Z→9
///
/// `default_cc` is the dialing prefix to assume for bare national numbers
/// (e.g. `"1"` for US/CA).
pub fn parse_spoken_phone(text: &str, default_cc: &str) -> ParsedPhone {
    let cc = {
        let d: String = default_cc.chars().filter(|c| c.is_ascii_digit()).collect();
        if d.is_empty() { "1".to_string() } else { d }
    };
    let digits = spoken_to_digits(text);
    let nlen = national_len(&cc);
    let complete = digits.len() == nlen
        || (digits.len() == cc.len() + nlen && digits.starts_with(&cc));
    let e164 = if digits.is_empty() {
        String::new()
    } else if digits.len() == nlen {
        format!("+{cc}{digits}")
    } else if digits.len() == cc.len() + nlen && digits.starts_with(&cc) {
        format!("+{digits}")
    } else if digits.starts_with(&cc) && digits.len() > cc.len() {
        // Already has a country-code prefix (complete or partial) — don't double it.
        format!("+{digits}")
    } else if digits.len() > nlen {
        // Likely international without matching our default CC.
        format!("+{digits}")
    } else {
        // Short national fragment — best-effort with default CC.
        format!("+{cc}{digits}")
    };
    let display = format_display(&digits, &cc);
    ParsedPhone {
        e164,
        digit_count: digits.len(),
        digits,
        complete,
        display,
    }
}

fn format_display(digits: &str, cc: &str) -> String {
    if digits.is_empty() {
        return String::new();
    }
    // NANP pretty-print when it fits.
    if cc == "1" {
        let d = if digits.len() == 11 && digits.starts_with('1') {
            &digits[1..]
        } else if digits.len() == 10 {
            digits
        } else {
            return digits.to_string();
        };
        return format!(
            "1-{}-{}-{}",
            &d[0..3],
            &d[3..6],
            &d[6..10]
        );
    }
    digits.to_string()
}

fn spoken_to_digits(text: &str) -> String {
    let lower = text.to_lowercase();
    // Normalize separators to spaces; keep letters/digits.
    let mut cleaned = String::with_capacity(lower.len());
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            cleaned.push(c);
        } else if c == '+' {
            // Leading + is country-code signal; skip the glyph, digits follow.
            cleaned.push(' ');
        } else {
            cleaned.push(' ');
        }
    }
    let tokens: Vec<&str> = cleaned.split_whitespace().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i];
        if is_filler(tok) {
            i += 1;
            continue;
        }
        // Multipliers bind to the next token.
        if let Some(reps) = multiplier(tok) {
            if let Some(next) = tokens.get(i + 1) {
                if let Some(piece) = token_to_digits(next) {
                    for _ in 0..reps {
                        out.push_str(&piece);
                    }
                    i += 2;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        if tok == "hundred" {
            out.push_str("00");
            i += 1;
            continue;
        }
        if tok == "thousand" {
            out.push_str("000");
            i += 1;
            continue;
        }
        if let Some(piece) = token_to_digits(tok) {
            out.push_str(&piece);
            i += 1;
            continue;
        }
        // Unknown token — skip.
        i += 1;
    }
    out
}

fn is_filler(tok: &str) -> bool {
    matches!(
        tok,
        "a" | "an" | "the" | "area" | "code" | "number" | "is" | "its" | "it's"
            | "my" | "please" | "dial" | "call" | "phone" | "extension" | "ext"
            | "um" | "uh" | "like" | "so" | "then" | "and" | "at" | "on" | "into"
            | "conference" | "in" | "me" | "him" | "her" | "them" | "us"
            | "this" | "that" | "with" | "from" | "of" | "to" // "to" as prep — see note
    )
    // Note: homophone "to"/"too"→2 is handled via explicit digit words below
    // when NOT classified as filler. We treat bare "to" as filler (prep) so
    // "call to 555…" doesn't inject a spurious 2. Use "two"/"too" for the digit.
}

fn multiplier(tok: &str) -> Option<usize> {
    match tok {
        "double" | "twice" => Some(2),
        "triple" | "thrice" => Some(3),
        "quadruple" => Some(4),
        _ => None,
    }
}

fn token_to_digits(tok: &str) -> Option<String> {
    // Pure digit run (possibly multi-digit like "800" or "1212").
    if tok.chars().all(|c| c.is_ascii_digit()) {
        return Some(tok.to_string());
    }
    // Prefer digit-words over vanity for single letters ("o"→0, not keypad 6).
    if let Some(word) = digit_word(tok) {
        return Some(word);
    }
    // Single vanity letter.
    if tok.len() == 1 {
        let c = tok.chars().next().unwrap();
        if let Some(d) = letter_to_digit(c) {
            return Some(d.to_string());
        }
    }
    // Multi-letter vanity word (FLOWERS, etc.) — only if ALL chars map.
    if tok.len() >= 2 && tok.chars().all(|c| c.is_ascii_alphabetic()) {
        let mut buf = String::new();
        let mut ok = true;
        for c in tok.chars() {
            match letter_to_digit(c) {
                Some(d) => buf.push(d),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if ok && !buf.is_empty() {
            return Some(buf);
        }
    }
    None
}

fn digit_word(tok: &str) -> Option<String> {
    let s = match tok {
        "zero" | "oh" | "o" | "nought" | "naught" => "0",
        "one" | "won" => "1",
        "two" | "too" | "tu" => "2",
        "three" | "tree" => "3",
        "four" | "for" | "fore" => "4",
        "five" | "fife" => "5",
        "six" | "sicks" => "6",
        "seven" => "7",
        "eight" | "ate" => "8",
        "nine" | "niner" => "9",
        "ten" => "10",
        "eleven" => "11",
        "twelve" => "12",
        "thirteen" => "13",
        "fourteen" => "14",
        "fifteen" => "15",
        "sixteen" => "16",
        "seventeen" => "17",
        "eighteen" => "18",
        "nineteen" => "19",
        "twenty" => "20",
        "thirty" => "30",
        "forty" | "fourty" => "40",
        "fifty" => "50",
        "sixty" => "60",
        "seventy" => "70",
        "eighty" => "80",
        "ninety" => "90",
        _ => return None,
    };
    Some(s.to_string())
}

fn letter_to_digit(c: char) -> Option<char> {
    match c.to_ascii_uppercase() {
        'A' | 'B' | 'C' => Some('2'),
        'D' | 'E' | 'F' => Some('3'),
        'G' | 'H' | 'I' => Some('4'),
        'J' | 'K' | 'L' => Some('5'),
        'M' | 'N' | 'O' => Some('6'),
        'P' | 'Q' | 'R' | 'S' => Some('7'),
        'T' | 'U' | 'V' => Some('8'),
        'W' | 'X' | 'Y' | 'Z' => Some('9'),
        _ => None,
    }
}

/// Normalize a dial target that may be spoken words or formatted digits to E.164.
/// Returns the original trimmed string when no digits can be recovered.
pub fn resolve_dialable(raw: &str, default_cc: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return String::new();
    }
    // Fast path: already digit-formatted (optional + and punctuation).
    let digit_like = t
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '(' | ')' | '.' | ' '));
    if digit_like {
        let parsed = parse_spoken_phone(t, default_cc);
        return if parsed.e164.is_empty() { t.to_string() } else { parsed.e164 };
    }
    let parsed = parse_spoken_phone(t, default_cc);
    if parsed.digits.is_empty() {
        t.to_string()
    } else {
        parsed.e164
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn classic_toll_free_formatted() {
        let p = parse_spoken_phone("1-800-221-1212", "1");
        assert_eq!(p.e164, "+18002211212");
        assert!(p.complete);
        assert_eq!(p.display, "1-800-221-1212");
    }

    #[test]
    fn classic_toll_free_spoken() {
        let p = parse_spoken_phone(
            "one eight hundred two two one one two one two",
            "1",
        );
        assert_eq!(p.e164, "+18002211212");
        assert!(p.complete);
    }

    #[test]
    fn eight_hundred_word() {
        // "eight hundred" → 800 (hundred expands to 00 after the leading 8).
        let p = parse_spoken_phone("one eight hundred two two one twelve twelve", "1");
        assert_eq!(p.e164, "+18002211212");
        assert!(p.complete);
    }

    #[test]
    fn hundred_appends_zeros_on_digit_run() {
        // Mixed: "1-800-hundred" → 1 + 800 + 00 = 180000 (incomplete without the rest).
        let p = parse_spoken_phone("1-800-hundred", "1");
        assert_eq!(p.digits, "180000");
        assert!(!p.complete);
    }

    #[test]
    fn double_triple_oh() {
        let p = parse_spoken_phone("five five five double oh one two three four", "1");
        assert_eq!(p.digits, "555001234");
        // 9 digits — incomplete NANP
        assert!(!p.complete);
    }

    #[test]
    fn triple_five() {
        let p = parse_spoken_phone("area code triple five one two three four five six seven", "1");
        assert_eq!(p.e164, "+15551234567");
        assert!(p.complete);
    }

    #[test]
    fn oh_and_homophones() {
        let p = parse_spoken_phone("five one oh four for two too eight ate", "1");
        assert_eq!(p.digits, "510442288");
    }

    #[test]
    fn vanity_letters() {
        let p = parse_spoken_phone("1-800-FLOWERS", "1");
        assert_eq!(p.digits, "18003569377");
        assert_eq!(p.e164, "+18003569377");
        assert!(p.complete);
    }

    #[test]
    fn bare_ten_digit() {
        let p = parse_spoken_phone("503-447-2755", "1");
        assert_eq!(p.e164, "+15034472755");
        assert!(p.complete);
    }

    #[test]
    fn resolve_dialable_spoken() {
        assert_eq!(
            resolve_dialable("one eight hundred two two one one two one two", "1"),
            "+18002211212"
        );
        assert_eq!(resolve_dialable("1-800-221-1212", "1"), "+18002211212");
    }

    #[test]
    fn golden_vector_file() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/test-vectors/spoken_phone.json");
        let raw = fs::read_to_string(path).expect("spoken_phone.json");
        let cases: Value = serde_json::from_str(&raw).unwrap();
        for c in cases.as_array().unwrap() {
            let input = c["input"].as_str().unwrap();
            let cc = c["default_cc"].as_str().unwrap_or("1");
            let expect = c["e164"].as_str().unwrap();
            let p = parse_spoken_phone(input, cc);
            assert_eq!(p.e164, expect, "input={input:?}");
            if let Some(complete) = c.get("complete").and_then(|v| v.as_bool()) {
                assert_eq!(p.complete, complete, "complete for {input:?}");
            }
        }
    }
}
