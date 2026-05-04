//! `pageprobe screenshot` — captures a PNG (or JPEG with `--quality`) of
//! the attached tab.
//!
//! Default: viewport-only PNG to `/tmp/pageprobe-<unix-ts>.png`. With
//! `--full`, capture the entire scrollable page via
//! `Page.captureScreenshot { captureBeyondViewport: true }`. With
//! `--clip "x,y,w,h"`, capture only the given region (in CSS pixels).
use anyhow::{Context, Result, anyhow};
use chromiumoxide::cdp::browser_protocol::page::{
    CaptureScreenshotFormat, CaptureScreenshotParams, Viewport,
};
use chromiumoxide::page::ScreenshotParams;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::{cdp, state};

pub async fn run(
    path: Option<PathBuf>,
    full: bool,
    quality: Option<i64>,
    clip: Option<String>,
) -> Result<()> {
    if let Some(q) = quality
        && !(0..=100).contains(&q)
    {
        return Err(anyhow!("--quality must be 0-100, got {q}"));
    }

    let viewport = match clip.as_deref() {
        Some(s) => Some(parse_clip(s)?),
        None => None,
    };

    let is_jpeg = quality.is_some();
    let resolved_path = resolve_output_path(path, is_jpeg);

    let s = state::load()?;
    let port = s.port_or_default();
    let tab_id = s
        .attached_tab_id
        .clone()
        .ok_or_else(|| anyhow!("no tab attached. Run `pageprobe attach <pattern>` first."))?;

    let (mut browser, handle) = cdp::connect(port).await?;
    let page = browser
        .pages()
        .await?
        .into_iter()
        .find(|p| p.target_id().as_ref() == tab_id.as_str())
        .ok_or_else(|| {
            anyhow!("attached tab id no longer matches any open tab; run `pageprobe attach` again")
        })?;

    let mut cdp_params = CaptureScreenshotParams::default();
    if is_jpeg {
        cdp_params.format = Some(CaptureScreenshotFormat::Jpeg);
        cdp_params.quality = quality;
    } else {
        cdp_params.format = Some(CaptureScreenshotFormat::Png);
    }
    if let Some(v) = viewport {
        cdp_params.clip = Some(v);
    }

    let mut builder = ScreenshotParams::builder();
    if is_jpeg {
        builder = builder.format(CaptureScreenshotFormat::Jpeg);
        if let Some(q) = quality {
            builder = builder.quality(q);
        }
    }
    if full {
        builder = builder.full_page(true).capture_beyond_viewport(true);
    }
    if let Some(v) = cdp_params.clip.clone() {
        builder = builder.clip(v);
    }

    let bytes = page
        .save_screenshot(builder.build(), &resolved_path)
        .await
        .context("Page.captureScreenshot")?;

    // Drop closes the WebSocket; we deliberately do NOT call
    // `browser.close()` (that would shut Chrome down).
    handle.abort();
    drop(browser);

    println!(
        "{}  ({} bytes)",
        resolved_path.display(),
        bytes.len()
    );
    Ok(())
}

/// Parses a "x,y,w,h" string into a `Viewport`.
pub fn parse_clip(raw: &str) -> Result<Viewport> {
    let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
    if parts.len() != 4 {
        return Err(anyhow!(
            "--clip must be \"x,y,w,h\" (got {} parts in {raw:?})",
            parts.len()
        ));
    }
    let x: f64 = parts[0]
        .parse()
        .with_context(|| format!("clip.x: {:?}", parts[0]))?;
    let y: f64 = parts[1]
        .parse()
        .with_context(|| format!("clip.y: {:?}", parts[1]))?;
    let width: f64 = parts[2]
        .parse()
        .with_context(|| format!("clip.width: {:?}", parts[2]))?;
    let height: f64 = parts[3]
        .parse()
        .with_context(|| format!("clip.height: {:?}", parts[3]))?;
    if width <= 0.0 || height <= 0.0 {
        return Err(anyhow!(
            "--clip width and height must be positive (got {width}x{height})"
        ));
    }
    Ok(Viewport {
        x,
        y,
        width,
        height,
        scale: 1.0,
    })
}

/// Resolves the output path. If `path` is `None`, returns
/// `/tmp/pageprobe-<unix-ts>.<ext>`. If the user supplied a path with no
/// extension, attaches `.png` (or `.jpg`).
pub fn resolve_output_path(path: Option<PathBuf>, is_jpeg: bool) -> PathBuf {
    let ext = if is_jpeg { "jpg" } else { "png" };
    match path {
        Some(p) => {
            if p.extension().is_some() {
                p
            } else {
                p.with_extension(ext)
            }
        }
        None => {
            let ts = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            PathBuf::from(format!("/tmp/pageprobe-{ts}.{ext}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_clip_basic() {
        let v = parse_clip("0,0,800,600").unwrap();
        assert_eq!(v.x, 0.0);
        assert_eq!(v.y, 0.0);
        assert_eq!(v.width, 800.0);
        assert_eq!(v.height, 600.0);
        assert_eq!(v.scale, 1.0);
    }

    #[test]
    fn parse_clip_with_spaces() {
        let v = parse_clip(" 10 , 20 , 100 , 200 ").unwrap();
        assert_eq!(v.x, 10.0);
        assert_eq!(v.y, 20.0);
    }

    #[test]
    fn parse_clip_rejects_wrong_arity() {
        assert!(parse_clip("0,0,100").is_err());
        assert!(parse_clip("0,0,100,200,300").is_err());
    }

    #[test]
    fn parse_clip_rejects_zero_dimensions() {
        assert!(parse_clip("0,0,0,100").is_err());
        assert!(parse_clip("0,0,100,0").is_err());
    }

    #[test]
    fn parse_clip_rejects_non_numeric() {
        assert!(parse_clip("a,b,c,d").is_err());
    }

    #[test]
    fn resolve_output_path_default_png() {
        let p = resolve_output_path(None, false);
        let s = p.to_string_lossy();
        assert!(s.starts_with("/tmp/pageprobe-"), "got: {s}");
        assert!(s.ends_with(".png"), "got: {s}");
    }

    #[test]
    fn resolve_output_path_default_jpg() {
        let p = resolve_output_path(None, true);
        assert!(p.to_string_lossy().ends_with(".jpg"));
    }

    #[test]
    fn resolve_output_path_keeps_user_extension() {
        let p = resolve_output_path(Some(PathBuf::from("/tmp/foo.png")), false);
        assert_eq!(p, PathBuf::from("/tmp/foo.png"));
    }

    #[test]
    fn resolve_output_path_appends_extension_when_missing() {
        let p = resolve_output_path(Some(PathBuf::from("/tmp/foo")), false);
        assert_eq!(p, PathBuf::from("/tmp/foo.png"));
        let p2 = resolve_output_path(Some(PathBuf::from("/tmp/bar")), true);
        assert_eq!(p2, PathBuf::from("/tmp/bar.jpg"));
    }
}
