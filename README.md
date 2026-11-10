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
- **Debugger (DAP)** — breakpoints, stepping, variables tree, call stack, watches and a debug console. Works with CodeLLDB, lldb-dap and gdb for Rust/C/C++, debugpy for Python and Delve for Go. Missing adapters are downloaded on first use, Rust projects are built automatically and `.vscode/launch.json` is supported.
- **Find and replace** — inline find bar with live highlighting, match counter, case/whole-word/regex options, capture-group replacement and single-step undo for replace-all.
- **Navigation** — document symbols, workspace symbol search, references, jump list.
- **Workspace** — file tree, tabs, folder picker, recent workspaces, shell command integration (`:!cargo build`).
- **System clipboard** — yank, delete and paste are synced with the OS clipboard on Windows, macOS, X11 and Wayland.
- **Syntax highlighting** for Rust, C/C++, Python, JavaScript/TypeScript, Go, Shell, Lua, TOML/YAML and JSON.
- **AI assistant** — side panel with streaming chat for OpenAI-compatible APIs (OpenAI, OpenRouter, Groq, DeepSeek, Mistral, Ollama, LM Studio), Anthropic and Google Gemini. File or selection context, one-key code application, persistent chat history.
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
| `Ctrl+Z` / `Ctrl+Y` | undo / redo (also `u` / `Ctrl+R`) |
| `Ctrl+F` / `Ctrl+H` | find / find and replace |

### LSP

| Key | Action |
|---|---|
| `Tab` / `Enter` | accept completion |
| `Ctrl+Space` | trigger completion |
| `K` | hover documentation |
| `gd`, `F12`, `Ctrl+Click` | go to definition |
| `Ctrl+O` | jump back |
| `gr`, `Shift+F12` | references |
| `gs`, `Ctrl+Shift+O` | document symbols |
| `gS`, `Ctrl+T` | workspace symbols |
| `ga`, `Ctrl+.` | code actions |
| `]d` / `[d` | next / previous diagnostic |
| `F2` | rename symbol |
| `:format` | format document |
| `:diag` | list diagnostics |
| `:lsp`, `:lsp restart` | server status / restart |

### Debugging

| Key | Action |
|---|---|
| `F5` | start / continue |
| `Shift+F5` / `Ctrl+Shift+F5` | stop / restart |
| `F9` or click the gutter | toggle breakpoint |
| `F10` / `F11` / `Shift+F11` | step over / into / out |
| `F6` | pause |
| `K` | evaluate the expression under the cursor while paused |

### AI panel

| Key | Action |
|---|---|
| `Ctrl+L` | toggle the panel (attaches the visual selection, if any) |
| `Enter` / `Ctrl+J` | send / newline |
| `Ctrl+T` | toggle file context |
| `Ctrl+N` / `Ctrl+H` | new chat / chat history |
| `Ctrl+C` | stop streaming |

## Debugger

zetsu speaks the Debug Adapter Protocol. Open a file and press `F5`; no manual adapter setup is required. When no suitable adapter is found, zetsu downloads one into its data directory the first time and reuses it afterwards:

| Language | Adapter | Provisioning |
|---|---|---|
| Rust, C, C++ | CodeLLDB | latest release from [vadimcn/codelldb](https://github.com/vadimcn/codelldb/releases) for the current OS and CPU (~55 MB download) |
| Python | debugpy | universal wheel from PyPI, loaded via `PYTHONPATH`; neither `pip` nor a global install is needed |
| Go | Delve | built with `go install` into the data directory |

Adapters already on the system are used first: `codelldb`, `lldb-dap` or `gdb` 14+ on `PATH`, a Python environment that can import `debugpy`, or `dlv`. If a download fails (offline, blocked network), zetsu falls back to `lldb-dap` or `gdb` when available and reports the error in the debug console.

Python 3.8+ must be installed for Python debugging, and Go for Go debugging, since those are the runtimes being debugged.

Downloads honour `HTTPS_PROXY`/`HTTP_PROXY`/`NO_PROXY` and the operating system's certificate store.

| Command | |
|---|---|
| `:install` | show which tools are installed |
| `:install <codelldb\|debugpy\|delve>` | install or update a tool |
| `:uninstall <name>` | remove a downloaded tool |

Tools are stored in `%LOCALAPPDATA%\zetsu\tools` on Windows and `$XDG_DATA_HOME/zetsu/tools` (default `~/.local/share/zetsu/tools`) on Linux and macOS.

For Rust, zetsu runs `cargo build` and debugs the produced binary. For C/C++, pass the program with `:debug ./a.out [args]`. Program arguments can be set with `:debug args ...`.

If the workspace contains `.vscode/launch.json`, the first `launch` configuration with a supported type (`lldb`, `lldb-dap`, `cppdbg`, `gdb`, `debugpy`, `python`, `go`) is used. Comments, trailing commas and the common `${workspaceFolder}`/`${file}` variables are supported, as is CodeLLDB's `cargo` field.

CodeLLDB gives the best Rust experience: it pretty-prints `Vec`, `String`, `HashMap` and other standard types. In the debug console expressions are evaluated directly; prefix a line with `` ` `` to run a raw LLDB command (`` `frame info ``).

## AI assistant

The panel talks to the provider directly from your machine using your own API key. Nothing is proxied.

```
:ai provider openai|anthropic|gemini
:ai key <API_KEY>
:ai model                 # fetches the model list from the provider; type to filter
```

Any OpenAI-compatible endpoint can be used by changing the base URL:

```
:ai url https://openrouter.ai/api/v1
:ai url https://api.groq.com/openai/v1
:ai url http://localhost:11434/v1        # Ollama, no key required
```

Keys can also be supplied through `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, `ANTHROPIC_API_KEY` and `GEMINI_API_KEY`. Keys set with `:ai key` are stored in plain text in the config file.

When file context is enabled the active buffer is sent with the first message and again only after it changes. A visual selection captured with `Ctrl+L` is sent instead of the whole file, and **apply** on a reply replaces exactly those lines; otherwise the code block is inserted at the cursor. Every apply is a single undo step.

Chats are saved as JSON under `<config dir>/chats/` and the most recent one is restored on startup.

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
| `ai_provider` | `:ai provider` | active AI provider |
| `ai_key_<provider>` | `:ai key` | API key per provider |
| `ai_model_<provider>` | `:ai model` | model per provider |
| `ai_url_<provider>` | `:ai url` | base URL per provider |

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
  ai.rs          AI providers, SSE streaming, chat storage
  ai_ui.rs       AI panel
  find.rs        find and replace bar, search matcher
  dap.rs         Debug Adapter Protocol client, adapter discovery, launch configuration
  debug_ui.rs    debugger session, breakpoints and the debug panel
  tools.rs       debug adapter download and installation
  discord.rs     Discord IPC client
  i18n.rs        localization
  config.rs      settings file
```

## Contributing

Contributions are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for the development setup, coding guidelines and the pull request process.

## License

[MIT](LICENSE)
