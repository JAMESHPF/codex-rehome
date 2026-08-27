use rehome_desktop_lib::core::project_scan::count_project_files;
use std::fs;
use tempfile::tempdir;

#[test]
fn counts_only_regular_migratable_project_files() {
    let root = tempdir().expect("temporary root");
    let project = root.path().join("project");
    fs::create_dir_all(project.join("src")).expect("source directory");
    fs::create_dir_all(project.join(".git")).expect("git directory");
    fs::create_dir_all(project.join("node_modules/pkg")).expect("dependency directory");
    fs::create_dir_all(project.join("empty")).expect("empty directory");
    fs::write(project.join("README.md"), "readme").expect("readme");
    fs::write(project.join("src/main.rs"), "fn main() {}").expect("source");
    fs::write(project.join(".env"), "SECRET=hidden").expect("sensitive file");
    fs::write(project.join(".git/config"), "[core]").expect("git metadata");
    fs::write(
        project.join("node_modules/pkg/index.js"),
        "module.exports = {}",
    )
    .expect("dependency file");

    #[cfg(unix)]
    {
        use std::os::unix::{fs::symlink, net::UnixListener};

        symlink(project.join("README.md"), project.join("readme-link")).expect("file symlink");
        symlink(project.join("src"), project.join("src-link")).expect("directory symlink");
        let _socket = UnixListener::bind(project.join("worker.sock")).expect("unix socket");

        assert_eq!(count_project_files(&project).expect("project count"), 2);
    }

    #[cfg(not(unix))]
    assert_eq!(count_project_files(&project).expect("project count"), 2);
}

#[test]
fn counts_an_empty_project_as_zero() {
    let root = tempdir().expect("temporary root");
    let project = root.path().join("empty-project");
    fs::create_dir_all(&project).expect("project directory");

    assert_eq!(count_project_files(&project).expect("empty count"), 0);
}

#[test]
fn rejects_missing_and_symlinked_project_roots() {
    let root = tempdir().expect("temporary root");
    assert!(count_project_files(&root.path().join("missing")).is_err());

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let project = root.path().join("project");
        let linked = root.path().join("linked-project");
        fs::create_dir_all(&project).expect("project directory");
        symlink(&project, &linked).expect("project symlink");
        assert!(count_project_files(&linked).is_err());
    }
}
