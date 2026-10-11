# kelson-templates

The project files [`kelson new`](../tools/kelson-console) materializes, compiled
into the console binary.

## Why this is a crate

A `kelson` binary installed from crates.io has no checkout beside it. If the
templates were read off disk, `kelson new` would find nothing and generate
nothing for everyone who had not cloned the repository.

Cargo packages a crate's own directory and nothing above it, so a
`include_str!("../../templates/...")` living in `tools/kelson-console` would
compile happily in a checkout and then fail to compile for anyone downloading
the crate. Keeping the templates inside their own crate root is what makes them
survive being published.

This crate has no dependencies and no code beyond two `&'static str` tables.

## Contents

| Path | Template |
|---|---|
| `starter-template/` | `kelson new --with-starter`: auth plus CRUD, a SPA shell, and a Playwright smoke kit |
| `project-template/` | `kelson new`: one module with a single read route |

These are the directories STAGE-0-SPEC §2 records as `platform/templates/`. The
spec's topology is unchanged; this crate simply sits at that path.

## Editing a template

Edit the files in place. They are read from disk when a checkout is present, so
a contributor sees their edit on the next `kelson new` without rebuilding, and
the compiled-in copy is regenerated on the next build. A test in this crate
asserts the embedded tables stay consistent with the files on disk.
