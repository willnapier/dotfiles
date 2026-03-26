//! Port of roughjs path perturbation to Rust.
//! Generates hand-drawn/sketchy versions of geometric primitives.

use std::f64::consts::PI;

// ── Simple seedable PRNG (xorshift64) ───────────────────────────────

/// A simple xorshift64 PRNG for deterministic wobble.
#[derive(Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        // Avoid zero state
        Rng { state: if seed == 0 { 1 } else { seed } }
    }

    /// Returns a value in [0.0, 1.0).
    pub fn next_f64(&mut self) -> f64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        (self.state as f64) / (u64::MAX as f64)
    }

    /// Returns a value in [-1.0, 1.0).
    pub fn next_signed(&mut self) -> f64 {
        self.next_f64() * 2.0 - 1.0
    }
}

// ── Core perturbation ───────────────────────────────────────────────

/// Perturb a single point by an offset proportional to roughness and segment length.
fn offset_point(x: f64, y: f64, roughness: f64, seg_len: f64, rng: &mut Rng) -> [f64; 2] {
    let max_offset = roughness * seg_len * 0.15;
    [
        x + rng.next_signed() * max_offset,
        y + rng.next_signed() * max_offset,
    ]
}

// ── rough_line ──────────────────────────────────────────────────────

/// Jitter a straight line into a wobbly path.
/// Returns a series of points from (x1,y1) to (x2,y2) with perturbation.
pub fn rough_line(x1: f64, y1: f64, x2: f64, y2: f64, roughness: f64, seed: u64) -> Vec<[f64; 2]> {
    if roughness <= 0.0 {
        return vec![[x1, y1], [x2, y2]];
    }

    let mut rng = Rng::new(seed);
    let dx = x2 - x1;
    let dy = y2 - y1;
    let seg_len = (dx * dx + dy * dy).sqrt();

    // Number of subdivisions scales with roughness and length
    let num_steps = ((seg_len / 20.0) * roughness).ceil().max(2.0) as usize;

    let mut pts = Vec::with_capacity(num_steps + 1);
    for i in 0..=num_steps {
        let t = i as f64 / num_steps as f64;
        let px = x1 + dx * t;
        let py = y1 + dy * t;

        if i == 0 || i == num_steps {
            // Keep endpoints close to original (less perturbation)
            let p = offset_point(px, py, roughness * 0.3, seg_len, &mut rng);
            pts.push(p);
        } else {
            pts.push(offset_point(px, py, roughness, seg_len, &mut rng));
        }
    }

    pts
}

// ── rough_rect ──────────────────────────────────────────────────────

/// Generate an SVG path `d` attribute for a rough rectangle.
/// Two passes (double stroke) for the sketchy look.
pub fn rough_rect(x: f64, y: f64, w: f64, h: f64, roughness: f64, seed: u64) -> String {
    if roughness <= 0.0 {
        return format!(
            "M{:.2},{:.2} L{:.2},{:.2} L{:.2},{:.2} L{:.2},{:.2} Z",
            x, y, x + w, y, x + w, y + h, x, y + h
        );
    }

    let mut paths = String::new();

    // Two passes for sketchy look (roughjs double-stroke technique)
    let passes = if roughness >= 2.0 { 2 } else { 1 };

    for pass in 0..passes {
        let pass_seed = seed.wrapping_add(pass as u64 * 37);
        let edges = [
            (x, y, x + w, y),         // top
            (x + w, y, x + w, y + h), // right
            (x + w, y + h, x, y + h), // bottom
            (x, y + h, x, y),         // left
        ];

        for (i, &(x1, y1, x2, y2)) in edges.iter().enumerate() {
            let edge_seed = pass_seed.wrapping_add(i as u64 * 13);
            let pts = rough_line(x1, y1, x2, y2, roughness, edge_seed);
            for (j, p) in pts.iter().enumerate() {
                if i == 0 && j == 0 {
                    paths.push_str(&format!("M{:.2},{:.2}", p[0], p[1]));
                } else {
                    paths.push_str(&format!(" L{:.2},{:.2}", p[0], p[1]));
                }
            }
        }
        paths.push_str(" Z ");
    }

    paths.trim().to_string()
}

// ── rough_ellipse ───────────────────────────────────────────────────

/// Generate an SVG path `d` attribute for a rough ellipse.
pub fn rough_ellipse(cx: f64, cy: f64, rx: f64, ry: f64, roughness: f64, seed: u64) -> String {
    if roughness <= 0.0 {
        // Clean ellipse approximated with cubic beziers
        return clean_ellipse_path(cx, cy, rx, ry);
    }

    let mut rng = Rng::new(seed);

    // Number of points around the ellipse
    let num_points = ((2.0 * PI * (rx.max(ry))) / 15.0).ceil().max(16.0) as usize;
    let circumference = 2.0 * PI * ((rx * rx + ry * ry) / 2.0).sqrt();
    let seg_len = circumference / num_points as f64;

    let mut paths = String::new();
    let passes = if roughness >= 2.0 { 2 } else { 1 };

    for pass in 0..passes {
        let mut pass_rng = Rng::new(seed.wrapping_add(pass as u64 * 53));

        for i in 0..=num_points {
            let angle = (i as f64 / num_points as f64) * 2.0 * PI;
            let px = cx + rx * angle.cos();
            let py = cy + ry * angle.sin();

            let p = offset_point(px, py, roughness, seg_len, &mut pass_rng);

            if i == 0 {
                paths.push_str(&format!("M{:.2},{:.2}", p[0], p[1]));
            } else {
                paths.push_str(&format!(" L{:.2},{:.2}", p[0], p[1]));
            }
        }
        paths.push_str(" Z ");
    }

    // Suppress unused rng warning
    let _ = rng.next_f64();

    paths.trim().to_string()
}

/// Clean ellipse path using cubic bezier approximation (4-arc Bezier).
fn clean_ellipse_path(cx: f64, cy: f64, rx: f64, ry: f64) -> String {
    // Kappa constant for cubic Bezier approximation of a quarter circle
    let k = 0.5522847498;
    let kx = rx * k;
    let ky = ry * k;

    format!(
        "M{:.2},{:.2} C{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} C{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} C{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} C{:.2},{:.2} {:.2},{:.2} {:.2},{:.2} Z",
        cx, cy - ry,
        cx + kx, cy - ry, cx + rx, cy - ky, cx + rx, cy,
        cx + rx, cy + ky, cx + kx, cy + ry, cx, cy + ry,
        cx - kx, cy + ry, cx - rx, cy + ky, cx - rx, cy,
        cx - rx, cy - ky, cx - kx, cy - ry, cx, cy - ry,
    )
}

// ── rough_arc ───────────────────────────────────────────────────────

/// Generate an SVG path for a rough arc segment of an ellipse.
/// `start` and `end` are angles in radians.
pub fn rough_arc(
    cx: f64, cy: f64, rx: f64, ry: f64,
    start: f64, end: f64, roughness: f64, seed: u64,
) -> String {
    if roughness <= 0.0 {
        // Clean arc
        let num_pts = 16;
        let mut path = String::new();
        for i in 0..=num_pts {
            let t = start + (end - start) * (i as f64 / num_pts as f64);
            let px = cx + rx * t.cos();
            let py = cy + ry * t.sin();
            if i == 0 {
                path.push_str(&format!("M{:.2},{:.2}", px, py));
            } else {
                path.push_str(&format!(" L{:.2},{:.2}", px, py));
            }
        }
        return path;
    }

    let mut rng = Rng::new(seed);
    let arc_len = (end - start).abs() * ((rx + ry) / 2.0);
    let num_pts = ((arc_len / 15.0) * roughness).ceil().max(8.0) as usize;
    let seg_len = arc_len / num_pts as f64;

    let mut path = String::new();
    for i in 0..=num_pts {
        let t = start + (end - start) * (i as f64 / num_pts as f64);
        let px = cx + rx * t.cos();
        let py = cy + ry * t.sin();
        let p = offset_point(px, py, roughness, seg_len, &mut rng);

        if i == 0 {
            path.push_str(&format!("M{:.2},{:.2}", p[0], p[1]));
        } else {
            path.push_str(&format!(" L{:.2},{:.2}", p[0], p[1]));
        }
    }

    path
}

// ── hachure_fill ────────────────────────────────────────────────────

/// Generate SVG path data for hachure fill (angled parallel strokes).
pub fn hachure_fill(
    x: f64, y: f64, w: f64, h: f64,
    angle: f64, gap: f64, roughness: f64, seed: u64,
) -> String {
    let mut rng = Rng::new(seed);

    // Rotate the fill lines by the given angle
    let cos_a = angle.cos();
    let sin_a = angle.sin();

    // Calculate the extent we need to cover when rotating
    let diagonal = (w * w + h * h).sqrt();
    let cx = x + w / 2.0;
    let cy = y + h / 2.0;

    let mut paths = String::new();
    let num_lines = (diagonal / gap).ceil() as i32;

    for i in (-num_lines)..=num_lines {
        let offset = i as f64 * gap;

        // Line in rotated space (horizontal line at offset)
        let lx1 = -diagonal;
        let ly1 = offset;
        let lx2 = diagonal;
        let ly2 = offset;

        // Rotate back
        let rx1 = cx + lx1 * cos_a - ly1 * sin_a;
        let ry1 = cy + lx1 * sin_a + ly1 * cos_a;
        let rx2 = cx + lx2 * cos_a - ly2 * sin_a;
        let ry2 = cy + lx2 * sin_a + ly2 * cos_a;

        // Clip to rectangle bounds
        if let Some((cx1, cy1, cx2, cy2)) = clip_line_to_rect(rx1, ry1, rx2, ry2, x, y, x + w, y + h) {
            // Add roughness to the hachure strokes
            let seg_len = ((cx2 - cx1).powi(2) + (cy2 - cy1).powi(2)).sqrt();
            let max_offset = roughness * seg_len * 0.05;

            let sx = cx1 + rng.next_signed() * max_offset;
            let sy = cy1 + rng.next_signed() * max_offset;
            let ex = cx2 + rng.next_signed() * max_offset;
            let ey = cy2 + rng.next_signed() * max_offset;

            paths.push_str(&format!("M{:.2},{:.2} L{:.2},{:.2} ", sx, sy, ex, ey));
        }
    }

    paths.trim().to_string()
}

/// Cohen-Sutherland line clipping to a rectangle.
fn clip_line_to_rect(
    mut x1: f64, mut y1: f64, mut x2: f64, mut y2: f64,
    xmin: f64, ymin: f64, xmax: f64, ymax: f64,
) -> Option<(f64, f64, f64, f64)> {
    const INSIDE: u8 = 0;
    const LEFT: u8 = 1;
    const RIGHT: u8 = 2;
    const BOTTOM: u8 = 4;
    const TOP: u8 = 8;

    let compute_code = |x: f64, y: f64| -> u8 {
        let mut code = INSIDE;
        if x < xmin { code |= LEFT; }
        else if x > xmax { code |= RIGHT; }
        if y < ymin { code |= TOP; }
        else if y > ymax { code |= BOTTOM; }
        code
    };

    let mut code1 = compute_code(x1, y1);
    let mut code2 = compute_code(x2, y2);

    for _ in 0..20 {
        if (code1 | code2) == 0 {
            return Some((x1, y1, x2, y2));
        }
        if (code1 & code2) != 0 {
            return None;
        }

        let code_out = if code1 != 0 { code1 } else { code2 };
        let (x, y);

        if code_out & TOP != 0 {
            x = x1 + (x2 - x1) * (ymin - y1) / (y2 - y1);
            y = ymin;
        } else if code_out & BOTTOM != 0 {
            x = x1 + (x2 - x1) * (ymax - y1) / (y2 - y1);
            y = ymax;
        } else if code_out & RIGHT != 0 {
            y = y1 + (y2 - y1) * (xmax - x1) / (x2 - x1);
            x = xmax;
        } else {
            y = y1 + (y2 - y1) * (xmin - x1) / (x2 - x1);
            x = xmin;
        }

        if code_out == code1 {
            x1 = x; y1 = y;
            code1 = compute_code(x1, y1);
        } else {
            x2 = x; y2 = y;
            code2 = compute_code(x2, y2);
        }
    }

    None
}

// ── Jitter points (for organic connector outlines) ──────────────────

/// Add slight jitter to a set of polygon points.
pub fn jitter_points(points: &[[f64; 2]], roughness: f64, seed: u64) -> Vec<[f64; 2]> {
    if roughness <= 0.0 {
        return points.to_vec();
    }

    let mut rng = Rng::new(seed);
    points
        .iter()
        .map(|p| {
            let max_offset = roughness * 1.5;
            [
                p[0] + rng.next_signed() * max_offset,
                p[1] + rng.next_signed() * max_offset,
            ]
        })
        .collect()
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rough_line_produces_more_points() {
        let pts = rough_line(0.0, 0.0, 100.0, 0.0, 1.5, 42);
        assert!(pts.len() > 2, "rough line should have more than 2 points, got {}", pts.len());
    }

    #[test]
    fn rough_rect_valid_svg_path() {
        let path = rough_rect(10.0, 10.0, 100.0, 60.0, 1.5, 42);
        assert!(path.starts_with('M'), "path should start with M: {}", path);
        assert!(path.contains('L'), "path should contain L commands: {}", path);
        assert!(path.contains('Z'), "path should be closed with Z: {}", path);
    }

    #[test]
    fn deterministic_same_seed() {
        let a = rough_line(0.0, 0.0, 100.0, 50.0, 2.0, 123);
        let b = rough_line(0.0, 0.0, 100.0, 50.0, 2.0, 123);
        assert_eq!(a, b, "same seed should produce identical output");
    }

    #[test]
    fn different_seeds_differ() {
        let a = rough_line(0.0, 0.0, 100.0, 50.0, 2.0, 123);
        let b = rough_line(0.0, 0.0, 100.0, 50.0, 2.0, 456);
        assert_ne!(a, b, "different seeds should produce different output");
    }

    #[test]
    fn zero_roughness_minimal_perturbation() {
        let pts = rough_line(0.0, 0.0, 100.0, 0.0, 0.0, 42);
        assert_eq!(pts.len(), 2, "zero roughness should produce a straight line");
        assert_eq!(pts[0], [0.0, 0.0]);
        assert_eq!(pts[1], [100.0, 0.0]);
    }

    #[test]
    fn rough_ellipse_valid_path() {
        let path = rough_ellipse(50.0, 50.0, 40.0, 30.0, 1.5, 42);
        assert!(path.starts_with('M'), "ellipse path should start with M");
        assert!(path.contains('Z'), "ellipse path should be closed");
    }

    #[test]
    fn rough_rect_double_stroke_at_high_roughness() {
        let path = rough_rect(0.0, 0.0, 100.0, 50.0, 3.0, 42);
        // At roughness >= 2, we get two passes, each ending with Z
        let z_count = path.matches('Z').count();
        assert!(z_count >= 2, "high roughness should produce double stroke (got {} Z)", z_count);
    }

    #[test]
    fn hachure_fill_produces_lines() {
        let path = hachure_fill(0.0, 0.0, 100.0, 80.0, -41.0_f64.to_radians(), 8.0, 1.0, 42);
        assert!(!path.is_empty(), "hachure fill should produce some strokes");
        assert!(path.contains('M'), "hachure should have move commands");
        assert!(path.contains('L'), "hachure should have line commands");
    }

    #[test]
    fn jitter_points_adds_noise() {
        let pts = vec![[0.0, 0.0], [50.0, 50.0], [100.0, 0.0]];
        let jittered = jitter_points(&pts, 2.0, 42);
        assert_eq!(jittered.len(), pts.len());
        // At least one point should differ
        let any_different = jittered.iter().zip(pts.iter()).any(|(a, b)| a[0] != b[0] || a[1] != b[1]);
        assert!(any_different, "jitter should move at least one point");
    }

    #[test]
    fn jitter_zero_roughness_unchanged() {
        let pts = vec![[10.0, 20.0], [30.0, 40.0]];
        let result = jitter_points(&pts, 0.0, 42);
        assert_eq!(result, pts);
    }
}
