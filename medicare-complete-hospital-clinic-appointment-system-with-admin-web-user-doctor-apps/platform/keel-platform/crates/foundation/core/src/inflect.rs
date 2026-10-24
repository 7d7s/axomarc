//! English inflection subset (Wave 8, docs/19 "Helpers"): `plural` /
//! `singular` over regular rules plus curated irregular and uncountable
//! tables.
//!
//! This is a **documented subset**, not a full inflector (docs/16 records the
//! posture): regular rules cover the productive patterns (s/x/z/ch/sh -> es,
//! consonant+y -> ies, default +s and their reversals), the irregular table
//! carries the exceptions that break those rules (person/people,
//! knife/knives, potato/potatoes, matrix/matrices, quiz/quizzes...), and the
//! uncountable table is a no-op set (sheep, series, information). The tables
//! are the extension point for anything outside the subset.

/// (singular, plural) pairs the regular rules get wrong.
static IRREGULAR: &[(&str, &str)] = &[
    ("person", "people"),
    ("man", "men"),
    ("woman", "women"),
    ("child", "children"),
    ("foot", "feet"),
    ("tooth", "teeth"),
    ("mouse", "mice"),
    ("goose", "geese"),
    ("ox", "oxen"),
    ("datum", "data"),
    ("medium", "media"),
    ("criterion", "criteria"),
    ("phenomenon", "phenomena"),
    ("analysis", "analyses"),
    ("hypothesis", "hypotheses"),
    ("thesis", "theses"),
    ("crisis", "crises"),
    ("diagnosis", "diagnoses"),
    ("appendix", "appendices"),
    ("index", "indices"),
    ("matrix", "matrices"),
    ("vertex", "vertices"),
    ("radius", "radii"),
    ("focus", "foci"),
    ("fungus", "fungi"),
    ("nucleus", "nuclei"),
    ("syllabus", "syllabi"),
    ("alumnus", "alumni"),
    ("cactus", "cacti"),
    ("curriculum", "curricula"),
    ("memorandum", "memoranda"),
    ("bacterium", "bacteria"),
    ("knife", "knives"),
    ("wife", "wives"),
    ("life", "lives"),
    ("leaf", "leaves"),
    ("wolf", "wolves"),
    ("shelf", "shelves"),
    ("half", "halves"),
    ("thief", "thieves"),
    ("loaf", "loaves"),
    ("calf", "calves"),
    ("potato", "potatoes"),
    ("tomato", "tomatoes"),
    ("hero", "heroes"),
    ("echo", "echoes"),
    // s-final singulars whose +es plural cannot be stripped back by the
    // generic "strip one s" reversal (buses -> buse would be wrong).
    ("quiz", "quizzes"),
    ("bus", "buses"),
    ("gas", "gases"),
    ("atlas", "atlases"),
    ("lens", "lenses"),
];

/// Words that do not inflect (returned unchanged by both directions).
static UNCOUNTABLE: &[&str] = &[
    "equipment",
    "information",
    "money",
    "rice",
    "knowledge",
    "news",
    "series",
    "species",
    "fish",
    "sheep",
    "deer",
    "aircraft",
    "luggage",
    "furniture",
    "traffic",
    "weather",
    "advice",
    "research",
    "progress",
    "software",
    "hardware",
    "homework",
    "offspring",
    "salmon",
    "trout",
    "moose",
    "police",
    "staff",
];

/// Carry the sample's capitalization onto the produced form
/// (`Person` -> `People`; anything else stays as produced).
fn match_case(sample: &str, produced: &str) -> String {
    let mut chars = produced.chars();
    match (sample.chars().next(), chars.next()) {
        (Some(first), Some(pfirst)) if first.is_uppercase() && pfirst.is_lowercase() => {
            first.to_uppercase().collect::<String>() + chars.as_str()
        }
        _ => produced.to_string(),
    }
}

fn ends_with_vowel_y(word: &str) -> bool {
    let bytes = word.as_bytes();
    bytes.len() >= 2
        && bytes[bytes.len() - 1] == b'y'
        && matches!(bytes[bytes.len() - 2], b'a' | b'e' | b'i' | b'o' | b'u')
}

/// Plural form (documented subset). Order: uncountable -> already-plural
/// table column -> irregular singular column -> regular rules.
pub fn plural(word: &str) -> String {
    let lowered = word.to_lowercase();
    if UNCOUNTABLE.contains(&lowered.as_str()) {
        return word.to_string();
    }
    if IRREGULAR.iter().any(|(_, p)| *p == lowered) {
        return word.to_string();
    }
    if let Some((_, p)) = IRREGULAR.iter().find(|(s, _)| *s == lowered) {
        return match_case(word, p);
    }
    let regular = if ends_with_vowel_y(&lowered) {
        format!("{lowered}s")
    } else if lowered.ends_with('y') {
        format!("{}ies", &lowered[..lowered.len() - 1])
    } else if lowered.ends_with('s')
        || lowered.ends_with('x')
        || lowered.ends_with('z')
        || lowered.ends_with("ch")
        || lowered.ends_with("sh")
    {
        format!("{lowered}es")
    } else {
        format!("{lowered}s")
    };
    match_case(word, &regular)
}

/// Singular form (documented subset). Order: uncountable -> already-singular
/// table column -> irregular plural column -> regular reversals. The
/// reversals stop at `ss`/`us`/`is` stems so `class`, `bus` and `analysis`
/// are never mangled by the generic "strip one s".
pub fn singular(word: &str) -> String {
    let lowered = word.to_lowercase();
    if UNCOUNTABLE.contains(&lowered.as_str()) {
        return word.to_string();
    }
    if IRREGULAR.iter().any(|(s, _)| *s == lowered) {
        return word.to_string();
    }
    if let Some((s, _)) = IRREGULAR.iter().find(|(_, p)| *p == lowered) {
        return match_case(word, s);
    }
    let regular = if lowered.ends_with("ies") && lowered.len() > 4 {
        format!("{}y", &lowered[..lowered.len() - 3])
    } else if lowered.ends_with("sses")
        || lowered.ends_with("ches")
        || lowered.ends_with("shes")
        || lowered.ends_with("xes")
        || lowered.ends_with("zzes")
    {
        lowered[..lowered.len() - 2].to_string()
    } else if lowered.ends_with('s')
        && lowered.len() > 2
        && !lowered.ends_with("ss")
        && !lowered.ends_with("us")
        && !lowered.ends_with("is")
    {
        lowered[..lowered.len() - 1].to_string()
    } else {
        word.to_string()
    };
    match_case(word, &regular)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plural_follows_the_regular_rules() {
        assert_eq!(plural("cat"), "cats");
        assert_eq!(plural("city"), "cities");
        assert_eq!(plural("party"), "parties");
        assert_eq!(plural("boy"), "boys");
        assert_eq!(plural("key"), "keys");
        assert_eq!(plural("box"), "boxes");
        assert_eq!(plural("church"), "churches");
        assert_eq!(plural("dish"), "dishes");
        assert_eq!(plural("buzz"), "buzzes");
        assert_eq!(plural("tax"), "taxes");
        assert_eq!(plural("bus"), "buses");
        assert_eq!(plural("house"), "houses");
    }

    #[test]
    fn plural_reads_the_irregular_table() {
        assert_eq!(plural("person"), "people");
        assert_eq!(plural("child"), "children");
        assert_eq!(plural("mouse"), "mice");
        assert_eq!(plural("analysis"), "analyses");
        assert_eq!(plural("matrix"), "matrices");
        assert_eq!(plural("knife"), "knives");
        assert_eq!(plural("leaf"), "leaves");
        assert_eq!(plural("potato"), "potatoes");
        assert_eq!(plural("quiz"), "quizzes");
        assert_eq!(plural("focus"), "foci");
        assert_eq!(plural("criterion"), "criteria");
    }

    #[test]
    fn uncountables_never_inflect() {
        for word in ["sheep", "series", "fish", "information", "aircraft"] {
            assert_eq!(plural(word), word, "plural {word}");
            assert_eq!(singular(word), word, "singular {word}");
        }
    }

    #[test]
    fn capitalization_carries_onto_the_produced_form() {
        assert_eq!(plural("Person"), "People");
        assert_eq!(plural("Child"), "Children");
        assert_eq!(singular("People"), "Person");
        assert_eq!(singular("Cities"), "City");
        assert_eq!(plural("Cat"), "Cats");
    }

    #[test]
    fn singular_reverses_regular_and_table_forms() {
        assert_eq!(singular("cats"), "cat");
        assert_eq!(singular("cities"), "city");
        assert_eq!(singular("boxes"), "box");
        assert_eq!(singular("dishes"), "dish");
        assert_eq!(singular("churches"), "church");
        assert_eq!(singular("buzzes"), "buzz");
        assert_eq!(singular("houses"), "house");
        assert_eq!(singular("people"), "person");
        assert_eq!(singular("knives"), "knife");
        assert_eq!(singular("analyses"), "analysis");
        assert_eq!(singular("buses"), "bus");
        assert_eq!(singular("matrices"), "matrix");
    }

    #[test]
    fn round_trip_holds_over_the_documented_subset() {
        for (singular_form, plural_form) in IRREGULAR {
            assert_eq!(plural(singular_form), *plural_form, "{singular_form}");
            assert_eq!(singular(plural_form), *singular_form, "{plural_form}");
        }
        for pair in [
            ("cat", "cats"),
            ("city", "cities"),
            ("boy", "boys"),
            ("box", "boxes"),
            ("church", "churches"),
            ("house", "houses"),
            ("buzz", "buzzes"),
        ] {
            assert_eq!(singular(&plural(pair.0)), pair.0, "{}", pair.0);
            assert_eq!(plural(pair.0), pair.1, "{}", pair.0);
        }
    }

    #[test]
    fn table_forms_are_idempotent() {
        // Table-covered plurals and uncountables pass through unchanged;
        // regular-rule plurals are the documented subset (plural() assumes
        // a singular input, as Laravel's Str::plural does for bare nouns).
        assert_eq!(plural("people"), "people");
        assert_eq!(plural("children"), "children");
        assert_eq!(singular("cat"), "cat");
        assert_eq!(singular("analysis"), "analysis");
        assert_eq!(plural("sheep"), "sheep");
    }
}
