//! `claims.toml` — the E8 doc-claim gate (MVVC contract C24).
//!
//! A document may say a control is "Aligned" or a rule is enforced only when
//! the claim names the check that refuses the violation. The failure this
//! gate exists for is the one that stayed invisible for weeks: the compliance
//! document asserted "Aligned" while the audit had four open S1 rows, and a
//! reader had no way to tell which sentence was machine-backed.
//!
//! # What is checked
//!
//! 1. The register parses; ids are unique; every claim has a non-empty
//!    `clause` and `statement`.
//! 2. `status = "enforced"` names exactly one of `check` (a gate code, which
//!    must occur as a string literal in the workspace sources) or `test` (a
//!    test function name that exists). `status = "open"` must name neither
//!    and must say `reason`.
//! 3. Every `[claim:<id>]` reference in `docs/**/*.md` and `README.md`
//!    resolves to a registered claim.
//! 4. A line in the scanned compliance surface that contains an affirmative
//!    posture word ("Aligned") must reference at least one claim whose status
//!    is `enforced`.
//!
//! # The honest limit
//!
//! Check 2 proves the named check EXISTS in the tree, not that it is green.
//! Green is the CI run that executes this gate and the checks it names in the
//! same commit. The gate prints the enforced/open split on every run so a
//! registry that stayed quiet by marking everything "open" is visible.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const CLAIM_FILE: &str = "claims.toml";
pub const CODE: &str = "DOC-CLAIM-UNCHECKED";

/// The docs where an affirmative posture word is scanned. The list grows as
/// each document is baselined; the audit package itself is deliberately not
/// here — it *describes* the gaps and must keep saying so.
const CLAIM_DOCS: &[&str] = &["docs/20-SECURITY-COMPLIANCE.md"];

/// Posture words that assert a control holds.
const AFFIRMATIVE: &[&str] = &["Aligned"];

struct Claim {
    id: String,
    status: String,
    check: Option<String>,
    test: Option<String>,
}

pub struct Report {
    pub total: usize,
    pub enforced: usize,
    pub open: usize,
    pub references: usize,
    pub violations: Vec<String>,
}

impl Report {
    pub fn summary(&self) -> String {
        format!(
            "{} claim(s): {} enforced, {} open; {} doc reference(s) resolved; \
             every enforced check/test exists in the tree (green is the CI run itself)",
            self.total, self.enforced, self.open, self.references
        )
    }
}

pub fn check(root: &Path) -> Report {
    let mut report = Report {
        total: 0,
        enforced: 0,
        open: 0,
        references: 0,
        violations: Vec::new(),
    };
    let claims = match load(root, &mut report.violations) {
        Ok(claims) => claims,
        Err(e) => {
            report.violations.push(e);
            return report;
        }
    };
    report.total = claims.len();
    for claim in claims.values() {
        match claim.status.as_str() {
            "enforced" => report.enforced += 1,
            "open" => report.open += 1,
            _ => {}
        }
    }

    let source = workspace_sources(root);
    for claim in claims.values() {
        match claim.status.as_str() {
            "enforced" => {
                if claim.check.is_some() == claim.test.is_some() {
                    report.violations.push(format!(
                        "claim {}: enforced claims name exactly one of `check` or `test`",
                        claim.id
                    ));
                    continue;
                }
                if let Some(code) = &claim.check {
                    if !source.contains(&format!("\"{code}\"")) {
                        report.violations.push(format!(
                            "claim {}: the named check {code:?} is not emitted anywhere in the \
                             workspace sources",
                            claim.id
                        ));
                    }
                }
                if let Some(test) = &claim.test {
                    let needle = format!("fn {test}(");
                    if !source.contains(&needle) {
                        report.violations.push(format!(
                            "claim {}: the named test {test:?} does not exist",
                            claim.id
                        ));
                    }
                }
            }
            "open" => {
                if claim.check.is_some() || claim.test.is_some() {
                    report.violations.push(format!(
                        "claim {}: an open claim must not name a check or test",
                        claim.id
                    ));
                }
            }
            other => report.violations.push(format!(
                "claim {}: unknown status {other:?} (enforced|open)",
                claim.id
            )),
        }
    }

    let mut docs = Vec::new();
    collect_markdown(&root.join("docs"), &mut docs);
    let readme = root.join("README.md");
    if readme.is_file() {
        docs.push(readme);
    }
    for doc in docs {
        let Ok(text) = std::fs::read_to_string(&doc) else {
            continue;
        };
        for (idx, line) in text.lines().enumerate() {
            for id in claim_ids(line) {
                report.references += 1;
                if !claims.contains_key(&id) {
                    report.violations.push(format!(
                        "{}:{}: [claim:{id}] does not resolve in {CLAIM_FILE}",
                        rel(root, &doc),
                        idx + 1
                    ));
                }
            }
        }
    }

    for doc in CLAIM_DOCS {
        let path = root.join(doc);
        let Ok(text) = std::fs::read_to_string(&path) else {
            report.violations.push(format!(
                "{doc}: the compliance surface this gate scans is missing"
            ));
            continue;
        };
        for (idx, line) in text.lines().enumerate() {
            let Some(word) = AFFIRMATIVE.iter().find(|w| contains_word(line, w)) else {
                continue;
            };
            let ids = claim_ids(line);
            let backed = ids.iter().any(|id| {
                claims
                    .get(id)
                    .is_some_and(|claim| claim.status == "enforced")
            });
            if !backed {
                report.violations.push(format!(
                    "{doc}:{}: asserts {word:?} without an enforced [claim:<id>] on the line",
                    idx + 1
                ));
            }
        }
    }

    report
}

fn load(root: &Path, violations: &mut Vec<String>) -> Result<BTreeMap<String, Claim>, String> {
    let path = root.join(CLAIM_FILE);
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("{CLAIM_FILE}: cannot read {}: {e}", path.display()))?;
    let value: toml::Value = raw.parse().map_err(|e| format!("{CLAIM_FILE}: {e}"))?;
    let entries = value
        .get("claim")
        .and_then(|c| c.as_array())
        .ok_or_else(|| format!("{CLAIM_FILE}: no [[claim]] entries"))?;
    let mut claims = BTreeMap::new();
    for (idx, entry) in entries.iter().enumerate() {
        let field = |key: &str| {
            entry
                .get(key)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };
        let Some(id) = field("id") else {
            violations.push(format!("{CLAIM_FILE}: claim #{} has no id", idx + 1));
            continue;
        };
        if claims.contains_key(&id) {
            violations.push(format!("{CLAIM_FILE}: duplicate claim id {id:?}"));
            continue;
        }
        for required in ["clause", "statement", "status"] {
            if field(required).as_deref().unwrap_or("").trim().is_empty() {
                violations.push(format!("claim {id}: missing non-empty {required:?}"));
            }
        }
        if entry.get("reason").is_none() && field("status").as_deref() == Some("open") {
            violations.push(format!("claim {id}: an open claim must say `reason`"));
        }
        claims.insert(
            id.clone(),
            Claim {
                id,
                status: field("status").unwrap_or_default(),
                check: field("check"),
                test: field("test"),
            },
        );
    }
    Ok(claims)
}

fn workspace_sources(root: &Path) -> String {
    let mut out = String::new();
    collect_rs(root, &mut out);
    out
}

fn collect_rs(dir: &Path, out: &mut String) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if matches!(name.as_ref(), "target" | ".git" | "node_modules") {
                continue;
            }
            collect_rs(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            if let Ok(text) = std::fs::read_to_string(&path) {
                out.push_str(&text);
                out.push('\n');
            }
        }
    }
}

fn collect_markdown(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_markdown(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn claim_ids(line: &str) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let mut rest = line;
    while let Some(start) = rest.find("[claim:") {
        rest = &rest[start + "[claim:".len()..];
        if let Some(end) = rest.find(']') {
            let id = rest[..end].trim();
            if !id.is_empty() {
                ids.insert(id.to_string());
            }
            rest = &rest[end + 1..];
        } else {
            break;
        }
    }
    ids
}

fn contains_word(line: &str, word: &str) -> bool {
    let mut rest = line;
    while let Some(pos) = rest.find(word) {
        let before_ok = pos == 0
            || !rest[..pos]
                .chars()
                .last()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let after = &rest[pos + word.len()..];
        let after_ok = after
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        if before_ok && after_ok {
            return true;
        }
        rest = &rest[pos + word.len()..];
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_claim_reference_is_parsed_out_of_prose() {
        let ids = claim_ids("| HIPAA | Partial [claim:hipaa] | see [claim:gdpr] |");
        assert_eq!(
            ids.into_iter().collect::<Vec<_>>(),
            vec!["gdpr".to_string(), "hipaa".to_string()]
        );
    }

    #[test]
    fn the_affirmative_word_needs_a_boundary() {
        assert!(contains_word("| HIPAA | Aligned |", "Aligned"));
        assert!(!contains_word("Unaligned", "Aligned"));
        assert!(!contains_word("Alignedness", "Aligned"));
    }
}
