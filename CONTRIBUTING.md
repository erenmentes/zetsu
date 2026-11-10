# Contributing to zetsu

Thanks for your interest in improving zetsu. This document describes how to set up the project, the conventions the codebase follows and how changes get merged.

## Getting started

1. Fork the repository and clone your fork.
2. Install a stable Rust toolchain from [rustup.rs](https://rustup.rs).
3. Build and run:

   ```sh
   cargo run -- path/to/some/project
   ```

4. Create a branch for your change:

   ```sh
   git checkout -b fix/completion-popup-position
   ```

   Use a short, descriptive name prefixed with `fix/`, `feat/`, `docs/` or `refactor/`.

## Before opening a pull request

CI runs the following checks on every pull request. Run them locally first:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

`cargo fmt` fixes formatting automatically. Clippy warnings are treated as errors.

## Code guidelines

- Keep the editor fast. Avoid work proportional to file size on every keystroke, and never block the main loop on I/O; long-running work (LSP, HTTP, IPC) belongs on a background thread that reports back through a channel.
- Every user-facing string must be translated. Use `t!("Türkçe", "English")` for static text and `tf!("… {}", "… {}", args)` for formatted text.
- Keep dependencies to a minimum and avoid crates that need system libraries; the release binaries are fully static on Linux.
- The codebase does not use inline comments. Prefer clear names and small functions.
- New keybindings should be documented in both `src/help_en.txt` and `src/help_tr.txt`, and in the README when relevant.

## Project structure

| Path | Purpose |
|---|---|
| `src/main.rs` | terminal setup and event loop |
| `src/editor.rs` | modes, key and mouse handling, commands |
| `src/ide.rs` | LSP features: completion, diagnostics, code actions, menus |
| `src/lsp.rs` | LSP client and JSON-RPC transport |
| `src/ai.rs`, `src/ai_ui.rs` | AI providers and the AI panel |
| `src/find.rs` | find and replace bar, search matcher |
| `src/dap.rs`, `src/debug_ui.rs` | debugger: DAP client and the debug panel |
| `src/buffer.rs` | text buffer, undo/redo, motions |
| `src/render.rs`, `src/screen.rs` | drawing and the diff-based screen buffer |
| `src/highlight.rs` | syntax highlighting |

## Pull requests

- Keep each pull request focused on a single change. Unrelated fixes should go in separate pull requests.
- Describe what changed and why. Include a screenshot or recording for visual changes.
- Link the related issue with `Closes #123` if there is one.
- Pull requests are merged with **squash and merge**, so the commit history on your branch does not need to be tidy.

## Reporting bugs and requesting features

Use the issue templates. For bugs, include your operating system, terminal emulator, zetsu version (`zetsu --version`) and the steps to reproduce. For language server problems, attach the log produced with `ZETSU_LSP_LOG=lsp.log`.

Issues labeled **good first issue** are a good place to start.

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
