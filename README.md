# 🐹 hamster

> *A tiny rodent helps you find email addresses. And then accidentally replaced your entire mail indexer.*

**hamster** started as a simple idea: "What if `notmuch address` was faster and could forgive my typos?" Somewhere along the way, it gained a full‑text search engine, a tagging system, folder‑to‑tag reconciliation, a three‑pane terminal UI, and a concerning amount of personality. It is now a complete, standalone replacement for Notmuch — written in pure Rust, powered by Tantivy, and operated by a very hardworking hamster.

---

## ✨ Features

- **⚡ Incremental Indexing** – `hamster index` scans your Maildir, but only processes new or changed messages after the first run. Subsequent runs are lightning fast. The index notebook (`hamster_meta.json`) tracks every file’s modification time and flags independently – the hamster never re‑chews unchanged seeds.
- **🖥️ Three‑Pane Terminal UI** – `hamster tui` is a calm, keyboard‑driven thinking space. Live search, an interactive filter tree (mailbox → tags), a preview pane, and a footer that teaches you the keys as you go. Every action lives behind a `Ctrl` chord so the search box stays always live. Press `?` for context‑sensitive help.
- **🏷️ Tagging (in‑TUI & CLI)** – Add or remove tags with `+inbox -unread` syntax, just like Notmuch. The TUI lets you tag, archive, toggle read/unread, and copy message‑IDs without ever leaving the keyboard. All tag operations use Tantivy’s stored fields – zero file I/O, zero MIME re‑parsing.
- **📁 Folder‑Based Tagging** – Turn your old Maildir folder tree into a clean set of notmuch‑style tags. Interactive wizards, bulk assignment, inheritance, declarative reconciliation (`hamster folder sync`), and structure repair (`hamster folder structure`).
- **🚩 Maildir Flag Sync** – Reads standard Maildir flags (`S`, `R`, `F`, …) and synchronises them with tags (`unread`, `replied`, `flagged`). Integrated into the folder sync, respecting your folder rules.
- **🔍 Full‑Text Search** – Search across `from`, `to`, `subject`, `body`, and `tags` with Tantivy's query language. Results are pretty, colorful, and sorted by date. The query parser catches mistakes before they ruin your search.
- **🐹 Fuzzy Address Lookup** – The feature that started it all. `hamster address` finds email addresses from a tiny, automatically‑maintained address book (`hamster_addresses.json`), ranks them by frequency and fuzzy match, and outputs in `mutt` or `aerc` format. Typos are forgiven. Instant, even on 100k+ mailboxes.
- **🧠 Explainability** – Understand why a message got its tags (`hamster folder explain` or `Ctrl‑x` in the TUI). It shows folder rules, inherited tags, flag contributions, managed tags, and what would change.
- **😂 A Hamster on a Wheel** – The progress bar features a tiny rodent doing warm‑up stretches, sniffing email wheels, and questioning its life choices. It's the emotional support you didn't know you needed.

---

## 📦 Installation

### From Source

```bash
git clone https://github.com/micdzu/hamster.git
cd hamster
cargo build --release
```

The binary will be at `./target/release/hamster`. Copy it somewhere in your `$PATH`.

### Dependencies

Only a C++ toolchain for Tantivy (no external C libraries required!):

| OS | Command |
|----|---------|
| Debian/Ubuntu | `sudo apt install build-essential` |
| Fedora | `sudo dnf install gcc` |
| Arch Linux | `sudo pacman -S base-devel` |
| macOS | `xcode-select --install` |

That's it. No `libnotmuch`, no Xapian, no GMime. Pure Rust.

---

## 🚀 Quick Start

### 1. Setup

Tell the hamster who you are, where your mail lives, and optionally map your folders to tags right away.

```bash
hamster setup
```

This creates `~/.hamster.toml`. During the interactive wizard you can assign tags to your Maildir folders (bulk or one‑by‑one), set flag sync, and configure managed tags. You can re‑run `hamster setup` at any time to tweak your rules — it preserves your existing configuration and offers a tweak menu.

### 2. Build the Index

Scan your Maildir and build the Tantivy index. The first run takes a minute or two (grab a coffee ☕), but subsequent runs are incremental and nearly instant.

```bash
hamster index
```

### 3. Sync Folder Tags

Apply your folder‑based rules to the index. This also honours Maildir flags if you enabled `sync_flags` during setup.

```bash
hamster folder sync
```

Run it again — if nothing changed, the hamster will nap. It’s idempotent.

### 4. Search for Messages

```bash
hamster search "from:boss and subject:urgent"
```

Add `--format json` for machine‑readable output.

### 5. Fuzzy Address Lookup (The Original Feature)

```bash
hamster address franca
```

Use `--format mutt` or `--format aerc` to integrate with your mail client.

### 6. Manage Tags Manually

```bash
hamster tag +important -spam "subject:newsletter"
```

---

## 🧙 Folder‑Based Tagging (the `hamster folder` family)

Hamster can turn your existing Maildir folder tree into a clean set of tags. No hand‑written config required.

### `hamster folder sync`

Applies your folder rules to the index. It compares desired tags (from rules + flags) with current tags and updates only what’s needed. Supports `--dry-run` and `--quiet`.

### `hamster folder structure`

Checks your rules against the current Maildir layout. It finds:

- **Orphaned rules** (no longer matching any folder)
- **Untagged folders** (new folders without a rule)

You can delete, update, or create rules interactively. Works with `--dry-run`.

### `hamster folder explain <path>`

Shows why a specific message gets its tags. Prints the matching folder, rules, flags, and the computed diff.

```bash
hamster folder explain /home/you/mail/.../cur/msg
```

---

## ⚙️ Configuration

Example `~/.hamster.toml`:

```toml
name = "Hamster User"
primary_email = "hamster@example.com"
maildir = "/home/you/mail"

[folder_tags]
enabled = true
sync_flags = true          # automatically sync read/unread from Maildir flags
managed_tags = []          # empty = infer from rules and flag sync

[[folder_tags.rules]]
pattern = "INBOX"
tags = ["inbox", "unread"]
inherit = false

[[folder_tags.rules]]
pattern = "Archive"
tags = ["archive"]
inherit = true
```

**Pattern matching is case‑insensitive** by default, so `"inbox"` matches both `INBOX` and `Inbox`. Glob patterns (`**`, `?`, `*`) are supported.

Tags prefixed with `-` force removal (e.g., `-inbox`).

---

## 📧 Integration with Email Clients

### Mutt

Add to `~/.muttrc`:

```
set query_command = "hamster address --format=mutt '%s'"
```

Press `Q` in the compose screen.

### Aerc

In `aerc.conf`:

```ini
[compose]
address-book-cmd = hamster address --format=aerc %s
```

---

## 🧽 Keeping Everything Fresh

Run periodically (or hook into your mail sync tool):

```bash
hamster index   # pick up new mail
hamster folder sync --quiet   # keep tags aligned
```

---

## 🖥️ The Terminal UI

`hamster tui` gives you a three‑pane live search cockpit:

- **Left pane** – interactive filter tree (mailboxes and their tags). Select a filter to instantly narrow your results.
- **Center pane** – message list (date‑sorted, unread indicator, truncated sender).
- **Right pane** – full‑message preview (HTML stripped to readable text).
- **Footer** – context‑sensitive key hints that change depending on which pane is focused.

All actions are behind `Ctrl` chords so the search box stays always active:

| Key | Action |
|-----|--------|
| `Ctrl‑j` / `Ctrl‑k` | Move up/down in message list |
| `Ctrl‑f` / `Ctrl‑b` | Page down/up |
| `Ctrl‑t` / `Ctrl‑r` | Add / remove tag |
| `Ctrl‑d` | Toggle read/unread |
| `Ctrl‑a` | Archive (tag‑based, no file moves) |
| `Ctrl‑y` | Copy message‑ID to status bar |
| `Ctrl‑o` | Open message in `$PAGER` |
| `Ctrl‑e` | Toggle preview pane |
| `Ctrl‑x` | Show tag explain panel |
| `Ctrl‑g` | Query syntax help |
| `Ctrl‑h` | Previous query history |
| `Tab` | Cycle focus between panes |
| `?` | Context‑sensitive help |
| `Esc` | Clear search / close overlays |

The TUI never reads emails from disk for tag changes — all metadata comes from Tantivy’s stored fields, making it fast even on enormous mailboxes.

---

## 🤔 Why Not Just Use `notmuch`?

`notmuch` is great! But `hamster` is for people who:

- Want a single, pure‑Rust binary with no external C dependencies.
- Want folder‑to‑tag migration **built in** — no extra scripts or tools.
- Make typos and want fuzzy address completion.
- Want a built‑in TUI that explains *why* every message has its tags.
- Enjoy watching a tiny rodent do data entry.

---

## 🙏 Acknowledgments

- **Adrian Perez** for the original `notmuch-addrlookup-c`.
- The **Notmuch** team for proving that email can be fun.
- The **Tantivy** and **mail-parser** teams for making pure‑Rust email indexing possible.
- **You**, for reading this far. May your inbox be manageable and your address completions swift.
- **The Hamster** 🐹, for its tireless dedication. It deserves a raise.

---

## 📜 License

MIT — because sharing is caring. See the [LICENSE](LICENSE) file for details.

---

**Made with 🦾, a healthy dose of sarcasm, a genuine love for email efficiency, and one very proud hamster.**
