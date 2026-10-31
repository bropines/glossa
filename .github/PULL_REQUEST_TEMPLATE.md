<!-- What changes and why, in a few sentences. Link the issue it fixes, if any. -->

- [ ] `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass
- [ ] Docs updated where behaviour changed; a change to the module protocol (`glossa-module-sdk::wire`) updates `docs/modules/` in the same pull request
- [ ] `config.toml` written by an earlier version still loads (new fields have defaults, renamed ones an alias)
- [ ] No code, strings or assets from QTranslate, and no keys or tokens (see [AGENTS.md](../AGENTS.md))
- [ ] `CHANGELOG.md` has a line under *Unreleased* for anything a user would notice
