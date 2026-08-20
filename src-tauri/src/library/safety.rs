use std::path::{Path, PathBuf};

pub fn canonical_clips_root(root: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(root).map_err(|error| {
        format!(
            "Could not create the permanent Clips directory '{}': {error}",
            root.display()
        )
    })?;
    root.canonicalize().map_err(|error| {
        format!(
            "Could not resolve the permanent Clips directory '{}': {error}",
            root.display()
        )
    })
}

pub fn validate_owned_clip(root: &Path, candidate: &Path) -> Result<PathBuf, String> {
    let canonical_root = canonical_clips_root(root)?;
    let canonical = candidate.canonicalize().map_err(|error| {
        format!(
            "Could not resolve the library clip '{}': {error}",
            candidate.display()
        )
    })?;
    if !canonical.starts_with(&canonical_root) || canonical == canonical_root {
        return Err("The requested clip is outside the owned permanent Clips directory.".into());
    }
    if canonical.parent() != Some(canonical_root.as_path()) {
        return Err(
            "Stage 12 only permits permanent MP4 files directly inside the Clips directory.".into(),
        );
    }
    if !canonical.is_file() {
        return Err("The requested clip is not a regular file.".into());
    }
    if !has_mp4_extension(&canonical) {
        return Err("The requested library file is not an MP4 clip.".into());
    }
    Ok(canonical)
}

pub fn owned_missing_path(root: &Path, candidate: &Path) -> bool {
    let Ok(canonical_root) = canonical_clips_root(root) else {
        return false;
    };
    let Some(parent) = candidate.parent() else {
        return false;
    };
    let Ok(canonical_parent) = parent.canonicalize() else {
        return false;
    };
    canonical_parent == canonical_root && has_mp4_extension(candidate)
}

pub fn is_reconciliation_candidate(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    if name.starts_with('.')
        || name.to_ascii_lowercase().contains(".partial")
        || name.to_ascii_lowercase().contains("video-only")
    {
        return false;
    }
    has_mp4_extension(path)
}

fn has_mp4_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("mp4"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("stage12-safety-{name}-{}", std::process::id()))
    }

    #[test]
    fn accepts_owned_file_and_rejects_sibling_traversal_directory_and_extension() {
        let base = root("containment");
        let clips = base.join("Clips");
        let sibling = base.join("Clips-Backup");
        fs::create_dir_all(&clips).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        let valid = clips.join("valid.mp4");
        let outside = sibling.join("outside.mp4");
        let wrong = clips.join("wrong.txt");
        fs::write(&valid, b"video").unwrap();
        fs::write(&outside, b"video").unwrap();
        fs::write(&wrong, b"text").unwrap();
        assert_eq!(
            validate_owned_clip(&clips, &valid).unwrap(),
            valid.canonicalize().unwrap()
        );
        assert!(validate_owned_clip(&clips, &outside).is_err());
        assert!(validate_owned_clip(&clips, &clips.join(r"..\Clips-Backup\outside.mp4")).is_err());
        assert!(validate_owned_clip(&clips, &clips).is_err());
        assert!(validate_owned_clip(&clips, &wrong).is_err());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn reconciliation_ignores_partial_hidden_and_non_mp4_files() {
        assert!(is_reconciliation_candidate(Path::new("clip.mp4")));
        assert!(is_reconciliation_candidate(Path::new("CLIP.MP4")));
        assert!(!is_reconciliation_candidate(Path::new("clip.partial.mp4")));
        assert!(!is_reconciliation_candidate(Path::new(".hidden.mp4")));
        assert!(!is_reconciliation_candidate(Path::new("video-only.mp4")));
        assert!(!is_reconciliation_candidate(Path::new("manifest.txt")));
    }
}
