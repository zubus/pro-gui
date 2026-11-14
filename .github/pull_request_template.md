## Goal

Describe the purpose of this pull request and the problem it solves. Link the related issue, if any (for example, `Closes #12`).

## Solution

- [ ] Dependencies (`Cargo.toml` / `Cargo.lock`)
- [ ] Provider APIs (request or response shape, models, API keys)
- [ ] Database schema (SQLite tables, columns, indexes or triggers in `src/db.rs`)
- [ ] User interface (add screenshots below)

Explain the technical decisions you made and how you implemented the solution. Optionally include screenshots in this section.

## How to test

List, as numbered steps, how to confirm that the change works.

1.
2.
3.

## Checklist

- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test --all-targets` passes
- [ ] New unit tests live in a sibling `tests.rs`, and public API tests in `tests/`
- [ ] No API keys, personal paths or other secrets in code, tests or screenshots
