use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::env;
use std::fs;

#[derive(Debug)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug)]
struct Text {
    x: f64,
    y: f64,
    font_size: f64,
    content: String,
}

#[derive(Debug)]
struct Diamond {
    cx: f64,
    cy: f64,
    width: f64,
    height: f64,
}

#[derive(Debug)]
struct Arrow {
    segments: Vec<(f64, f64, f64, f64)>,
}

fn parse_svg(svg: &str) -> Result<(Vec<Rect>, Vec<Text>, Vec<Arrow>, Vec<Diamond>)> {
    let mut rects = Vec::new();
    let mut texts = Vec::new();
    let mut arrows = Vec::new();
    let mut diamonds = Vec::new();
    let mut reader = Reader::from_str(svg);
    let mut buf = Vec::new();
    let mut current_text_x = 0.0;
    let mut current_text_y = 0.0;
    let mut current_text_fs = 12.0;
    let mut in_text = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();

                if name == "rect" {
                    let (mut x, mut y, mut w, mut h) = (0.0, 0.0, 0.0, 0.0);
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let val = String::from_utf8_lossy(&attr.value).to_string();
                        match key.as_str() {
                            "x" => x = val.parse().unwrap_or(0.0),
                            "y" => y = val.parse().unwrap_or(0.0),
                            "width" => w = val.parse().unwrap_or(0.0),
                            "height" => h = val.parse().unwrap_or(0.0),
                            _ => {}
                        }
                    }
                    if w > 0.0 && h > 0.0 {
                        rects.push(Rect { x, y, width: w, height: h });
                    }
                }

                if name == "text" {
                    let (mut x, mut y, mut fs) = (0.0, 0.0, 12.0);
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let val = String::from_utf8_lossy(&attr.value).to_string();
                        match key.as_str() {
                            "x" => x = val.parse().unwrap_or(0.0),
                            "y" => y = val.parse().unwrap_or(0.0),
                            "font-size" => fs = val.parse().unwrap_or(12.0),
                            _ => {}
                        }
                    }
                    current_text_x = x;
                    current_text_y = y;
                    current_text_fs = fs;
                    in_text = true;
                }

                if name == "line" {
                    let (mut x1, mut y1, mut x2, mut y2) = (0.0, 0.0, 0.0, 0.0);
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let val = String::from_utf8_lossy(&attr.value).to_string();
                        match key.as_str() {
                            "x1" => x1 = val.parse().unwrap_or(0.0),
                            "y1" => y1 = val.parse().unwrap_or(0.0),
                            "x2" => x2 = val.parse().unwrap_or(0.0),
                            "y2" => y2 = val.parse().unwrap_or(0.0),
                            _ => {}
                        }
                    }
                    arrows.push(Arrow { segments: vec![(x1, y1, x2, y2)] });
                }

                if name == "polyline" {
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        if key == "points" {
                            let val = String::from_utf8_lossy(&attr.value).to_string();
                            let pts: Vec<(f64, f64)> = val
                                .split_whitespace()
                                .filter_map(|p| {
                                    let parts: Vec<&str> = p.split(',').collect();
                                    if parts.len() == 2 {
                                        Some((
                                            parts[0].parse().unwrap_or(0.0),
                                            parts[1].parse().unwrap_or(0.0),
                                        ))
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            let segments: Vec<_> = (0..pts.len().saturating_sub(1))
                                .map(|i| (pts[i].0, pts[i].1, pts[i + 1].0, pts[i + 1].1))
                                .collect();
                            if !segments.is_empty() {
                                arrows.push(Arrow { segments });
                            }
                        }
                    }
                }

                if name == "polygon" {
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        if key == "points" {
                            let val = String::from_utf8_lossy(&attr.value).to_string();
                            let pts: Vec<(f64, f64)> = val
                                .split_whitespace()
                                .filter_map(|p| {
                                    let parts: Vec<&str> = p.split(',').collect();
                                    if parts.len() == 2 {
                                        Some((
                                            parts[0].parse().unwrap_or(0.0),
                                            parts[1].parse().unwrap_or(0.0),
                                        ))
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            if pts.len() == 4 {
                                let min_x = pts.iter().map(|p| p.0).fold(f64::MAX, f64::min);
                                let max_x = pts.iter().map(|p| p.0).fold(f64::MIN, f64::max);
                                let min_y = pts.iter().map(|p| p.1).fold(f64::MAX, f64::min);
                                let max_y = pts.iter().map(|p| p.1).fold(f64::MIN, f64::max);
                                diamonds.push(Diamond {
                                    cx: (min_x + max_x) / 2.0,
                                    cy: (min_y + max_y) / 2.0,
                                    width: max_x - min_x,
                                    height: max_y - min_y,
                                });
                            }
                        }
                    }
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_text {
                    let content = String::from_utf8_lossy(e.as_ref()).to_string();
                    if !content.trim().is_empty() {
                        texts.push(Text {
                            x: current_text_x,
                            y: current_text_y,
                            font_size: current_text_fs,
                            content: content.trim().to_string(),
                        });
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                if String::from_utf8_lossy(e.name().as_ref()) == "text" {
                    in_text = false;
                }
            }
            Err(e) => eprintln!("XML warning: {}", e),
            _ => {}
        }
        buf.clear();
    }

    Ok((rects, texts, arrows, diamonds))
}

/// Find the smallest content rect containing a text element.
/// Skips background/zone rects (too wide or too tall) and very small rects (legend dots).
fn find_containing_rect<'a>(text: &Text, rects: &'a [Rect]) -> Option<&'a Rect> {
    rects.iter()
        .filter(|r| {
            // Skip viewport/zone backgrounds (too wide or too tall)
            r.width < 400.0 && r.height < 100.0
            // Skip tiny rects (legend colour dots)
            && r.width > 20.0 && r.height > 20.0
            // Text must be inside
            && text.x >= r.x
            && text.x <= r.x + r.width
            && text.y >= r.y
            && text.y <= r.y + r.height + 5.0
        })
        // Pick the smallest containing rect (most specific match)
        .min_by(|a, b| (a.width * a.height).partial_cmp(&(b.width * b.height)).unwrap())
}

fn estimate_text_width(text: &str, font_size: f64) -> f64 {
    text.len() as f64 * font_size * 0.55
}

fn check_text_overflow(texts: &[Text], rects: &[Rect]) -> Vec<String> {
    let mut failures = Vec::new();
    for text in texts {
        if let Some(rect) = find_containing_rect(text, rects) {
            let est_width = estimate_text_width(&text.content, text.font_size);
            let margin_left = text.x - rect.x;
            let margin_right = rect.x + rect.width - text.x;
            let available = margin_left.min(margin_right) * 2.0;
            if est_width > available * 0.95 {
                failures.push(format!(
                    "TEXT OVERFLOW: \"{}\" est {:.0}px in rect w={:.0} (avail {:.0}) at y={:.0}",
                    &text.content[..text.content.len().min(40)],
                    est_width, rect.width, available, text.y
                ));
            }
        }
    }
    failures
}

/// Check text inside diamonds (inscribed area is ~50% of diamond width).
fn check_diamond_text(texts: &[Text], diamonds: &[Diamond]) -> Vec<String> {
    let mut failures = Vec::new();
    for text in texts {
        for d in diamonds {
            if (text.x - d.cx).abs() < d.width / 2.0
                && (text.y - d.cy).abs() < d.height / 2.0
            {
                let est_width = estimate_text_width(&text.content, text.font_size);
                let inscribed = d.width * 0.5; // diamond inscribes ~50% width for text
                if est_width > inscribed * 1.05 {
                    failures.push(format!(
                        "DIAMOND TEXT OVERFLOW: \"{}\" est {:.0}px, inscribed {:.0}px (diamond w={:.0})",
                        &text.content[..text.content.len().min(30)],
                        est_width, inscribed, d.width
                    ));
                }
                break;
            }
        }
    }
    failures
}

fn check_arrow_length(arrows: &[Arrow]) -> Vec<String> {
    let mut failures = Vec::new();
    for arrow in arrows {
        let total: f64 = arrow.segments.iter()
            .map(|(x1, y1, x2, y2)| ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt())
            .sum();
        if total < 15.0 && total > 0.0 {
            let (x1, y1, _, _) = arrow.segments[0];
            failures.push(format!("SHORT ARROW: {:.0}px (min 15) at ({:.0},{:.0})", total, x1, y1));
        }
    }
    failures
}

fn check_rect_overlap(rects: &[Rect]) -> Vec<String> {
    let mut failures = Vec::new();
    let content: Vec<&Rect> = rects.iter()
        .filter(|r| r.width < 400.0 && r.height > 30.0 && r.height < 80.0)
        .collect();
    for i in 0..content.len() {
        for j in (i + 1)..content.len() {
            let (a, b) = (content[i], content[j]);
            if (a.y - b.y).abs() < 10.0 && a.x < b.x {
                let gap = b.x - (a.x + a.width);
                if gap < 0.0 {
                    failures.push(format!(
                        "OVERLAP: rects at x={:.0} w={:.0} and x={:.0} gap={:.0} at y={:.0}",
                        a.x, a.width, b.x, gap, a.y
                    ));
                }
            }
        }
    }
    failures
}

/// Check: content has symmetrical margins within zone backgrounds.
/// Finds zone rects (wide, tall) and content rects within them,
/// checks left margin ≈ right margin (within 10px tolerance).
fn check_symmetrical_margins(rects: &[Rect]) -> Vec<String> {
    let mut failures = Vec::new();

    // Zone backgrounds: wide (>400) and tall (>100)
    let zones: Vec<&Rect> = rects.iter()
        .filter(|r| r.width > 400.0 && r.height > 100.0)
        .collect();

    // Content rects: reasonable size, not zones
    let content: Vec<&Rect> = rects.iter()
        .filter(|r| r.width > 20.0 && r.width < 400.0 && r.height > 30.0 && r.height < 80.0)
        .collect();

    for zone in &zones {
        // Find content rects within this zone
        let inside: Vec<&&Rect> = content.iter()
            .filter(|c| c.x >= zone.x && c.x + c.width <= zone.x + zone.width
                && c.y >= zone.y && c.y + c.height <= zone.y + zone.height)
            .collect();

        if inside.is_empty() {
            continue;
        }

        let leftmost = inside.iter().map(|r| r.x).fold(f64::MAX, f64::min);
        let rightmost = inside.iter().map(|r| r.x + r.width).fold(f64::MIN, f64::max);

        let left_margin = leftmost - zone.x;
        let right_margin = (zone.x + zone.width) - rightmost;

        if (left_margin - right_margin).abs() > 15.0 {
            failures.push(format!(
                "ASYMMETRIC MARGINS: zone at y={:.0} left={:.0}px right={:.0}px (diff {:.0})",
                zone.y, left_margin, right_margin, (left_margin - right_margin).abs()
            ));
        }
    }

    failures
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: diagram-lint <file.svg> [file2.svg ...]");
        std::process::exit(1);
    }

    let mut total_pass = 0;
    let mut total_fail = 0;

    for path in &args[1..] {
        let svg = fs::read_to_string(path).with_context(|| format!("Failed to read: {}", path))?;
        let (rects, texts, arrows, diamonds) = parse_svg(&svg)?;

        println!("=== {} ===", path);
        println!(
            "  Parsed: {} rects, {} texts, {} arrows, {} diamonds",
            rects.len(), texts.len(), arrows.len(), diamonds.len()
        );

        let mut failures = Vec::new();
        failures.extend(check_text_overflow(&texts, &rects));
        failures.extend(check_diamond_text(&texts, &diamonds));
        failures.extend(check_arrow_length(&arrows));
        failures.extend(check_rect_overlap(&rects));
        failures.extend(check_symmetrical_margins(&rects));

        if failures.is_empty() {
            println!("  ✓ All checks passed");
            total_pass += 1;
        } else {
            for f in &failures {
                println!("  ✗ {}", f);
            }
            total_fail += 1;
        }
        println!();
    }

    if total_fail > 0 {
        println!("{} passed, {} failed", total_pass, total_fail);
        std::process::exit(1);
    } else {
        println!("All {} files passed", total_pass);
    }
    Ok(())
}
