# 🐹 hamster

> *A tiny rodent helps you find email addresses. And then accidentally replaced your entire mail indexer.*

**hamster** started as a simple idea: "What if `notmuch address` was faster and could forgive my typos?" Somewhere along the way, it gained a full‑text search engine, a tagging system, folder‑to‑tag migration, and a cozy terminal UI. Now it's a full Notmuch replacement — written in pure Rust, with zero C dependencies.

---

## ✨ Features

- **⚡ Incremental Indexing** – `hamster index` scans your Maildir, but only processes new or changed messages after the first run. Subsequent runs are lightning fast. The index lives in `~/.hamster_index` (a Tantivy database).
- **🖥️ Three‑Pane Terminal UI** – `hamster tui` is a calm, keyboard‑driven thinking space. Live search, an interactive filter tree (mailbox → tags), a preview pane, and a footer that tells you exactly which keys do what.
- **🏷️ Tagging (in‑TUI & CLI)** – Add or remove tags with `+inbox -unread` syntax, just like Notmuch. The TUI lets you tag, archive, toggle read/unread, and copy message‑IDs without ever leaving the search box.
- **📁 Folder‑Based Tagging** – Turn your old Maildir folder tree into a clean set of notmuch‑style tags. Interactive wizards, bulk assignment, inheritance, declarative reconciliation (`hamster folder structure`).
- **🚩 Maildir Flag Sync** – Reads standard Maildir flags (`S`, `R`, `F`, …) and synchronises them with tags (`unread`, `replied`, `flagged`). Integrated into the folder sync, respecting your manually‑assigned tags.
- **🔍 Full‑Text Search** – Search across `from`, `to`, `subject`, `body`, and `tags` with Tantivy's query language. Results are pretty, colorful, and sorted by date. The query parser catches typos gracefully.
- **🐹 Fuzzy Address Lookup** – The feature that started it all. `hamster address` finds email addresses from a tiny, automatically‑maintained address book (`hamster_addresses.json`), ranks by frequency, and forgives typos.
- **🧠 Explainability** – Understand why a message got its tags (`hamster folder explain` or `Ctrl‑x` in the TUI). It shows folder rules, inherited tags, flag contributions, managed tags, and the final diff.
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

This creates `~/.hamster.toml`. During the interactive wizard you can assign tags to your Maildir folders (bulk or one‑by‑one), set flag sync, and configure managed tags. You can re‑run `hamster setup` anytime to tweak settings.

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

Run it again — if nothing changed, the hamster will nap. It's idempotent.

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

Applies your folder rules to the index. It compares desired tags (from rules + flags) with current tags and updates only what's needed. Supports `--dry-run` and `--quiet`.

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

The TUI never reads emails from disk for tag changes — all metadata comes from Tantivy's stored fields, making it fast even on enormous mailboxes.

---

## 🤔 Why Not Just Use `notmuch`?

`notmuch` is great! But `hamster` is for people who:

- Want a single, pure‑Rust binary with no external C dependencies.
- Want folder‑to‑tag migration **built in** — no extra scripts or tools.
- Make typos and want fuzzy address completion.
- Want a built‑in TUI that explains *why* every message has its tags.
- Enjoy watching a tiny rodent do data entry.

---

## ⚠️ Known Limitations

- **Maildir only** – hamster works with local Maildir directories. No IMAP, POP3, or other remote protocols.
- **Single-user** – One hamster per system. Multi-user support is not planned.
- **TUI is keyboard-only** – Mouse support is not implemented and unlikely to be added.
- **No encryption** – The Tantivy index is stored as plaintext. Treat `~/.hamster_index` as sensitive (readable by all users on the system).
- **Tags are Tantivy-only** – Folder sync is one-way: from rules to tags. Tags don't automatically write back to Maildir flags (though flag sync reads Maildir flags and applies them as tags).

---

## 🔄 Backup & Recovery

The index lives at `~/.hamster_index` and is **always safe to delete**:

```bash
rm -rf ~/.hamster_index
hamster index              # rebuild the index from scratch
hamster folder sync        # reapply your folder rules
```

Your original emails in `~/.hamster.toml`'s `maildir` directory are **never modified** by hamster. They remain untouched on disk in their original Maildir structure.

If something goes wrong:
1. Delete `~/.hamster_index`
2. Verify `~/.hamster.toml` points to the correct Maildir
3. Run `hamster index` again

---

## 🐛 Troubleshooting

### Q: I get "Maildir path does not exist" error

**A:** Check the `maildir` setting in `~/.hamster.toml`. Re-run `hamster setup` and provide the correct path. Common issues:
- Typo in the path
- Path uses `~` instead of absolute path (use `~/.config/mail` → `/home/you/.config/mail`)
- The directory was deleted or moved

### Q: Why is the first index run so slow?

**A:** Correct! Tantivy must read, parse, and index every email in your Maildir. This is a one-time cost.

Subsequent runs only process new and modified messages, so they're nearly instant. You can monitor progress with the progress bar (featuring a very busy hamster 🐹).

### Q: Can I use hamster with multiple email accounts?

**A:** Currently, hamster is single-account. To manage multiple accounts:
- Create separate Maildir directories (e.g., `~/.mail/personal`, `~/.mail/work`)
- Run `hamster setup` and choose one directory per invocation
- Or symlink one account's mail into the main Maildir under separate folders

Multi-account support may be added in a future version.

### Q: How do I keep my tags synced with Maildir flags?

**A:** Enable `sync_flags = true` in the `[folder_tags]` section of `~/.hamster.toml`, then run:

```bash
hamster folder sync
```

This syncs Maildir flags (Seen, Flagged, Replied) to tags (unread, flagged, replied) based on your rules. Re-run periodically.

### Q: The TUI search seems slow

**A:** If searching on a very large mailbox (100k+ messages), Tantivy may take a moment. This is normal for the first search in a session. Subsequent searches are cached.

If it's consistently slow, consider:
- Narrowing your search with `from:`, `to:`, `subject:` prefixes
- Using `hamster folder sync` to organize mail into fewer, tagged folders

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
