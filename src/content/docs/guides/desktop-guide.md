---
title: Desktop Guide
description: How to install and use the Utsuwa desktop application with overlay mode.
---

# Desktop Guide

Utsuwa Desktop is an application that brings your AI companion to your desktop with a transparent overlay mode. Your companion can float over other applications, always visible while you work.

Available for **macOS**, **Windows**, and **Linux**.

## Installation

### Download

Head to the [GitHub Releases](https://github.com/JuiceBoxxGames/utsuwa/releases) page and grab the build for your platform:

| Platform | File | Install |
|----------|------|---------|
| **macOS** | `.dmg` (universal) | Open the disk image and drag Utsuwa to your Applications folder |
| **Windows** | `.exe` | Run the installer |
| **Linux** | `.AppImage` | `chmod +x` the file and run it |
| **Linux** | `.deb` / `.rpm` | Install with your package manager |

#### Opening an unsigned build

The desktop app is in beta and currently **unsigned**, so your OS will warn you the first time you open it. This is expected.

- **macOS:** right-click the app → **Open** → **Open**. Or run `xattr -dr com.apple.quarantine /Applications/Utsuwa.app` once.
- **Windows:** on the SmartScreen prompt, click **More info** → **Run anyway**.
- **Linux:** AppImages just need the executable bit (`chmod +x Utsuwa.AppImage`).

### Building from Source

If you prefer to build it yourself:

#### Prerequisites

- Node.js 22+
- [Rust toolchain](https://rustup.rs/) (for Tauri)
- pnpm

```bash
# Clone the repo
git clone https://github.com/JuiceBoxxGames/utsuwa.git
cd utsuwa

# Install dependencies
pnpm install

# Run in development mode
pnpm tauri dev

# Or build a release binary
pnpm tauri build
```

The dev command launches both a development server and the desktop window. The build command produces an installer for your current platform in `src-tauri/target/release/bundle/`.

## Remote control (MCP)

The desktop app hosts a [Model Context Protocol](https://modelcontextprotocol.io) server on loopback so another local agent (for example [Grok Build](https://docs.x.ai)) can make the companion speak without going through her chat LLM.

- **URL:** `http://127.0.0.1:8787/mcp`
- **Bind:** `127.0.0.1` only. Override with `UTSUWA_MCP_BIND` (for example `127.0.0.1:9876`).
- **Name:** each connecting client is a session. The display name defaults to this machine (`shizuku.local` → **Shizuku**). Override with `UTSUWA_MCP_NAME` or the `X-Utsuwa-Name` header.
- **Topic:** a 1–7 word description of what that terminal is working on. Call `set_session` with it so two Grok sessions on the same computer can be told apart.
- **Tools:** `set_session`, `take_user_message`, `speak` (payload only; optional `language`, optional `plain` to skip the template), `stop_speech`, `get_status`
- **Instructions:** on connect, clients always get hardcoded usage rules (poll `take_user_message`, payload-only `speak`, no dumps). A separate **Preferences** box under **Settings > Chat (LLM) > MCP notifications** overlays tone and frequency.
- The app must be open. If the overlay is visible, speech plays there; otherwise it plays in the main window.

### MCP Mode (chat bar → client)

Under **Settings > Chat (LLM)**, pick **MCP Mode** instead of a normal provider. The chat bar then shows a session dropdown (name + smaller topic). Lines you send are queued for that session; the client should call `take_user_message` and treat `prompt` as the user prompt.

When the client answers, call `speak` with **only the spoken payload** in `text` (one or two sentences). Phrasing comes from the MCP preferences prompt, not a separate template.

Several clients can stay connected. Idle sessions drop after 10 minutes.

Grok Build (`~/.grok/config.toml`):

```
[mcp_servers.utsuwa]
url = "http://127.0.0.1:8787/mcp"
enabled = true
```

Then `/mcps` → refresh, or restart Grok. Anyone on this machine can drive the avatar while the app is running; do not expose the port beyond loopback. Topic is per terminal — set it with `set_session`, not a shared header.

## Updating

The desktop app keeps itself up to date. On launch it quietly checks for a new release, and when one is available a small banner appears offering to **Install & Restart** — click it and the app downloads the update, installs it, and relaunches.

You can also check manually any time from the **About** dialog (the info button in the app) via **Check for updates**.

> Auto-updates work for the macOS `.dmg`, the Windows `.exe`, and the Linux `.AppImage`. If you installed via `.deb` or `.rpm`, update through your package manager instead.

## Features

### Main Window

The main window provides the full Utsuwa experience — same as the web version with all features:

- VRM avatar with animations
- Chat interface
- Settings and configuration
- Memory and relationship systems

A blue **monitor icon** in the top-right corner launches overlay mode.

### Overlay Mode

Overlay mode detaches your companion into a transparent, always-on-top window:

- **Transparent Background**: Only the character is visible; everything else is see-through
- **Always on Top**: The companion stays visible over all other windows
- **Draggable**: Click and drag anywhere on the character to reposition
- **Floating Chat**: Click the chat icon at the bottom to expand a chat input
- **Speech Bubbles**: Responses appear in a docked dialog bubble above the bottom controls (the window moves around, so a head-tracking bubble would be unreadable)
- **Status Indicator**: The mood/relationship status pill appears above the chat icon
- **Resizable**: Drag the top-left corner tab to resize the overlay; the size is remembered across launches
- **Lockable**: The lock button in the hover controls pins the overlay in place so clicks cannot drag it
- **Overlay Camera**: The hover controls include a camera panel with zoom, height, and field-of-view sliders independent from the main window's framing

#### Controls

| Action | How |
|--------|-----|
| Move character | Click and drag on the character |
| Open chat | Click the chat icon at the bottom |
| Send message | Type and press Enter |
| Close chat | Send a message (auto-collapses) |
| Exit overlay | Click the X button in the top-right corner |
| Push-to-talk | `Ctrl+Shift+Space` (global hotkey) |
| Toggle overlay | `Ctrl+Shift+U` (global hotkey) |
| Focus chat | `Ctrl+Shift+C` (global hotkey) |

### Switching Between Modes

- **Main → Overlay**: Click the blue monitor icon in the top-right
- **Overlay → Main**: Click the X button in the overlay's top-right corner

Both windows share the same data — your conversation, memories, and relationship state persist across modes.

## Known Limitations

Some features are still being worked on:

| Feature | Status |
|---------|--------|
| macOS support | ✅ Available |
| Windows support | ✅ Available |
| Linux support | ✅ Available |
| Click-through transparency | ❌ Disabled (blocks UI) |
| Global hotkeys | ✅ Available |
| In-app auto-updates | ✅ Available |
| Size and lock persistence | ✅ Available (window position across relaunch still planned) |
| System tray | ⏳ Planned |

## Troubleshooting

### App won't start

If you built from source, make sure Rust is installed:

```bash
rustc --version
```

If not installed, run:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

If you downloaded a release binary and it won't launch, try downloading it again or check the [GitHub Issues](https://github.com/JuiceBoxxGames/utsuwa/issues) page.

### Overlay background not transparent

This can happen if the renderer isn't properly configured. Try:

1. Exit and relaunch the app
2. Make sure you're on the latest version from [Releases](https://github.com/JuiceBoxxGames/utsuwa/releases)

### Character facing wrong direction

The camera is locked in overlay mode. If the character appears rotated, exit overlay and re-enter.

### Voice input not working

The desktop app uses Tauri's webview, which does not support the browser's Web Speech API. For voice input on desktop, configure a local Whisper server, a Groq API key, or an OpenAI API key in **Settings > Character** under the Voice Input (STT) section.

### Can't interact with overlay UI

The X button and chat icon should always be clickable. If they're not responding, the window may have lost focus — click anywhere on the overlay first.

## Technical Details

The desktop app uses:

- **Tauri v2** — Rust-based framework for desktop apps
- **Same SvelteKit codebase** — No fork, shared components
- **Platform detection** — `isTauri()` checks for Tauri environment
- **Multi-window** — Main window + overlay window managed separately

For architecture details, see [Architecture Overview](/docs/technology/architecture).
