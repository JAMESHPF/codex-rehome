use crate::core::{
    backup::{
        begin_bundle_stage, begin_bundle_swap, cleanup_bundle_quarantines, ensure_applied_states,
        prepare_transaction, record_applied_mutation, record_bundle_phase,
        record_file_write_intent, rollback_prepared, update_status, BundlePhase,
        PreparedTransaction,
    },
    bridge::{
        apply_bridge_plan_for_transaction, apply_file_source_for_transaction,
        register_project_with_detected_cli,
    },
    error::{ErrorCode, RehomeError},
    models::{
        ChangeKind, OperationKind, PendingRecovery, ProjectRegistration, RecoveryStatus,
        ReferenceRewriteKind, RegistrationStatus, RestoreOptions, RestorePlan, RestoreReport,
        RollbackReport, SkillLockFileV3, SourceOs, TransactionHistory, TransactionSummary,
        VerificationReport, VerificationStatus,
    },
    package::{inspect_package_for_planning, VerifiedPackage},
    paths::normalize_entry,
    shared_skills::{merge_skill_lock, tree_hash, LockMergeResult},
    stable_fs::PinnedParent,
};
use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    io::{self, Read, Write},
    path::Path,
};
use tempfile::NamedTempFile;
use uuid::Uuid;

const SESSION_INDEX_SOURCE: &str = "codex/session_index.jsonl";
const THREAD_METADATA_SOURCE: &str = "codex/metadata/threads.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreFaultPoint {
    SkillTargetQuarantined,
    SkillBundleReplaced,
    BeforeSkillLockWrite,
    AfterSkillLockWrite,
}

pub fn apply_restore(
    plan: RestorePlan,
    options: RestoreOptions,
) -> Result<RestoreReport, RehomeError> {
    let plan = crate::core::plan_store::load_exact(&plan)?;
    apply_server_plan(plan, options, |target_os, project| {
        register_project_with_detected_cli(target_os, project)
    })
}

pub fn apply_restore_by_id(
    plan_id: Uuid,
    options: RestoreOptions,
) -> Result<RestoreReport, RehomeError> {
    let plan = crate::core::plan_store::load(plan_id)?;
    apply_server_plan(plan, options, |target_os, project| {
        register_project_with_detected_cli(target_os, project)
    })
}

pub fn apply_restore_with_registrar(
    plan: RestorePlan,
    options: RestoreOptions,
    registrar: impl FnMut(SourceOs, &Path) -> RegistrationStatus,
) -> Result<RestoreReport, RehomeError> {
    let plan = crate::core::plan_store::load_exact(&plan)?;
    apply_server_plan(plan, options, registrar)
}

fn apply_server_plan(
    plan: RestorePlan,
    options: RestoreOptions,
    registrar: impl FnMut(SourceOs, &Path) -> RegistrationStatus,
) -> Result<RestoreReport, RehomeError> {
    apply_server_plan_with_fault(plan, options, registrar, |_| Ok(()))
}

fn apply_server_plan_with_fault(
    plan: RestorePlan,
    options: RestoreOptions,
    mut registrar: impl FnMut(SourceOs, &Path) -> RegistrationStatus,
    mut fault: impl FnMut(RestoreFaultPoint) -> Result<(), RehomeError>,
) -> Result<RestoreReport, RehomeError> {
    if !options.codex_closed_confirmed {
        return Err(RehomeError::new(
            ErrorCode::CodexRunning,
            "restore requires confirmation that current Codex work is saved",
        ));
    }
    validate_plan(&plan)?;
    let verified = inspect_package_for_planning(&plan.package_path)?;
    validate_package_identity(&plan, &verified)?;
    if verified.preview.forbidden_files_total > 0 {
        return Err(RehomeError::new(
            ErrorCode::PackageInvalid,
            "restore package contains forbidden files",
        ));
    }
    validate_preserved_targets(&plan)?;
    let mut transaction = prepare_transaction(&plan, &options.backup_root)?;

    let result = apply_transaction(
        &plan,
        &options,
        &verified,
        &mut transaction,
        &mut registrar,
        &mut fault,
    );
    match result {
        Ok(report) => Ok(report),
        Err(error) => match rollback_prepared(&mut transaction) {
            Ok(_) => Err(error),
            Err(rollback_error) => Err(RehomeError::new(
                ErrorCode::RollbackFailed,
                format!(
                    "restore failed: {}; automatic rollback failed: {}",
                    error.message, rollback_error.message
                ),
            )),
        },
    }
}

pub fn rollback(transaction_id: Uuid) -> Result<RollbackReport, RehomeError> {
    crate::core::backup::rollback(transaction_id)
}

pub fn list_transactions() -> Result<Vec<TransactionSummary>, RehomeError> {
    crate::core::backup::list_transactions()
}

pub fn list_transaction_history() -> Result<TransactionHistory, RehomeError> {
    crate::core::backup::list_transaction_history()
}

pub fn transaction_summary(
    transaction_id: Uuid,
) -> Result<Option<TransactionSummary>, RehomeError> {
    crate::core::backup::transaction_summary(transaction_id)
}

pub fn recover_incomplete_transactions() -> Result<Vec<PendingRecovery>, RehomeError> {
    crate::core::backup::recover_incomplete_transactions()
}

fn apply_transaction(
    plan: &RestorePlan,
    options: &RestoreOptions,
    verified: &VerifiedPackage,
    transaction: &mut PreparedTransaction,
    registrar: &mut impl FnMut(SourceOs, &Path) -> RegistrationStatus,
    fault: &mut impl FnMut(RestoreFaultPoint) -> Result<(), RehomeError>,
) -> Result<RestoreReport, RehomeError> {
    update_status(transaction, RecoveryStatus::Applying)?;
    let transaction_id = transaction.journal.transaction_id;
    let (mut restored_files, mut restored_bytes) =
        apply_regular_files(plan, verified, transaction, fault)?;
    let bridge = apply_bridge_plan_for_transaction(plan, transaction_id, |target| {
        record_applied_mutation(transaction, target)
    })
    .map_err(|error| {
        if plan
            .operations
            .iter()
            .any(|operation| operation.package_source == THREAD_METADATA_SOURCE)
        {
            RehomeError::new(
                error.code,
                format!("SQLite/index bridge update failed: {}", error.message),
            )
        } else {
            error
        }
    })?;
    restored_files += (bridge.sessions_written
        + bridge.index_entries_merged
        + bridge.sqlite_threads_imported) as u64;
    restored_bytes += changed_target_bytes(plan)?;

    update_status(transaction, RecoveryStatus::Verifying)?;
    let mut verification = verify_restore(plan, verified)?;
    if !data_verification_passed(&verification) {
        // Verification opens the restored SQLite database after the bridge has
        // recorded its first applied state. In WAL mode that read can create
        // fresh sidecars, so refresh the journal before rollback decides
        // whether a sidecar belongs to this transaction.
        if let Some(operation) = plan
            .operations
            .iter()
            .find(|operation| operation.package_source == THREAD_METADATA_SOURCE)
        {
            record_applied_mutation(transaction, &operation.target)?;
        }
        return Err(restore_failed(format!(
            "restore verification did not pass: {verification:?}"
        )));
    }
    ensure_applied_states(transaction)?;
    cleanup_bundle_quarantines(transaction)?;
    update_status(transaction, RecoveryStatus::Committed)?;
    let registrations = register_projects(plan, options, verified, registrar);
    verification.app_registration_valid = options.register_projects
        && registrations
            .iter()
            .all(|result| result.status == RegistrationStatus::Registered);
    verification.app_visible_ready =
        data_verification_passed(&verification) && verification.app_registration_valid;

    Ok(RestoreReport {
        transaction_id: transaction.journal.transaction_id,
        package_id: plan.package_id,
        completed_at: timestamp(),
        restored_files,
        restored_bytes,
        registrations,
        verification,
    })
}

fn validate_plan(plan: &RestorePlan) -> Result<(), RehomeError> {
    if !plan.package_path.is_absolute()
        || !plan.target_codex_home.is_absolute()
        || !plan.projects_root.is_absolute()
        || !plan.target_agents_skills_root.is_absolute()
        || !plan.target_skill_lock_path.is_absolute()
    {
        return Err(restore_failed("restore plan paths must be absolute"));
    }
    if plan.conflict_count > 0
        || plan
            .operations
            .iter()
            .any(|operation| operation.action == ChangeKind::Conflict)
    {
        return Err(RehomeError::new(
            ErrorCode::ProjectConflict,
            "restore plan contains unresolved conflicts",
        ));
    }
    if plan.target_codex_home.starts_with(&plan.projects_root)
        || plan.projects_root.starts_with(&plan.target_codex_home)
        || plan
            .target_codex_home
            .starts_with(&plan.target_agents_skills_root)
        || plan
            .target_agents_skills_root
            .starts_with(&plan.target_codex_home)
        || plan
            .projects_root
            .starts_with(&plan.target_agents_skills_root)
        || plan
            .target_agents_skills_root
            .starts_with(&plan.projects_root)
    {
        return Err(restore_failed("restore roots must not overlap"));
    }
    Ok(())
}

fn validate_package_identity(
    plan: &RestorePlan,
    verified: &VerifiedPackage,
) -> Result<(), RehomeError> {
    if verified.preview.manifest.package_id != plan.package_id {
        return Err(RehomeError::new(
            ErrorCode::PackageInvalid,
            "restore plan package ID does not match the package",
        ));
    }
    if !verified
        .preview
        .archive_hash
        .eq_ignore_ascii_case(&plan.archive_hash)
    {
        return Err(RehomeError::new(
            ErrorCode::PackageInvalid,
            "restore plan archive hash does not match the package",
        ));
    }
    for operation in &plan.operations {
        match operation.operation_kind {
            OperationKind::File | OperationKind::SkillLock => {
                if !verified.payloads.contains_key(&operation.package_source) {
                    return Err(RehomeError::new(
                        ErrorCode::PackageInvalid,
                        format!(
                            "restore operation references a missing package payload: {}",
                            operation.package_source
                        ),
                    ));
                }
            }
            OperationKind::SkillBundle => {
                let prefix = format!("{}/", operation.package_source);
                if operation.content_id.is_none()
                    || !verified
                        .payloads
                        .keys()
                        .any(|source| source.starts_with(&prefix))
                {
                    return Err(RehomeError::new(
                        ErrorCode::PackageInvalid,
                        "Skill bundle operation references missing package payloads",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn apply_regular_files(
    plan: &RestorePlan,
    verified: &VerifiedPackage,
    transaction: &mut PreparedTransaction,
    fault: &mut impl FnMut(RestoreFaultPoint) -> Result<(), RehomeError>,
) -> Result<(u64, u64), RehomeError> {
    let mut restored_files = 0_u64;
    let mut restored_bytes = 0_u64;
    for kind in [
        OperationKind::File,
        OperationKind::SkillBundle,
        OperationKind::SkillLock,
    ] {
        for operation in &plan.operations {
            if operation.operation_kind != kind
                || !matches!(operation.action, ChangeKind::Add | ChangeKind::Update)
                || is_bridge_operation(plan, &operation.package_source)
            {
                continue;
            }
            let (files, bytes) = match operation.operation_kind {
                OperationKind::File => {
                    let mut staged = NamedTempFile::new().map_err(|error| {
                        restore_failed(format!("could not stage restored payload: {error}"))
                    })?;
                    let bytes = verified.write_authenticated_payload(
                        &operation.package_source,
                        staged.as_file_mut(),
                    )?;
                    staged.as_file().sync_all().map_err(|error| {
                        restore_failed(format!("could not flush restored payload: {error}"))
                    })?;
                    let root = operation_root(plan, &operation.target)?;
                    apply_file_source_for_transaction(
                        root,
                        operation,
                        staged.path(),
                        transaction.journal.transaction_id,
                    )?;
                    record_applied_mutation(transaction, &operation.target)?;
                    (1, bytes)
                }
                OperationKind::SkillBundle => {
                    apply_skill_bundle(plan, verified, operation, transaction, fault)?
                }
                OperationKind::SkillLock => {
                    let bytes = apply_skill_lock(plan, verified, operation, transaction, fault)?;
                    (1, bytes)
                }
            };
            restored_files = restored_files
                .checked_add(files)
                .ok_or_else(|| restore_failed("restored file count overflowed"))?;
            restored_bytes = restored_bytes
                .checked_add(bytes)
                .ok_or_else(|| restore_failed("restored byte count overflowed"))?;
        }
    }
    Ok((restored_files, restored_bytes))
}

fn apply_skill_bundle(
    plan: &RestorePlan,
    verified: &VerifiedPackage,
    operation: &crate::core::models::PlannedOperation,
    transaction: &mut PreparedTransaction,
    fault: &mut impl FnMut(RestoreFaultPoint) -> Result<(), RehomeError>,
) -> Result<(u64, u64), RehomeError> {
    let content_id = operation
        .content_id
        .ok_or_else(|| restore_failed("Skill bundle operation has no content ID"))?;
    let manifest = verified
        .preview
        .manifest
        .shared_skills
        .iter()
        .find(|skill| skill.content_id == content_id)
        .ok_or_else(|| restore_failed("Skill bundle manifest entry is missing"))?;
    if manifest.archive_root != operation.package_source {
        return Err(restore_failed(
            "Skill bundle operation does not match its manifest entry",
        ));
    }
    let parent = operation
        .target
        .parent()
        .ok_or_else(|| restore_failed("Skill bundle target has no parent"))?;
    fs::create_dir_all(parent)
        .map_err(|error| restore_failed(format!("could not create shared Skills root: {error}")))?;
    crate::core::bridge::validate_restore_target_ancestry(
        &plan.target_agents_skills_root,
        &operation.target,
    )?;
    let pinned = PinnedParent::open(parent)
        .map_err(|error| restore_failed(format!("could not pin shared Skills root: {error}")))?;
    let _lock = BundleTargetLock::acquire(
        parent,
        operation
            .target
            .file_name()
            .ok_or_else(|| restore_failed("Skill bundle target has no name"))?,
        transaction.journal.transaction_id,
    )?;
    let stage_path = begin_bundle_stage(transaction, &operation.target)?;
    let stage_component = stage_path
        .file_name()
        .ok_or_else(|| restore_failed("Skill staging path has no name"))?;
    if pinned
        .child_exists(stage_component)
        .map_err(|error| restore_failed(format!("could not inspect Skill staging: {error}")))?
    {
        return Err(restore_failed(
            "owned Skill staging directory already exists",
        ));
    }
    pinned
        .create_directory(stage_component)
        .map_err(|error| restore_failed(format!("could not create Skill staging: {error}")))?;

    let result = (|| {
        let prefix = format!("{}/", manifest.archive_root);
        let mut file_count = 0_u64;
        let mut byte_count = 0_u64;
        for (source, payload) in verified
            .payloads
            .iter()
            .filter(|(source, _)| source.starts_with(&prefix))
        {
            let relative = source
                .strip_prefix(&prefix)
                .ok_or_else(|| restore_failed("Skill payload path is malformed"))?;
            let normalized = normalize_entry(Path::new(relative))
                .map_err(|error| restore_failed(error.message))?;
            if normalized != relative {
                return Err(restore_failed("Skill payload path is not normalized"));
            }
            let destination = stage_path.join(Path::new(relative));
            let destination_parent = destination
                .parent()
                .ok_or_else(|| restore_failed("Skill payload target has no parent"))?;
            fs::create_dir_all(destination_parent).map_err(|error| {
                restore_failed(format!("could not create Skill payload directory: {error}"))
            })?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)
                .map_err(|error| {
                    restore_failed(format!("could not create staged Skill file: {error}"))
                })?;
            let written = verified.write_authenticated_payload(source, &mut output)?;
            output.sync_all().map_err(|error| {
                restore_failed(format!("could not flush staged Skill file: {error}"))
            })?;
            if written != payload.size_bytes {
                return Err(restore_failed("staged Skill payload size changed"));
            }
            file_count += 1;
            byte_count = byte_count
                .checked_add(written)
                .ok_or_else(|| restore_failed("staged Skill byte count overflowed"))?;
        }
        if file_count != manifest.file_count || byte_count != manifest.content_bytes {
            return Err(restore_failed(
                "staged Skill counts do not match the package manifest",
            ));
        }
        let staged_hash = tree_hash(&stage_path).map_err(|error| restore_failed(error.message))?;
        if !staged_hash.eq_ignore_ascii_case(&manifest.tree_hash) {
            return Err(restore_failed(
                "staged Skill tree hash does not match the package manifest",
            ));
        }

        let quarantine = begin_bundle_swap(transaction, &operation.target)?;
        let target_name = operation
            .target
            .file_name()
            .ok_or_else(|| restore_failed("Skill bundle target has no name"))?;
        let quarantine_name = quarantine
            .file_name()
            .ok_or_else(|| restore_failed("Skill quarantine has no name"))?;
        match fs::symlink_metadata(&operation.target) {
            Ok(metadata) => {
                if operation.action != ChangeKind::Update
                    || metadata_is_link_or_reparse(&metadata)
                    || !metadata.is_dir()
                {
                    return Err(restore_failed(
                        "Skill bundle target type changed after planning",
                    ));
                }
                let current_hash =
                    tree_hash(&operation.target).map_err(|error| restore_failed(error.message))?;
                if !operation
                    .expected_previous_hash
                    .as_deref()
                    .is_some_and(|expected| current_hash.eq_ignore_ascii_case(expected))
                {
                    return Err(restore_failed("Skill bundle target changed after planning"));
                }
                pinned
                    .rename_child_if_absent(target_name, quarantine_name)
                    .map_err(|error| {
                        restore_failed(format!("could not quarantine target Skill: {error}"))
                    })?;
                record_bundle_phase(
                    transaction,
                    &operation.target,
                    BundlePhase::TargetQuarantined,
                )?;
                fault(RestoreFaultPoint::SkillTargetQuarantined)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if operation.action != ChangeKind::Add {
                    return Err(restore_failed(
                        "Skill bundle target disappeared after planning",
                    ));
                }
            }
            Err(error) => {
                return Err(restore_failed(format!(
                    "could not inspect target Skill before replacement: {error}"
                )))
            }
        }
        pinned
            .rename_child_if_absent(stage_component, target_name)
            .map_err(|error| {
                restore_failed(format!(
                    "could not atomically install target Skill: {error}"
                ))
            })?;
        record_bundle_phase(transaction, &operation.target, BundlePhase::Replaced)?;
        fault(RestoreFaultPoint::SkillBundleReplaced)?;
        record_applied_mutation(transaction, &operation.target)?;
        Ok((file_count, byte_count))
    })();
    if result.is_err()
        && fs::symlink_metadata(&stage_path)
            .is_ok_and(|metadata| metadata.is_dir() && !metadata_is_link_or_reparse(&metadata))
    {
        let _ = fs::remove_dir_all(&stage_path);
    }
    result
}

fn apply_skill_lock(
    plan: &RestorePlan,
    verified: &VerifiedPackage,
    operation: &crate::core::models::PlannedOperation,
    transaction: &mut PreparedTransaction,
    fault: &mut impl FnMut(RestoreFaultPoint) -> Result<(), RehomeError>,
) -> Result<u64, RehomeError> {
    const MAX_LOCK_BYTES: u64 = 4 * 1024 * 1024;

    let package_bytes = verified.authenticated_planning_payload(&operation.package_source)?;
    let package_lock: SkillLockFileV3 = serde_json::from_slice(package_bytes)
        .map_err(|_| restore_failed("verified package Skill lock is invalid"))?;
    let target_bytes = match fs::symlink_metadata(&operation.target) {
        Ok(metadata) if metadata_is_link_or_reparse(&metadata) || !metadata.is_file() => {
            return Err(restore_failed(
                "target Skill lock is no longer a regular file",
            ));
        }
        Ok(metadata) if metadata.len() > MAX_LOCK_BYTES => {
            return Err(restore_failed(
                "target Skill lock exceeds the supported size limit",
            ));
        }
        Ok(_) => Some(fs::read(&operation.target).map_err(|error| {
            restore_failed(format!("could not read target Skill lock: {error}"))
        })?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(restore_failed(format!(
                "could not inspect target Skill lock: {error}"
            )))
        }
    };
    let decisions = verified
        .preview
        .manifest
        .shared_skills
        .iter()
        .map(|skill| {
            let action = plan
                .operations
                .iter()
                .find(|candidate| candidate.content_id == Some(skill.content_id))
                .map(|candidate| candidate.action)
                .ok_or_else(|| restore_failed("Skill bundle decision is missing"))?;
            Ok((skill.relative_path.clone(), action))
        })
        .collect::<Result<BTreeMap<_, _>, RehomeError>>()?;
    let bytes = match merge_skill_lock(&package_lock, target_bytes.as_deref(), &decisions)
        .map_err(|error| restore_failed(error.message))?
    {
        LockMergeResult::Write(bytes) => bytes,
        LockMergeResult::Unchanged | LockMergeResult::SkippedInvalidTarget => {
            return Err(restore_failed(
                "target Skill lock no longer matches the writable restore plan",
            ))
        }
    };
    let final_hash = format!("{:x}", Sha256::digest(&bytes));
    if !operation
        .expected_final_hash
        .as_deref()
        .is_some_and(|expected| final_hash.eq_ignore_ascii_case(expected))
    {
        return Err(restore_failed(
            "merged Skill lock changed after restore planning",
        ));
    }
    let mut staged = NamedTempFile::new()
        .map_err(|error| restore_failed(format!("could not stage merged Skill lock: {error}")))?;
    staged
        .write_all(&bytes)
        .map_err(|error| restore_failed(format!("could not write merged Skill lock: {error}")))?;
    staged
        .as_file()
        .sync_all()
        .map_err(|error| restore_failed(format!("could not flush merged Skill lock: {error}")))?;
    let root = operation_root(plan, &operation.target)?;
    fs::create_dir_all(root).map_err(|error| {
        restore_failed(format!("could not create Skill lock directory: {error}"))
    })?;
    record_file_write_intent(transaction, &operation.target)?;
    fault(RestoreFaultPoint::BeforeSkillLockWrite)?;
    apply_file_source_for_transaction(
        root,
        operation,
        staged.path(),
        transaction.journal.transaction_id,
    )?;
    fault(RestoreFaultPoint::AfterSkillLockWrite)?;
    record_applied_mutation(transaction, &operation.target)?;
    Ok(bytes.len() as u64)
}

struct BundleTargetLock {
    parent: PinnedParent,
    name: String,
    token: String,
}

impl BundleTargetLock {
    fn acquire(
        parent: &Path,
        target_name: &OsStr,
        transaction_id: Uuid,
    ) -> Result<Self, RehomeError> {
        let parent = PinnedParent::open(parent)
            .map_err(|error| restore_failed(format!("could not pin Skill lock parent: {error}")))?;
        let name = format!(".{}.codex-rehome.lock", target_name.to_string_lossy());
        let token = transaction_id.to_string();
        let mut file = parent.create_new_file(OsStr::new(&name)).map_err(|error| {
            restore_failed(format!("could not acquire Skill bundle lock: {error}"))
        })?;
        file.write_all(token.as_bytes()).map_err(|error| {
            restore_failed(format!("could not write Skill bundle lock: {error}"))
        })?;
        file.sync_all().map_err(|error| {
            restore_failed(format!("could not flush Skill bundle lock: {error}"))
        })?;
        Ok(Self {
            parent,
            name,
            token,
        })
    }
}

impl Drop for BundleTargetLock {
    fn drop(&mut self) {
        let name = OsStr::new(&self.name);
        let mut contents = String::new();
        let owned = self
            .parent
            .open_file(name)
            .and_then(|mut file| file.read_to_string(&mut contents))
            .is_ok()
            && contents == self.token;
        if owned {
            let _ = self.parent.remove_file(name);
        }
    }
}

fn is_bridge_operation(plan: &RestorePlan, source: &str) -> bool {
    source == SESSION_INDEX_SOURCE
        || source == THREAD_METADATA_SOURCE
        || plan
            .sessions
            .iter()
            .any(|session| session.package_source == source)
}

fn changed_target_bytes(plan: &RestorePlan) -> Result<u64, RehomeError> {
    plan.operations
        .iter()
        .filter(|operation| {
            matches!(operation.action, ChangeKind::Add | ChangeKind::Update)
                && is_bridge_operation(plan, &operation.package_source)
        })
        .try_fold(0_u64, |total, operation| {
            match fs::metadata(&operation.target) {
                Ok(metadata) if metadata.is_file() => total
                    .checked_add(metadata.len())
                    .ok_or_else(|| restore_failed("restored byte count overflowed")),
                Ok(_) => Err(restore_failed("restored target is not a regular file")),
                Err(error) => Err(restore_failed(format!(
                    "could not inspect restored target {}: {error}",
                    operation.target.display()
                ))),
            }
        })
}

fn verify_restore(
    plan: &RestorePlan,
    verified: &VerifiedPackage,
) -> Result<VerificationReport, RehomeError> {
    let current = inspect_package_for_planning(&plan.package_path)?;
    let package_checksum_valid = current.preview.checksum_valid
        && current.preview.manifest.package_id == plan.package_id
        && current
            .preview
            .archive_hash
            .eq_ignore_ascii_case(&plan.archive_hash);
    let files_valid = verify_plain_files(plan, verified)?;
    let sessions_valid = plan.sessions.iter().try_fold(true, |valid, session| {
        Ok::<_, RehomeError>(
            valid
                && hash_optional_file(&session.target)?.is_some_and(|hash| {
                    hash.eq_ignore_ascii_case(&session.expected_final_content_hash)
                }),
        )
    })?;
    let bridge = verify_bridge_metadata(plan)?;
    let forbidden_files_absent = current.preview.forbidden_files_total == 0;
    let project_files_valid = verify_project_files(plan, verified)?;
    let shared_skill_files_valid = verify_shared_skill_files(plan)?;
    let skill_lock_merge = verify_skill_lock(plan)?;
    Ok(VerificationReport {
        package_checksum_valid,
        files_valid,
        sessions_valid,
        session_index_valid: bridge.session_index_valid,
        sqlite_threads_valid: bridge.sqlite_threads_valid,
        path_mapping_valid: bridge.path_mapping_valid,
        forbidden_files_absent,
        project_files_valid,
        app_registration_valid: false,
        app_visible_ready: false,
        shared_skill_files_valid,
        codex_skill_discovery: VerificationStatus::NotRun,
        skill_lock_merge,
        functional_sampling: VerificationStatus::NotRun,
    })
}

fn verify_plain_files(plan: &RestorePlan, verified: &VerifiedPackage) -> Result<bool, RehomeError> {
    for operation in &plan.operations {
        if operation.action == ChangeKind::Conflict
            || operation.operation_kind != OperationKind::File
            || is_bridge_operation(plan, &operation.package_source)
        {
            continue;
        }
        if operation.action == ChangeKind::Preserve {
            if !preserved_target_matches(operation)? {
                return Ok(false);
            }
            continue;
        }
        let expected = &verified
            .payloads
            .get(&operation.package_source)
            .ok_or_else(|| restore_failed("verified payload metadata is missing"))?
            .content_hash;
        if !hash_optional_file(&operation.target)?
            .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn validate_preserved_targets(plan: &RestorePlan) -> Result<(), RehomeError> {
    for operation in plan
        .operations
        .iter()
        .filter(|operation| operation.action == ChangeKind::Preserve)
    {
        if !preserved_target_matches(operation)? {
            return Err(restore_failed(format!(
                "preserved target changed after planning: {}",
                operation.target.display()
            )));
        }
    }
    Ok(())
}

fn preserved_target_matches(
    operation: &crate::core::models::PlannedOperation,
) -> Result<bool, RehomeError> {
    let metadata = match fs::symlink_metadata(&operation.target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(operation.expected_previous_hash.is_none())
        }
        Err(error) => {
            return Err(restore_failed(format!(
                "could not inspect preserved target {}: {error}",
                operation.target.display()
            )))
        }
    };
    if operation.operation_kind == OperationKind::SkillBundle {
        if metadata_is_link_or_reparse(&metadata) || !metadata.is_dir() {
            return Ok(operation.expected_previous_hash.is_none());
        }
        let Some(expected) = operation.expected_previous_hash.as_deref() else {
            return Ok(true);
        };
        let actual = tree_hash(&operation.target).map_err(|error| restore_failed(error.message))?;
        return Ok(actual.eq_ignore_ascii_case(expected));
    }
    if metadata_is_link_or_reparse(&metadata) || !metadata.is_file() {
        return Ok(false);
    }
    let Some(expected) = operation.expected_previous_hash.as_deref() else {
        return Ok(false);
    };
    Ok(hash_optional_file(&operation.target)?
        .is_some_and(|actual| actual.eq_ignore_ascii_case(expected)))
}

fn verify_project_files(
    plan: &RestorePlan,
    verified: &VerifiedPackage,
) -> Result<bool, RehomeError> {
    for operation in plan
        .operations
        .iter()
        .filter(|operation| operation.target.starts_with(&plan.projects_root))
        .filter(|operation| operation.operation_kind == OperationKind::File)
        .filter(|operation| operation.action != ChangeKind::Conflict)
    {
        let expected = &verified
            .payloads
            .get(&operation.package_source)
            .ok_or_else(|| restore_failed("project payload metadata is missing"))?
            .content_hash;
        if !hash_optional_file(&operation.target)?
            .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn verify_shared_skill_files(plan: &RestorePlan) -> Result<bool, RehomeError> {
    for operation in plan
        .operations
        .iter()
        .filter(|operation| operation.operation_kind == OperationKind::SkillBundle)
    {
        if operation.action == ChangeKind::Conflict {
            return Ok(false);
        }
        if operation.action == ChangeKind::Preserve {
            if !preserved_target_matches(operation)? {
                return Ok(false);
            }
            continue;
        }
        let expected = operation
            .expected_final_hash
            .as_deref()
            .ok_or_else(|| restore_failed("Skill bundle final tree hash is missing"))?;
        let metadata = match fs::symlink_metadata(&operation.target) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(restore_failed(format!(
                    "could not inspect restored Skill bundle: {error}"
                )))
            }
        };
        if metadata_is_link_or_reparse(&metadata) || !metadata.is_dir() {
            return Ok(false);
        }
        let actual = tree_hash(&operation.target).map_err(|error| restore_failed(error.message))?;
        if !actual.eq_ignore_ascii_case(expected) {
            return Ok(false);
        }
        let marker = operation.target.join("SKILL.md");
        let marker_metadata = match fs::symlink_metadata(&marker) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(restore_failed(format!(
                    "could not inspect restored SKILL.md: {error}"
                )))
            }
        };
        if metadata_is_link_or_reparse(&marker_metadata) || !marker_metadata.is_file() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn verify_skill_lock(plan: &RestorePlan) -> Result<VerificationStatus, RehomeError> {
    let Some(operation) = plan
        .operations
        .iter()
        .find(|operation| operation.operation_kind == OperationKind::SkillLock)
    else {
        return Ok(VerificationStatus::Skipped);
    };
    if operation.action == ChangeKind::Preserve {
        return Ok(VerificationStatus::Skipped);
    }
    if operation.action == ChangeKind::Unchanged && operation.expected_final_hash.is_none() {
        return Ok(VerificationStatus::Skipped);
    }
    let Some(expected) = operation.expected_final_hash.as_deref() else {
        return Ok(VerificationStatus::Failed);
    };
    Ok(match hash_optional_file(&operation.target)? {
        Some(actual) if actual.eq_ignore_ascii_case(expected) => VerificationStatus::Passed,
        _ => VerificationStatus::Failed,
    })
}

struct BridgeVerification {
    session_index_valid: bool,
    sqlite_threads_valid: bool,
    path_mapping_valid: bool,
}

fn verify_bridge_metadata(plan: &RestorePlan) -> Result<BridgeVerification, RehomeError> {
    let index_rows = read_index_rows(plan)?;
    let sqlite_rows = read_sqlite_rows(plan)?;
    let requires_index = plan.bridge_verification.session_index.is_some();
    let requires_sqlite = plan.bridge_verification.sqlite_database.is_some();
    let mut index_valid = !requires_index;
    let mut sqlite_valid = !requires_sqlite;
    let mut mapping_valid = true;

    if requires_index {
        index_valid = plan.sessions.iter().all(|session| {
            index_rows
                .get(&session.target_task_id.to_string())
                .is_some_and(|row| {
                    row.get("rollout_path").and_then(Value::as_str) == session.target.to_str()
                })
        });
    }
    if requires_sqlite {
        sqlite_valid = plan.sessions.iter().all(|session| {
            sqlite_rows
                .get(&session.target_task_id.to_string())
                .is_some_and(|(_, rollout)| rollout.as_deref() == session.target.to_str())
        });
    }

    for session in &plan.sessions {
        let expected_project_paths = plan
            .reference_rewrites
            .iter()
            .filter(|rewrite| {
                rewrite.source_task_id == session.source_task_id
                    && rewrite.kind == ReferenceRewriteKind::ProjectPath
                    && rewrite.package_source == session.package_source
            })
            .map(|rewrite| rewrite.to.as_str())
            .collect::<Vec<_>>();
        if expected_project_paths.is_empty() {
            continue;
        }
        let session_bytes = fs::read(&session.target).map_err(|error| {
            restore_failed(format!(
                "could not read restored session for verification: {error}"
            ))
        })?;
        let session_values = parse_jsonl_values(&session_bytes)?;
        let index_cwd = index_rows
            .get(&session.target_task_id.to_string())
            .and_then(|row| row.get("cwd"))
            .and_then(Value::as_str);
        let sqlite_cwd = sqlite_rows
            .get(&session.target_task_id.to_string())
            .and_then(|(cwd, _)| cwd.as_deref());
        let mapped = expected_project_paths.iter().any(|expected| {
            session_values
                .iter()
                .any(|value| json_contains_string(value, expected))
                // Some Codex versions use a minimal session_index row containing only
                // id/title/timestamps/rollout_path. In that schema the authoritative
                // project binding is the session metadata plus the SQLite thread row;
                // do not require a cwd field that the index does not expose.
                && (!requires_index || index_cwd.is_none_or(|cwd| cwd == *expected))
                && (!requires_sqlite || sqlite_cwd == Some(*expected))
        });
        mapping_valid &= mapped;
    }

    Ok(BridgeVerification {
        session_index_valid: index_valid,
        sqlite_threads_valid: sqlite_valid,
        path_mapping_valid: mapping_valid,
    })
}

fn parse_jsonl_values(bytes: &[u8]) -> Result<Vec<Value>, RehomeError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| restore_failed("restored session JSONL is not UTF-8"))?;
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|error| {
                restore_failed(format!("restored session JSONL is invalid: {error}"))
            })
        })
        .collect()
}

fn json_contains_string(value: &Value, expected: &str) -> bool {
    match value {
        Value::String(value) => value == expected,
        Value::Array(values) => values
            .iter()
            .any(|value| json_contains_string(value, expected)),
        Value::Object(values) => values
            .values()
            .any(|value| json_contains_string(value, expected)),
        _ => false,
    }
}

fn read_index_rows(plan: &RestorePlan) -> Result<BTreeMap<String, Value>, RehomeError> {
    let Some(target) = plan.bridge_verification.session_index.as_deref() else {
        return Ok(BTreeMap::new());
    };
    let bytes = fs::read(target).map_err(|error| {
        restore_failed(format!("could not read restored session index: {error}"))
    })?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| restore_failed("restored session index is not UTF-8"))?;
    let mut rows = BTreeMap::new();
    for line in text.lines().filter(|line| !line.is_empty()) {
        let value: Value = serde_json::from_str(line).map_err(|error| {
            restore_failed(format!("restored session index is invalid: {error}"))
        })?;
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            rows.insert(id.to_owned(), value);
        }
    }
    Ok(rows)
}

type SqliteRows = BTreeMap<String, (Option<String>, Option<String>)>;

fn read_sqlite_rows(plan: &RestorePlan) -> Result<SqliteRows, RehomeError> {
    let Some(target) = plan.bridge_verification.sqlite_database.as_deref() else {
        return Ok(BTreeMap::new());
    };
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = Connection::open_with_flags(target, flags).map_err(|error| {
        restore_failed(format!("could not open restored SQLite database: {error}"))
    })?;
    let mut statement = connection
        .prepare("SELECT id, cwd, rollout_path FROM threads")
        .map_err(|error| {
            restore_failed(format!("could not inspect restored SQLite rows: {error}"))
        })?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|error| {
            restore_failed(format!("could not query restored SQLite rows: {error}"))
        })?;
    let mut result = BTreeMap::new();
    for row in rows {
        let (id, cwd, rollout) = row.map_err(|error| {
            restore_failed(format!("could not read restored SQLite row: {error}"))
        })?;
        result.insert(id, (cwd, rollout));
    }
    Ok(result)
}

fn register_projects(
    plan: &RestorePlan,
    options: &RestoreOptions,
    verified: &VerifiedPackage,
    registrar: &mut impl FnMut(SourceOs, &Path) -> RegistrationStatus,
) -> Vec<ProjectRegistration> {
    if !options.register_projects {
        return Vec::new();
    }
    let target_os = if cfg!(target_os = "macos") {
        SourceOs::Macos
    } else {
        SourceOs::Windows
    };
    verified
        .preview
        .manifest
        .projects
        .iter()
        .map(|project| {
            let project_path = plan.projects_root.join(&project.name);
            let status = registrar(target_os, &project_path);
            ProjectRegistration {
                project_id: project.project_id,
                project_path,
                status,
            }
        })
        .collect()
}

fn data_verification_passed(report: &VerificationReport) -> bool {
    report.package_checksum_valid
        && report.files_valid
        && report.sessions_valid
        && report.session_index_valid
        && report.sqlite_threads_valid
        && report.path_mapping_valid
        && report.forbidden_files_absent
        && report.project_files_valid
        && report.shared_skill_files_valid
        && report.skill_lock_merge != VerificationStatus::Failed
}

fn operation_root<'a>(plan: &'a RestorePlan, target: &Path) -> Result<&'a Path, RehomeError> {
    if target.starts_with(&plan.target_codex_home) {
        Ok(&plan.target_codex_home)
    } else if target.starts_with(&plan.projects_root) {
        Ok(&plan.projects_root)
    } else if target.starts_with(&plan.target_agents_skills_root) {
        Ok(&plan.target_agents_skills_root)
    } else if target == plan.target_skill_lock_path {
        plan.target_skill_lock_path
            .parent()
            .ok_or_else(|| restore_failed("target Skill lock has no parent"))
    } else {
        Err(restore_failed(format!(
            "restore target escapes the planned roots: {}",
            target.display()
        )))
    }
}

fn hash_optional_file(path: &Path) -> Result<Option<String>, RehomeError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(restore_failed(format!(
                "could not read restored file {}: {error}",
                path.display()
            )))
        }
    };
    Ok(Some(format!("{:x}", Sha256::digest(bytes))))
}

#[cfg(windows)]
fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink()
        || metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
}

#[cfg(not(windows))]
fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn restore_failed(message: impl Into<String>) -> RehomeError {
    RehomeError::new(ErrorCode::RestoreFailed, message)
}

#[cfg(test)]
mod fault_injection_tests {
    use super::*;
    use crate::core::{
        models::{
            ContentCounts, CreatePackageRequest, FileConflictResolution, SkillLockEntryV3,
            TargetInventory,
        },
        package::{create_package, inspect_package},
        planner::build_restore_plan_with_skill_resolutions,
    };
    use std::{
        collections::BTreeMap,
        env,
        ffi::OsString,
        panic::{catch_unwind, AssertUnwindSafe},
        path::PathBuf,
        sync::Mutex,
    };

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn injected_skill_bundle_and_lock_crashes_restore_the_original_state() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _env = EnvGuard::capture("LOCALAPPDATA");
        for point in [
            RestoreFaultPoint::SkillTargetQuarantined,
            RestoreFaultPoint::SkillBundleReplaced,
            RestoreFaultPoint::BeforeSkillLockWrite,
            RestoreFaultPoint::AfterSkillLockWrite,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(temp.path()).unwrap();
            let app_data = tempfile::tempdir().unwrap();
            let app_data_root = fs::canonicalize(app_data.path()).unwrap();
            env::set_var("LOCALAPPDATA", &app_data_root);
            let (plan, target_skill, target_lock, original_lock) = fault_fixture(&root);
            let options = RestoreOptions {
                codex_closed_confirmed: true,
                backup_root: root.join("backups"),
                register_projects: false,
            };
            let mut injected = false;
            let crashed = catch_unwind(AssertUnwindSafe(|| {
                apply_server_plan_with_fault(
                    plan,
                    options,
                    |_, _| RegistrationStatus::ManualOpenRequired,
                    |observed| {
                        if observed == point && !injected {
                            injected = true;
                            panic!("synthetic process crash at {observed:?}");
                        }
                        Ok(())
                    },
                )
                .unwrap();
            }));
            assert!(injected, "fault point {point:?} was not reached");
            assert!(crashed.is_err(), "fault point {point:?} did not crash");
            let pending = recover_incomplete_transactions().unwrap();
            assert_eq!(pending.len(), 1, "{point:?}: {pending:?}");
            assert!(rollback(pending[0].transaction_id).unwrap().success);
            assert_eq!(
                fs::read(target_skill.join("SKILL.md")).unwrap(),
                b"# Original Skill\n",
                "{point:?}"
            );
            assert!(target_skill.join("local-only.txt").exists(), "{point:?}");
            assert!(!target_skill.join("guide.md").exists(), "{point:?}");
            assert_eq!(fs::read(&target_lock).unwrap(), original_lock, "{point:?}");
            let leftovers = fs::read_dir(target_skill.parent().unwrap())
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|name| name.contains("codex-rehome"))
                .collect::<Vec<_>>();
            assert!(
                leftovers.iter().all(|name| name.ends_with(".rollback")),
                "{point:?}: {leftovers:?}"
            );
            assert!(leftovers.len() <= 1, "{point:?}: {leftovers:?}");
            let transactions = app_data_root
                .join("com.rehome.desktop")
                .join("transactions");
            let journal_path = fs::read_dir(transactions)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "json")
                })
                .unwrap();
            let journal: serde_json::Value =
                serde_json::from_slice(&fs::read(journal_path).unwrap()).unwrap();
            assert_eq!(journal["status"], "rolled_back", "{point:?}");
        }
    }

    fn fault_fixture(root: &Path) -> (RestorePlan, PathBuf, PathBuf, Vec<u8>) {
        let source_codex = root.join("source").join(".codex");
        let source_agents = root.join("source").join(".agents");
        let source_skill = source_agents.join("skills").join("fault-skill");
        fs::create_dir_all(&source_codex).unwrap();
        fs::create_dir_all(&source_skill).unwrap();
        fs::write(source_skill.join("SKILL.md"), b"# Incoming Skill\n").unwrap();
        fs::write(source_skill.join("guide.md"), b"incoming guide\n").unwrap();
        let package_lock = SkillLockFileV3 {
            version: 3,
            skills: BTreeMap::from([("fault-skill".into(), lock_entry("main"))]),
            dismissed: None,
            last_selected_agents: None,
        };
        fs::write(
            source_agents.join(".skill-lock.json"),
            serde_json::to_vec_pretty(&package_lock).unwrap(),
        )
        .unwrap();
        let package_path = root.join("fault.rehome");
        create_package(CreatePackageRequest {
            codex_home: source_codex,
            project_paths: vec![],
            conversation_ids: vec![],
            output_path: package_path.clone(),
            source_device_id: Uuid::nil(),
            skill_paths: vec![],
            shared_skill_paths: vec![source_skill],
            plugin_paths: vec![],
            generated_image_paths: vec![],
        })
        .unwrap();
        let preview = inspect_package(&package_path).unwrap();

        let target_root = root.join("target");
        let target_codex = target_root.join(".codex");
        let target_agents = target_root.join(".agents");
        let target_skills = target_agents.join("skills");
        let target_skill = target_skills.join("fault-skill");
        let target_lock = target_agents.join(".skill-lock.json");
        let projects = root.join("projects");
        fs::create_dir_all(&target_codex).unwrap();
        fs::create_dir_all(&target_skill).unwrap();
        fs::create_dir_all(&projects).unwrap();
        fs::write(target_skill.join("SKILL.md"), b"# Original Skill\n").unwrap();
        fs::write(target_skill.join("local-only.txt"), b"preserve me\n").unwrap();
        let target_lock_file = SkillLockFileV3 {
            version: 3,
            skills: BTreeMap::from([
                ("fault-skill".into(), lock_entry("target")),
                ("unrelated".into(), lock_entry("target")),
            ]),
            dismissed: Some(serde_json::json!({"keep": true})),
            last_selected_agents: Some(serde_json::json!(["codex"])),
        };
        let mut original_lock = serde_json::to_vec_pretty(&target_lock_file).unwrap();
        original_lock.push(b'\n');
        fs::write(&target_lock, &original_lock).unwrap();
        let target = TargetInventory {
            codex_home: target_codex,
            agents_skills_root: target_skills,
            skill_lock_path: target_lock.clone(),
            target_os: if cfg!(target_os = "macos") {
                SourceOs::Macos
            } else {
                SourceOs::Windows
            },
            target_arch: env::consts::ARCH.into(),
            counts: ContentCounts::default(),
            projects: vec![],
            conversations: vec![],
        };
        let content_id = preview.manifest.shared_skills[0].content_id;
        let resolutions = BTreeMap::from([(content_id, FileConflictResolution::UsePackage)]);
        let plan = build_restore_plan_with_skill_resolutions(
            &preview,
            &target,
            &projects,
            None,
            &resolutions,
        )
        .unwrap();
        (plan, target_skill, target_lock, original_lock)
    }

    fn lock_entry(reference: &str) -> SkillLockEntryV3 {
        SkillLockEntryV3 {
            source: "github".into(),
            source_type: "github".into(),
            source_url: "https://github.com/example/synthetic-skills".into(),
            r#ref: Some(reference.into()),
            skill_path: Some("skills/fault-skill".into()),
            skill_folder_hash: "a".repeat(64),
            installed_at: "2026-08-19T00:00:00Z".into(),
            updated_at: "2026-08-19T00:00:00Z".into(),
            plugin_name: None,
        }
    }

    struct EnvGuard {
        name: &'static str,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        fn capture(name: &'static str) -> Self {
            Self {
                name,
                previous: env::var_os(name),
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = self.previous.take() {
                env::set_var(self.name, value);
            } else {
                env::remove_var(self.name);
            }
        }
    }
}
