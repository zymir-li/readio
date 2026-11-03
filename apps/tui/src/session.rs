//! Local working-directory facts; no remote model calls or synthetic spending.
use std::path::Path;
use std::process::Command;
#[derive(Debug)]
pub struct Facts {
    pub directory: String,
    pub branch: String,
}
impl Facts {
    pub fn current() -> Self {
        Self::at(&std::env::current_dir().unwrap_or_default())
    }
    pub fn at(dir: &Path) -> Self {
        let directory = if dirs::home_dir().as_deref() == Some(dir) {
            "~".into()
        } else {
            dir.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        };
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("--no-optional-locks")
                .args(args)
                .current_dir(dir)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        };
        let branch = run(&["symbolic-ref", "--short", "HEAD"])
            .or_else(|| run(&["rev-parse", "--short", "HEAD"]))
            .map(|b| {
                let dirty = run(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
                format!("{b}{}", if dirty { "*" } else { "" })
            })
            .unwrap_or_else(|| "—".into());
        Self { directory, branch }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn branch_is_real_and_changes_are_marked() {
        let dir = std::env::temp_dir().join(format!("readio-git-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "--initial-branch=demo"])
                .arg(&dir)
                .output()
                .unwrap()
                .status
                .success()
        );
        assert_eq!(Facts::at(&dir).branch, "demo");
        std::fs::write(dir.join("example.txt"), "test").unwrap();
        assert_eq!(Facts::at(&dir).branch, "demo*");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
