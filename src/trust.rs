use crate::config::{absolute, home_dir};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn copilot_home() -> PathBuf {
    match std::env::var_os("COPILOT_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => home_dir().join(".copilot"),
    }
}

pub fn copilot_config_path() -> PathBuf {
    copilot_home().join("config.json")
}

fn strip_comments(text: &str) -> String {
    text.lines().filter(|line| !line.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

fn same(a: &Path, b: &Path) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// Adds a folder to Copilot's trustedFolders so a hidden session never stops at the
/// folder trust prompt. Copilot's config has leading // comments, so this edits the
/// text in place instead of rewriting the file. Returns None if it was already trusted.
pub fn add_trusted_folder(text: &str, folder: &Path) -> Result<Option<String>> {
    let path = absolute(folder);
    let config: Value = if text.trim().is_empty() { Value::Object(Default::default()) } else { serde_json::from_str(&strip_comments(text)).context("Copilot's config.json is not valid JSON")? };
    let trusted = config.get("trustedFolders").and_then(|v| v.as_array());
    if let Some(list) = trusted {
        if list.iter().filter_map(|e| e.as_str()).any(|e| same(&absolute(e), &path)) {
            return Ok(None);
        }
    }
    let entry = serde_json::to_string(&path.to_string_lossy())?;
    let next = if trusted.is_some() {
        let key = text.find("\"trustedFolders\"").context("trustedFolders key not found")?;
        let open = key + text[key..].find('[').context("trustedFolders is not an array")?;
        let after = &text[open + 1..];
        let rest = after.trim_start();
        if rest.starts_with(']') {
            format!("{}[{}{}", &text[..open], entry, rest)
        } else {
            let space = &after[..after.len() - rest.len()];
            format!("{}[\n    {},{}{}", &text[..open], entry, space, rest)
        }
    } else if !text.trim().is_empty() {
        text.replacen('{', &format!("{{\n  \"trustedFolders\": [{entry}],"), 1)
    } else {
        format!("{}\n", serde_json::to_string_pretty(&serde_json::json!({ "trustedFolders": [path.to_string_lossy()] }))?)
    };
    serde_json::from_str::<Value>(&strip_comments(&next)).context("Could not add the folder to trustedFolders")?;
    Ok(Some(next))
}

pub fn ensure_trusted(folder: &Path, config_path: &Path) -> Result<bool> {
    let text = match std::fs::read_to_string(config_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    match add_trusted_folder(&text, folder)? {
        Some(next) => {
            if let Some(parent) = config_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(config_path, next)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trusted(text: &str) -> Vec<String> {
        let v: Value = serde_json::from_str(&strip_comments(text)).unwrap();
        v["trustedFolders"].as_array().unwrap().iter().map(|e| e.as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn creates_config() {
        let dir = absolute("/work");
        let next = add_trusted_folder("", &dir).unwrap().unwrap();
        assert_eq!(trusted(&next), vec![dir.to_string_lossy().to_string()]);
    }

    #[test]
    fn keeps_comments_and_appends() {
        let text = "// comment\n{\n  \"model\": \"x\",\n  \"trustedFolders\": [\n    \"/a\"\n  ]\n}\n";
        let next = add_trusted_folder(text, Path::new("/b")).unwrap().unwrap();
        assert!(next.starts_with("// comment"));
        assert_eq!(trusted(&next).len(), 2);
        assert!(add_trusted_folder(&next, Path::new("/b")).unwrap().is_none());
    }

    #[test]
    fn handles_empty_array_and_missing_key() {
        assert_eq!(trusted(&add_trusted_folder("{\"trustedFolders\": []}", Path::new("/a")).unwrap().unwrap()).len(), 1);
        assert_eq!(trusted(&add_trusted_folder("{\"x\": 1}", Path::new("/a")).unwrap().unwrap()).len(), 1);
    }
}
