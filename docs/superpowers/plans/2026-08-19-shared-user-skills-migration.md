# Shared User Skills Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend ReHome Desktop to migrate user-level `~/.agents/skills` and safe v3 lock metadata with bundle-atomic conflict handling and rollback.

**Architecture:** Add a focused shared-skills module for path resolution, discovery, tree hashing, lock sanitization, packaging, planning, and verification. Extend the existing package schema and transaction model without changing schema v1 output when no shared Skills are selected.

**Tech Stack:** Rust, Tauri 2, React/TypeScript, serde, zip, sha2, walkdir, rusqlite, Vitest.

---

### Task 1: Model and path contracts

**Files:**
- Modify: `desktop/src-tauri/src/core/models.rs`
- Modify: `desktop/src/lib/types.ts`
- Create: `desktop/src-tauri/src/core/shared_skills.rs`
- Modify: `desktop/src-tauri/src/core/mod.rs`

- [ ] Add `SkillRootKind`, shared Skill inventory/manifest types, v3 lock types, bundle operation types, Agents root fields, and verification fields.
- [ ] Add path resolution tests proving `CODEX_HOME` never changes the Agents root and `XDG_STATE_HOME` changes only the lock path.
- [ ] Run `cargo test shared_skills::tests::resolve --manifest-path desktop/src-tauri/Cargo.toml`; expect all resolver tests to pass.
- [ ] Mirror serialized fields in TypeScript and run `npm test -- --run` from `desktop`.

### Task 2: Discovery and safety classification

**Files:**
- Modify: `desktop/src-tauri/src/core/discovery.rs`
- Modify: `desktop/src-tauri/src/core/shared_skills.rs`
- Modify: `desktop/src-tauri/src/core/exclusions.rs`
- Test: `desktop/src-tauri/tests/discovery_test.rs`

- [ ] Add failing tests for real shared roots, root symlinks, legacy aliases, `.system`, nested symlinks, special files, sensitive files, excluded cache counts, Windows names, case collisions, and NFC collisions.
- [ ] Implement canonical-root scanning with `symlink_metadata`, ordinary-file-only traversal, per-bundle tree hashing, stable UUIDv5 IDs, lock availability, and blocked reasons.
- [ ] Re-run discovery tests and the full Rust suite; expect no regression in legacy inventory counts.

### Task 3: Package schema v2

**Files:**
- Modify: `desktop/src-tauri/src/core/package.rs`
- Modify: `desktop/src-tauri/src/core/shared_skills.rs`
- Test: `desktop/src-tauri/tests/package_test.rs`

- [ ] Add failing tests showing legacy-only packages remain v1 and shared-Skill packages become v2.
- [ ] Stage selected bundles under `agents/skills`, stage sanitized lock metadata, add all payloads to checksums, and reject creation above inspection entry/byte limits.
- [ ] Accept schema v1/v2 on inspection, authenticate lock planning bytes, and reject malformed shared Skill manifest references.
- [ ] Run package tests, including corruption, traversal, secret, path-collision, and limit cases.

### Task 4: Bundle planning and UI conflicts

**Files:**
- Modify: `desktop/src-tauri/src/core/planner.rs`
- Modify: `desktop/src-tauri/src/core/models.rs`
- Modify: `desktop/src/features/receive/ReceivePage.tsx`
- Modify: `desktop/src/lib/api.ts`
- Test: `desktop/src-tauri/tests/planner_test.rs`
- Test: `desktop/src/App.test.tsx`

- [ ] Add failing planner tests for missing, unchanged, preserved, package-winning, case-insensitive, and NFC-equivalent Skill directories.
- [ ] Map `agents/skills` to the independent target Agents root and validate pairwise separation of Codex, project, Agents, and backup roots.
- [ ] Add per-Skill conflict choices with keep-existing as the default; prevent restore until every bundle conflict is resolved.
- [ ] Run Rust planner tests and React tests; expect existing file conflict behavior to remain unchanged.

### Task 5: Directory transaction and rollback

**Files:**
- Modify: `desktop/src-tauri/src/core/backup.rs`
- Modify: `desktop/src-tauri/src/core/restore.rs`
- Modify: `desktop/src-tauri/src/core/stable_fs.rs`
- Test: `desktop/src-tauri/tests/restore_test.rs`

- [ ] Add failing tests for directory backup, same-volume staging, quarantine, atomic replacement, unrelated target preservation, and rollback.
- [ ] Extend journal validation with the Agents root, `Directory` backup kind, bundle phases, path containment, and restart recovery.
- [ ] Add crash-injection tests after quarantine, after replacement, and around lock writing.
- [ ] Run restore, bridge, backup, and full Rust tests on macOS; require all to pass.

### Task 6: Lock merge and verification

**Files:**
- Modify: `desktop/src-tauri/src/core/shared_skills.rs`
- Modify: `desktop/src-tauri/src/core/restore.rs`
- Modify: `desktop/src-tauri/src/core/models.rs`
- Test: `desktop/src-tauri/tests/restore_test.rs`

- [ ] Add tests for safe v3 fields including `ref`, unsafe URLs/paths, target-only metadata, missing lock, malformed lock, and unknown versions.
- [ ] Generate merged lock bytes only from final bundle choices, write them atomically inside the transaction, and preserve target preferences and unrelated entries.
- [ ] Verify bundle tree hashes and lock outcome separately; report discovery and functional sampling as distinct statuses.
- [ ] Run the full Rust test suite.

### Task 7: Send UI, documentation, and validation

**Files:**
- Modify: `desktop/src/features/send/SendPage.tsx`
- Modify: `desktop/src/features/home/HomePage.tsx`
- Modify: `desktop/src/lib/i18n.tsx`
- Modify: `README.md`
- Modify: `README.en.md`
- Modify: `README.zh-CN.md`
- Modify: `docs/validation-status.md`
- Test: `desktop/src/App.test.tsx`

- [ ] Display shared/legacy origins, lock state, exclusions, blocked reasons, and separate counts without selecting items by default.
- [ ] Document that ReHome migrates Skill content but not runtimes, credentials, system Skills, project roots, or continuous sync.
- [ ] Run `npm test -- --run`, `npm run build`, `cargo test --manifest-path desktop/src-tauri/Cargo.toml`, and `cargo clippy --manifest-path desktop/src-tauri/Cargo.toml --all-targets -- -D warnings`.

### Task 8: Private end-to-end and publication

**Files:**
- Modify only validation documentation with non-sensitive aggregate results.

- [ ] Build a side-by-side test app with updater disabled and create a private `.rehome` from current Mac shared Skills.
- [ ] Restore on Win11, compare bundle hashes, inspect lock preservation, query Codex `skills/list`, invoke one Pinfei and one third-party instruction-only Skill, and test conflict rollback with a disposable fixture.
- [ ] Inspect `git status`, final diff, and test results; stage only task files and commit with `feat: 支持迁移共享用户 Skills`.
- [ ] Push `codex/agents-skills-migration` to `JAMESHPF/codex-rehome` and open a draft Chinese PR against `CalebYcj/codex-rehome:main`.
