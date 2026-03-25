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

fn check_rect_overlap_and_gap(rects: &[Rect]) -> Vec<String> {
    let mut failures = Vec::new();
    let content: Vec<&Rect> = rects.iter()
        .filter(|r| r.width < 400.0 && r.height > 30.0 && r.height < 80.0)
        .collect();
    for i in 0..content.len() {
        for j in (i + 1)..content.len() {
            let (a, b) = (content[i], content[j]);
            // Same row?
            if (a.y - b.y).abs() < 10.0 && a.x < b.x {
                let gap = b.x - (a.x + a.width);
                if gap < 0.0 {
                    failures.push(format!(
                        "OVERLAP: rects at x={:.0} w={:.0} and x={:.0} gap={:.0} at y={:.0}",
                        a.x, a.width, b.x, gap, a.y
                    ));
                } else if gap > 0.0 && gap < 12.0 {
                    failures.push(format!(
                        "GAP TOO SMALL: {:.0}px (min 12) between rects at x={:.0} and x={:.0} at y={:.0}",
                        gap, a.x, b.x, a.y
                    ));
                }
            }
            // Same column?
            if (a.x - b.x).abs() < 10.0 && a.y < b.y {
                let gap = b.y - (a.y + a.height);
                if gap > 0.0 && gap < 12.0 {
                    failures.push(format!(
                        "VERTICAL GAP TOO SMALL: {:.0}px (min 12) between rects at y={:.0} and y={:.0} at x={:.0}",
                        gap, a.y, b.y, a.x
                    ));
                }
            }
        }
    }
    failures
}

/// Check: all arrows have consistent strokeWidth.
fn check_consistent_stroke(arrows: &[Arrow], svg: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let mut widths = std::collections::HashMap::new();

    // Parse stroke-width from line and polyline elements
    for cap in regex_lite::Regex::new(r#"<(?:line|polyline)[^>]*stroke-width="([\d.]+)"[^>]*>"#)
        .unwrap()
        .captures_iter(svg)
    {
        let sw: f64 = cap[1].parse().unwrap_or(0.0);
        if sw > 0.0 {
            *widths.entry(format!("{:.1}", sw)).or_insert(0u32) += 1;
        }
    }

    if widths.len() > 1 {
        let detail: Vec<String> = widths.iter().map(|(k, v)| format!("{}×sw={}", v, k)).collect();
        failures.push(format!("INCONSISTENT STROKE: {}", detail.join(", ")));
    }

    let _ = arrows; // used indirectly via svg parsing
    failures
}

/// Check: no arrow segment crosses an unrelated element's bounding box.
fn check_arrow_crossing(arrows: &[Arrow], rects: &[Rect]) -> Vec<String> {
    let mut failures = Vec::new();

    // Content rects only (skip zones/backgrounds)
    let content: Vec<&Rect> = rects.iter()
        .filter(|r| r.width < 400.0 && r.width > 20.0 && r.height > 20.0 && r.height < 100.0)
        .collect();

    for arrow in arrows {
        for &(x1, y1, x2, y2) in &arrow.segments {
            let seg_left = x1.min(x2);
            let seg_right = x1.max(x2);
            let seg_top = y1.min(y2);
            let seg_bot = y1.max(y2);

            for rect in &content {
                // Does segment bounding box intersect rect?
                if seg_right >= rect.x
                    && seg_left <= rect.x + rect.width
                    && seg_bot >= rect.y
                    && seg_top <= rect.y + rect.height
                {
                    // Check: is this arrow connected to this rect?
                    // Connected = arrow starts or ends at rect boundary (within 5px)
                    let starts_at = (x1 >= rect.x - 5.0 && x1 <= rect.x + rect.width + 5.0
                        && y1 >= rect.y - 5.0 && y1 <= rect.y + rect.height + 5.0);
                    let ends_at = (x2 >= rect.x - 5.0 && x2 <= rect.x + rect.width + 5.0
                        && y2 >= rect.y - 5.0 && y2 <= rect.y + rect.height + 5.0);

                    if !starts_at && !ends_at {
                        // Check it's not just a close pass — require actual overlap
                        // (segment must pass through the interior, not just graze the edge)
                        let margin = 3.0;
                        if seg_right >= rect.x + margin
                            && seg_left <= rect.x + rect.width - margin
                            && seg_bot >= rect.y + margin
                            && seg_top <= rect.y + rect.height - margin
                        {
                            failures.push(format!(
                                "ARROW CROSSES ELEMENT: segment ({:.0},{:.0})→({:.0},{:.0}) crosses rect at ({:.0},{:.0} w={:.0})",
                                x1, y1, x2, y2, rect.x, rect.y, rect.width
                            ));
                        }
                    }
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

        if (left_margin - right_margin).abs() > 25.0 {
            failures.push(format!(
                "ASYMMETRIC MARGINS: zone at y={:.0} left={:.0}px right={:.0}px (diff {:.0})",
                zone.y, left_margin, right_margin, (left_margin - right_margin).abs()
            ));
        }
    }

    failures
}

/// Auto-fix: centre content within zone backgrounds by shifting x-coordinates.
/// Shifts all elements (rects, texts, lines, polylines) in the zone's y-range.
fn fix_symmetrical_margins(svg: &str, rects: &[Rect]) -> String {
    let mut result = svg.to_string();

    let zones: Vec<&Rect> = rects.iter()
        .filter(|r| r.width > 400.0 && r.height > 100.0)
        .collect();
    let content: Vec<&Rect> = rects.iter()
        .filter(|r| r.width > 20.0 && r.width < 400.0 && r.height > 30.0 && r.height < 80.0)
        .collect();

    for zone in &zones {
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

        if (left_margin - right_margin).abs() <= 25.0 {
            continue;
        }

        let shift = ((right_margin - left_margin) / 2.0).round();
        let y_min = zone.y;
        let y_max = zone.y + zone.height;
        let x_min = zone.x;
        let x_max = zone.x + zone.width;

        // Shift all x-values in elements within this zone
        // We need to find numeric x values in elements whose y is in range
        // Strategy: find all x="N" preceded by y="M" where M is in range

        // Shift rect x values
        let re_rect = regex_lite::Regex::new(
            r#"<rect x="([\d.]+)" y="([\d.]+)" width="([\d.]+)""#
        ).unwrap();

        let mut replacements: Vec<(String, String)> = Vec::new();
        for cap in re_rect.captures_iter(&result) {
            let x: f64 = cap[1].parse().unwrap_or(0.0);
            let y: f64 = cap[2].parse().unwrap_or(0.0);
            let w: f64 = cap[3].parse().unwrap_or(0.0);
            // Only shift content rects in this zone (not the zone bg itself)
            if y >= y_min && y <= y_max && x >= x_min && x + w <= x_max && w < 400.0 {
                let old = format!("x=\"{}\" y=\"{}\" width=\"{}\"", &cap[1], &cap[2], &cap[3]);
                let new_x = x + shift;
                let new_s = format!("x=\"{:.0}\" y=\"{}\" width=\"{}\"", new_x, &cap[2], &cap[3]);
                replacements.push((old, new_s));
            }
        }

        // Shift text x values
        let re_text = regex_lite::Regex::new(
            r#"<text x="([\d.]+)" y="([\d.]+)""#
        ).unwrap();
        for cap in re_text.captures_iter(&result) {
            let x: f64 = cap[1].parse().unwrap_or(0.0);
            let y: f64 = cap[2].parse().unwrap_or(0.0);
            if y >= y_min && y <= y_max && x >= x_min && x <= x_max {
                let old = format!("<text x=\"{}\" y=\"{}\"", &cap[1], &cap[2]);
                let new_x = x + shift;
                let new_s = format!("<text x=\"{:.0}\" y=\"{}\"", new_x, &cap[2]);
                replacements.push((old, new_s));
            }
        }

        // Shift line x1/x2 values
        let re_line = regex_lite::Regex::new(
            r#"x1="([\d.]+)" y1="([\d.]+)" x2="([\d.]+)" y2="([\d.]+)""#
        ).unwrap();
        for cap in re_line.captures_iter(&result) {
            let x1: f64 = cap[1].parse().unwrap_or(0.0);
            let y1: f64 = cap[2].parse().unwrap_or(0.0);
            let x2: f64 = cap[3].parse().unwrap_or(0.0);
            let y2: f64 = cap[4].parse().unwrap_or(0.0);
            if (y1 >= y_min && y1 <= y_max) || (y2 >= y_min && y2 <= y_max) {
                if x1 >= x_min && x2 >= x_min {
                    let old = format!("x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"",
                        &cap[1], &cap[2], &cap[3], &cap[4]);
                    let new_s = format!("x1=\"{:.0}\" y1=\"{}\" x2=\"{:.0}\" y2=\"{}\"",
                        x1 + shift, &cap[2], x2 + shift, &cap[4]);
                    replacements.push((old, new_s));
                }
            }
        }

        // Shift polyline points
        let re_poly = regex_lite::Regex::new(r#"points="([^"]*)""#).unwrap();
        for cap in re_poly.captures_iter(&result.clone()) {
            let pts_str = &cap[1];
            let pts: Vec<(f64, f64)> = pts_str.split_whitespace()
                .filter_map(|p| {
                    let parts: Vec<&str> = p.split(',').collect();
                    if parts.len() == 2 {
                        Some((parts[0].parse().unwrap_or(0.0), parts[1].parse().unwrap_or(0.0)))
                    } else {
                        None
                    }
                })
                .collect();

            let in_zone = pts.iter().any(|(_, y)| *y >= y_min && *y <= y_max);
            if in_zone {
                let new_pts: Vec<String> = pts.iter()
                    .map(|(x, y)| {
                        if *x >= x_min {
                            format!("{:.0},{}", x + shift, y)
                        } else {
                            format!("{},{}", x, y)
                        }
                    })
                    .collect();
                let old = format!("points=\"{}\"", pts_str);
                let new_s = format!("points=\"{}\"", new_pts.join(" "));
                replacements.push((old, new_s));
            }
        }

        // Apply all replacements
        for (old, new_s) in &replacements {
            result = result.replacen(old, new_s, 1);
        }
    }

    result
}

/// Auto-fix: extend short arrows to minimum 18px shaft length.
/// Works on <line> elements by moving the endpoint further from the start.
fn fix_short_arrows(svg: &str) -> String {
    let mut result = svg.to_string();

    let re = regex_lite::Regex::new(
        r#"x1="([\d.]+)" y1="([\d.]+)" x2="([\d.]+)" y2="([\d.]+)""#
    ).unwrap();

    let mut replacements: Vec<(String, String)> = Vec::new();

    for cap in re.captures_iter(svg) {
        let x1: f64 = cap[1].parse().unwrap_or(0.0);
        let y1: f64 = cap[2].parse().unwrap_or(0.0);
        let x2: f64 = cap[3].parse().unwrap_or(0.0);
        let y2: f64 = cap[4].parse().unwrap_or(0.0);

        let dx = x2 - x1;
        let dy = y2 - y1;
        let len = (dx * dx + dy * dy).sqrt();

        if len > 0.0 && len < 15.0 {
            // Extend to 18px in the same direction
            let scale = 18.0 / len;
            let new_x2 = x1 + dx * scale;
            let new_y2 = y1 + dy * scale;

            let old = format!("x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\"",
                &cap[1], &cap[2], &cap[3], &cap[4]);
            let new_s = format!("x1=\"{}\" y1=\"{}\" x2=\"{:.0}\" y2=\"{:.0}\"",
                &cap[1], &cap[2], new_x2, new_y2);
            replacements.push((old, new_s));
        }
    }

    for (old, new_s) in &replacements {
        result = result.replacen(old, new_s, 1);
    }

    result
}

/// Auto-fix: standardise all arrow stroke-widths to 1.
fn fix_stroke_consistency(svg: &str) -> String {
    regex_lite::Regex::new(r#"(<(?:line|polyline)[^>]*stroke-width=")[\d.]+(")"#)
        .unwrap()
        .replace_all(svg, r#"${1}1${2}"#)
        .to_string()
}

/// Auto-fix: widen rects where text overflows.
fn fix_text_overflow(svg: &str, texts: &[Text], rects: &[Rect]) -> String {
    let mut result = svg.to_string();

    for text in texts {
        if let Some(rect) = find_containing_rect(text, rects) {
            let est_width = estimate_text_width(&text.content, text.font_size);
            let margin_left = text.x - rect.x;
            let margin_right = rect.x + rect.width - text.x;
            let available = margin_left.min(margin_right) * 2.0;

            if est_width > available * 0.95 {
                // Need to widen: new width = est_width / 0.9 (give 10% padding)
                let new_width = (est_width / 0.85).max(rect.width);
                let width_delta = new_width - rect.width;
                let new_x = rect.x - width_delta / 2.0;

                let old = format!(
                    "x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"",
                    rect.x, rect.y, rect.width, rect.height
                );
                let new_attr = format!(
                    "x=\"{:.0}\" y=\"{}\" width=\"{:.0}\" height=\"{}\"",
                    new_x, rect.y, new_width, rect.height
                );
                result = result.replacen(&old, &new_attr, 1);
            }
        }
    }

    result
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let fix_mode = args.iter().any(|a| a == "--fix");
    let paths: Vec<&String> = args[1..].iter().filter(|a| *a != "--fix").collect();

    if paths.is_empty() {
        eprintln!("Usage: diagram-lint [--fix] <file.svg> [file2.svg ...]");
        std::process::exit(1);
    }

    let mut total_pass = 0;
    let mut total_fail = 0;

    for path in &paths {
        let mut svg = fs::read_to_string(path).with_context(|| format!("Failed to read: {}", path))?;
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
        failures.extend(check_rect_overlap_and_gap(&rects));
        failures.extend(check_consistent_stroke(&arrows, &svg));
        failures.extend(check_arrow_crossing(&arrows, &rects));
        failures.extend(check_symmetrical_margins(&rects));

        if fix_mode && !failures.is_empty() {
            let mut fixed = Vec::new();

            // Fix stroke consistency
            if failures.iter().any(|f| f.starts_with("INCONSISTENT STROKE")) {
                svg = fix_stroke_consistency(&svg);
                fixed.push("stroke consistency");
            }

            // Fix text overflow
            if failures.iter().any(|f| f.starts_with("TEXT OVERFLOW")) {
                svg = fix_text_overflow(&svg, &texts, &rects);
                fixed.push("text overflow (widened rects)");
            }

            // Fix margin asymmetry
            if failures.iter().any(|f| f.starts_with("ASYMMETRIC MARGINS")) {
                svg = fix_symmetrical_margins(&svg, &rects);
                fixed.push("margin symmetry (shifted content)");
            }

            // Fix short arrows
            if failures.iter().any(|f| f.starts_with("SHORT ARROW")) {
                svg = fix_short_arrows(&svg);
                fixed.push("short arrows (extended shafts)");
            }

            // Fix overlap and min gap
            if failures.iter().any(|f| f.starts_with("OVERLAP") || f.starts_with("GAP TOO SMALL") || f.starts_with("VERTICAL GAP")) {
                svg = fix_element_spacing(&svg);
                fixed.push("element spacing (shifted downstream)");
            }

            // Fix diamond text overflow
            if failures.iter().any(|f| f.starts_with("DIAMOND TEXT")) {
                svg = fix_diamond_text(&svg);
                fixed.push("diamond text (widened diamond)");
            }

            if !fixed.is_empty() {
                fs::write(path, &svg)
                    .with_context(|| format!("Failed to write: {}", path))?;
                println!("  Fixed: {}", fixed.join(", "));

                // Re-check after fixes
                let (rects2, texts2, arrows2, diamonds2) = parse_svg(&svg)?;
                failures.clear();
                failures.extend(check_text_overflow(&texts2, &rects2));
                failures.extend(check_diamond_text(&texts2, &diamonds2));
                failures.extend(check_arrow_length(&arrows2));
                failures.extend(check_rect_overlap_and_gap(&rects2));
                failures.extend(check_consistent_stroke(&arrows2, &svg));
                failures.extend(check_arrow_crossing(&arrows2, &rects2));
                failures.extend(check_symmetrical_margins(&rects2));
            }
        }

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
