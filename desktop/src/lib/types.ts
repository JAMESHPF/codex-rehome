export type SourceOs = "windows" | "macos";
export type RecoveryStatus =
  | "prepared"
  | "applying"
  | "verifying"
  | "committed"
  | "rolling_back"
  | "rolled_back"
  | "rollback_failed";
export type ChangeKind = "add" | "update" | "unchanged" | "preserve" | "conflict";
export type FileConflictResolution = "keep_existing" | "use_package";
export type SkillRootKind = "shared_agents" | "legacy_codex";
export type SkillLockStatus =
  | "available"
  | "missing"
  | "content_only"
  | "invalid"
  | "unsupported"
  | "not_applicable";
export type RestoreRootKind =
  | "codex_home"
  | "projects"
  | "agents_skills"
  | "agents_metadata";
export type OperationKind = "file" | "skill_bundle" | "skill_lock";
export type VerificationStatus = "passed" | "failed" | "skipped" | "not_run";
export type RegistrationStatus =
  | "registered"
  | "command_unavailable"
  | "manual_open_required"
  | { invocation_failed: { message: string } };

export interface RehomeError {
  code: string;
  message: string;
}

export interface ContentCounts {
  projects: number;
  project_files: number;
  conversations: number;
  skills: number;
  plugins: number;
  generated_images: number;
  sqlite_threads: number;
}

export interface ProjectEntry {
  project_id: string;
  name: string;
  source_path: string;
  source_available: boolean;
  archive_path: string;
  file_count: number;
  content_bytes: number;
  git_remote: string | null;
  git_branch: string | null;
  git_head: string | null;
}

export interface ConversationEntry {
  task_id: string;
  project_id: string | null;
  title: string;
  updated_at: string;
  content_hash: string;
  archive_path: string;
  classification: {
    parent_task_id: string | null;
    agent_path: string | null;
    agent_nickname: string | null;
    depth: number | null;
  } | null;
}

export interface OptionalContentEntry {
  content_id: string;
  name: string;
  source_path: string;
  relative_path: string;
  size_bytes: number;
  thumbnail_data_url: string | null;
  reveal_id: string | null;
  skill_root_kind?: SkillRootKind | null;
  lock_status?: SkillLockStatus | null;
  exclusions?: ExclusionSummary;
  blocked_reason?: string | null;
  tree_hash?: string | null;
}

export interface ExclusionSummary {
  excluded_files: number;
  excluded_bytes: number;
  rules: string[];
}

export interface SharedSkillEntry {
  content_id: string;
  name: string;
  root_kind: SkillRootKind;
  relative_path: string;
  archive_root: string;
  file_count: number;
  content_bytes: number;
  tree_hash: string;
  exclusions: ExclusionSummary;
  lock_key: string | null;
}

export interface SharedSkillLockMetadata {
  archive_path: string;
  entry_count: number;
  content_only_count: number;
}

export interface CodexInventory {
  codex_home: string;
  agents_skills_root: string | null;
  agents_skills_canonical_root: string | null;
  skill_lock_path: string | null;
  source_os: SourceOs;
  source_arch: string;
  source_device_id: string;
  counts: ContentCounts;
  projects: ProjectEntry[];
  project_paths: string[];
  conversations: ConversationEntry[];
  conversation_paths: string[];
  session_index_path: string | null;
  state_db_path: string | null;
  skill_paths: string[];
  shared_skill_paths: string[];
  plugin_paths: string[];
  generated_image_paths: string[];
  skills: OptionalContentEntry[];
  shared_skills: OptionalContentEntry[];
  plugins: OptionalContentEntry[];
  generated_images: OptionalContentEntry[];
  warnings: string[];
}

export interface CreatePackageRequest {
  project_ids: string[];
  conversation_ids: string[];
  skill_ids: string[];
  shared_skill_ids: string[];
  plugin_ids: string[];
  generated_image_ids: string[];
}

export interface CreatePackageReport {
  package_path: string;
  package_id: string;
  bytes_written: number;
  counts: ContentCounts;
  archive_hash: string;
  reveal_id: string;
}

export interface PackageManifest {
  format: string;
  schema_version: number;
  package_id: string;
  created_at: string;
  source_os: SourceOs;
  source_arch: string;
  source_device_id: string;
  mode: "full";
  parent_checkpoint: string | null;
  counts: ContentCounts;
  projects: ProjectEntry[];
  conversations: ConversationEntry[];
  exclusions: ExclusionSummary;
  shared_skills?: SharedSkillEntry[];
  shared_skill_lock?: SharedSkillLockMetadata | null;
}

export interface PackagePreview {
  selection_id: string;
  package_path: string;
  archive_hash: string;
  manifest: PackageManifest;
  checksum_valid: boolean;
  entries: string[];
  forbidden_files_total: number;
}

export interface PlannedOperation {
  package_source: string;
  target: string;
  expected_previous_hash: string | null;
  action: ChangeKind;
  rollback_required: boolean;
  root_kind?: RestoreRootKind;
  operation_kind?: OperationKind;
  content_id?: string | null;
  expected_final_hash?: string | null;
}

export interface RestorePlan {
  plan_id: string;
  package_path: string;
  package_id: string;
  archive_hash: string;
  target_codex_home: string;
  projects_root: string;
  target_agents_skills_root: string;
  target_skill_lock_path: string;
  operations: PlannedOperation[];
  sessions: unknown[];
  reference_rewrites: unknown[];
  bridge_verification: {
    session_index: string | null;
    sqlite_database: string | null;
  };
  conflict_count: number;
  required_bytes: number;
}

export interface RestoreOptions {
  codex_closed_confirmed: boolean;
  register_projects: boolean;
}

export interface RestoreLocationSelection {
  selection_id: string;
  target_codex_home: string;
  projects_root: string;
  backup_root: string;
}

export interface VerificationReport {
  package_checksum_valid: boolean;
  files_valid: boolean;
  sessions_valid: boolean;
  session_index_valid: boolean;
  sqlite_threads_valid: boolean;
  path_mapping_valid: boolean;
  forbidden_files_absent: boolean;
  project_files_valid: boolean;
  app_registration_valid: boolean;
  app_visible_ready: boolean;
  shared_skill_files_valid: boolean;
  codex_skill_discovery: VerificationStatus;
  skill_lock_merge: VerificationStatus;
  functional_sampling: VerificationStatus;
}

export interface ProjectRegistration {
  project_id: string;
  project_path: string;
  status: RegistrationStatus;
}

export interface RestoreReport {
  transaction_id: string;
  package_id: string;
  completed_at: string;
  restored_files: number;
  restored_bytes: number;
  registrations: ProjectRegistration[];
  verification: VerificationReport;
}

export interface RollbackReport {
  transaction_id: string;
  completed_at: string;
  restored_files: number;
  success: boolean;
}

export interface TransactionSummary {
  transaction_id: string;
  package_id: string;
  created_at: string;
  status: RecoveryStatus;
  backup_root: string;
  transaction_backup_path: string;
  target_codex_home: string;
  projects_root: string;
  target_agents_skills_root: string;
  restored_project_paths: string[];
  changed_files: number;
}

export interface TransactionHistory {
  transactions: TransactionSummary[];
  warnings: string[];
}

export type RollbackAction = "rollback" | "resume";

export function errorMessage(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) {
    return String((error as { message: unknown }).message);
  }
  return String(error);
}

export function registrationIsComplete(status: RegistrationStatus): boolean {
  return status === "registered";
}
