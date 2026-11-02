# sd-profile

Rust CLI that profiles SD and microSD cards: identity, speed, reader,
partitions, filesystems, mounts. macOS first. Linux comes later, and every
change keeps the code ready for it.

## Rust

- Model data with types: enums and newtypes instead of strings, magic numbers
  and boolean flags, so invalid states can't be represented.
- Propagate errors with `?`. Library modules return typed errors
  (`thiserror`); `main.rs` turns them into messages and exit codes.
  `unwrap`/`expect` belong in tests, or on a true invariant with the `expect`
  message naming it.
- `unsafe` lives only in the `platform/` files that call the OS, and each
  block has a `// SAFETY:` comment.
- Every device is opened read-only.
- Fix what clippy reports (pedantic lints are on, see `Cargo.toml`). An
  `#[allow(...)]` needs a comment saying why.
- Add a dependency only when it replaces substantial code, and pick widely
  used, maintained crates. Give each one a short comment in `Cargo.toml`
  saying what it's for.

Done means `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
and `cargo test` all pass.

## Testing

Test against the disk images in `tests/fixtures/`. Running against a real
card needs `sudo`; leave that to the user.

## Git workflow

1. Start each change on a new branch from `main`, named `<type>/<short-name>`
   (e.g. `feat/gpt-parser`).
2. Commit as you go. Every commit is one line, Conventional Commits with a
   scope: `type(scope): summary`, e.g. `feat(disk): parse GPT headers`. No
   body, no trailers, no agent or tool names.
3. When the done checks pass, squash-merge into `main` as one commit in the
   same format:
   `git switch main && git merge --squash <branch> && git commit -m "type(scope): summary"`.
   Then delete the branch with `git branch -D` (after a squash merge, `-d`
   doesn't see it as merged).
4. Push `main`: `git push origin main`. Feature branches stay local.

Scopes follow the modules: `cli`, `model`, `sd`, `disk`, `macos`, `linux`,
`render`, plus `docs`, `deps`, `release`, and `repo` for repo-level files
(`Cargo.toml` settings, lints, toolchain, `cog.toml`, README, this file).

Once per clone, install the commit-msg hook: `cog install-hook --all` (needs
cocogitto: `cargo install --locked cocogitto`). It rejects any message that
isn't one conventional line.

## Versioning

Semantic versioning, computed by cocogitto (`cog`) from the commits on `main`:

| Commit | Version bump |
|---|---|
| `fix` | patch |
| `feat` | minor |
| `!` after the scope, e.g. `feat(cli)!: ...` | major (minor while on 0.x) |
| anything else | none |

Breaking means: a CLI flag or argument removed or renamed, an exit code
changed, or a JSON key removed, renamed or retyped (also bump
`schema_version`). New JSON keys and new flags are not breaking.
Human-readable output can change freely.

The version in `Cargo.toml`, `CHANGELOG.md` and the tags come only from
`cog bump`: `cog bump --auto` for a release, `cog bump --auto --pre "rc.*"`
for a release candidate. Releases happen when the user asks for one.
