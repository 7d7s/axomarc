//! `cargo xtask cite-audit <doc> [<doc>...]` — the citation gate.
//!
//! # Why this exists as a gate
//!
//! The program's ledger documents (`docs/20-ARCHITECTURE-VERIFICATION.md`,
//! `CHANGELOG.md`, `PUBLISHING.md`) make claims of the form "X is enforced at
//! `path:123`". A claim like that is only worth what it costs to check, and a
//! citation that has drifted is worse than no citation: it reads as evidence.
//!
//! # What is checked
//!
//! Two properties, independently:
//!
//! 1. **It resolves.** The cited file exists at the tree the citation names;
//!    for a *bare* path (no tree prefix), it exists in exactly one tree, or
//!    its cited span is byte-identical in both. The cited range is inside the
//!    file. This is the check the tool has always had.
//! 2. **It says what the sentence says.** The words the same *clause* puts in
//!    backticks must actually appear in the cited span. This is the check that
//!    was missing, and it is the one that finds real drift: five citations in
//!    the verification doc had resolved cleanly for weeks while pointing at
//!    entirely different code, because a citation can resolve and still say
//!    nothing of the kind.
//!
//! # What "the same clause" means
//!
//! A citation is judged against the backticks between it and the end of its
//! own clause — `.` `;` `,` `!` `?` `|` end one, and so does an em dash.
//! That definition was not obvious, and the two wrong answers both cost a lot:
//!
//! * A fixed-width window is too wide. Every table row here packs five to ten
//!   claims onto one line, so a window reads its neighbours' symbols, and a
//!   first version of this check produced 36 false alarms against a PowerShell
//!   baseline of zero.
//! * A newline is not a boundary, though it looks like one. These documents
//!   wrap a citation across two or three lines with the symbol two lines above
//!   the path it qualifies, so treating the wrap as a boundary makes every
//!   such citation unverifiable — which is how a stale line number in
//!   CHANGELOG.md stayed invisible for a whole release.
//!
//! The em dash is the one *partial* boundary, and it is where the design
//! turned. Walking left it takes exactly one span across it and stops: these
//! documents write `` `ModuleManifest` — `crates/registry/lib.rs:604` ``, and
//! the span on the far side of the dash belongs to the citation. Taking none
//! of them loses that idiom; taking all of them sweeps in the previous claim.
//! One span keeps both, and it is the single change that turned the noise
//! floor from 36 false alarms to zero.
//!
//! One rule is about the citation rather than the prose: a token equal to the
//! cited file's own stem is not a claim about the file. A sentence that says
//! "the known `hot_reload` flake (`hot_reload.rs:493`)" is naming the file its
//! path already names, and requiring a timing assertion to mention its own
//! module would condemn a citation that is exactly right.
//!
//! # Where the gate stands
//!
//! Over the three ledger documents the counts are **193 citations, 184
//! resolved and 9 declared external, 62 content-verified, 122 with no token to
//! check, 0 failures**. That 122 is the honest measure of how little the check
//! can do on prose, and it is printed on every run for that reason: a gate
//! that stayed green by having nothing to say would be worse than none. The
//! 62 it does check are not a sample of convenience — every one of them is a
//! sentence that named its own subject, and four of the stale line numbers it
//! found were in the newest changelog entry, written the same week as the
//! code it pointed at.
//!
//! # The honest limits of check 2
//!
//! It is a contradiction detector, not a proof of relevance, and it is only
//! as strong as the document's habit of naming things.
//!
//! * A citation is only checked when its own clause backticks something
//!   code-shaped (`prepare`, `GRAPH-MANIFEST-ABI`). A clause whose subject is
//!   prose — "parsed from disk without recompiling — `graph_routes.rs:88`" —
//!   has nothing to check against, and is counted as **unverifiable** rather
//!   than passed. The count is printed every run, because a gate that stayed
//!   green by never having anything to say would be worse than none.
//! * A **bare** line citation (`:38`, `:950-959`) names no file, so it is not
//!   extracted at all. The ledger documents use these heavily — they inherit
//!   their file from an earlier citation in the same row — and the gate
//!   therefore does not see them. This is the largest blind spot, and it is
//!   named here rather than quietly worked around, because the attachment rule
//!   it would need is a judgement about prose.
//! * It reads only *backticked* tokens. A sentence can cite the right symbol
//!   for the wrong reason and still pass.
//!
//! The rule is deliberately one-sided in every direction: a false pass is
//! possible, a false alarm is not worth it, because a noisy gate is a gate
//! people stop reading.
//!
//! # Why Rust
//!
//! The gate this replaces was a PowerShell script that lived in a temporary
//! directory. Nothing in the repository produced it, nothing documented it,
//! and it would not run on anything but Windows — for a workspace whose other
//! gates (`dep-rules`, `lint`, `deny`) are all cargo-driven, that made it the
//! one gate a contributor could not reproduce. It is here, beside them, for
//! the same reason `env_var_violations` is: so a test can pin its behaviour.

use std::path::{Path, PathBuf};

/// Citations that are *deliberately* outside both workspace trees — the
/// source of a third-party crate, verified from the cargo registry rather
/// than from this repository. Anything not named here must resolve in-tree.
/// Each was opened and read at the cited span when it was added; the entry is
/// a declaration, not a shortcut past the check.
const EXTERNAL_PATHS: &[&str] = &[
    "src/migrate/source.rs",
    "wasmtime-36.0.17/src/runtime/component/mod.rs",
    "wasmtime-36.0.17/src/runtime.rs",
    "wasmtime-36.0.17/src/runtime/component/component.rs",
    "wast-261.0.0/src/wat.rs",
    "wast-261.0.0/src/component/binary.rs",
];

/// File extensions a citation may name. Deliberately enumerated rather than
/// "any word followed by a colon": prose is full of `e.g.` and `5:30`, and a
/// gate that reports those is a gate that gets ignored.
const CITED_EXTENSIONS: &[&str] = &[
    "rs", "toml", "md", "sql", "json", "wit", "wat", "yml", "yaml",
];

/// One `path:START(-END)` occurrence in a document.
#[derive(Debug, Clone)]
struct Cite {
    /// The path exactly as the document wrote it.
    file: String,
    start: usize,
    end: usize,
    /// Byte offset of the citation in the document, for the prose lookback.
    pos: usize,
    /// Byte offset of the backtick that opens the citation, or `pos` when the
    /// citation is not backticked. The clause walk starts just left of this.
    span_start: usize,
    /// Byte offset just past the backtick that closes the citation, or the end
    /// of its own number when there is none.
    span_end: usize,
}

/// Where a citation resolved to.
#[derive(Debug)]
enum Resolved {
    /// In-tree; `lines` is the cited span as it appears in the file.
    Found { path: PathBuf, lines: String },
    /// Not in the repository at all, and declared so.
    External,
}

/// A citation that could not be tied to a file.
#[derive(Debug)]
struct Unresolved(String);

/// A character that ends a clause. These documents pack a whole table row
/// onto one line, with five to ten independent claims separated by `.` and
/// `;` and `|` — so a fixed-width window always crosses into a neighbour's
/// claim and borrows its symbols, which is how a correct citation ends up
/// reported as a contradiction.
///
/// What is *not* a boundary matters as much: `—` connects a prose subject to
/// the citation it belongs to (`` `ModuleManifest` — `platform/` `lib.rs:1``)
/// and `(` opens a parenthetical (`` `WasmHost::prepare` (`keel-platform`
/// `lib.rs:761` ``), so both are crossed. Inside a backtick span nothing is a
/// boundary, which is what keeps `lib.rs` and `e.g.` from splitting a clause.
///
/// A newline is *not* a boundary, and that is deliberate: these documents wrap
/// a citation across two or three lines, with the symbol two lines above the
/// path it qualifies, and treating the wrap as a boundary would make every
/// such citation unverifiable — which is how a genuinely stale line number in
/// CHANGELOG.md stayed invisible. Sentences still end at `.`, `;`, and `!`, so
/// a clause cannot run away into the next claim; what a newline is allowed to
/// do is continue one.
fn is_clause_stop(c: char) -> bool {
    matches!(c, '.' | ';' | ',' | '!' | '?' | '|')
}

/// The em dash that carries a claim to the citation supporting it.
///
/// It is a *partial* boundary, and the asymmetry is the whole design. Walking
/// left, it stops the clause after taking exactly one span back across it:
/// `` `SYMBOL` — `path:1-9` `` is the idiom these documents use to say "this
/// symbol is what the citation shows", so the span on the far side of the dash
/// belongs to the citation. Walking right, it stops outright, because text
/// after a dash has started a new claim.
///
/// Taking exactly *one* span is the load-bearing detail. Crossing the dash
/// entirely sweeps in the whole preceding sentence — which is how a citation
/// about `KSN_WASM_FUEL` ended up judged against a line that pins a different
/// constant, three clauses away. Refusing to cross it at all loses
/// `` `ModuleManifest` — `registry/src/lib.rs:604-650` ``, the very pattern the
/// dash exists for. One span keeps both.
const EM_DASH: char = '\u{2014}';

/// Take one backticked span ending at `j`, if there is one, and return the
/// index the clause should start at.
fn take_one_span_left(text: &str, j: usize, out: &mut Vec<String>) -> usize {
    let mut k = j;
    while k > 0 {
        let Some(p) = text[..k].chars().next_back() else { break };
        if p == ' ' || p == '\t' {
            k -= p.len_utf8();
        } else {
            break;
        }
    }
    if k == 0 || !text[..k].ends_with('`') {
        return k;
    }
    let close = k - 1;
    let Some(open) = text[..close].rfind('`') else { return k };
    if let Some(tok) = normalize_token(&text[open + 1..close]) {
        out.push(tok);
    }
    open
}

/// Walk left from `i` — which sits just outside the citation's own span —
/// collecting the backticked tokens in the same clause, and return where the
/// clause begins.
fn walk_left(text: &str, mut i: usize, out: &mut Vec<String>) -> usize {
    while i > 0 {
        let Some(c) = text[..i].chars().next_back() else { break };
        if is_clause_stop(c) {
            return i;
        }
        if c == '`' {
            // The backtick closes the preceding span, so everything between it
            // and the next backtick leftwards is that span's content.
            let close = i - 1;
            let Some(open) = text[..close].rfind('`') else { return i };
            if let Some(tok) = normalize_token(&text[open + 1..close]) {
                out.push(tok);
            }
            i = open; // prose immediately left of that opening backtick
            continue;
        }
        if c == EM_DASH {
            return take_one_span_left(text, i - c.len_utf8(), out);
        }
        i -= c.len_utf8();
    }
    i
}

/// Walk right from `i` — just past the citation's own span — the same way,
/// and return where the clause ends.
fn walk_right(text: &str, mut i: usize, out: &mut Vec<String>) -> usize {
    while i < text.len() {
        let Some(c) = text[i..].chars().next() else { break };
        if is_clause_stop(c) || c == EM_DASH {
            return i;
        }
        if c == '`' {
            let Some(close) = text[i + 1..].find('`').map(|x| i + 1 + x) else { return i };
            if let Some(tok) = normalize_token(&text[i + 1..close]) {
                out.push(tok);
            }
            i = close + 1;
            continue;
        }
        i += c.len_utf8();
    }
    i
}

/// The byte range of the clause `cite` belongs to.
fn clause_range(text: &str, cite: &Cite) -> (usize, usize) {
    let mut sink = Vec::new();
    let start = walk_left(text, cite.span_start, &mut sink);
    let end = walk_right(text, cite.span_end, &mut sink);
    (start, end)
}

/// Every backticked token in the same clause as `cite`.
fn clause_tokens(text: &str, cite: &Cite) -> Vec<String> {
    let mut out = Vec::new();
    walk_left(text, cite.span_start, &mut out);
    walk_right(text, cite.span_end, &mut out);
    // The cited file's own name is not a claim about the file. A sentence that
    // says "the known `hot_reload` flake (`hot_reload.rs:493`)" names the very
    // file its path already names; holding the cited line to that would demand
    // a timing assertion mention its own module, and would condemn a citation
    // that is exactly right.
    if let Some(stem) = file_stem(&cite.file) {
        out.retain(|t| t != &stem);
    }
    out.sort();
    out.dedup();
    out
}

/// The lower-cased file name of `path` without its extension, if long enough
/// to mean anything (`lib.rs` would match half the tree).
fn file_stem(path: &str) -> Option<String> {
    let name = path.rsplit(['/', '\\']).next()?;
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    (stem.len() >= 4).then(|| stem.to_ascii_lowercase())
}

/// The clause itself, for a failure message. A contradiction that does not
/// print the sentence it contradicted leaves the reader to go hunting.
fn clause_text(text: &str, cite: &Cite) -> String {
    let (start, end) = clause_range(text, cite);
    text.get(start..end)
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Trim a clause to something a terminal can show whole, on a char boundary.
fn clamp(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let end = floor_boundary(s, max);
    format!("{}…", &s[..end])
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Pull the backticked tokens out of `window` that are shaped like code —
/// identifiers, paths of identifiers, error codes.
///
/// Everything else is skipped on purpose: backticks in these documents also
/// carry prose (`publish = false`), paths (`crates/x/src/lib.rs`), and
/// fragments, and treating any of those as a content claim would make the
/// check fire on style rather than on drift.
///
/// This is the predicate only; deciding *which* backticks belong to a
/// citation is [`clause_tokens`]' job.
#[cfg(test)]
fn code_tokens(window: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = window;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else { break };
        if let Some(tok) = normalize_token(&after[..close]) {
            out.push(tok);
        }
        rest = &after[close + 1..];
    }
    out.sort();
    out.dedup();
    out
}

/// The canonical form of a backticked span as a content claim, or `None` if
/// it is not one. Lower-cased, with section brackets removed.
///
/// `[A-Za-z_][A-Za-z0-9_:<>,!*]*`, at least two characters.
///
/// The character set is the whole of the design here: it admits
/// `GRAPH-MANIFEST-ABI`, `Option<u8>` and the macro path `bindgen!`, and
/// rejects `crates/x/lib.rs` (a path, not a claim), `publish = false`
/// (prose), and `#`-forms (`#[serde(default)]`).
///
/// Hyphens carry one extra rule: a hyphen is not legal in a Rust identifier,
/// so a hyphenated token is only a claim when every letter is upper case —
/// the shape of a violation code (`GRAPH-MANIFEST-ABI`). Lower-case kebab is
/// a path fragment, and the tree name `keel-platform` is backticked on nearly
/// every line of the verification doc while appearing in no source file it
/// cites; letting it through would make it the token that always fails to
/// match, drowning the one that matters.
///
/// Surrounding brackets are stripped first, because a TOML section *is* a
/// name: `[budget]` and `[[routes]]` are how the documents refer to things
/// the code names `ModuleBudget` and the route set, and requiring the code to
/// carry its section brackets would guarantee a miss on the one row that
/// does name its subject.
fn normalize_token(s: &str) -> Option<String> {
    let s = s.trim_matches(|c| c == '[' || c == ']');
    if s.len() < 2 {
        return None;
    }
    let mut chars = s.chars();
    let first = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    let mut has_hyphen = false;
    for c in chars {
        match c {
            '-' => has_hyphen = true,
            c if c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '<' | '>' | ',' | '*' | '!') => {}
            _ => return None,
        }
    }
    if has_hyphen && !s.chars().all(|c| !c.is_ascii_alphabetic() || c.is_ascii_uppercase()) {
        return None;
    }
    Some(s.to_ascii_lowercase())
}

/// Find every `path:START(-END)` occurrence.
///
/// Hand-rolled rather than regex: `xtask` has no regex dependency and adding
/// one would be a new external, which the workspace's own dependency policy
/// caps. The scan is anchored on `.EXT:`, which is what makes prose periods
/// harmless.
fn extract_cites(text: &str) -> Vec<Cite> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    for (dot, _) in text.match_indices('.') {
        // Read the extension immediately after the dot.
        let mut ext_end = dot + 1;
        while ext_end < bytes.len() && bytes[ext_end].is_ascii_alphanumeric() {
            ext_end += 1;
        }
        if ext_end >= bytes.len() || bytes[ext_end] != b':' {
            continue;
        }
        let ext = &text[dot + 1..ext_end];
        if !CITED_EXTENSIONS.contains(&ext) {
            continue;
        }
        // Walk back over the path characters.
        let mut path_start = dot;
        while path_start > 0 {
            let c = bytes[path_start - 1];
            if c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'/' | b'\\' | b'-') {
                path_start -= 1;
            } else {
                break;
            }
        }
        if path_start == dot {
            continue; // no path before the extension
        }
        // Read START, then an optional -END.
        let num_start = ext_end + 1;
        let mut cursor = num_start;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if cursor == num_start {
            continue;
        }
        let Ok(start) = text[num_start..cursor].parse::<usize>() else {
            continue;
        };
        let mut end = start;
        let mut num_end = cursor;
        if cursor < bytes.len() && bytes[cursor] == b'-' {
            let e_start = cursor + 1;
            let mut e_cur = e_start;
            while e_cur < bytes.len() && bytes[e_cur].is_ascii_digit() {
                e_cur += 1;
            }
            if e_cur > e_start {
                if let Ok(e) = text[e_start..e_cur].parse::<usize>() {
                    end = e.max(start);
                }
                num_end = e_cur;
            }
        }
        // Where the citation's own backticks are, so the clause walk knows
        // where the citation stops and prose begins. A citation that is not
        // backticked is treated as starting and ending at its own text.
        let span_start = if path_start > 0 && bytes[path_start - 1] == b'`' {
            path_start - 1
        } else {
            path_start
        };
        let span_end = text[num_end..]
            .find('`')
            .map(|x| num_end + x + 1)
            .filter(|&x| x - num_end <= 4)
            .unwrap_or(num_end);
        out.push(Cite {
            file: text[path_start..dot].to_string() + "." + ext,
            start,
            end,
            pos: text[..path_start].len(),
            span_start,
            span_end,
        });
    }
    out
}

/// Read a file and return its cited span, or why that is impossible.
fn read_span(path: &Path, start: usize, end: usize) -> Result<String, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("{}: unreadable ({e})", path.display()))?;
    let lines: Vec<&str> = content.lines().collect();
    if start == 0 || end < start {
        return Err(format!(
            "{}: cited range {start}-{end} is not a valid range (lines are 1-based)",
            path.display()
        ));
    }
    if lines.len() < end {
        return Err(format!(
            "{}: cited line {end} is past the end of the file ({} lines)",
            path.display(),
            lines.len()
        ));
    }
    Ok(lines[start - 1..end].join("\n").trim().to_string())
}

/// How far back to look for a tree name written in prose. The gate this
/// replaces allowed 90 characters, and the ledger documents rely on it: a row
/// reads `` `keel-platform` `crates/x/src/lib.rs:1-9` `` with the tree named
/// once at the head of a long cell, nowhere near the path itself.
const QUALIFIER_CHARS: usize = 90;

/// The tree a citation's own prose names, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tree {
    /// `platform/` — the root tree, `<repo>/platform`.
    Root,
    /// `keel-platform/` — the canonical tree, `<repo>/platform/keel-platform`.
    Canonical,
}

fn is_wordish(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.' || c == '-'
}

/// Read the 90 characters before `pos` for a tree name.
///
/// Precedence is `keel-platform` first, then a *standalone* `platform`. That
/// order matters: `keel-platform` contains `platform`, and a bare `platform`
/// match inside it would resolve half the citations to the wrong tree. The
/// boundary test excludes `[\w.-]` on both sides, so `keel-platform`,
/// `my.platform`, and `platform_x` are all left alone while
/// `platform/docs/20-...` and the path head `keel-platform` are honoured —
/// the same rules the PowerShell gate used, kept because the documents are
/// already written to them.
fn tree_named_in(text: &str, pos: usize) -> Option<Tree> {
    let start = floor_boundary(text, pos.saturating_sub(QUALIFIER_CHARS));
    let before = text.get(start..pos)?;
    if before.to_ascii_lowercase().contains("keel-platform") {
        return Some(Tree::Canonical);
    }
    let lower = before.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(rel) = lower[from..].find("platform") {
        let at = from + rel;
        let end = at + "platform".len();
        let prev = text[start..start + at].chars().next_back();
        let next = text[start + end..pos].chars().next();
        if !prev.map(is_wordish).unwrap_or(false) && !next.map(is_wordish).unwrap_or(false) {
            return Some(Tree::Root);
        }
        from = at + 1;
        if from >= lower.len() {
            break;
        }
    }
    None
}

/// Read a path that the citation or its prose has already pinned to one tree.
fn probe(path: &Path, cite: &Cite) -> Result<Resolved, Unresolved> {
    let label = format!("{}:{}-{}", cite.file, cite.start, cite.end);
    if !path.exists() {
        return Err(Unresolved(format!("MISSING {label}")));
    }
    match read_span(path, cite.start, cite.end) {
        Ok(lines) => Ok(Resolved::Found {
            path: path.to_path_buf(),
            lines,
        }),
        Err(e) => Err(Unresolved(format!("OUT-OF-RANGE {label} ({e})"))),
    }
}

/// Resolve a citation against the two trees, in the order the gate it
/// replaces used: pinned by its own path, then pinned by nearby prose, then
/// left bare and required to be unambiguous.
///
/// `platform_root` is `<repo>/platform`, so the root tree is `platform_root`
/// and the canonical tree is `platform_root/keel-platform`.
fn resolve(text: &str, platform_root: &Path, cite: &Cite) -> Result<Resolved, Unresolved> {
    let norm = cite.file.replace('\\', "/");

    // (1) Already fully qualified in the path itself -> no guesswork.
    if norm.starts_with("platform/") || norm.starts_with("keel-platform/") {
        let repo = platform_root
            .parent()
            .ok_or_else(|| Unresolved("no repo root above platform/".into()))?;
        return probe(&repo.join(&norm), cite).map_err(|Unresolved(e)| Unresolved(format!("EXPLICIT {e}")));
    }

    // (2) A tree name in the prose just before it decides the tree.
    match tree_named_in(text, cite.pos) {
        Some(Tree::Canonical) => {
            let p = platform_root.join("keel-platform").join(&norm);
            return probe(&p, cite).map_err(|Unresolved(e)| Unresolved(format!("KEEL-PLATFORM {e}")));
        }
        Some(Tree::Root) => {
            let p = platform_root.join(&norm);
            return probe(&p, cite).map_err(|Unresolved(e)| Unresolved(format!("ROOT {e}")));
        }
        None => {}
    }

    // (3) Bare: one tree must carry it, or both must agree on the cited span.
    let mut exists: Vec<PathBuf> = Vec::new();
    for p in [platform_root.join(&norm), platform_root.join("keel-platform").join(&norm)] {
        if p.exists() {
            exists.push(p);
        }
    }
    if exists.is_empty() {
        if EXTERNAL_PATHS.contains(&norm.as_str()) {
            return Ok(Resolved::External);
        }
        return Err(Unresolved(format!(
            "UNRESOLVED   {}:{}-{}",
            cite.file, cite.start, cite.end
        )));
    }
    let mut valid: Vec<(PathBuf, String)> = Vec::new();
    for p in &exists {
        if let Ok(lines) = read_span(p, cite.start, cite.end) {
            valid.push((p.clone(), lines));
        }
    }
    if valid.is_empty() {
        return Err(Unresolved(format!(
            "OUT-OF-RANGE {}:{}-{} (neither copy is that long)",
            cite.file, cite.start, cite.end
        )));
    }
    // Only one tree carried a readable copy, so the citation is unambiguous.
    if valid.len() == 1 {
        let (path, lines) = valid.pop().unwrap();
        return Ok(Resolved::Found { path, lines });
    }
    if valid[0].1 == valid[1].1 {
        let (path, lines) = valid.swap_remove(0);
        return Ok(Resolved::Found { path, lines });
    }
    Err(Unresolved(format!(
        "UNQUALIFIED-BUT-DIVERGES  {}:{}-{} (the two trees differ here, so the citation must name one)",
        cite.file, cite.start, cite.end
    )))
}

/// The gate's report for one document.
#[derive(Default)]
struct Report {
    citations: usize,
    resolved: usize,
    external: usize,
    /// Resolved, and a sentence token was found in the cited span.
    content_ok: usize,
    /// Resolved, but the sentence named no code-like token at all — nothing
    /// to check against. Not a failure; reported so the number is visible.
    unverifiable: usize,
    failures: Vec<String>,
}

fn audit_document(platform_root: &Path, doc: &Path) -> Result<Report, String> {
    let text = std::fs::read_to_string(doc)
        .map_err(|e| format!("cannot read {}: {e}", doc.display()))?;
    let cites = extract_cites(&text);
    let mut rep = Report {
        citations: cites.len(),
        ..Report::default()
    };

    for cite in &cites {
        match resolve(&text, platform_root, cite) {
            Err(u) => rep.failures.push(u.0),
            Ok(Resolved::External) => rep.external += 1,
            Ok(Resolved::Found { path, lines }) => {
                rep.resolved += 1;
                let tokens = clause_tokens(&text, cite);
                if tokens.is_empty() {
                    rep.unverifiable += 1;
                    continue;
                }
                let span = lines.to_ascii_lowercase();
                if tokens.iter().any(|t| claims_of(t).iter().any(|c| span.contains(c))) {
                    rep.content_ok += 1;
                } else {
                    rep.failures.push(format!(
                        "CONTRADICTION  {}:{}-{} cites `{}` but the clause says [{}] — \"{}\"",
                        cite.file,
                        cite.start,
                        cite.end,
                        path.display(),
                        tokens.join(", "),
                        clamp(&clause_text(&text, cite), 220)
                    ));
                }
            }
        }
    }
    Ok(rep)
}

/// Everything a backticked token is claiming to find in the cited span.
///
/// Two relaxations, both because the sentence and the source phrase the same
/// thing differently rather than because they mean different things:
///
/// * `WasmHost::prepare` also claims `prepare`. A line reading `pub fn
///   prepare(` is the very item the sentence named; requiring the full path
///   would fail every citation that names a method with its type.
/// * `sock_*` also claims `sock_`. A trailing star is a stated prefix, and
///   the cited code writes the members out in full.
///
/// This is where the check stays honest: these widen what counts as a match,
/// so they make the detector *more* likely to pass a bad citation, not less.
/// The trade is deliberate — the failure mode that matters is a gate that
/// cries wolf on correct citations, because then nobody reads the one that
/// is right.
fn claims_of(token: &str) -> Vec<String> {
    let mut out = vec![token.to_ascii_lowercase()];
    if let Some((_, last)) = token.rsplit_once("::") {
        if last.len() >= 2 {
            out.push(last.to_ascii_lowercase());
        }
    }
    if let Some(prefix) = token.strip_suffix('*') {
        if prefix.len() >= 2 {
            out.push(prefix.to_ascii_lowercase());
        }
    }
    out
}

/// `cargo xtask cite-audit <doc> [<doc>...]`, paths relative to the repo root.
pub fn run(repo_root: &Path, docs: &[String]) -> std::process::ExitCode {
    let platform_root = repo_root.join("platform");
    let mut total = Report::default();
    let mut unreadable = false;

    for doc in docs {
        let path = repo_root.join(doc);
        match audit_document(&platform_root, &path) {
            Err(e) => {
                println!("cite-audit [!!] {e}");
                unreadable = true;
            }
            Ok(rep) => {
                println!(
                    "cite-audit: {doc}\n  citations {} | resolved {} | external {} | content-verified {} | no-token-to-check {}",
                    rep.citations, rep.resolved, rep.external, rep.content_ok, rep.unverifiable
                );
                for f in &rep.failures {
                    println!("  [!!] {f}");
                }
                total.citations += rep.citations;
                total.resolved += rep.resolved;
                total.external += rep.external;
                total.content_ok += rep.content_ok;
                total.unverifiable += rep.unverifiable;
                total.failures.extend(rep.failures);
            }
        }
    }

    println!(
        "CITE-AUDIT: citations={} resolved={} external={} content-verified={} unverifiable={} failures={}",
        total.citations,
        total.resolved,
        total.external,
        total.content_ok,
        total.unverifiable,
        total.failures.len()
    );
    if total.failures.is_empty() && !unreadable {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kelson-xtask-citeaudit-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A path, its line range, and nothing else. Used to pin the extractor.
    #[test]
    fn extracts_paths_and_ranges_but_not_prose_periods() {
        let text = "see `crates/x/src/lib.rs:12-20` and `docs/a.md:3`, \
                    but not `e.g.` or the time 5:30.";
        let cites = extract_cites(text);
        assert_eq!(cites.len(), 2, "prose periods and clock times are not citations");
        assert_eq!(cites[0].file, "crates/x/src/lib.rs");
        assert_eq!((cites[0].start, cites[0].end), (12, 20));
        assert_eq!((cites[1].start, cites[1].end), (3, 3));
    }

    /// The content check is one-sided: a sentence naming the right symbol
    /// passes, a sentence naming one the span lacks is a contradiction, and
    /// a sentence naming no code at all is neither.
    #[test]
    fn content_check_accepts_match_and_rejects_contradiction() {
        let span = "pub fn prepare(&self) -> Result<Self, E> {";
        assert!(normalize_token("prepare").is_some());
        assert!(code_tokens("`prepare` and `Result<Self, E>`").contains(&"prepare".to_string()));
        // A path or a prose phrase is not a content claim.
        assert!(code_tokens("`crates/x/src/lib.rs:7`").is_empty());
        assert!(code_tokens("`publish = false`").is_empty());
        assert!(code_tokens("`v1`").contains(&"v1".to_string()));
        assert!(span.to_ascii_lowercase().contains("prepare"));
        // `MODULE_ABI` is not in this span, so a window naming it contradicts.
        assert!(!span.to_ascii_lowercase().contains("module_abi"));
        // A type-qualified name also claims its last segment, and a trailing
        // star claims its prefix; without both, `WasmHost::prepare` would be
        // a guaranteed false alarm on a line reading `fn prepare(`.
        assert!(claims_of("WasmHost::prepare").contains(&"prepare".to_string()));
        assert!(claims_of("sock_*").contains(&"sock_".to_string()));
        assert!(claims_of("prepare").len() == 1, "a plain name claims only itself");
        // A TOML section and a macro path are both names. Losing either to its
        // brackets or its bang would turn a real claim into silence, and the
        // row that names `[budget]` is the one row that does name its subject.
        assert_eq!(normalize_token("[budget]"), Some("budget".to_string()));
        assert_eq!(
            normalize_token("wasmtime::component::bindgen!"),
            Some("wasmtime::component::bindgen!".to_string())
        );
        // But a lower-case kebab is a path fragment or the tree name, never a
        // claim; and a path is not one either.
        assert_eq!(normalize_token("keel-platform"), None);
        assert_eq!(normalize_token("crates/x/src/lib.rs:7"), None);
    }

    /// The whole point of moving the gate into the tree: a bare path that
    /// resolves in only one tree is fine, and one that resolves differently
    /// in both must name its tree.
    #[test]
    fn bare_citation_must_be_unambiguous_or_identical() {
        let root = temp_root("trees");
        std::fs::create_dir_all(root.join("platform/keel-platform/crates")).unwrap();
        std::fs::create_dir_all(root.join("platform/crates")).unwrap();
        std::fs::write(root.join("platform/crates/shared.rs"), "fn same() {}\n").unwrap();
        std::fs::write(
            root.join("platform/keel-platform/crates/shared.rs"),
            "fn same() {}\n",
        )
        .unwrap();
        std::fs::write(root.join("platform/crates/only_root.rs"), "fn a() {}\nfn b() {}\n")
            .unwrap();
        std::fs::write(
            root.join("platform/keel-platform/crates/differs.rs"),
            "fn canonical() {}\n",
        )
        .unwrap();
        std::fs::write(root.join("platform/crates/differs.rs"), "fn root() {}\n").unwrap();

        let platform_root = root.join("platform");
        let cite = |file: &str, s: usize| Cite {
            file: file.into(),
            start: s,
            end: s,
            pos: 0,
            span_start: 0,
            span_end: 0,
        };

        assert!(resolve("", &platform_root, &cite("crates/shared.rs", 1)).is_ok());
        assert!(resolve("", &platform_root, &cite("crates/only_root.rs", 1)).is_ok());
        let diverges = resolve("", &platform_root, &cite("crates/differs.rs", 1));
        assert!(
            matches!(&diverges, Err(u) if u.0.contains("DIVERGES")),
            "a bare path the trees disagree on must be refused, got {diverges:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The rule the prose lookback exists for — and the one a first port of
    /// this gate dropped, which turned every prose-qualified citation in the
    /// verification document into a false `DIVERGES`.
    #[test]
    fn a_tree_name_in_the_prose_decides_the_tree() {
        let root = temp_root("qual");
        std::fs::create_dir_all(root.join("platform/keel-platform/crates")).unwrap();
        std::fs::create_dir_all(root.join("platform/crates")).unwrap();
        std::fs::write(root.join("platform/crates/both.rs"), "fn root_tree() {}\n").unwrap();
        std::fs::write(root.join("platform/keel-platform/crates/both.rs"), "fn canonical() {}\n")
            .unwrap();
        let platform_root = root.join("platform");
        let pinned = |text: &str| {
            let cites = extract_cites(text);
            assert_eq!(cites.len(), 1, "fixture must hold exactly one citation");
            resolve(text, &platform_root, &cites[0])
        };

        // `keel-platform` in the window -> canonical copy, despite both trees
        // holding the file at the same line with different content.
        match pinned("`keel-platform` `crates/both.rs:1`") {
            Ok(Resolved::Found { path, .. }) => assert!(
                path.starts_with(root.join("platform/keel-platform")),
                "expected the canonical tree, got {}",
                path.display()
            ),
            other => panic!("expected a pin, got {other:?}"),
        }
        // `platform` in the window -> root copy.
        match pinned("`platform` `crates/both.rs:1`") {
            Ok(Resolved::Found { path, .. }) => assert!(
                path.starts_with(root.join("platform").join("crates")),
                "expected the root tree, got {}",
                path.display()
            ),
            other => panic!("expected a pin, got {other:?}"),
        }
        // Embedded in a longer word it is not a tree name, so the citation
        // stays bare and must still be refused for diverging.
        assert!(
            matches!(pinned("notplatform `crates/both.rs:1`"), Err(u) if u.0.contains("DIVERGES")),
            "a tree name only counts on its own"
        );
        // Neither is the word further than 90 characters away.
        let far = format!("{} `crates/both.rs:1`", "x".repeat(90));
        assert!(
            matches!(pinned(&far), Err(u) if u.0.contains("DIVERGES")),
            "the lookback must not reach past 90 characters"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The clause, not the window. A citation whose own subject is prose must
    /// be counted *unverifiable* rather than condemned on a neighbour's
    /// symbols — every table row in the verification doc packs five to ten
    /// claims onto one line, which is exactly how a windowed version of this
    /// check produced 36 false alarms against a PowerShell baseline of zero.
    #[test]
    fn a_citation_is_never_judged_by_a_neighbouring_clause() {
        let doc = "unknown operation — `a/b.rs:1-9`; mismatch -> `GRAPH-MANIFEST-ABI` — `c/d.rs:3`";
        let cites = extract_cites(doc);
        assert_eq!(cites.len(), 2);
        assert!(
            clause_tokens(doc, &cites[0]).is_empty(),
            "a prose subject yields nothing to check, not a neighbour's symbol"
        );
        let second = clause_tokens(doc, &cites[1]);
        assert!(
            second.contains(&"graph-manifest-abi".to_string()),
            "the adjacent symbol is this citation's, got {second:?}"
        );
    }

    /// The em dash is these documents' "this symbol is what the citation
    /// shows" idiom, so walking left it takes *one* span back across it and
    /// then stops.
    ///
    /// Both halves of that are load-bearing, and the first version of this
    /// gate got the second one wrong. Taking *none* loses
    /// `` `ModuleManifest` — `crates/registry/lib.rs:604` ``, the very pattern
    /// the dash exists for. Taking *all* of them sweeps in the preceding
    /// sentence, which is how a citation about one constant ended up judged
    /// against a line pinning a different one, three clauses away.
    #[test]
    fn an_em_dash_carries_exactly_one_symbol_back() {
        // The idiom: the span immediately left of the dash is this
        // citation's subject.
        let doc = "`ModuleManifest` — `keel-platform` `crates/registry/lib.rs:7`";
        let cites = extract_cites(doc);
        assert_eq!(cites.len(), 1);
        let t = clause_tokens(doc, &cites[0]);
        assert_eq!(t, vec!["modulemanifest".to_string()], "got {t:?}");

        // A dash preceded by prose is a break, not a carrier. The citation is
        // unverifiable — it is not condemned on the prose's behalf.
        let doc = "the route plane (api/web) — `crates/engine.rs:7`";
        let cites = extract_cites(doc);
        assert_eq!(cites.len(), 1);
        assert!(clause_tokens(doc, &cites[0]).is_empty(), "prose before a dash is not a claim");

        // Walking right, the dash is an ordinary stop: what follows has
        // started a new claim, even when that claim is about another file.
        let doc = "`keel-platform` `crates/lib.rs:7` — there is no swap path in `load_wasm_plugins`";
        let cites = extract_cites(doc);
        assert_eq!(cites.len(), 1);
        assert!(clause_tokens(doc, &cites[0]).is_empty(), "text after a dash is a new claim");
    }

    /// Naming a file is not a claim about it.
    ///
    /// A sentence that says "the known `hot_reload` flake
    /// (`hot_reload.rs:493`)" names the very file its path already names.
    /// Holding the cited line to that would demand a timing assertion mention
    /// its own module, and would condemn a citation that is exactly right —
    /// it was one of the three failures this rule was written to clear.
    #[test]
    fn a_file_is_not_held_to_its_own_name() {
        let doc = "the known `hot_reload` wall-clock flake (`src/hot_reload.rs:493`)";
        let cites = extract_cites(doc);
        assert_eq!(cites.len(), 1);
        assert!(
            clause_tokens(doc, &cites[0]).is_empty(),
            "the file's own name is not a claim about its contents"
        );

        // A stem too short to mean anything is not special-cased: `lib.rs`
        // would otherwise excuse a claim about `lib` across half the tree.
        let doc = "`helpers` — `crates/lib.rs:9`";
        let cites = extract_cites(doc);
        assert_eq!(clause_tokens(doc, &cites[0]), vec!["helpers".to_string()]);
    }

    /// Two boundaries a fixed-width window gets wrong, in opposite
    /// directions.
    ///
    /// A newline is *not* one: these documents wrap a citation across two or
    /// three lines with the symbol two lines above the path it qualifies, and
    /// treating the wrap as a boundary is what let a stale line number in
    /// CHANGELOG.md stay invisible. A comma *is* one: a parenthetical that
    /// lists two citations is two claims, and a citation must not be judged
    /// against the other one's symbol.
    #[test]
    fn a_newline_continues_a_clause_and_a_comma_ends_it() {
        let doc = "`WasmHost::prepare` (`keel-platform`\n  `crates/lib.rs:12`)";
        let cites = extract_cites(doc);
        assert_eq!(cites.len(), 1);
        assert_eq!(
            clause_tokens(doc, &cites[0]),
            vec!["wasmhost::prepare".to_string()],
            "a line wrap is still one clause"
        );

        let doc = "`prepare` (`crates/lib.rs:12`, `resolve_budget` `:678`)";
        let cites = extract_cites(doc);
        assert_eq!(cites.len(), 1, "a bare `:678` names no file, so is not extracted");
        assert_eq!(
            clause_tokens(doc, &cites[0]),
            vec!["prepare".to_string()],
            "the comma ends the clause before the second claim"
        );
    }

    /// The failure this gate was strengthened to find: a citation that
    /// resolves, sits in range, and points at code that says none of what the
    /// sentence claims.
    #[test]
    fn flags_a_resolved_citation_that_contradicts_its_sentence() {
        let root = temp_root("contra");
        std::fs::create_dir_all(root.join("platform/keel-platform/crates")).unwrap();
        std::fs::write(
            root.join("platform/keel-platform/crates/lib.rs"),
            "struct Unrelated;\nstruct AlsoUnrelated;\nstruct MODULE_ABI;\n",
        )
        .unwrap();
        let doc = root.join("doc.md");
        // Line 3 holds `MODULE_ABI`; line 1 does not, but resolves and is in
        // range. The sentence names it, so the citation is wrong.
        std::fs::write(
            &doc,
            "The gate is `keel-platform` `crates/lib.rs:1` (`MODULE_ABI`).\n",
        )
        .unwrap();

        let rep = audit_document(&root.join("platform"), &doc).unwrap();
        assert_eq!(rep.resolved, 1, "it resolves — that is what makes it insidious");
        assert_eq!(rep.content_ok, 0);
        assert!(
            rep.failures.iter().any(|f| f.contains("CONTRADICTION")),
            "expected a contradiction, got {:?}",
            rep.failures
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The gate runs clean on the documents it exists to audit. This is the
    /// assertion that makes a doc edit a build failure when a citation drifts.
    #[test]
    fn the_ledger_documents_pass_the_gate() {
        let root = crate::root_dir()
            .parent()
            .expect("keel-platform has a parent")
            .parent()
            .expect("platform has a parent");
        let docs = [
            "platform/docs/20-ARCHITECTURE-VERIFICATION.md",
            "platform/CHANGELOG.md",
            "platform/PUBLISHING.md",
        ];
        let mut bad = Vec::new();
        for doc in docs {
            match audit_document(&root.join("platform"), &root.join(doc)) {
                Err(e) => bad.push(e),
                Ok(rep) => {
                    for f in rep.failures {
                        bad.push(format!("{doc}: {f}"));
                    }
                }
            }
        }
        assert!(bad.is_empty(), "citation gate failed:\n{}", bad.join("\n"));
    }
}
