//! File picking through ranger's `--choosefiles` mode.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Split the configured command into program and arguments.
fn parse_command(command: &str) -> Option<(String, Vec<String>)> {
    let mut parts = command.split_whitespace().map(String::from);
    let program = parts.next()?;
    Some((program, parts.collect()))
}

/// True when the configured command resolves to something runnable.
pub fn is_available(command: &str) -> bool {
    let Some((program, _)) = parse_command(command) else {
        return false;
    };
    if program.contains('/') {
        return Path::new(&program).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(&program).is_file()))
}

/// Parse the chooser file: one path per line, directories skipped.
pub fn read_chooser_file(path: &Path) -> Vec<PathBuf> {
    let Ok(raw) = std::fs::read(path) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&raw)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .filter(|p| !p.is_dir())
        .filter_map(|p| p.canonicalize().ok())
        .collect()
}

pub fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    paths
        .into_iter()
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Run ranger on the real terminal and return the chosen files. The caller must
/// have left raw mode / the alternate screen first. Any failure yields an empty list.
pub fn pick_files(command: &str) -> Vec<PathBuf> {
    let Some((program, args)) = parse_command(command) else {
        return Vec::new();
    };
    let Ok(tmp) = tempfile::Builder::new()
        .prefix("rsnotebook_ranger_")
        .suffix(".txt")
        .tempfile()
    else {
        return Vec::new();
    };
    let ran = Command::new(program)
        .args(args)
        .arg(format!("--choosefiles={}", tmp.path().display()))
        .status();
    if ran.is_err() {
        return Vec::new();
    }
    dedupe_paths(read_chooser_file(tmp.path()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooser_file_skips_dirs_blanks_and_missing() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let list = d.path().join("list");
        std::fs::write(
            &list,
            format!(
                "{}\n\n{}\n{}\n/nope/missing\n",
                f.display(),
                d.path().display(),
                f.display()
            ),
        )
        .unwrap();
        let got = dedupe_paths(read_chooser_file(&list));
        assert_eq!(got, vec![f.canonicalize().unwrap()]);
    }

    #[test]
    fn availability() {
        assert!(!is_available(""));
        assert!(!is_available("definitely-not-a-real-binary-xyz"));
        assert!(is_available("sh"));
    }
}
