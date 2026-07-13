use std::env;
use std::path::{Path, PathBuf};

const FALLBACK_PATH_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

const HOME_REL_PATH_DIRS: &[&str] = &["Kota/bin", ".local/bin", ".cargo/bin", ".bun/bin", "bin"];

pub fn augmented_path(home: Option<&Path>) -> String {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            push_unique(&mut dirs, dir);
        }
    }

    for dir in FALLBACK_PATH_DIRS {
        push_unique(&mut dirs, PathBuf::from(dir));
    }

    if let Some(home) = home {
        for dir in HOME_REL_PATH_DIRS {
            push_unique(&mut dirs, home.join(dir));
        }
    }

    env::join_paths(dirs)
        .ok()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| {
            FALLBACK_PATH_DIRS
                .iter()
                .copied()
                .collect::<Vec<_>>()
                .join(":")
        })
}

pub fn resolve_on_augmented_path(program: &str, home: Option<&Path>) -> String {
    let raw = Path::new(program);
    if raw.components().count() > 1 {
        return program.to_string();
    }

    let path = augmented_path(home);
    for dir in env::split_paths(&path) {
        let candidate = dir.join(program);
        if is_executable_file(&candidate) {
            return candidate.display().to_string();
        }
    }

    program.to_string()
}

fn push_unique(dirs: &mut Vec<PathBuf>, dir: PathBuf) {
    if !dirs.iter().any(|existing| existing == &dir) {
        dirs.push(dir);
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn augmented_path_includes_homebrew_defaults() {
        let path = augmented_path(None);
        let dirs: Vec<_> = env::split_paths(&path).collect();
        assert!(dirs.iter().any(|dir| dir == Path::new("/opt/homebrew/bin")));
        assert!(dirs.iter().any(|dir| dir == Path::new("/usr/local/bin")));
    }

    #[test]
    fn resolves_cli_on_augmented_path_when_installed() {
        let candidate = Path::new("/opt/homebrew/bin/claude");
        if candidate.exists() {
            let resolved = resolve_on_augmented_path("claude", None);
            let resolved_path = Path::new(&resolved);
            assert_eq!(
                resolved_path.file_name().and_then(|name| name.to_str()),
                Some("claude")
            );
            assert!(resolved_path.is_file());

            let dirs: Vec<_> = env::split_paths(&augmented_path(None)).collect();
            let resolved_index = dirs
                .iter()
                .position(|dir| dir.join("claude") == resolved_path)
                .expect("resolved executable should come from augmented PATH");
            let homebrew_index = dirs
                .iter()
                .position(|dir| dir.join("claude") == candidate)
                .expect("Homebrew fallback should be on augmented PATH");
            assert!(resolved_index <= homebrew_index);
        }
    }
}
