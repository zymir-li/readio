//! Open illustrations in macOS Preview; rasterize SVG only on demand.
use anyhow::{Context, Result, bail};
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn open(path: &Path) -> Result<()> {
    let cache = dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("readio/image-previews");
    let image = prepare(path, &cache)?.canonicalize()?;
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("/usr/bin/open");
        c.args(["-a", "Preview"]);
        c
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = Command::new("xdg-open");
    let status = command
        .arg(&image)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("无法启动系统图片查看器")?;
    if !status.success() {
        bail!("预览未能打开图片");
    }
    Ok(())
}

fn prepare(path: &Path, cache: &Path) -> Result<PathBuf> {
    if !path.is_file() {
        bail!("图片文件不存在");
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cache;
        Ok(path.to_path_buf())
    }
    #[cfg(target_os = "macos")]
    {
        prepare_svg(path, cache)
    }
}

#[cfg(target_os = "macos")]
fn prepare_svg(path: &Path, cache: &Path) -> Result<PathBuf> {
    if !path
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("svg"))
    {
        return Ok(path.to_path_buf());
    }
    let bytes = std::fs::read(path).context("无法读取 SVG 图片")?;
    let key = format!("{:x}", Sha256::digest(&bytes));
    let dir = cache.join(key);
    let png = dir.join(format!(
        "{}.png",
        path.file_name()
            .context("图片没有文件名")?
            .to_string_lossy()
    ));
    if png.is_file() {
        return Ok(png);
    }
    std::fs::create_dir_all(&dir).context("无法创建图片预览缓存")?;
    let mut child = Command::new("/usr/bin/qlmanage")
        .args(["-t", "-s", "2400", "-o"])
        .arg(&dir)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("无法生成 SVG 预览")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() || !png.is_file() {
                bail!("SVG 预览生成失败");
            }
            return Ok(png);
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("SVG 预览生成超时，请重试");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn svg_preview_is_generated_and_reused_without_changing_the_source() {
        let dir = std::env::temp_dir().join(format!("readio-svg-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let svg = dir.join("diagram with spaces.svg");
        let original = r##"<svg xmlns="http://www.w3.org/2000/svg" width="640" height="320"><rect width="640" height="320" fill="#faf4ed"/><text x="20" y="100" font-size="40">Preview check</text></svg>"##;
        std::fs::write(&svg, original).unwrap();
        let png = prepare(&svg, &dir.join("cache")).unwrap();
        assert!(image::open(&png).unwrap().width() >= 640);
        let modified = std::fs::metadata(&png).unwrap().modified().unwrap();
        assert_eq!(prepare(&svg, &dir.join("cache")).unwrap(), png);
        assert_eq!(
            std::fs::metadata(&png).unwrap().modified().unwrap(),
            modified
        );
        assert_eq!(std::fs::read_to_string(&svg).unwrap(), original);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn raster_images_are_opened_directly_and_missing_files_fail() {
        let path = std::env::temp_dir().join(format!("readio-raster-{}.png", std::process::id()));
        std::fs::write(&path, b"test").unwrap();
        assert_eq!(prepare(&path, Path::new("/unused")).unwrap(), path);
        std::fs::remove_file(&path).unwrap();
        assert!(prepare(&path, Path::new("/unused")).is_err());
    }
}
