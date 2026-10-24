//! # kelson-templates
//!
//! The project files `kelson new` materializes, compiled into the console.
//!
//! A binary installed from crates.io has no checkout beside it, so reading
//! templates off disk would leave `kelson new` generating nothing. Each
//! template file is embedded here as a `&'static str` instead.
//!
//! This crate exists rather than an inline `include_str!` table in the
//! console for one concrete reason: cargo packages a crate's own directory
//! and nothing above it, so an `include_str!("../../templates/...")` in
//! `tools/kelson-console` would compile in a checkout and then fail to
//! compile for anyone downloading the crate. Keeping the templates inside
//! their own crate root is what makes them survive being published.
//!
//! The templates remain exactly where STAGE-0-SPEC §2 records them -
//! `platform/templates/` - and this crate simply sits at that path.

/// One template file: its path relative to the generated project root, and
/// its body.
///
/// Paths always use forward slashes, on every platform, so a project
/// generated on Windows and one generated on Linux differ only in where it
/// was written.
pub type TemplateFile = (&'static str, &'static str);

/// The `--with-starter` template: full auth plus CRUD, a SPA shell and a
/// browser smoke kit.
///
/// Filename-ordered, which is the order `kelson new` writes them in.
pub const STARTER_TEMPLATE: &[TemplateFile] = &[
    ("Cargo.toml", include_str!("../starter-template/Cargo.toml")),
    ("README.md", include_str!("../starter-template/README.md")),
    (
        "e2e/auth.spec.ts",
        include_str!("../starter-template/e2e/auth.spec.ts"),
    ),
    (
        "e2e/notes.spec.ts",
        include_str!("../starter-template/e2e/notes.spec.ts"),
    ),
    (
        "e2e/package.json",
        include_str!("../starter-template/e2e/package.json"),
    ),
    (
        "e2e/playwright.config.ts",
        include_str!("../starter-template/e2e/playwright.config.ts"),
    ),
    (
        "module.toml",
        include_str!("../starter-template/module.toml"),
    ),
    (
        "resources/views/errors/404.html",
        include_str!("../starter-template/resources/views/errors/404.html"),
    ),
    (
        "resources/views/errors/500.html",
        include_str!("../starter-template/resources/views/errors/500.html"),
    ),
    (
        "resources/views/errors/maintenance.html",
        include_str!("../starter-template/resources/views/errors/maintenance.html"),
    ),
    ("src/lib.rs", include_str!("../starter-template/src/lib.rs")),
    ("web/app.js", include_str!("../starter-template/web/app.js")),
    (
        "web/index.html",
        include_str!("../starter-template/web/index.html"),
    ),
];

/// The default template: one module with a single read route, for a project
/// that is not starting from the starter.
pub const PROJECT_TEMPLATE: &[TemplateFile] = &[
    ("Cargo.toml", include_str!("../project-template/Cargo.toml")),
    ("README.md", include_str!("../project-template/README.md")),
    (
        "module.toml",
        include_str!("../project-template/module.toml"),
    ),
    ("src/lib.rs", include_str!("../project-template/src/lib.rs")),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Apply the three substitutions exactly as `kelson new` does.
    fn render(body: &str) -> String {
        body.replace("__PROJECT_NAME__", "blog")
            .replace("__PROJECT_CRATE__", "kelson-blog")
            .replace("__PROJECT_DESCRIPTION__", "A blog")
    }

    /// Both templates must be complete and self-consistent: every entry is
    /// non-empty, no path appears twice (a duplicate would mean one file
    /// silently overwrites another when the project is written), and the
    /// manifest that names the crate is present in both.
    #[test]
    fn embedded_templates_are_complete_and_unambiguous() {
        for (label, template) in [("starter", STARTER_TEMPLATE), ("project", PROJECT_TEMPLATE)] {
            assert!(!template.is_empty(), "{label} template is empty");
            let mut seen = std::collections::BTreeSet::new();
            for (rel, body) in template {
                assert!(!rel.is_empty(), "{label}: empty path");
                assert!(!rel.contains('\\'), "{label}: {rel} uses a backslash");
                assert!(!rel.starts_with('/'), "{label}: {rel} is absolute");
                assert!(!body.is_empty(), "{label}: {rel} has no content");
                assert!(
                    seen.insert(*rel),
                    "{label}: {rel} appears twice; one copy would overwrite the other"
                );
            }
            assert!(
                seen.contains("Cargo.toml"),
                "{label} template has no Cargo.toml"
            );
        }
    }

    /// Every file must be fully substituted, or a generated project carries a
    /// literal `__PROJECT_NAME__` into its manifest or its source.
    ///
    /// The check runs against the *rendered* body, not the raw one, because a
    /// raw check misreads a legitimate adjacent underscore: the starter has
    /// `create___PROJECT_NAME___notes`, which is a `create_` prefix and a
    /// `_notes` suffix around the marker and renders to `create_blog_notes`.
    /// Flagging the raw bytes would invite a "fix" that produces
    /// `create_blog__notes` instead.
    #[test]
    fn substitution_leaves_no_marker_behind() {
        for (label, template) in [("starter", STARTER_TEMPLATE), ("project", PROJECT_TEMPLATE)] {
            let mut names = 0;
            let mut crates = 0;
            let mut descriptions = 0;
            for (rel, body) in template {
                names += body.matches("__PROJECT_NAME__").count();
                crates += body.matches("__PROJECT_CRATE__").count();
                descriptions += body.matches("__PROJECT_DESCRIPTION__").count();
                assert!(
                    !render(body).contains("__PROJECT_"),
                    "{label}: {rel} carries a marker this crate does not substitute"
                );
            }
            assert!(names > 0, "{label}: no __PROJECT_NAME__ anywhere");
            assert!(crates > 0, "{label}: no __PROJECT_CRATE__ anywhere");
            assert!(descriptions > 0, "{label}: no __PROJECT_DESCRIPTION__");
        }
    }

    /// A marker sitting next to an underscore must yield one separator, not
    /// two. Pinned because it is the easiest template edit to get subtly
    /// wrong: "tidying" the extra underscore here would silently generate
    /// `create_blog__notes` for every project anyone created afterwards.
    #[test]
    fn a_marker_next_to_an_underscore_renders_one_separator() {
        assert_eq!(render("create___PROJECT_NAME___notes"), "create_blog_notes");
        assert_eq!(
            render("ModelDef::new(\"Note\", \"__PROJECT_NAME___notes\")"),
            "ModelDef::new(\"Note\", \"blog_notes\")"
        );
    }
}
