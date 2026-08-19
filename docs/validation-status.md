# ReHome Desktop Beta Validation

ReHome Desktop is in public beta. A migration is only complete after the package is verified, restored, and the restored project is reopened in Codex Desktop.

| Direction | Current coverage | Remaining beta boundary |
|---|---|---|
| Windows → Windows | Real-source isolated acceptance covers package creation, restore, checksums, conversations, indexes, SQLite threads, path mapping, project files, and exclusions. | A second physical Windows machine and its live sidebar registration still need final release acceptance. |
| Mac → Mac | Real package, isolated restore, and Codex project registration have passed on Intel macOS. | Apple Silicon native run remains a release check. |
| Windows → macOS | Package compatibility and target restore logic are covered. | Final current-build physical Windows-to-Mac App run remains required before stable release. |
| macOS → Windows | Package compatibility and target restore logic are covered. | Final current-build physical Mac-to-Windows App run remains required before stable release. |

Shared user Skills currently have synthetic coverage for independent `HOME`/`CODEX_HOME` roots, XDG lock discovery, root and nested links, special and sensitive files, exclusion accounting, portable-name collisions, schema v1/v2, whole-directory conflicts, v3 lock merging, and crash recovery at each replacement/write boundary. A private physical Mac → Windows 11 run must still confirm target tree hashes, unchanged unrelated Skills and lock entries, Codex `skills/list` discovery, one first-party and one third-party instruction-only invocation, and runtime-dependent exceptions. No personal Skill or lock data belongs in this repository or its pull requests.

## What the app verifies

- Package checksums and required files
- Selected conversation files and session indexes
- SQLite thread records and target-path mapping
- Selected project files and default exclusions
- Best-effort project registration through Codex Desktop
- Restored shared Skill tree hashes and v3 lock merge status

## What still needs a user check

Open Codex after a restore. Confirm the project is visible and reopen it if needed. Historical conversation text can be restored while an old task's original working-directory handle no longer works after a cross-platform move. Continue from the restored project in a new task when necessary.

For shared Skills, separately confirm that files match, Codex discovers the Skill from the user-level `.agents/skills` root, and an actual invocation succeeds. These are distinct checks. ReHome does not install a Skill's language runtimes, external CLIs, credentials, or API keys.

Do not treat a successful file copy alone as a successful migration.
