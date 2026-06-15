// P7: Natural language intent parser — fallback when no /command is used.
//
// Tries to match common phrases to bot commands. Used as a fallback
// when the user doesn't use a /command prefix.

/// A parsed intent from natural language input.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedIntent {
    pub command: String,
    pub args: Vec<String>,
    pub confidence: f32, // 0.0 - 1.0
}

/// Parse natural language text into a command intent.
///
/// Returns `None` if no pattern matches with sufficient confidence.
pub fn parse_intent(text: &str) -> Option<ParsedIntent> {
    let lower = text.trim().to_lowercase();

    // Deploy patterns
    if let Some(app) = extract_after(&lower, &["deploy ", "deploy the ", "deploy my ", "push ", "ship "]) {
        return Some(ParsedIntent {
            command: "deploy".into(),
            args: vec![app],
            confidence: 0.9,
        });
    }

    // Rollback patterns
    if let Some(app) = extract_after(&lower, &["rollback ", "rollback the ", "revert ", "undo "]) {
        return Some(ParsedIntent {
            command: "rollback".into(),
            args: vec![app],
            confidence: 0.85,
        });
    }

    // Backup patterns
    if let Some(app) = extract_after(&lower, &["backup ", "backup the ", "snapshot "]) {
        return Some(ParsedIntent {
            command: "backup".into(),
            args: vec![app],
            confidence: 0.8,
        });
    }

    // Status patterns
    if matches!(
        lower.as_str(),
        "status"
            | "show status"
            | "fleet status"
            | "how are things"
            | "how's it going"
            | "what's up"
            | "everything ok"
            | "everything okay"
            | "are we up"
            | "system status"
    ) {
        return Some(ParsedIntent {
            command: "status".into(),
            args: vec![],
            confidence: 0.7,
        });
    }

    // Apps patterns
    if matches!(
        lower.as_str(),
        "list apps"
            | "show apps"
            | "my apps"
            | "what apps"
            | "which apps"
            | "app list"
    ) {
        return Some(ParsedIntent {
            command: "apps".into(),
            args: vec![],
            confidence: 0.7,
        });
    }

    // Doctor patterns
    if matches!(
        lower.as_str(),
        "doctor"
            | "run doctor"
            | "health check"
            | "check health"
            | "diagnostics"
            | "check system"
    ) {
        return Some(ParsedIntent {
            command: "doctor".into(),
            args: vec![],
            confidence: 0.6,
        });
    }

    // Deployments patterns
    if let Some(app) = extract_after(&lower, &["deployments ", "deployment history ", "history "]) {
        return Some(ParsedIntent {
            command: "deployments".into(),
            args: vec![app],
            confidence: 0.75,
        });
    }

    // Help patterns
    if matches!(
        lower.as_str(),
        "help"
            | "commands"
            | "what can you do"
            | "menu"
            | "options"
            | "?"
    ) {
        return Some(ParsedIntent {
            command: "help".into(),
            args: vec![],
            confidence: 0.5,
        });
    }

    // Secret patterns
    if let Some(rest) = lower.strip_prefix("secret ") {
        if let Some(app) = extract_after(rest, &["list ", "keys "]) {
            return Some(ParsedIntent {
                command: "secret".into(),
                args: vec!["list".into(), app],
                confidence: 0.7,
            });
        }
    }

    None
}

/// Extract the text after one of the given prefixes.
fn extract_after(text: &str, prefixes: &[&str]) -> Option<String> {
    for prefix in prefixes {
        if let Some(rest) = text.strip_prefix(prefix) {
            let rest = rest.trim().to_string();
            if !rest.is_empty() {
                return Some(rest);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deploy_intent() {
        let intent = parse_intent("deploy myapp").unwrap();
        assert_eq!(intent.command, "deploy");
        assert_eq!(intent.args, vec!["myapp"]);
        assert!(intent.confidence > 0.8);
    }

    #[test]
    fn deploy_with_the() {
        let intent = parse_intent("deploy the api service").unwrap();
        assert_eq!(intent.command, "deploy");
        assert_eq!(intent.args, vec!["the api service"]);
    }

    #[test]
    fn rollback_intent() {
        let intent = parse_intent("rollback myapp").unwrap();
        assert_eq!(intent.command, "rollback");
        assert_eq!(intent.args, vec!["myapp"]);
    }

    #[test]
    fn revert_intent() {
        let intent = parse_intent("revert the api").unwrap();
        assert_eq!(intent.command, "rollback");
        assert_eq!(intent.args, vec!["the api"]);
    }

    #[test]
    fn backup_intent() {
        let intent = parse_intent("backup myapp").unwrap();
        assert_eq!(intent.command, "backup");
        assert_eq!(intent.args, vec!["myapp"]);
    }

    #[test]
    fn status_intent_variants() {
        for input in &["status", "show status", "how are things", "everything ok", "are we up"] {
            let intent = parse_intent(input).unwrap();
            assert_eq!(intent.command, "status", "failed for: {input}");
        }
    }

    #[test]
    fn apps_intent() {
        let intent = parse_intent("list apps").unwrap();
        assert_eq!(intent.command, "apps");
    }

    #[test]
    fn doctor_intent() {
        let intent = parse_intent("health check").unwrap();
        assert_eq!(intent.command, "doctor");
    }

    #[test]
    fn help_intent() {
        let intent = parse_intent("what can you do").unwrap();
        assert_eq!(intent.command, "help");
    }

    #[test]
    fn deployments_intent() {
        let intent = parse_intent("deployments myapp").unwrap();
        assert_eq!(intent.command, "deployments");
        assert_eq!(intent.args, vec!["myapp"]);
    }

    #[test]
    fn secret_list_intent() {
        let intent = parse_intent("secret list myapp").unwrap();
        assert_eq!(intent.command, "secret");
        assert_eq!(intent.args, vec!["list", "myapp"]);
    }

    #[test]
    fn gibberish_returns_none() {
        assert!(parse_intent("asdfghjkl").is_none());
        assert!(parse_intent("the quick brown fox").is_none());
    }

    #[test]
    fn case_insensitive() {
        let intent = parse_intent("DEPLOY MyApp").unwrap();
        assert_eq!(intent.command, "deploy");
        assert_eq!(intent.args, vec!["myapp"]);
    }

    #[test]
    fn ship_is_deploy_synonym() {
        let intent = parse_intent("ship myapp").unwrap();
        assert_eq!(intent.command, "deploy");
    }

    #[test]
    fn snapshot_is_backup_synonym() {
        let intent = parse_intent("snapshot myapp").unwrap();
        assert_eq!(intent.command, "backup");
    }
}
