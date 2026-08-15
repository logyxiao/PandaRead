# Novalyte

<p align="center">
  <strong>A local-first fiction library and immersive reader</strong>
</p>

<p align="center">
  <a href="README.md">简体中文</a> ｜ English
</p>

Novalyte is a Tauri-based desktop application for managing, reading, and editing fiction stored in local folders. Your manuscripts stay on your device: no account and no upload are required.

## Features

- **Local libraries**: Add multiple folders and keep their file trees indexed automatically.
- **Supported reading formats**: TXT, EPUB, and Markdown files named exactly `正文.md`.
- **Single- and two-column reading**: Switch between continuous scrolling and paginated columns with the same settings and toolbar.
- **Markdown reading and editing**: Render `正文.md` as Markdown while reading; edit the original source with autosave.
- **Manual classification**: Organize documents as Favorite, Pending, or Dropped across desktop and mobile views.
- **Tree and card views**: Use a folder tree or preview cards in the desktop sidebar; mobile uses a flat card list.
- **Reader preferences**: Preview theme, font, size, line height, page width, margins, and brightness in real time.
- **Night theme**: A neutral ink-gray background with warm gray text for long reading sessions.
- **Reading progress**: Automatically restore the last reading position for each document.
- **Tags, annotations, and clips**: Tag documents and save selected passages as annotations or reusable material.
- **QR mobile reading**: Read over the local network or an optional cloudflared tunnel, with two-way document-follow prompts.
- **Word export**: Convert TXT or `正文.md` directly to DOCX in the source directory.
- **File operations**: Refresh, rename, create, move, and send files or folders to the system trash.

## Library Inclusion Rules

Novalyte indexes:

- `*.txt`
- `*.epub`
- Markdown files named exactly `正文.md`

Other Markdown files such as `设定.md`, `大纲.md`, and `README.md` are intentionally excluded from the reading list.

## Word Export Formatting

The Word conversion button in the reader toolbar creates `{novel-title}.docx` beside the source file:

- Body font: SimSun (`宋体`)
- Body size: 12 pt (`小四`)
- Line spacing: 1.5
- First-line indent: 2 characters
- A centered, bold novel title at the top
- Chapter markers such as `###1.` and `###2` become `第一章` and `第二章`
- For `正文.md` or `正文.txt`, the parent folder name is used as the novel title

An existing DOCX with the same name is overwritten. Export fails with a visible error if the file is locked by Word or another application.

## Keyboard Controls

| Key | Single column | Two columns |
| --- | --- | --- |
| `←` / `→` | Scroll one screen up / down | Previous / next page |
| `↑` / `↓` | Scroll one line | - |
| `PgUp` / `PgDn` | Previous / next document | Previous / next document |
| `⌘K` / `Ctrl+K` | Focus quick search | Focus quick search |

## Mobile QR Reading

1. Click the phone icon in the desktop app.
2. Start the mobile reading service.
3. Connect the phone and computer to the same Wi-Fi network and scan the QR code.
4. Choose a library and document on the phone.

Reading progress and manual classifications are synchronized. When either device opens another document, the other side shows an optional jump prompt.

Public access requires the optional cloudflared tunnel and the pairing code displayed by the desktop app. Sessions are held in memory; devices must scan again after the service is stopped.

## Installation

Download the installer for your platform from GitHub Actions artifacts or project Releases:

- macOS Apple Silicon (arm64) DMG
- macOS Intel (x64) DMG
- Windows x64 NSIS installer

Unsigned macOS builds may need to be allowed manually under System Settings → Privacy & Security on first launch.

## Development

### Requirements

- Node.js LTS
- npm
- Rust stable
- Platform dependencies required by Tauri 2

### Install Dependencies

```bash
npm ci
```

### Start the Desktop App

```bash
npm run dev
```

The Vite development server listens on:

```text
http://127.0.0.1:15620
```

### Start Only the Web Frontend

```bash
npm run dev:web
```

### Build the Frontend

```bash
npm run build:web-only
```

### Build Desktop Installers

```bash
npm run tauri build
```

### Checks and Tests

```bash
npx tsc --noEmit
npm run build:web-only
cd src-tauri && cargo check && cargo test
```

## Technology

- Tauri 2
- Rust
- React 19
- TypeScript
- Vite
- Zustand
- SQLite / rusqlite
- CodeMirror
- marked + DOMPurify

## Data and Privacy

- Manuscripts remain in the local folders selected by the user.
- The local SQLite database stores indexes, settings, tags, annotations, clips, and reading progress.
- Novalyte does not require an account and does not automatically upload library content.
- The reading server is exposed publicly only when the user explicitly starts the cloudflared tunnel.

## Project Status

Novalyte is under active development. Keep independent backups of important manuscripts and verify critical reading and export workflows after upgrades.
