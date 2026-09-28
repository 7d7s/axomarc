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
}
