use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceOs {
    Windows,
    Macos,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PackageMode {
    Full,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillRootKind {
    SharedAgents,
    LegacyCodex,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillLockStatus {
    Available,
    Missing,
    ContentOnly,
    Invalid,
    Unsupported,
    NotApplicable,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContentCounts {
    pub projects: u64,
    pub project_files: u64,
    pub conversations: u64,
    pub skills: u64,
    pub plugins: u64,
    pub generated_images: u64,
    pub sqlite_threads: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectEntry {
    pub project_id: Uuid,
    pub name: String,
    pub source_path: String,
    #[serde(default = "project_source_available_by_default")]
    pub source_available: bool,
    pub archive_path: String,
    pub file_count: u64,
    pub content_bytes: u64,
    pub git_remote: Option<String>,
    pub git_branch: Option<String>,
    pub git_head: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProjectFileScanResult {
    Counted { project_id: Uuid, file_count: u64 },
    Failed { project_id: Uuid, message: String },
}

fn project_source_available_by_default() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationEntry {
    pub task_id: Uuid,
    pub project_id: Option<Uuid>,
    pub title: String,
    pub updated_at: String,
    pub content_hash: String,
    pub archive_path: String,
    pub classification: Option<ConversationClassification>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationClassification {
    pub parent_task_id: Option<Uuid>,
    pub agent_path: Option<String>,
    pub agent_nickname: Option<String>,
    pub depth: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OptionalContentEntry {
    pub content_id: Uuid,
    pub name: String,
    pub source_path: PathBuf,
    pub relative_path: String,
    pub size_bytes: u64,
    pub thumbnail_data_url: Option<String>,
    pub reveal_id: Option<Uuid>,
    #[serde(default)]
    pub skill_root_kind: Option<SkillRootKind>,
    #[serde(default)]
    pub lock_status: Option<SkillLockStatus>,
    #[serde(default)]
    pub exclusions: ExclusionSummary,
    #[serde(default)]
    pub blocked_reason: Option<String>,
    #[serde(default)]
    pub tree_hash: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExclusionSummary {
    pub excluded_files: u64,
    pub excluded_bytes: u64,
    pub rules: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SharedSkillEntry {
    pub content_id: Uuid,
    pub name: String,
    pub root_kind: SkillRootKind,
    pub relative_path: String,
    pub archive_root: String,
    pub file_count: u64,
    pub content_bytes: u64,
    pub tree_hash: String,
    #[serde(default)]
    pub exclusions: ExclusionSummary,
    pub lock_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillLockEntryV3 {
    pub source: String,
    pub source_type: String,
    pub source_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_path: Option<String>,
    pub skill_folder_hash: String,
    pub installed_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillLockFileV3 {
    pub version: u32,
    #[serde(default)]
    pub skills: BTreeMap<String, SkillLockEntryV3>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_selected_agents: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SharedSkillLockMetadata {
    pub archive_path: String,
    pub entry_count: u64,
    pub content_only_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageManifest {
    pub format: String,
    pub schema_version: u32,
    pub package_id: Uuid,
    pub created_at: String,
    pub source_os: SourceOs,
    pub source_arch: String,
    pub source_device_id: Uuid,
    pub mode: PackageMode,
    pub parent_checkpoint: Option<Uuid>,
    pub counts: ContentCounts,
    pub projects: Vec<ProjectEntry>,
    pub conversations: Vec<ConversationEntry>,
    pub exclusions: ExclusionSummary,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_skills: Vec<SharedSkillEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_skill_lock: Option<SharedSkillLockMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexInventory {
    pub codex_home: PathBuf,
    pub agents_skills_root: Option<PathBuf>,
    pub agents_skills_canonical_root: Option<PathBuf>,
    pub skill_lock_path: Option<PathBuf>,
    pub source_os: SourceOs,
    pub source_arch: String,
    pub source_device_id: Uuid,
    pub counts: ContentCounts,
    pub projects: Vec<ProjectEntry>,
    pub project_paths: Vec<PathBuf>,
    pub conversations: Vec<ConversationEntry>,
    pub conversation_paths: Vec<PathBuf>,
    pub session_index_path: Option<PathBuf>,
    pub state_db_path: Option<PathBuf>,
    pub skill_paths: Vec<PathBuf>,
    pub shared_skill_paths: Vec<PathBuf>,
    pub plugin_paths: Vec<PathBuf>,
    pub generated_image_paths: Vec<PathBuf>,
    pub skills: Vec<OptionalContentEntry>,
    pub shared_skills: Vec<OptionalContentEntry>,
    pub plugins: Vec<OptionalContentEntry>,
    pub generated_images: Vec<OptionalContentEntry>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetInventory {
    pub codex_home: PathBuf,
    pub agents_skills_root: PathBuf,
    pub skill_lock_path: PathBuf,
    pub target_os: SourceOs,
    pub target_arch: String,
    pub counts: ContentCounts,
    pub projects: Vec<ProjectEntry>,
    pub conversations: Vec<ConversationEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreatePackageRequest {
    pub codex_home: PathBuf,
    pub project_paths: Vec<PathBuf>,
    pub conversation_ids: Vec<Uuid>,
    pub output_path: PathBuf,
    pub source_device_id: Uuid,
    pub skill_paths: Vec<PathBuf>,
    pub shared_skill_paths: Vec<PathBuf>,
    pub plugin_paths: Vec<PathBuf>,
    pub generated_image_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreatePackageReport {
    pub package_path: PathBuf,
    pub package_id: Uuid,
    pub bytes_written: u64,
    pub counts: ContentCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackagePreview {
    pub package_path: PathBuf,
    pub archive_hash: String,
    pub manifest: PackageManifest,
    pub checksum_valid: bool,
    pub entries: Vec<String>,
    pub forbidden_files_total: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Add,
    Update,
    Unchanged,
    Preserve,
    Conflict,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileConflictResolution {
    KeepExisting,
    UsePackage,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RestoreRootKind {
    #[default]
    CodexHome,
    Projects,
    AgentsSkills,
    AgentsMetadata,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    #[default]
    File,
    SkillBundle,
    SkillLock,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackupKind {
    File,
    Directory,
    Absent,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionAction {
    Skip,
    Import,
    ImportAsBranch,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceRewriteKind {
    ConversationId,
    ConversationTitle,
    ProjectPath,
    SessionPath,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationStatus {
    Registered,
    CommandUnavailable,
    InvocationFailed { message: String },
    ManualOpenRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReferenceRewrite {
    pub source_task_id: Uuid,
    pub package_source: String,
    pub kind: ReferenceRewriteKind,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannedSession {
    pub package_source: String,
    pub target: PathBuf,
    pub source_task_id: Uuid,
    pub target_task_id: Uuid,
    pub title: String,
    pub source_content_hash: String,
    pub expected_final_content_hash: String,
    pub action: SessionAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannedOperation {
    pub package_source: String,
    pub target: PathBuf,
    pub expected_previous_hash: Option<String>,
    pub action: ChangeKind,
    pub rollback_required: bool,
    #[serde(default)]
    pub root_kind: RestoreRootKind,
    #[serde(default)]
    pub operation_kind: OperationKind,
    #[serde(default)]
    pub content_id: Option<Uuid>,
    #[serde(default)]
    pub expected_final_hash: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeVerificationRequirements {
    pub session_index: Option<PathBuf>,
    pub sqlite_database: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestorePlan {
    pub plan_id: Uuid,
    pub package_path: PathBuf,
    pub package_id: Uuid,
    pub archive_hash: String,
    pub target_codex_home: PathBuf,
    pub projects_root: PathBuf,
    pub target_agents_skills_root: PathBuf,
    pub target_skill_lock_path: PathBuf,
    pub operations: Vec<PlannedOperation>,
    pub sessions: Vec<PlannedSession>,
    pub reference_rewrites: Vec<ReferenceRewrite>,
    #[serde(default)]
    pub bridge_verification: BridgeVerificationRequirements,
    pub conflict_count: u64,
    pub required_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestoreOptions {
    pub codex_closed_confirmed: bool,
    pub backup_root: PathBuf,
    pub register_projects: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationReport {
    pub package_checksum_valid: bool,
    pub files_valid: bool,
    pub sessions_valid: bool,
    pub session_index_valid: bool,
    pub sqlite_threads_valid: bool,
    pub path_mapping_valid: bool,
    pub forbidden_files_absent: bool,
    pub project_files_valid: bool,
    pub app_registration_valid: bool,
    pub app_visible_ready: bool,
    pub shared_skill_files_valid: bool,
    pub codex_skill_discovery: VerificationStatus,
    pub skill_lock_merge: VerificationStatus,
    pub functional_sampling: VerificationStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Passed,
    Failed,
    Skipped,
    NotRun,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectRegistration {
    pub project_id: Uuid,
    pub project_path: PathBuf,
    pub status: RegistrationStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestoreReport {
    pub transaction_id: Uuid,
    pub package_id: Uuid,
    pub completed_at: String,
    pub restored_files: u64,
    pub restored_bytes: u64,
    pub registrations: Vec<ProjectRegistration>,
    pub verification: VerificationReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RollbackReport {
    pub transaction_id: Uuid,
    pub completed_at: String,
    pub restored_files: u64,
    pub success: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStatus {
    Prepared,
    Applying,
    Verifying,
    Committed,
    RollingBack,
    RolledBack,
    RollbackFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingRecovery {
    pub transaction_id: Uuid,
    pub package_id: Uuid,
    pub created_at: String,
    pub status: RecoveryStatus,
    pub backup_root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransactionSummary {
    pub transaction_id: Uuid,
    pub package_id: Uuid,
    pub created_at: String,
    pub status: RecoveryStatus,
    pub backup_root: PathBuf,
    pub transaction_backup_path: PathBuf,
    pub target_codex_home: PathBuf,
    pub projects_root: PathBuf,
    pub target_agents_skills_root: PathBuf,
    pub restored_project_paths: Vec<PathBuf>,
    pub changed_files: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransactionHistory {
    pub transactions: Vec<TransactionSummary>,
    pub warnings: Vec<String>,
}
