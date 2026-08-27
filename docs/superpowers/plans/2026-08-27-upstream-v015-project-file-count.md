# ReHome 0.1.15 Merge and Project File Count Implementation Plan

> **For agentic workers:** Execute this plan task-by-task with red-green TDD. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Merge ReHome Desktop 0.1.15 into the shared-Skills feature branch and show the current migratable file count for every discovered project without blocking the UI.

**Architecture:** Preserve the existing package v1/v2 and Skill transaction behavior while adopting upstream package and restore improvements. Add a read-only Rust project scanner behind a server-resolved Tauri command, cache one background scan in `App`, and render explicit scanning, counted, failed, and missing states in the Send page.

**Tech Stack:** Rust, Tauri 2, React/TypeScript, WalkDir, serde, Vitest.

---

### Task 1: Merge the fixed upstream release

**Files:**
- Modify through merge: `desktop/src-tauri/src/core/package.rs`
- Modify through merge: `desktop/src-tauri/src/core/restore.rs`
- Accept automatically merged upstream files and lockfiles.

- [ ] Fetch `desktop-v0.1.15` and verify its commit is `5cb4621f4cbca9a258d82fb63480ef501c4eba25`.
- [ ] Merge with `git merge --no-ff --no-commit desktop-v0.1.15`.
- [ ] Resolve `package.rs` by retaining schema v1/v2 and shared-Skill validation while adopting the 100,000-entry limit, 16 GiB planning limit, 8-attempt stable-read retry, archive identity fields, and `AuthenticatedPayloadArchive`.
- [ ] Resolve `restore.rs` by opening one authenticated payload archive and passing it to both regular-file and Skill-bundle restoration; retain lock planning bytes, bundle transactions, rollback, and fault points.
- [ ] Run Rust format, Clippy, and the full Rust suite before committing the merge.
- [ ] Commit as `chore: 合并 ReHome Desktop 0.1.15`.

### Task 2: Add the project-counting backend seam

**Files:**
- Create: `desktop/src-tauri/src/core/project_scan.rs`
- Modify: `desktop/src-tauri/src/core/mod.rs`
- Modify: `desktop/src-tauri/src/core/models.rs`
- Modify: `desktop/src-tauri/src/workflow.rs`
- Modify: `desktop/src-tauri/src/lib.rs`

- [ ] Add a failing Rust test that counts two ordinary files while excluding `.git`, `node_modules`, `.env`, symlinks, and non-regular entries.
- [ ] Add failing tests for an empty directory, a missing directory, duplicate request IDs, unknown IDs, unavailable projects, and isolation of one failed project from successful results.
- [ ] Implement a checked `WalkDir` scanner using `follow_links(false)`, `is_forbidden`, and `normalize_entry` without copying or hashing files.
- [ ] Add `ScanProjectFilesRequest` and the tagged `ProjectFileScanResult::{Counted, Failed}` response.
- [ ] Add `scan_project_files` as a blocking Tauri command that re-discovers projects and resolves only server-owned paths.
- [ ] Run the targeted Rust tests until green.

### Task 3: Add the background UI seam

**Files:**
- Modify: `desktop/src/lib/types.ts`
- Modify: `desktop/src/lib/api.ts`
- Modify: `desktop/src/App.tsx`
- Modify: `desktop/src/features/send/SendPage.tsx`
- Modify: `desktop/src/lib/i18n.tsx`
- Test: `desktop/src/App.test.tsx`

- [ ] Add a failing App test for the `正在统计文件…` state while the Tauri promise is pending.
- [ ] Add failing tests for positive and zero counts, per-project failure, missing projects, no file status on unassociated conversations, English strings, and no repeated request after navigation.
- [ ] Add the TypeScript tagged result and `scanProjectFiles(projectIds)` API wrapper.
- [ ] Start one scan after inventory succeeds, cache the result in App state, and ignore late results after unmount.
- [ ] Render `正在统计文件…`, `N 个文件`, `文件统计失败`, or the existing `项目文件缺失` without changing selection behavior.
- [ ] Remove the obsolete `已检测 个文件` translation and run the App tests until green.

### Task 4: Full local validation and commit

- [ ] Run `cargo fmt --manifest-path desktop/src-tauri/Cargo.toml -- --check`.
- [ ] Run `cargo clippy --manifest-path desktop/src-tauri/Cargo.toml --all-targets -- -D warnings`.
- [ ] Run `cargo test --manifest-path desktop/src-tauri/Cargo.toml`.
- [ ] From `desktop`, run `npm test -- --run` and `npm run build`.
- [ ] From `desktop`, run `npm run tauri -- build --bundles app`.
- [ ] Open the local app and confirm every current project row shows an integer; an empty project must show `0 个文件`.
- [ ] Run `git diff --check`, inspect the complete diff, and stage only feature-related files.
- [ ] Commit as `fix: 显示项目可迁移文件数`.
- [ ] Confirm the worktree is clean and `agents-skills-preview-v0.1.11^{commit}` still resolves to `743583e5c2cc67c9eed9e596a2c17caa64d897a0`.
- [ ] Do not push, tag, create a Release, or open a PR.
