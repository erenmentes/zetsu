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
- **Git** — change markers in the gutter, inline blame, diff views, staging and reverting individual changes, a Source Control tab with commit, AI commit messages, branches, push/pull/fetch and history, and merge conflict resolution.
- **Live flowchart** — control-flow diagram of the function under the cursor, generated with tree-sitter. Follows the debugger as you step, marks breakpoints, and zooms semantically from a single function out to the whole file and project, with a braille minimap at low zoom levels. Optional AI summaries explain what each function does.
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

Requires a stable Rust toolchain ([rustup.rs](https://rustup.rs)) and a C compiler for the tree-sitter grammars: the Visual Studio Build Tools with the C++ workload on Windows (already needed by the MSVC Rust toolchain), `gcc` or `clang` on Linux (`build-essential` on Debian/Ubuntu), and the Xcode Command Line Tools on macOS (`xcode-select --install`).

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

### Git

| Key | Action |
|---|---|
| `Alt+G` | open the Git tab in the sidebar |
| `]c` / `[c` | next / previous change or conflict |
| click a gutter marker | stage, revert or diff that change |
| `:git blame`, `:git diff` | toggle inline blame, diff of the current file |
| `:branch <name>` | switch to or create a branch |

### Flowchart

| Key | Action |
|---|---|
| `Ctrl+G` | toggle the flowchart panel |
| `+` / `-`, `Alt+wheel` | zoom in / out (function → file → project) |
| drag, wheel, `hjkl` | pan |
| click / right-click | go to line / toggle breakpoint |
| `Tab` / `Enter` | select next node / open it |
| `0` | fit to panel |
| `AI` button | toggle AI function summaries |

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

## Git

zetsu drives the `git` executable on your `PATH`, so your configuration, credentials, SSH keys and hooks apply unchanged. All git commands run in the background; the editor never waits on them.

**In the editor.** The gutter marks lines that differ from the index: green for added, blue for modified, red for deleted. `]c` / `[c` jump between changes. Clicking a marker (or right-clicking the line) offers *Stage change*, *Revert change* and *Show file diff*: staging applies only that hunk to the index, reverting restores the indexed text in the buffer and can be undone. The cursor line shows its author, age and commit subject as dim inline text (`:git blame off` to hide it). Diffs open as separate buffers with `+`/`-` coloring; `Enter` on a diff line jumps to that line in the file.

**Git tab.** `Alt+G` switches the sidebar to the Git tab: current branch with ahead/behind counts, a commit message box, and buttons for commit, AI message, push, pull and fetch. Below are conflicted, staged and changed files and the recent history. `s` stages or unstages the selected file, `Enter` opens its diff (or the selected commit), `d` twice discards changes. Committing with nothing staged stages all changes first. Push sets the upstream automatically the first time. `b` opens the branch picker, which can also create a branch.

**AI commit messages.** `a` (or the *AI* button) sends the staged diff, or the working diff if nothing is staged, together with recent commit subjects to the model selected in the AI panel, and fills the message box with a suggested message in the style of the existing history.

**Merge conflicts.** Conflict blocks are tinted (ours green, theirs blue) and the `<<<<<<<` line carries *Accept ours*, *Accept theirs* and *Accept both* buttons; `:conflict ours|theirs|both` does the same from the keyboard and `]c` moves between conflicts. After resolving, save and stage the file; the Git tab shows `MERGE` until the merge commit is made. Files in the tree are colored by status.

## Flowchart

`Ctrl+G` opens a panel that draws the control flow of the function under the cursor: statements, `if`/`else` chains, loops with their back edges, `match`/`switch` arms, `try`/`except`, and early `return`/`break`/`throw` exits. Consecutive simple statements are folded into one node. Functions are parsed with [tree-sitter](https://tree-sitter.github.io) for Rust, Python, Go, C, C++, JavaScript and TypeScript, and the diagram is rebuilt as you type.

The node containing the cursor is highlighted. While the debugger is paused the panel follows the execution line instead: it switches to the function being executed, highlights the current node and scrolls it into view. Breakpoints are shown on their nodes; right-click a node to toggle one, click it to jump to its line.

Zoom is semantic. At 100% nodes are drawn with box characters; zooming out switches to a braille rendering at 50%, 25% and 12.5% that shows the shape of large functions. Zooming out further shows every function in the file with its size and branch count, then every source file in the project. Zooming in, or clicking an item, goes back down. Use `+`/`-`, `Alt`+wheel (`Ctrl`+wheel where the terminal forwards it) or the buttons in the panel header; the breadcrumb in the header is clickable.

### AI explanations

The **AI** button in the top-right corner of the flowchart panel turns on short explanations. Each function gets a one-sentence summary of what it does and what it returns or changes: shown under the header in the function view and inside every function box in the file view (hover a box for the full sentence). Summaries are written in the current UI language (`:lang`).

Explanations use the provider and model selected in the AI panel. All functions of a file are explained in a single request, sent once the file has been idle for about a second. Results are cached in `flow_ai.json` next to the config file, keyed by each function's source, so only new or edited functions are sent again. The setting is off by default and remembered (`flow_ai` in the config file).

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
| `discord_client_id` | `:discord id <id>` | custom Discord application ID (optional) |
| `ai_provider` | `:ai provider` | active AI provider |
| `ai_key_<provider>` | `:ai key` | API key per provider |
| `ai_model_<provider>` | `:ai model` | model per provider |
| `ai_url_<provider>` | `:ai url` | base URL per provider |

## Discord Rich Presence

Rich Presence works out of the box: while the Discord desktop app is running, your profile shows **Zetsu IDE** with the active file, the workspace name and the session duration. Nothing needs to be installed or configured; zetsu talks to Discord over its local IPC socket (Windows, macOS and Linux, including Flatpak and Snap builds of Discord).

If it does not appear, make sure *Settings → Activity Privacy → Share your detected activities with others* is enabled in Discord.

| Command | |
|---|---|
| `:discord` | show the connection status |
| `:discord off` / `:discord on` | disable / enable Rich Presence |
| `:discord id <ID>` | use your own Discord application instead (its art asset must be named `zetsu`) |
| `:discord id default` | return to the built-in application |

## Building a release

Releases are produced by GitHub Actions (`.github/workflows/build.yml`). Every push to `main` builds and tests all targets; pushing a version tag also publishes a GitHub Release with the binaries attached:

```sh
git tag v0.3.1
git push origin v0.3.1
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
  flow.rs        tree-sitter control-flow extraction, flowchart layout and rasterizer
  flow_ui.rs     flowchart panel: semantic zoom, braille rendering, debugger sync
  git.rs         git CLI runner, status parsing, line diffs, hunk patches, blame, conflicts
  git_ui.rs      Git sidebar tab, gutter markers, inline blame, diff buffers, conflict actions
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
