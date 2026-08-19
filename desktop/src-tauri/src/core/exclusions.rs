use std::path::Path;

const FORBIDDEN_COMPONENTS: &[&str] = &[
    ".ds_store",
    ".git",
    ".tmp",
    ".venv",
    "__pycache__",
    "build",
    "cache",
    "caches",
    "cachestorage",
    "code cache",
    "dist",
    "gpucache",
    "local storage",
    "logs",
    "node_modules",
    "process_manager",
    "session storage",
    "target",
    "tmp",
    "vendor_imports",
    "venv",
];

const FORBIDDEN_NAMES: &[&str] = &[
    "auth.json",
    "cookies",
    "cookies-journal",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "id_rsa",
    "login data",
    "login data for account",
    "login data for account-journal",
    "login data-journal",
    "runningchromeversion",
    "singletoncookie",
    "singletonlock",
    "singletonsocket",
];

const SKILL_EXCLUDED_COMPONENTS: &[&str] = &[
    ".git",
    ".tmp",
    ".venv",
    "__pycache__",
    "build",
    "cache",
    "caches",
    "dist",
    "node_modules",
    "target",
    "tmp",
    "venv",
];

const SKILL_SENSITIVE_NAMES: &[&str] = &[
    ".git-credentials",
    ".netrc",
    ".npmrc",
    ".pypirc",
    "auth.json",
    "credentials.json",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "id_rsa",
    "service-account.json",
    "secrets.json",
    "token.json",
    "tokens.json",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillPathPolicy {
    Include,
    Exclude(&'static str),
    Block(&'static str),
}

pub fn classify_skill_path(path: &Path) -> SkillPathPolicy {
    let rendered = path.as_os_str().to_string_lossy().replace('\\', "/");
    let parts = rendered
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();

    if parts.iter().any(|name| {
        SKILL_SENSITIVE_NAMES.contains(&name.as_str())
            || name == ".env"
            || name.starts_with(".env.")
            || name.starts_with("client_secret") && name.ends_with(".json")
            || name.starts_with("service_account") && name.ends_with(".json")
            || [".key", ".pem", ".p12", ".pfx"]
                .iter()
                .any(|extension| name.ends_with(extension))
    }) {
        return SkillPathPolicy::Block("sensitive credential or private-key path");
    }
    if parts
        .iter()
        .any(|name| SKILL_EXCLUDED_COMPONENTS.contains(&name.as_str()))
    {
        return SkillPathPolicy::Exclude("dependency, cache, build, or version-control data");
    }
    SkillPathPolicy::Include
}

pub fn is_forbidden(path: &Path) -> bool {
    let rendered = path.as_os_str().to_string_lossy().replace('\\', "/");

    rendered
        .split('/')
        .filter(|part| !part.is_empty())
        .any(|part| {
            let name = part.to_ascii_lowercase();
            FORBIDDEN_COMPONENTS.contains(&name.as_str())
                || FORBIDDEN_NAMES.contains(&name.as_str())
                || name == ".env"
                || name.starts_with(".env.")
                || (name.starts_with("logs_") && name.contains(".sqlite"))
                || [".ipc", ".key", ".pem", ".sock", ".socket"]
                    .iter()
                    .any(|extension| name.ends_with(extension))
        })
}
