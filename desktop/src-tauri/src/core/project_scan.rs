use crate::core::{
    error::{ErrorCode, RehomeError},
    exclusions::is_forbidden,
    paths::normalize_entry,
};
use std::{collections::HashSet, fs, path::Path};
use walkdir::WalkDir;

pub fn count_project_files(source_root: &Path) -> Result<u64, RehomeError> {
    let canonical = source_root
        .canonicalize()
        .map_err(|error| scan_failed(format!("selected project cannot be resolved: {error}")))?;
    let root_metadata = fs::symlink_metadata(source_root)
        .map_err(|error| scan_failed(format!("selected project cannot be inspected: {error}")))?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err(scan_failed("selected project must be a real directory"));
    }

    let mut file_count = 0_u64;
    let mut normalized_paths = HashSet::new();
    for entry in WalkDir::new(&canonical)
        .follow_links(false)
        .sort_by_file_name()
    {
        let entry = entry
            .map_err(|error| scan_failed(format!("could not walk selected project: {error}")))?;
        if entry.path() == canonical || entry.file_type().is_symlink() {
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(&canonical)
            .map_err(|_| scan_failed("selected project entry escapes the project root"))?;
        if is_forbidden(relative) {
            continue;
        }
        let normalized = normalize_entry(relative)?;
        if !normalized_paths.insert(normalized) {
            return Err(scan_failed(
                "selected project contains duplicate portable file paths",
            ));
        }
        file_count = file_count
            .checked_add(1)
            .ok_or_else(|| scan_failed("selected project file count overflowed"))?;
    }
    Ok(file_count)
}

fn scan_failed(message: impl Into<String>) -> RehomeError {
    RehomeError::new(ErrorCode::PackageInvalid, message)
}
