<p align="center">
  <img src="assets/zetsu-512.png" width="128" alt="zetsu">
</p>

<h1 align="center">zetsu</h1>

<p align="center">A modal terminal IDE written in Rust, with vim keybindings, full mouse support and built-in LSP.</p>

---

## Features

- **Modal editing** — Normal, Insert, Visual, Visual Line and Command modes with operator/motion composition (`d2w`, `cw`, `y3j`, `dt)`), counts and undo history.
- **Language Server Protocol** — completion (including auto-imports and snippet expansion), diagnostics, hover, go to definition, references, code actions with resolve support, signature help, rename and document formatting. Servers are discovered on `PATH` and started on demand.
- **Mouse** — click to place the cursor, drag to select, double/triple click for word/line selection, scroll, tab switching, context menu, Ctrl+click to jump to a definition, resizable file tree.
- **Workspace** — file tree, tabs, folder picker, recent workspaces, shell command integration (`:!cargo build`).
- **System clipboard** — yank, delete and paste are synced with the OS clipboard on Windows, macOS, X11 and Wayland.
- **Syntax highlighting** for Rust, C/C++, Python, JavaScript/TypeScript, Go, Shell, Lua, TOML/YAML and JSON.
- **Discord Rich Presence** over local IPC.
- **Localization** — English and Turkish UI, switchable at runtime (`:lang en`, `:lang tr`).
- **Rendering** — double-buffered, diff-based redraw; only changed rows are written to the terminal. Truecolor with automatic 256-color fallback.

## Installation

### Windows

```powershell
irm https://raw.githubusercontent.com/erenmentes/zetsu/main/install.ps1 | iex
```

Installs to `%LOCALAPPDATA%\Programs\zetsu`, adds it to the user `PATH` and creates Desktop and Start Menu shortcuts. Windows Terminal is used when available.

### Linux / macOS

```sh
curl -fsSL https://raw.githubusercontent.com/erenmentes/zetsu/main/install.sh | sh
```

Installs the binary to `~/.local/bin`.

- **Linux:** registers a `.desktop` entry with the application icon and places a launcher on the desktop. The Linux binary is statically linked (musl) and has no runtime dependencies.
- **macOS:** creates `~/Applications/Zetsu IDE.app` with a desktop alias. The launcher opens iTerm2 when installed, otherwise Terminal.app.

### Prebuilt binaries

Binaries for every release are available on the [Releases](https://github.com/erenmentes/zetsu/releases) page:

| Platform | Asset |
|---|---|
| Windows x86_64 | `zetsu-windows-x86_64.exe` |
| Linux x86_64 | `zetsu-linux-x86_64` |
| macOS Apple Silicon | `zetsu-macos-arm64` |
| macOS Intel | `zetsu-macos-x86_64` |

### From source

Requires a stable Rust toolchain ([rustup.rs](https://rustup.rs)). On macOS the Xcode Command Line Tools are also required (`xcode-select --install`).

```sh
git clone https://github.com/erenmentes/zetsu
cd zetsu
cargo install --path .
```

## Usage

```sh
zetsu                 # start screen
zetsu path/to/dir     # open a workspace
zetsu src/main.rs     # open a file
```

Press `:help` inside the editor for the full keymap.

### Essential keys

| Key | Action |
|---|---|
| `i` / `Esc` | enter / leave Insert mode |
| `Ctrl+S` | save |
| `:q` | close tab (quits on the last tab) |
| `Ctrl+K` | folder picker |
| `Ctrl+B` | toggle file tree |
| `Ctrl+W` | switch focus between tree and editor |
| `gt` / `gT` | next / previous tab |
| `u` / `Ctrl+R` | undo / redo |

### LSP

| Key | Action |
|---|---|
| `Tab` / `Enter` | accept completion |
| `Ctrl+Space` | trigger completion |
| `K` | hover documentation |
| `gd`, `F12`, `Ctrl+Click` | go to definition |
| `Ctrl+O` | jump back |
| `gr` | references |
| `ga` | code actions |
| `]d` / `[d` | next / previous diagnostic |
| `F2` | rename symbol |
| `:format` | format document |
| `:diag` | list diagnostics |
| `:lsp`, `:lsp restart` | server status / restart |

## Language servers

zetsu looks up the following executables on `PATH`:

| Language | Server |
|---|---|
| Rust | `rust-analyzer` |
| Python | `pyright-langserver`, `basedpyright-langserver`, `pylsp` |
| JavaScript / TypeScript | `typescript-language-server` |
| C / C++ | `clangd` |
| Go | `gopls` |
| Lua | `lua-language-server` |
| Shell | `bash-language-server` |

The workspace root is resolved from project markers (`Cargo.toml`, `package.json`, `go.mod`, `pyproject.toml`, `compile_commands.json`, `.git`). When no server is available, completion falls back to words from open buffers.

Set `ZETSU_LSP_LOG=<file>` to log all JSON-RPC traffic.

## Configuration

Settings are persisted by the editor itself:

| Platform | Path |
|---|---|
| Windows | `%APPDATA%\zetsu\config` |
| Linux / macOS | `$XDG_CONFIG_HOME/zetsu/config` or `~/.config/zetsu/config` |

| Key | Command | Description |
|---|---|---|
| `lang` | `:lang en\|tr` | interface language |
| `recent` | — | recent workspaces |
| `discord` | `:discord on\|off` | Rich Presence toggle |
| `discord_client_id` | `:discord id <id>` | Discord application ID |

## Discord Rich Presence

1. Create an application at the [Discord Developer Portal](https://discord.com/developers/applications) named `Zetsu IDE`.
2. Under **Rich Presence → Art Assets**, upload `assets/zetsu-1024.png` with the key `zetsu`.
3. Run `:discord id <APPLICATION_ID>` in zetsu.

The presence shows the active file, the workspace name and the session duration.

## Building a release

Releases are produced by GitHub Actions (`.github/workflows/build.yml`). Every push to `main` builds and tests all targets; pushing a version tag also publishes a GitHub Release with the binaries attached:

```sh
git tag v0.1.0
git push origin v0.1.0
```

## Project layout

```
src/
  main.rs        terminal setup and event loop
  editor.rs      modes, key and mouse handling, commands
  ide.rs         completion, diagnostics, code actions, menus
  lsp.rs         LSP client: process management, JSON-RPC, position encoding
  buffer.rs      text buffer, undo/redo, motions
  render.rs      UI layout and theme
  screen.rs      diff-based screen buffer
  highlight.rs   syntax highlighting
  tree.rs        file tree
  discord.rs     Discord IPC client
  i18n.rs        localization
  config.rs      settings file
```
