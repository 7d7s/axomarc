//! String utilities (blueprint L53, S3 item 43).
//!
//! Case conversions shared across codegen, routing, and storage layers:
//! snake_case, kebab-case, camelCase, StudlyCase (Pascal), URL slugs. All
//! conversions are pure and allocation-light; no unicode transliteration —
//! non-alphanumeric characters act as separators.

/// Split an identifier into word stems on `_`, `-`, `.`, whitespace, and
/// camel boundaries (`HTTPServer` -> [`HTTP`, `Server`]).
fn split_words(raw: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = raw.chars().peekable();
    let mut prev: Option<char> = None;
    while let Some(c) = chars.next() {
        if c == '_' || c == '-' || c == '.' || c.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            prev = None;
            continue;
        }
        let boundary = match prev {
            Some(p) if p.is_lowercase() || p.is_ascii_digit() => c.is_uppercase(),
            // Inside an acronym run, split before an uppercase letter that
            // starts a new word (next char lowercase): HTTP|Server.
            Some(p) if p.is_uppercase() => {
                c.is_uppercase() && chars.peek().map(|n| n.is_lowercase()).unwrap_or(false)
            }
            _ => false,
        };
        if boundary && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        current.push(c);
        prev = Some(c);
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// `HelloWorld` / `hello world` / `hello.world` -> `hello_world`.
pub fn to_snake_case(raw: &str) -> String {
    let words = split_words(raw);
    let mut out = String::new();
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            out.push('_');
        }
        out.extend(word.chars().flat_map(|c| c.to_lowercase()));
    }
    out
}

/// `HelloWorld` / `hello_world` -> `hello-world`.
pub fn to_kebab_case(raw: &str) -> String {
    to_snake_case(raw).replace('_', "-")
}

/// `hello_world` / `HelloWorld` -> `helloWorld`.
pub fn to_camel_case(raw: &str) -> String {
    let words = split_words(raw);
    let mut out = String::new();
    for (index, word) in words.iter().enumerate() {
        if index == 0 {
            out.extend(word.chars().flat_map(|c| c.to_lowercase()));
        } else {
            out.push_str(&capitalize(word));
        }
    }
    out
}

/// `hello_world` / `helloWorld` -> `HelloWorld` (PascalCase).
pub fn to_studly_case(raw: &str) -> String {
    split_words(raw)
        .iter()
        .map(|w| capitalize(w).as_str().to_string())
        .collect()
}

/// URL slug: lowercase alphanumerics joined by single hyphens; leading and
/// trailing separators trimmed (`Book Appointment!` -> `book-appointment`).
pub fn to_slug(raw: &str) -> String {
    let mut out = String::new();
    let mut pending_separator = false;
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_separator && !out.is_empty() {
                out.push('-');
            }
            pending_separator = false;
            out.extend(c.to_lowercase());
        } else {
            pending_separator = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Title case (Wave 8, docs/19 "Helpers"): every word stem capitalized,
/// identifiers included — `hello_world` / `helloWorld` -> `Hello World`.
/// Documented divergence from Laravel `Str::title` (whose `ucwords` keeps
/// `_` inside a word): Kelson normalizes every separator to a single space,
/// because snake_case identifiers ARE words here.
pub fn to_title_case(raw: &str) -> String {
    split_words(raw)
        .iter()
        .map(|w| capitalize(w).as_str().to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// First `limit` words, truncated with `end` when more followed
/// (`words("one two three", 2, "...")` -> `one two...`; at or under the
/// limit the input is returned untouched — no dangling marker).
pub fn words(raw: &str, limit: usize, end: &str) -> String {
    let trimmed = raw.trim();
    let mut taken: Vec<&str> = Vec::new();
    for (index, word) in trimmed.split_whitespace().enumerate() {
        if index == limit {
            return format!("{}{end}", taken.join(" "));
        }
        taken.push(word);
    }
    trimmed.to_string()
}

/// Context snippet (Wave 8, docs/19 "Helpers"): the first case-sensitive
/// occurrence of `search` with up to `before` chars of context ahead and
/// `after` behind, the whole window capped at `limit` chars — the before-side
/// never evicts the match, the after-side yields first — and `...` marking
/// any cut side. A missing `search` falls back to the first `limit` chars.
/// Char-boundary safe throughout (documented subset: case-sensitive match,
/// unlike Laravel's locale-aware helpers).
pub fn excerpt(haystack: &str, search: &str, before: usize, after: usize, limit: usize) -> String {
    let total = haystack.chars().count();
    let Some(byte_pos) = haystack.find(search) else {
        let head: String = haystack.chars().take(limit).collect();
        return if total > limit {
            format!("{head}...")
        } else {
            haystack.to_string()
        };
    };
    let match_chars = search.chars().count();
    let match_start = haystack[..byte_pos].chars().count();
    let match_end = match_start + match_chars;
    // The before-side gets at most limit - match_chars (so the match always
    // fits inside the limit window); with limit < match the match head wins.
    let before_budget = limit.saturating_sub(match_chars).min(before);
    let start = match_start.saturating_sub(before_budget);
    let end = (match_end + after).min(total).min(start + limit);
    let chars: Vec<char> = haystack.chars().collect();
    let body: String = chars[start..end].iter().collect();
    let prefix = if start > 0 { "..." } else { "" };
    let suffix = if end < total { "..." } else { "" };
    format!("{prefix}{body}{suffix}")
}

/// CSPRNG alphanumeric token of exactly `len` chars over
/// `[a-zA-Z0-9]` (Laravel `Str::random` parity) — `rand`'s thread-local
/// CSPRNG, the same source the session/token layers draw from.
pub fn random(len: usize) -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| {
            let idx = rng.gen_range(0..62);
            let table = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
            table[idx] as char
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_from_all_shapes() {
        assert_eq!(to_snake_case("HelloWorld"), "hello_world");
        assert_eq!(to_snake_case("helloWorld"), "hello_world");
        assert_eq!(to_snake_case("hello-world"), "hello_world");
        assert_eq!(to_snake_case("hello.world"), "hello_world");
        assert_eq!(to_snake_case("hello world"), "hello_world");
        assert_eq!(to_snake_case("HTTPServer"), "http_server");
        assert_eq!(to_snake_case("XMLHttpRequest"), "xml_http_request");
        assert_eq!(to_snake_case("user2Factor"), "user2_factor");
        assert_eq!(to_snake_case("already_snake"), "already_snake");
        assert_eq!(to_snake_case(""), "");
    }

    #[test]
    fn kebab_and_camel_and_studly() {
        assert_eq!(to_kebab_case("HelloWorld"), "hello-world");
        assert_eq!(to_kebab_case("hello_world"), "hello-world");
        assert_eq!(to_camel_case("hello_world"), "helloWorld");
        assert_eq!(to_camel_case("HelloWorld"), "helloWorld");
        assert_eq!(to_camel_case("hello-world"), "helloWorld");
        assert_eq!(to_studly_case("hello_world"), "HelloWorld");
        assert_eq!(to_studly_case("hello world"), "HelloWorld");
        assert_eq!(to_studly_case("helloWorld"), "HelloWorld");
    }

    #[test]
    fn acronym_splitting() {
        assert_eq!(
            split_words("HTTPServer"),
            vec!["HTTP".to_string(), "Server".to_string()]
        );
        assert_eq!(split_words("HTTPS"), vec!["HTTPS".to_string()]);
        assert_eq!(
            split_words("getHTTPResponse"),
            vec!["get", "HTTP", "Response"]
        );
    }

    #[test]
    fn slug_rules() {
        assert_eq!(to_slug("Book Appointment!"), "book-appointment");
        assert_eq!(to_slug("  --Hello   World--  "), "hello-world");
        assert_eq!(to_slug("UTF-8 safe?"), "utf-8-safe");
        assert_eq!(to_slug("!!!"), "");
        assert_eq!(to_slug(""), "");
    }

    #[test]
    fn conversions_are_idempotent() {
        assert_eq!(to_snake_case(&to_snake_case("HelloWorld")), "hello_world");
        assert_eq!(to_camel_case(&to_camel_case("hello_world")), "helloWorld");
        assert_eq!(to_studly_case(&to_studly_case("hello world")), "HelloWorld");
        assert_eq!(to_slug(&to_slug("Book Appointment!")), "book-appointment");
    }

    #[test]
    fn title_case_capitalizes_every_word_stem() {
        assert_eq!(to_title_case("hello world"), "Hello World");
        assert_eq!(to_title_case("hello_world"), "Hello World");
        assert_eq!(to_title_case("hello-world"), "Hello World");
        assert_eq!(to_title_case("HTTPServer"), "Http Server");
        assert_eq!(to_title_case("  spaced   out  "), "Spaced Out");
        assert_eq!(to_title_case(""), "");
    }

    #[test]
    fn words_truncates_at_the_limit_with_the_end_marker() {
        assert_eq!(words("one two three", 2, "..."), "one two...");
        assert_eq!(words("one two", 2, "..."), "one two");
        assert_eq!(words("only", 5, "..."), "only");
        assert_eq!(words(" one two three ", 1, "…"), "one…");
        assert_eq!(words("one two three", 0, "..."), "...");
        assert_eq!(words("", 2, "..."), "");
    }

    #[test]
    fn excerpt_windows_around_the_match() {
        // Match plus context, ellipses on the cut sides.
        assert_eq!(
            excerpt("the quick brown fox jumps", "brown", 6, 4, 100),
            "...quick brown fox..."
        );
        // Uncut on both sides: no markers.
        assert_eq!(excerpt("brown fox", "brown", 0, 4, 100), "brown fox");
        // The before-side never evicts the match; the after-side yields first.
        // (The limit caps the window; the `...` markers sit outside it.)
        let tight = excerpt("the quick brown fox", "brown", 50, 50, 8);
        assert!(tight.contains("brown"), "{tight}");
        assert!(tight.chars().count() <= 8 + 6, "{tight}");
        // Missing search: first `limit` chars plus an ellipsis.
        assert_eq!(excerpt("hello world", "nope", 0, 0, 5), "hello...");
        assert_eq!(excerpt("hi", "nope", 0, 0, 5), "hi");
        // Char-boundary safe on multibyte input.
        assert!(excerpt("héllo wörld", "wörld", 2, 2, 100).contains("wörld"));
    }

    #[test]
    fn random_draws_exact_length_alphanumeric_tokens() {
        assert_eq!(random(0), "");
        let token = random(24);
        assert_eq!(token.len(), 24);
        assert!(token.chars().all(|c| c.is_ascii_alphanumeric()), "{token}");
        assert_ne!(random(32), random(32));
    }
}
