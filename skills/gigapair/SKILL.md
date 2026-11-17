---
name: gigapair
description: Use the Gigapair CLI (`gigapair`, alias `gpair`) to send and receive very large files and folders between paired devices, end-to-end encrypted, verified and resumable. Covers installing Gigapair on Windows, macOS and Linux, pairing devices, sending, accepting incoming transfers, following progress, settings, the background service (`gigapaird`) and troubleshooting. Use this skill whenever the user mentions Gigapair, gigapair, gpair or gigapaird, wants to move big files (tens or hundreds of GB) to another of their computers or a server, asks why a transfer is slow, stuck or going through a relay, or wants to set up a machine that receives files unattended, even if they don't name the tool.
---

# Gigapair

Gigapair moves very large files (50–200+ GB) between devices that were paired once. Every file is checked with BLAKE3 before it is marked complete, and transfers resume after network drops, restarts and reboots.

## How it is built

Three programs, installed side by side:

| Program | Role |
| --- | --- |
| `gigapaird` | The background service. It owns everything: the network node, contacts, the queue, history and settings |
| `gigapair` (alias `gpair`) | The CLI. A thin client: each command is one request to the service over a local socket |
| `gigapair-app` | The desktop app. Another client of the same service |

So the CLI and the app always show the same state, and a transfer keeps going when the CLI exits or the app closes. Most commands start the service if it isn't running (stderr then says `Started the Gigapair service in the background.`).

Two kinds of names appear everywhere:

- **Contact:** a paired device, named by its name (any case), the start of its name, or the first 4+ characters of its node id.
- **Transfer id:** 8 hex characters (`5a861ebe`); any unique prefix works (`5a86`).

## Before anything else

Run `gigapair --version`. If the command is missing, read [references/install.md](references/install.md) first. A freshly installed CLI may not be on the `PATH` until a new shell; the install guide lists the full paths.

## Rules for driving the CLI from an agent

These follow from how the CLI behaves without a terminal. Breaking them makes a command hang or makes a security check meaningless.

1. **Don't wait on a transfer in the foreground.** `gigapair send` without `--no-wait` stays attached until the transfer ends, which for 200 GB can be hours, and redraws one progress line with `\r`. Use `send --no-wait`, then check with `gigapair status` now and then.
2. **Never run `gigapair status --watch`.** It clears the screen and never returns. Plain `gigapair status` is a snapshot.
3. **A transfer that finished leaves `status`.** `status` lists only active transfers. When an id disappears from it, look it up in `gigapair history` to see whether it ended `Completed`, `Failed`, `Cancelled` or `Rejected`, and `gigapair show <id>` for where the files ended up.
4. **Pairing is a security step that needs the human.** Both devices show the same four words; matching words prove nobody intercepted the code. Without a terminal, `pair` refuses unless given `--yes`, and `--yes` confirms right after printing the words, with no chance to compare. So: prefer to let the user pair (in the app or in their own terminal). If they ask you to do it, run it with `--yes`, show them the four words exactly as printed and ask them to compare with the other device; if the words differ, run `gigapair contacts remove <contact>` at once and say so.
5. **`gigapair pair` without a code blocks** for up to 10 minutes while it waits for the other device, then asks the same confirmation as rule 4. From an agent, run `gigapair pair --yes` in the background, give the user the printed code (the line starting `gp1-`), and when they say the other side used it, read the output and do the word check from rule 4.
6. **Ask before changes the user didn't request:** turning on auto-accept for a contact (it lets that device write to this disk without asking), changing `direct_only` or `relay_url`, removing contacts, `history --clear` or `--remove`, cancelling transfers, `service install`/`uninstall`, and above all `gigapair reset` (it deletes this device's identity and every contact; there is no undo).
7. **Output is plain text, no JSON.** Exit code 0 means success; on failure the exit code is 1 and stderr has one line starting `error:` that says what to do next. Read it before retrying.

## Common tasks

Send (paths can be files or folders, relative or absolute):

```sh
gigapair contacts                                  # who is paired, online or not
gigapair send "Studio Mac" ./footage --no-wait     # prints: Sending footage/ (3.2 GB) to Studio Mac [633efa0e]
gigapair status                                    # progress, speed, ETA, [direct] or [via relay ...]
gigapair history --limit 5                         # once it has left `status`
```

Receive:

```sh
gigapair requests                                  # incoming transfers waiting for an answer
gigapair accept 5a86 --to /data/incoming           # or without --to: the download folder
gigapair reject 5a86
```

## Where to go next

| You need to… | Read |
| --- | --- |
| Use any command: every option, how output looks, transfer states, settings and their values, two instances on one machine | [references/cli.md](references/cli.md) |
| Install, update or remove Gigapair, or set up an unattended receiving server | [references/install.md](references/install.md) |
| Fix an error message, a stuck or slow transfer, a relay connection, the service not starting, or collect logs for a bug report | [references/troubleshooting.md](references/troubleshooting.md) |
