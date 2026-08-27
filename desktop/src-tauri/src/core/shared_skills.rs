use crate::core::{
    error::{ErrorCode, RehomeError},
    exclusions::{classify_skill_path, SkillPathPolicy},
    models::{
        ChangeKind, ExclusionSummary, OptionalContentEntry, SharedSkillEntry, SkillLockEntryV3,
        SkillLockFileV3, SkillLockStatus, SkillRootKind, SourceOs,
    },
    paths::normalize_entry,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::{self, Read},
    path::{Path, PathBuf},
};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;
use walkdir::WalkDir;

const SECRET_SCAN_CHUNK_BYTES: usize = 64 * 1024;
const SECRET_SCAN_OVERLAP_BYTES: usize = 256;
const TREE_HASH_DOMAIN: &[u8] = b"rehome-shared-skill-tree-v1\0";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharedSkillsContext {
    pub user_profile: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub xdg_state_home: Option<PathBuf>,
}

impl SharedSkillsContext {
    pub fn from_process_environment() -> Self {
        Self {
            user_profile: env::var_os("USERPROFILE").map(PathBuf::from),
            home: env::var_os("HOME").map(PathBuf::from),
            xdg_state_home: env::var_os("XDG_STATE_HOME").map(PathBuf::from),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SkillFile {
    pub source_path: PathBuf,
    pub relative_path: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct SharedSkillScan {
    pub entry: OptionalContentEntry,
    pub files: Vec<SkillFile>,
}

#[derive(Debug, Clone)]
pub(crate) struct SharedSkillsInventory {
    pub canonical_root: Option<PathBuf>,
    pub entries: Vec<OptionalContentEntry>,
    pub bundle_paths: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockLoadStatus {
    Available,
    Missing,
    Invalid,
    Unsupported,
}

#[derive(Debug, Clone)]
pub(crate) enum SupportedLock {
    Available(SkillLockFileV3),
    Missing,
    Unusable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LockMergeResult {
    Write(Vec<u8>),
    Unchanged,
    SkippedInvalidTarget,
}

pub fn resolve_user_home(context: &SharedSkillsContext, source_os: SourceOs) -> Option<PathBuf> {
    let value = match source_os {
        SourceOs::Windows => context.user_profile.clone(),
        SourceOs::Macos => context.home.clone(),
    }?;
    (!value.as_os_str().is_empty()).then_some(value)
}

pub fn resolve_agents_skills_root(
    context: &SharedSkillsContext,
    source_os: SourceOs,
) -> Option<PathBuf> {
    resolve_user_home(context, source_os).map(|home| home.join(".agents").join("skills"))
}

pub fn resolve_skill_lock_path(
    context: &SharedSkillsContext,
    source_os: SourceOs,
) -> Option<PathBuf> {
    if let Some(state) = context
        .xdg_state_home
        .as_ref()
        .filter(|path| !path.as_os_str().is_empty())
    {
        return Some(state.join("skills").join(".skill-lock.json"));
    }
    resolve_user_home(context, source_os).map(|home| home.join(".agents").join(".skill-lock.json"))
}

pub(crate) fn discover_shared_skills(
    logical_root: PathBuf,
    lock_path: PathBuf,
) -> Result<SharedSkillsInventory, RehomeError> {
    let (lock_file, lock_load_status, lock_warning) = load_lock_file(&lock_path);
    let mut warnings = lock_warning.into_iter().collect::<Vec<_>>();
    let root_metadata = match fs::symlink_metadata(&logical_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(SharedSkillsInventory {
                canonical_root: None,
                entries: Vec::new(),
                bundle_paths: Vec::new(),
                warnings,
            })
        }
        Err(error) => {
            return Err(discovery_failed(format!(
                "could not inspect shared Skills root: {error}"
            )))
        }
    };
    if !root_metadata.is_dir() && !root_metadata.file_type().is_symlink() {
        return Err(discovery_failed(
            "shared Skills root is not a directory or directory symlink",
        ));
    }
    let canonical_root = fs::canonicalize(&logical_root).map_err(|error| {
        discovery_failed(format!("could not resolve shared Skills root: {error}"))
    })?;
    let canonical_metadata = fs::symlink_metadata(&canonical_root).map_err(|error| {
        discovery_failed(format!(
            "could not inspect resolved shared Skills root: {error}"
        ))
    })?;
    if !canonical_metadata.is_dir() || canonical_metadata.file_type().is_symlink() {
        return Err(discovery_failed(
            "resolved shared Skills root is not a real directory",
        ));
    }

    let mut children = fs::read_dir(&canonical_root)
        .map_err(|error| discovery_failed(format!("could not read shared Skills root: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| discovery_failed(format!("could not read shared Skill entry: {error}")))?;
    children.sort_by_key(|entry| entry.file_name());

    let mut scans = Vec::new();
    for child in children {
        let name = child.file_name().to_string_lossy().into_owned();
        if name == ".system" {
            continue;
        }
        let source = child.path();
        let metadata = fs::symlink_metadata(&source).map_err(|error| {
            discovery_failed(format!("could not inspect shared Skill {name}: {error}"))
        })?;
        if metadata.file_type().is_symlink() {
            scans.push(blocked_top_level_entry(
                &source,
                &name,
                "top-level Skill is a symbolic link or reparse point",
                lock_status_for(&name, lock_file.as_ref(), lock_load_status),
            ));
            continue;
        }
        if !metadata.is_dir() {
            warnings.push(format!(
                "Ignored non-directory entry in shared Skills root: {}",
                source.display()
            ));
            continue;
        }
        if !source.join("SKILL.md").is_file() {
            warnings.push(format!(
                "Ignored shared Skills directory without top-level SKILL.md: {}",
                source.display()
            ));
            continue;
        }
        let mut scan = scan_shared_skill_bundle(&canonical_root, &source)?;
        scan.entry.lock_status = Some(lock_status_for(
            &scan.entry.relative_path,
            lock_file.as_ref(),
            lock_load_status,
        ));
        scans.push(scan);
    }

    mark_portable_name_collisions(&mut scans);
    scans.sort_by(|left, right| {
        left.entry
            .name
            .cmp(&right.entry.name)
            .then(left.entry.relative_path.cmp(&right.entry.relative_path))
    });
    let entries = scans.iter().map(|scan| scan.entry.clone()).collect();
    let bundle_paths = scans
        .iter()
        .map(|scan| scan.entry.source_path.clone())
        .collect();

    Ok(SharedSkillsInventory {
        canonical_root: Some(canonical_root),
        entries,
        bundle_paths,
        warnings,
    })
}

pub(crate) fn scan_shared_skill_bundle(
    canonical_root: &Path,
    bundle_root: &Path,
) -> Result<SharedSkillScan, RehomeError> {
    let canonical_bundle = fs::canonicalize(bundle_root)
        .map_err(|error| discovery_failed(format!("could not resolve shared Skill: {error}")))?;
    if !canonical_bundle.starts_with(canonical_root) {
        return Err(discovery_failed("shared Skill escapes its canonical root"));
    }
    let relative = canonical_bundle
        .strip_prefix(canonical_root)
        .map_err(|_| discovery_failed("shared Skill escapes its canonical root"))?;
    let relative_path = match portable_relative_path(relative) {
        Ok(path) => path,
        Err(error) => {
            return Ok(blocked_top_level_entry(
                &canonical_bundle,
                &relative.to_string_lossy(),
                &error.message,
                SkillLockStatus::ContentOnly,
            ))
        }
    };
    if relative_path.contains('/') {
        return Err(discovery_failed(
            "shared Skill bundle must be a direct child of the shared root",
        ));
    }

    let mut included = Vec::new();
    let mut exclusions = ExclusionSummary::default();
    let mut exclusion_rules = BTreeSet::new();
    let mut blocked_reasons = BTreeSet::new();
    let mut portable_paths = BTreeMap::new();

    for entry in WalkDir::new(&canonical_bundle)
        .follow_links(false)
        .sort_by_file_name()
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                blocked_reasons.insert(format!("could not walk Skill tree: {error}"));
                continue;
            }
        };
        if entry.path() == canonical_bundle {
            continue;
        }
        let relative = match entry.path().strip_prefix(&canonical_bundle) {
            Ok(relative) => relative,
            Err(_) => {
                blocked_reasons.insert("Skill entry escapes its bundle root".to_owned());
                continue;
            }
        };
        let metadata = match fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(error) => {
                blocked_reasons
                    .insert(format!("could not inspect {}: {error}", relative.display()));
                continue;
            }
        };
        if metadata_is_link_or_reparse(&metadata) {
            blocked_reasons.insert(format!(
                "symbolic link or reparse point: {}",
                relative.display()
            ));
            continue;
        }
        if metadata.is_dir() {
            if let Err(error) = portable_relative_path(relative) {
                blocked_reasons.insert(format!("{}: {}", relative.display(), error.message));
            }
            continue;
        }
        if !metadata.is_file() {
            blocked_reasons.insert(format!("special filesystem entry: {}", relative.display()));
            continue;
        }

        match classify_skill_path(relative) {
            SkillPathPolicy::Block(reason) => {
                exclusions.excluded_files += 1;
                exclusions.excluded_bytes =
                    exclusions.excluded_bytes.saturating_add(metadata.len());
                exclusion_rules.insert(reason.to_owned());
                blocked_reasons.insert(format!("{reason}: {}", relative.display()));
                continue;
            }
            SkillPathPolicy::Exclude(reason) => {
                exclusions.excluded_files += 1;
                exclusions.excluded_bytes =
                    exclusions.excluded_bytes.saturating_add(metadata.len());
                exclusion_rules.insert(reason.to_owned());
                continue;
            }
            SkillPathPolicy::Include => {}
        }

        let normalized = match portable_relative_path(relative) {
            Ok(path) => path,
            Err(error) => {
                blocked_reasons.insert(format!("{}: {}", relative.display(), error.message));
                continue;
            }
        };
        let collision_key = portable_collision_key(&normalized);
        if let Some(previous) = portable_paths.insert(collision_key, normalized.clone()) {
            blocked_reasons.insert(format!(
                "portable path collision: {previous} and {normalized}"
            ));
            continue;
        }
        match contains_high_confidence_secret(entry.path()) {
            Ok(true) => {
                exclusions.excluded_files += 1;
                exclusions.excluded_bytes =
                    exclusions.excluded_bytes.saturating_add(metadata.len());
                exclusion_rules.insert("high-confidence credential content".to_owned());
                blocked_reasons.insert(format!(
                    "high-confidence credential content: {}",
                    relative.display()
                ));
                continue;
            }
            Ok(false) => {}
            Err(error) => {
                blocked_reasons.insert(format!(
                    "could not scan {} for credentials: {error}",
                    relative.display()
                ));
                continue;
            }
        }
        included.push(SkillFile {
            source_path: entry.path().to_path_buf(),
            relative_path: normalized,
            size_bytes: metadata.len(),
        });
    }

    included.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    exclusions.rules = exclusion_rules.into_iter().collect();
    let content_bytes = included.iter().try_fold(0_u64, |total, file| {
        total
            .checked_add(file.size_bytes)
            .ok_or_else(|| discovery_failed("shared Skill size overflowed"))
    })?;
    let tree_hash = hash_skill_files(&included)?;
    let name = canonical_bundle
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("skill")
        .to_owned();
    let blocked_reason = (!blocked_reasons.is_empty())
        .then(|| blocked_reasons.into_iter().collect::<Vec<_>>().join("; "));
    let content_id = shared_skill_content_id(&relative_path);
    Ok(SharedSkillScan {
        entry: OptionalContentEntry {
            content_id,
            name,
            source_path: canonical_bundle,
            relative_path,
            size_bytes: content_bytes,
            thumbnail_data_url: None,
            reveal_id: None,
            skill_root_kind: Some(SkillRootKind::SharedAgents),
            lock_status: None,
            exclusions,
            blocked_reason,
            tree_hash: Some(tree_hash),
        },
        files: included,
    })
}

pub(crate) fn manifest_entry_from_scan(
    scan: &SharedSkillScan,
    lock_key: Option<String>,
) -> Result<SharedSkillEntry, RehomeError> {
    if let Some(reason) = scan.entry.blocked_reason.as_deref() {
        return Err(discovery_failed(format!(
            "shared Skill {} is blocked: {reason}",
            scan.entry.relative_path
        )));
    }
    Ok(SharedSkillEntry {
        content_id: scan.entry.content_id,
        name: scan.entry.name.clone(),
        root_kind: SkillRootKind::SharedAgents,
        relative_path: scan.entry.relative_path.clone(),
        archive_root: format!("agents/skills/{}", scan.entry.relative_path),
        file_count: scan.files.len() as u64,
        content_bytes: scan.entry.size_bytes,
        tree_hash: scan
            .entry
            .tree_hash
            .clone()
            .ok_or_else(|| discovery_failed("shared Skill tree hash is missing"))?,
        exclusions: scan.entry.exclusions.clone(),
        lock_key,
    })
}

pub(crate) fn shared_skill_content_id(relative_path: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("shared_agents:{relative_path}").as_bytes(),
    )
}

pub(crate) fn sanitize_lock_entry(entry: &SkillLockEntryV3) -> Option<SkillLockEntryV3> {
    if !safe_source_field(&entry.source, 2048)
        || !safe_plain_field(&entry.source_type, 128)
        || !safe_source_url(&entry.source_url)
        || !safe_hash(&entry.skill_folder_hash)
        || !safe_plain_field(&entry.installed_at, 128)
        || !safe_plain_field(&entry.updated_at, 128)
        || entry.r#ref.as_deref().is_some_and(|value| !safe_ref(value))
        || entry
            .skill_path
            .as_deref()
            .is_some_and(|value| !safe_relative_lock_path(value))
        || entry
            .plugin_name
            .as_deref()
            .is_some_and(|value| !safe_plain_field(value, 256))
    {
        return None;
    }
    Some(entry.clone())
}

pub(crate) fn read_supported_lock(path: &Path) -> SupportedLock {
    let (lock, status, _) = load_lock_file(path);
    match (lock, status) {
        (Some(lock), LockLoadStatus::Available) => SupportedLock::Available(lock),
        (_, LockLoadStatus::Missing) => SupportedLock::Missing,
        _ => SupportedLock::Unusable,
    }
}

pub(crate) fn merge_skill_lock(
    package_lock: &SkillLockFileV3,
    target_bytes: Option<&[u8]>,
    bundle_decisions: &BTreeMap<String, ChangeKind>,
) -> Result<LockMergeResult, RehomeError> {
    if package_lock.version != 3
        || package_lock
            .skills
            .values()
            .any(|entry| sanitize_lock_entry(entry).as_ref() != Some(entry))
    {
        return Err(discovery_failed(
            "package Skill lock is not a sanitized v3 document",
        ));
    }
    let original = match target_bytes {
        Some(bytes) => match serde_json::from_slice::<SkillLockFileV3>(bytes) {
            Ok(lock) if lock.version == 3 => Some(lock),
            _ => return Ok(LockMergeResult::SkippedInvalidTarget),
        },
        None => None,
    };
    let mut merged = original.clone().unwrap_or(SkillLockFileV3 {
        version: 3,
        skills: BTreeMap::new(),
        dismissed: None,
        last_selected_agents: None,
    });
    for (key, decision) in bundle_decisions {
        match decision {
            ChangeKind::Add | ChangeKind::Update => {
                if let Some(source) = package_lock.skills.get(key) {
                    merged.skills.insert(key.clone(), source.clone());
                } else {
                    merged.skills.remove(key);
                }
            }
            ChangeKind::Unchanged => {
                if !merged.skills.contains_key(key) {
                    if let Some(source) = package_lock.skills.get(key) {
                        merged.skills.insert(key.clone(), source.clone());
                    }
                }
            }
            ChangeKind::Preserve | ChangeKind::Conflict => {}
        }
    }
    if original.is_none()
        && merged.skills.is_empty()
        && merged.dismissed.is_none()
        && merged.last_selected_agents.is_none()
    {
        return Ok(LockMergeResult::Unchanged);
    }
    if original.as_ref() == Some(&merged) {
        return Ok(LockMergeResult::Unchanged);
    }
    let mut bytes = serde_json::to_vec_pretty(&merged).map_err(|error| {
        discovery_failed(format!("could not serialize merged Skill lock: {error}"))
    })?;
    bytes.push(b'\n');
    Ok(LockMergeResult::Write(bytes))
}

pub(crate) fn tree_hash_from_payload_records<'a>(
    records: impl IntoIterator<Item = (&'a str, u64, &'a str)>,
) -> Result<String, RehomeError> {
    let mut records = records.into_iter().collect::<Vec<_>>();
    records.sort_by(|left, right| left.0.cmp(right.0));
    let mut tree = Sha256::new();
    tree.update(TREE_HASH_DOMAIN);
    for (relative_path, size_bytes, content_hash) in records {
        let path_bytes = relative_path.as_bytes();
        tree.update((path_bytes.len() as u64).to_be_bytes());
        tree.update(path_bytes);
        tree.update(size_bytes.to_be_bytes());
        tree.update(decode_sha256(content_hash)?);
    }
    Ok(format!("{:x}", tree.finalize()))
}

pub(crate) fn tree_hash(root: &Path) -> Result<String, RehomeError> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| discovery_failed(format!("could not inspect Skill tree: {error}")))?;
    if metadata_is_link_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(discovery_failed("Skill tree root is not a real directory"));
    }
    let parent = root
        .parent()
        .ok_or_else(|| discovery_failed("Skill tree root has no parent"))?;
    let scan = scan_shared_skill_bundle(parent, root)?;
    if scan.entry.blocked_reason.is_some() {
        return Err(discovery_failed("Skill tree contains blocked entries"));
    }
    scan.entry
        .tree_hash
        .ok_or_else(|| discovery_failed("Skill tree hash is missing"))
}

fn blocked_top_level_entry(
    source: &Path,
    name: &str,
    reason: &str,
    lock_status: SkillLockStatus,
) -> SharedSkillScan {
    let relative_path = name.to_owned();
    SharedSkillScan {
        entry: OptionalContentEntry {
            content_id: shared_skill_content_id(&relative_path),
            name: name.to_owned(),
            source_path: source.to_path_buf(),
            relative_path,
            size_bytes: 0,
            thumbnail_data_url: None,
            reveal_id: None,
            skill_root_kind: Some(SkillRootKind::SharedAgents),
            lock_status: Some(lock_status),
            exclusions: ExclusionSummary::default(),
            blocked_reason: Some(reason.to_owned()),
            tree_hash: None,
        },
        files: Vec::new(),
    }
}

fn mark_portable_name_collisions(scans: &mut [SharedSkillScan]) {
    let mut groups = BTreeMap::<String, Vec<usize>>::new();
    for (index, scan) in scans.iter().enumerate() {
        groups
            .entry(portable_collision_key(&scan.entry.relative_path))
            .or_default()
            .push(index);
    }
    for indices in groups.values().filter(|indices| indices.len() > 1) {
        let names = indices
            .iter()
            .map(|index| scans[*index].entry.relative_path.clone())
            .collect::<Vec<_>>()
            .join(", ");
        for index in indices {
            let reason = format!("portable Skill-name collision: {names}");
            scans[*index].entry.blocked_reason =
                Some(match scans[*index].entry.blocked_reason.take() {
                    Some(existing) => format!("{existing}; {reason}"),
                    None => reason,
                });
        }
    }
}

fn load_lock_file(path: &Path) -> (Option<SkillLockFileV3>, LockLoadStatus, Option<String>) {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return (None, LockLoadStatus::Missing, None)
        }
        Err(error) => {
            return (
                None,
                LockLoadStatus::Invalid,
                Some(format!("Could not read shared Skills lock: {error}")),
            )
        }
    };
    let value = match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(value) => value,
        Err(_) => {
            return (
                None,
                LockLoadStatus::Invalid,
                Some("Shared Skills lock is invalid JSON".to_owned()),
            )
        }
    };
    let Some(object) = value.as_object() else {
        return (
            None,
            LockLoadStatus::Invalid,
            Some("Shared Skills lock is not a JSON object".to_owned()),
        );
    };
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "version" | "skills" | "dismissed" | "lastSelectedAgents"
        )
    }) {
        return (
            None,
            LockLoadStatus::Invalid,
            Some("Shared Skills lock has unsupported top-level fields".to_owned()),
        );
    }
    let Some(version) = object.get("version").and_then(serde_json::Value::as_u64) else {
        return (
            None,
            LockLoadStatus::Invalid,
            Some("Shared Skills lock has no numeric version".to_owned()),
        );
    };
    if version != 3 {
        return (
            None,
            LockLoadStatus::Unsupported,
            Some(format!(
                "Shared Skills lock version {version} is unsupported; only v3 is read"
            )),
        );
    }
    let Some(raw_skills) = object.get("skills").and_then(serde_json::Value::as_object) else {
        return (
            None,
            LockLoadStatus::Invalid,
            Some("Shared Skills lock has no skills object".to_owned()),
        );
    };
    let mut skills = BTreeMap::new();
    let mut unsupported_entries = 0_u64;
    for (key, raw_entry) in raw_skills {
        match serde_json::from_value::<SkillLockEntryV3>(raw_entry.clone()) {
            Ok(entry) => {
                skills.insert(key.clone(), entry);
            }
            Err(_) => unsupported_entries += 1,
        }
    }
    let warning = (unsupported_entries > 0).then(|| {
        format!(
            "Ignored {unsupported_entries} shared Skill lock entries with unsupported or malformed fields; their content can still migrate"
        )
    });
    (
        Some(SkillLockFileV3 {
            version: 3,
            skills,
            dismissed: object.get("dismissed").cloned(),
            last_selected_agents: object.get("lastSelectedAgents").cloned(),
        }),
        LockLoadStatus::Available,
        warning,
    )
}

fn lock_status_for(
    key: &str,
    lock: Option<&SkillLockFileV3>,
    status: LockLoadStatus,
) -> SkillLockStatus {
    match status {
        LockLoadStatus::Missing => SkillLockStatus::Missing,
        LockLoadStatus::Invalid => SkillLockStatus::Invalid,
        LockLoadStatus::Unsupported => SkillLockStatus::Unsupported,
        LockLoadStatus::Available => lock
            .and_then(|lock| lock.skills.get(key))
            .and_then(sanitize_lock_entry)
            .map(|_| SkillLockStatus::Available)
            .unwrap_or(SkillLockStatus::ContentOnly),
    }
}

fn hash_skill_files(files: &[SkillFile]) -> Result<String, RehomeError> {
    let mut tree = Sha256::new();
    tree.update(TREE_HASH_DOMAIN);
    for file in files {
        let path_bytes = file.relative_path.as_bytes();
        tree.update((path_bytes.len() as u64).to_be_bytes());
        tree.update(path_bytes);
        tree.update(file.size_bytes.to_be_bytes());
        let mut source = fs::File::open(&file.source_path).map_err(|error| {
            discovery_failed(format!("could not read shared Skill file: {error}"))
        })?;
        let mut file_hash = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = source.read(&mut buffer).map_err(|error| {
                discovery_failed(format!("could not hash shared Skill file: {error}"))
            })?;
            if read == 0 {
                break;
            }
            file_hash.update(&buffer[..read]);
        }
        tree.update(file_hash.finalize());
    }
    Ok(format!("{:x}", tree.finalize()))
}

fn decode_sha256(value: &str) -> Result<[u8; 32], RehomeError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(discovery_failed("Skill payload hash is not SHA-256"));
    }
    let mut decoded = [0_u8; 32];
    for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let high = hex_nibble(pair[0]).ok_or_else(|| discovery_failed("invalid SHA-256 hash"))?;
        let low = hex_nibble(pair[1]).ok_or_else(|| discovery_failed("invalid SHA-256 hash"))?;
        decoded[index] = (high << 4) | low;
    }
    Ok(decoded)
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn portable_relative_path(path: &Path) -> Result<String, RehomeError> {
    let normalized = normalize_entry(path)?;
    for component in normalized.split('/') {
        if component.encode_utf16().count() > 255 {
            return Err(discovery_failed(
                "path component exceeds the Windows UTF-16 length limit",
            ));
        }
    }
    Ok(normalized)
}

fn portable_collision_key(value: &str) -> String {
    value.nfc().collect::<String>().to_lowercase()
}

fn contains_high_confidence_secret(path: &Path) -> io::Result<bool> {
    let mut source = fs::File::open(path)?;
    let mut buffer = [0_u8; SECRET_SCAN_CHUNK_BYTES];
    let mut tail = Vec::new();
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            return Ok(false);
        }
        let mut window = Vec::with_capacity(tail.len() + read);
        window.extend_from_slice(&tail);
        window.extend_from_slice(&buffer[..read]);
        if high_confidence_secret_bytes(&window) {
            return Ok(true);
        }
        let keep = window.len().min(SECRET_SCAN_OVERLAP_BYTES);
        tail.clear();
        tail.extend_from_slice(&window[window.len() - keep..]);
    }
}

fn high_confidence_secret_bytes(bytes: &[u8]) -> bool {
    contains_private_key_header(bytes)
        || [
            (b"ghp_".as_slice(), 36),
            (b"github_pat_", 48),
            (b"sk-", 32),
            (b"xoxb-", 32),
        ]
        .into_iter()
        .any(|(prefix, minimum)| contains_token(bytes, prefix, minimum))
}

fn contains_private_key_header(bytes: &[u8]) -> bool {
    const BEGIN: &[u8] = b"-----BEGIN ";
    const END: &[u8] = b"PRIVATE KEY-----";
    bytes
        .windows(BEGIN.len())
        .enumerate()
        .any(|(index, candidate)| {
            candidate == BEGIN
                && bytes[index..bytes.len().min(index + SECRET_SCAN_OVERLAP_BYTES)]
                    .windows(END.len())
                    .any(|candidate| candidate == END)
        })
}

fn contains_token(bytes: &[u8], prefix: &[u8], minimum: usize) -> bool {
    bytes
        .windows(prefix.len())
        .enumerate()
        .any(|(index, candidate)| {
            if candidate != prefix {
                return false;
            }
            let length = bytes[index + prefix.len()..]
                .iter()
                .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                .count()
                + prefix.len();
            length >= minimum
        })
}

fn safe_plain_field(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && !value.chars().any(|character| character.is_control())
        && !looks_like_absolute_local_path(value)
        && !high_confidence_secret_bytes(value.as_bytes())
}

fn safe_source_field(value: &str, max_bytes: usize) -> bool {
    safe_plain_field(value, max_bytes)
        && !value.contains(['?', '#', '@'])
        && (!value.contains("://") || safe_source_url(value))
}

fn safe_source_url(value: &str) -> bool {
    if !safe_plain_field(value, 4096)
        || value.contains('?')
        || value.contains('#')
        || value.starts_with("file:")
    {
        return false;
    }
    if let Some((_, authority_and_path)) = value.split_once("://") {
        let authority = authority_and_path.split('/').next().unwrap_or_default();
        if authority.contains('@') || authority.is_empty() {
            return false;
        }
    }
    true
}

fn safe_ref(value: &str) -> bool {
    safe_plain_field(value, 512)
        && !value.starts_with('-')
        && !value.contains("..")
        && !value.contains(['?', '#', ':', '\\'])
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._/-".contains(character))
}

fn safe_relative_lock_path(value: &str) -> bool {
    if looks_like_absolute_local_path(value) {
        return false;
    }
    normalize_entry(Path::new(value)).is_ok()
}

fn safe_hash(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn looks_like_absolute_local_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.starts_with('/')
        || value.starts_with('\\')
        || value.starts_with('~')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
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

fn discovery_failed(message: impl Into<String>) -> RehomeError {
    RehomeError::new(ErrorCode::PackageInvalid, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_lock_entry() -> SkillLockEntryV3 {
        SkillLockEntryV3 {
            source: "github".into(),
            source_type: "github".into(),
            source_url: "https://github.com/example/skills".into(),
            r#ref: Some("main".into()),
            skill_path: Some("skills/safe-skill".into()),
            skill_folder_hash: "a".repeat(64),
            installed_at: "2026-08-19T00:00:00Z".into(),
            updated_at: "2026-08-19T00:00:00Z".into(),
            plugin_name: None,
        }
    }

    fn write_skill(root: &Path, name: &str, body: &[u8]) -> PathBuf {
        let skill = root.join(name);
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), body).unwrap();
        skill
    }

    #[test]
    fn resolve_agents_root_is_independent_from_codex_home() {
        let context = SharedSkillsContext {
            user_profile: Some(PathBuf::from(r"C:\\Users\\pinfei")),
            home: Some(PathBuf::from("/Users/pinfei")),
            xdg_state_home: None,
        };
        assert_eq!(
            resolve_agents_skills_root(&context, SourceOs::Macos),
            Some(PathBuf::from("/Users/pinfei/.agents/skills"))
        );
        assert_eq!(
            resolve_agents_skills_root(&context, SourceOs::Windows),
            Some(
                PathBuf::from(r"C:\\Users\\pinfei")
                    .join(".agents")
                    .join("skills")
            )
        );
    }

    #[test]
    fn resolve_xdg_changes_only_lock_path() {
        let context = SharedSkillsContext {
            home: Some(PathBuf::from("/Users/pinfei")),
            xdg_state_home: Some(PathBuf::from("/state")),
            ..SharedSkillsContext::default()
        };
        assert_eq!(
            resolve_agents_skills_root(&context, SourceOs::Macos),
            Some(PathBuf::from("/Users/pinfei/.agents/skills"))
        );
        assert_eq!(
            resolve_skill_lock_path(&context, SourceOs::Macos),
            Some(PathBuf::from("/state/skills/.skill-lock.json"))
        );
    }

    #[test]
    fn resolve_default_lock_path_uses_agents_metadata_root() {
        let context = SharedSkillsContext {
            home: Some(PathBuf::from("/Users/pinfei")),
            ..SharedSkillsContext::default()
        };
        assert_eq!(
            resolve_skill_lock_path(&context, SourceOs::Macos),
            Some(PathBuf::from("/Users/pinfei/.agents/.skill-lock.json"))
        );
    }

    #[test]
    fn discovery_reports_exclusions_sensitive_files_and_lock_status_without_values() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(".agents").join("skills");
        fs::create_dir_all(&root).unwrap();
        let safe = write_skill(&root, "safe-skill", b"# Safe\n");
        fs::create_dir_all(safe.join("node_modules")).unwrap();
        fs::write(safe.join("node_modules").join("ignored.js"), b"ignored").unwrap();
        fs::write(safe.join("guide.md"), b"guide").unwrap();
        let blocked = write_skill(&root, "blocked-skill", b"# Blocked\n");
        let fake_secret = format!("TOKEN=ghp_{}", "x".repeat(40));
        fs::write(blocked.join("notes.txt"), fake_secret.as_bytes()).unwrap();
        let mut large_secret = vec![b'a'; SECRET_SCAN_CHUNK_BYTES * 17];
        large_secret.extend_from_slice(format!("\ngithub_pat_{}\n", "z".repeat(48)).as_bytes());
        fs::write(blocked.join("large-notes.txt"), large_secret).unwrap();
        fs::write(blocked.join(".env"), b"SYNTHETIC_ONLY=1").unwrap();

        let lock_path = temp.path().join(".agents").join(".skill-lock.json");
        let lock = SkillLockFileV3 {
            version: 3,
            skills: BTreeMap::from([("safe-skill".into(), sample_lock_entry())]),
            dismissed: Some(json!(["unrelated"])),
            last_selected_agents: Some(json!(["codex"])),
        };
        fs::write(&lock_path, serde_json::to_vec(&lock).unwrap()).unwrap();

        let inventory = discover_shared_skills(root, lock_path).unwrap();
        let safe = inventory
            .entries
            .iter()
            .find(|entry| entry.name == "safe-skill")
            .unwrap();
        assert_eq!(safe.lock_status, Some(SkillLockStatus::Available));
        assert_eq!(safe.exclusions.excluded_files, 1);
        assert!(safe.blocked_reason.is_none());
        assert!(safe.tree_hash.is_some());

        let blocked = inventory
            .entries
            .iter()
            .find(|entry| entry.name == "blocked-skill")
            .unwrap();
        let reason = blocked.blocked_reason.as_deref().unwrap();
        assert!(reason.contains("high-confidence credential content"));
        assert!(reason.contains("sensitive credential or private-key path"));
        assert!(!reason.contains("ghp_"));
        assert_eq!(blocked.lock_status, Some(SkillLockStatus::ContentOnly));
        assert_eq!(blocked.exclusions.excluded_files, 3);
    }

    #[cfg(unix)]
    #[test]
    fn shared_root_symlink_is_allowed_but_nested_and_special_entries_are_blocked() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt, os::unix::fs::symlink};

        let temp = tempfile::tempdir().unwrap();
        let actual = temp.path().join("actual-skills");
        fs::create_dir_all(&actual).unwrap();
        let skill = write_skill(&actual, "linked-root-skill", b"# Linked root\n");
        symlink(skill.join("SKILL.md"), skill.join("nested-link")).unwrap();
        let fifo = skill.join("events.fifo");
        let fifo_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `fifo_path` is a NUL-terminated path owned by this test, and
        // it remains alive for the duration of the libc call.
        assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);

        let logical_parent = temp.path().join("home").join(".agents");
        fs::create_dir_all(&logical_parent).unwrap();
        let logical = logical_parent.join("skills");
        symlink(&actual, &logical).unwrap();

        let inventory =
            discover_shared_skills(logical, logical_parent.join(".skill-lock.json")).unwrap();
        assert_eq!(
            inventory.canonical_root,
            Some(fs::canonicalize(actual).unwrap())
        );
        let entry = &inventory.entries[0];
        let reason = entry.blocked_reason.as_deref().unwrap();
        assert!(reason.contains("symbolic link"));
        assert!(reason.contains("special filesystem entry"));
    }

    #[test]
    fn portable_skill_names_and_paths_detect_case_unicode_reserved_and_utf16_collisions() {
        let mut scans = vec![
            blocked_top_level_entry(Path::new("Foo"), "Foo", "first", SkillLockStatus::Missing),
            blocked_top_level_entry(Path::new("foo"), "foo", "second", SkillLockStatus::Missing),
        ];
        mark_portable_name_collisions(&mut scans);
        assert!(scans.iter().all(|scan| scan
            .entry
            .blocked_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("portable Skill-name collision"))));
        assert_eq!(
            portable_collision_key("e\u{301}"),
            portable_collision_key("é")
        );
        assert!(portable_relative_path(Path::new("CON")).is_err());
        assert!(portable_relative_path(Path::new(&"a".repeat(256))).is_err());
    }

    #[test]
    fn lock_sanitizer_downgrades_queries_credentials_absolute_paths_and_invalid_refs() {
        let mut entry = sample_lock_entry();
        entry.source_url = "https://user@example.com/repo".into();
        assert!(sanitize_lock_entry(&entry).is_none());
        entry = sample_lock_entry();
        entry.source_url = "https://example.com/repo?token=synthetic".into();
        assert!(sanitize_lock_entry(&entry).is_none());
        entry = sample_lock_entry();
        entry.source = "https://user@example.com/private/repo".into();
        assert!(sanitize_lock_entry(&entry).is_none());
        entry = sample_lock_entry();
        entry.source = format!("github_pat_{}", "x".repeat(48));
        assert!(sanitize_lock_entry(&entry).is_none());
        entry = sample_lock_entry();
        entry.skill_path = Some("/Users/example/private-skill".into());
        assert!(sanitize_lock_entry(&entry).is_none());
        entry = sample_lock_entry();
        entry.r#ref = Some("../../unsafe".into());
        assert!(sanitize_lock_entry(&entry).is_none());
    }

    #[test]
    fn lock_merge_follows_bundle_decisions_and_preserves_target_preferences() {
        let source_entry = sample_lock_entry();
        let package = SkillLockFileV3 {
            version: 3,
            skills: BTreeMap::from([
                ("added".into(), source_entry.clone()),
                ("same".into(), source_entry.clone()),
                ("replaced".into(), source_entry.clone()),
            ]),
            dismissed: None,
            last_selected_agents: None,
        };
        let mut existing_entry = source_entry.clone();
        existing_entry.r#ref = Some("target".into());
        let target = SkillLockFileV3 {
            version: 3,
            skills: BTreeMap::from([
                ("unrelated".into(), existing_entry.clone()),
                ("same".into(), existing_entry.clone()),
                ("preserved".into(), existing_entry.clone()),
                ("replaced".into(), existing_entry.clone()),
                ("content-only".into(), existing_entry.clone()),
            ]),
            dismissed: Some(json!({"notice": true})),
            last_selected_agents: Some(json!(["codex"])),
        };
        let decisions = BTreeMap::from([
            ("added".into(), ChangeKind::Add),
            ("same".into(), ChangeKind::Unchanged),
            ("preserved".into(), ChangeKind::Preserve),
            ("replaced".into(), ChangeKind::Update),
            ("content-only".into(), ChangeKind::Update),
        ]);

        let LockMergeResult::Write(bytes) = merge_skill_lock(
            &package,
            Some(&serde_json::to_vec(&target).unwrap()),
            &decisions,
        )
        .unwrap() else {
            panic!("merge should write a changed target");
        };
        let merged: SkillLockFileV3 = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(merged.dismissed, target.dismissed);
        assert_eq!(merged.last_selected_agents, target.last_selected_agents);
        assert_eq!(merged.skills["unrelated"], existing_entry);
        assert_eq!(merged.skills["same"].r#ref.as_deref(), Some("target"));
        assert_eq!(merged.skills["preserved"].r#ref.as_deref(), Some("target"));
        assert_eq!(merged.skills["replaced"].r#ref.as_deref(), Some("main"));
        assert!(!merged.skills.contains_key("content-only"));
        assert!(merged.skills.contains_key("added"));
    }

    #[test]
    fn malformed_or_unknown_target_lock_is_never_overwritten() {
        let package = SkillLockFileV3 {
            version: 3,
            skills: BTreeMap::from([("skill".into(), sample_lock_entry())]),
            dismissed: None,
            last_selected_agents: None,
        };
        let decisions = BTreeMap::from([("skill".into(), ChangeKind::Add)]);
        assert_eq!(
            merge_skill_lock(&package, Some(b"not-json"), &decisions).unwrap(),
            LockMergeResult::SkippedInvalidTarget
        );
        let unknown = serde_json::to_vec(&json!({"version": 4, "skills": {}})).unwrap();
        assert_eq!(
            merge_skill_lock(&package, Some(&unknown), &decisions).unwrap(),
            LockMergeResult::SkippedInvalidTarget
        );
    }

    #[test]
    fn source_lock_downgrades_only_entries_with_fields_outside_the_v3_allowlist() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(".skill-lock.json");
        let mut unsupported = serde_json::to_value(sample_lock_entry()).unwrap();
        unsupported
            .as_object_mut()
            .unwrap()
            .insert("sourceBaseUrl".into(), json!("https://example.com"));
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "version": 3,
                "skills": {
                    "safe": sample_lock_entry(),
                    "new-field": unsupported
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let (lock, status, warning) = load_lock_file(&path);
        assert_eq!(status, LockLoadStatus::Available);
        let lock = lock.unwrap();
        assert!(lock.skills.contains_key("safe"));
        assert!(!lock.skills.contains_key("new-field"));
        assert!(warning
            .as_deref()
            .is_some_and(|message| message.contains("Ignored 1")));
    }
}
