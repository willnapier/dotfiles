//! Port of perfect-freehand by Steve Ruiz.
//! Converts a polyline with optional pressure into a filled outline polygon
//! with natural thick-to-thin tapering.

use std::f64::consts::PI;

// ── Vec2 utilities ───────────────────────────────────────────────────

type Vec2 = [f64; 2];

fn add(a: Vec2, b: Vec2) -> Vec2 { [a[0] + b[0], a[1] + b[1]] }
fn sub(a: Vec2, b: Vec2) -> Vec2 { [a[0] - b[0], a[1] - b[1]] }
fn mul(a: Vec2, n: f64) -> Vec2 { [a[0] * n, a[1] * n] }
fn neg(a: Vec2) -> Vec2 { [-a[0], -a[1]] }
fn per(a: Vec2) -> Vec2 { [a[1], -a[0]] }
fn dpr(a: Vec2, b: Vec2) -> f64 { a[0] * b[0] + a[1] * b[1] }
fn len2(a: Vec2) -> f64 { a[0] * a[0] + a[1] * a[1] }
fn len(a: Vec2) -> f64 { a[0].hypot(a[1]) }
fn dist2(a: Vec2, b: Vec2) -> f64 { len2(sub(a, b)) }
fn dist(a: Vec2, b: Vec2) -> f64 { len(sub(a, b)) }
fn uni(a: Vec2) -> Vec2 { let l = len(a); if l == 0.0 { a } else { [a[0] / l, a[1] / l] } }
fn lrp(a: Vec2, b: Vec2, t: f64) -> Vec2 { [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t] }
fn prj(a: Vec2, b: Vec2, c: f64) -> Vec2 { add(a, mul(b, c)) }
fn is_equal(a: Vec2, b: Vec2) -> bool { a[0] == b[0] && a[1] == b[1] }

fn rot_around(a: Vec2, c: Vec2, r: f64) -> Vec2 {
    let (sin_r, cos_r) = r.sin_cos();
    let dx = a[0] - c[0];
    let dy = a[1] - c[1];
    [c[0] + dx * cos_r - dy * sin_r, c[1] + dx * sin_r + dy * cos_r]
}

// ── Constants ────────────────────────────────────────────────────────

const RATE_OF_PRESSURE_CHANGE: f64 = 0.275;
const FIXED_PI: f64 = PI + 0.0001;
const MIN_RADIUS: f64 = 0.01;
const DEFAULT_FIRST_PRESSURE: f64 = 0.25;
const DEFAULT_PRESSURE: f64 = 0.5;

// ── Easing functions ─────────────────────────────────────────────────

pub fn ease_linear(t: f64) -> f64 { t }
pub fn ease_out_quad(t: f64) -> f64 { t * (2.0 - t) }
pub fn ease_out_cubic(t: f64) -> f64 { let t = t - 1.0; t * t * t + 1.0 }
pub fn ease_in_quad(t: f64) -> f64 { t * t }
pub fn ease_in_cubic(t: f64) -> f64 { t * t * t }
pub fn ease_in_out_cubic(t: f64) -> f64 {
    if t < 0.5 { 4.0 * t * t * t } else { let t = t - 1.0; (2.0 * t) * (2.0 * t) * t + 1.0 }
}
pub fn ease_in_sine(t: f64) -> f64 { 1.0 - (t * PI / 2.0).cos() }
pub fn ease_out_sine(t: f64) -> f64 { (t * PI / 2.0).sin() }

/// Resolve an easing function name (from customData) to a function.
pub fn easing_by_name(name: &str) -> fn(f64) -> f64 {
    match name {
        "linear" => ease_linear,
        "easeOutQuad" => ease_out_quad,
        "easeOutCubic" => ease_out_cubic,
        "easeInQuad" => ease_in_quad,
        "easeInCubic" => ease_in_cubic,
        "easeInOutCubic" => ease_in_out_cubic,
        "easeInSine" => ease_in_sine,
        "easeOutSine" => ease_out_sine,
        _ => ease_linear,
    }
}

// ── Options ──────────────────────────────────────────────────────────

/// Configuration for stroke generation.
pub struct StrokeOptions {
    pub size: f64,
    pub thinning: f64,
    pub smoothing: f64,
    pub streamline: f64,
    pub easing: fn(f64) -> f64,
    pub simulate_pressure: bool,
    pub start_taper: f64,      // 0 = no taper, >0 = taper distance in px
    pub start_cap: bool,
    pub start_easing: fn(f64) -> f64,
    pub end_taper: f64,
    pub end_cap: bool,
    pub end_easing: fn(f64) -> f64,
    pub last: bool,
}

impl Default for StrokeOptions {
    fn default() -> Self {
        StrokeOptions {
            size: 16.0,
            thinning: 0.5,
            smoothing: 0.5,
            streamline: 0.5,
            easing: ease_linear,
            simulate_pressure: true,
            start_taper: 0.0,
            start_cap: true,
            start_easing: ease_out_quad,
            end_taper: 0.0,
            end_cap: true,
            end_easing: ease_out_cubic,
            last: false,
        }
    }
}

/// Preset for mind map branches: thick at start, tapers to thin at end.
pub fn organic_branch(size: f64) -> StrokeOptions {
    StrokeOptions {
        size,
        thinning: 0.6,
        smoothing: 0.5,
        streamline: 0.5,
        easing: ease_linear,
        simulate_pressure: false,
        start_taper: 0.0,
        start_cap: false,
        start_easing: ease_out_quad,
        end_taper: f64::MAX, // taper entire length
        end_cap: false,
        end_easing: ease_out_cubic,
        last: true,
    }
}

// ── Internal types ───────────────────────────────────────────────────

struct StrokePoint {
    point: Vec2,
    pressure: f64,
    distance: f64,
    vector: Vec2,
    running_length: f64,
}

// ── Phase 1: getStrokePoints ─────────────────────────────────────────

fn get_stroke_points(input: &[[f64; 2]], pressures: Option<&[f64]>, opts: &StrokeOptions) -> Vec<StrokePoint> {
    let t = 0.15 + (1.0 - opts.streamline) * 0.85;

    if input.is_empty() {
        return Vec::new();
    }

    // Build normalised input with pressures
    let mut pts: Vec<(Vec2, f64)> = input.iter().enumerate()
        .map(|(i, p)| {
            let pressure = pressures.and_then(|pr| pr.get(i).copied()).unwrap_or(DEFAULT_PRESSURE);
            (*p, pressure)
        })
        .collect();

    // Handle degenerate cases
    if pts.len() == 1 {
        pts.push((add(pts[0].0, [1.0, 1.0]), pts[0].1));
    }
    if pts.len() == 2 {
        let a = pts[0];
        let b = pts[1];
        let mid1 = (lrp(a.0, b.0, 0.25), (a.1 + b.1) / 2.0);
        let mid2 = (lrp(a.0, b.0, 0.50), (a.1 + b.1) / 2.0);
        let mid3 = (lrp(a.0, b.0, 0.75), (a.1 + b.1) / 2.0);
        pts = vec![a, mid1, mid2, mid3, b];
    }

    let mut result = Vec::with_capacity(pts.len());

    // First point
    let first_pressure = if pressures.is_some() { pts[0].1 } else { DEFAULT_FIRST_PRESSURE };
    result.push(StrokePoint {
        point: pts[0].0,
        pressure: first_pressure,
        distance: 0.0,
        vector: [1.0, 1.0],
        running_length: 0.0,
    });

    let mut running_length = 0.0;
    let mut has_reached_min_length = false;

    for i in 1..pts.len() {
        let is_last = opts.last && i == pts.len() - 1;

        let prev_point = result.last().unwrap().point;
        let point = if is_last { pts[i].0 } else { lrp(prev_point, pts[i].0, t) };

        if is_equal(point, prev_point) {
            continue;
        }

        let d = dist(point, prev_point);
        running_length += d;

        if !has_reached_min_length {
            if running_length < opts.size {
                if !is_last {
                    continue;
                }
            }
            has_reached_min_length = true;
        }

        let vector = uni(sub(prev_point, point));

        result.push(StrokePoint {
            point,
            pressure: pts[i].1,
            distance: d,
            vector,
            running_length,
        });
    }

    // Fix first point's vector
    if result.len() > 1 {
        let v = result[1].vector;
        result[0].vector = v;
    }

    result
}

// ── Phase 2: getStrokeOutlinePoints ──────────────────────────────────

fn simulate_pressure(prev: f64, distance: f64, size: f64) -> f64 {
    let sp = (distance / size).min(1.0);
    let rp = (1.0 - sp).min(1.0);
    (prev + (rp - prev) * (sp * RATE_OF_PRESSURE_CHANGE)).min(1.0)
}

fn get_stroke_radius(size: f64, thinning: f64, pressure: f64, easing: fn(f64) -> f64) -> f64 {
    size * easing(0.5 - thinning * (0.5 - pressure))
}

fn get_stroke_outline_points(points: &[StrokePoint], opts: &StrokeOptions) -> Vec<Vec2> {
    if points.is_empty() {
        return Vec::new();
    }
    if points.len() == 1 {
        // Single dot
        return make_dot(&points[0].point, opts.size / 2.0);
    }

    let total_length = points.last().unwrap().running_length;

    let taper_start = if opts.start_taper <= 0.0 { 0.0 }
        else { opts.start_taper.min(total_length) };
    let taper_end = if opts.end_taper <= 0.0 { 0.0 }
        else { opts.end_taper.min(total_length) };

    let min_dist = (opts.size * opts.smoothing).powi(2);

    // Initial pressure average (first 10 points)
    let mut prev_pressure = points[0].pressure;
    for sp in points.iter().take(10) {
        let p = if opts.simulate_pressure {
            simulate_pressure(prev_pressure, sp.distance, opts.size)
        } else {
            sp.pressure
        };
        prev_pressure = (prev_pressure + p) / 2.0;
    }

    let mut left_pts: Vec<Vec2> = Vec::new();
    let mut right_pts: Vec<Vec2> = Vec::new();
    let mut prev_vector = points[0].vector;
    let mut radius;

    for (i, sp) in points.iter().enumerate() {
        // Skip near-end noise
        if i < points.len() - 1 && total_length - sp.running_length < 3.0 {
            continue;
        }

        // Compute radius from pressure
        if opts.thinning != 0.0 {
            let pressure = if opts.simulate_pressure {
                let p = simulate_pressure(prev_pressure, sp.distance, opts.size);
                prev_pressure = p;
                p
            } else {
                prev_pressure = sp.pressure;
                sp.pressure
            };
            radius = get_stroke_radius(opts.size, opts.thinning, pressure, opts.easing);
        } else {
            radius = opts.size / 2.0;
        }

        // Apply tapering
        let ts = if taper_start > 0.0 && sp.running_length < taper_start {
            (opts.start_easing)(sp.running_length / taper_start)
        } else { 1.0 };
        let te = if taper_end > 0.0 && total_length - sp.running_length < taper_end {
            (opts.end_easing)((total_length - sp.running_length) / taper_end)
        } else { 1.0 };
        radius = (radius * ts.min(te)).max(MIN_RADIUS);

        // Handle sharp corners
        if i > 0 && dpr(sp.vector, prev_vector) < 0.0 {
            // Corner cap: semicircle
            let offset = mul(per(prev_vector), radius);
            for j in 0..=13 {
                let angle = (j as f64 / 13.0) * PI;
                let rotated = rot_around(sub(sp.point, offset), sp.point, angle);
                left_pts.push(rotated);
                right_pts.push(rot_around(add(sp.point, offset), sp.point, angle));
            }
            prev_vector = sp.vector;
            continue;
        }

        // Compute offset direction
        let next_vector = if i < points.len() - 1 { points[i + 1].vector } else { sp.vector };
        let next_dpr = dpr(sp.vector, next_vector);
        let offset = mul(per(lrp(next_vector, sp.vector, next_dpr)), radius);

        let left = sub(sp.point, offset);
        let right = add(sp.point, offset);

        // Smoothing: skip points too close together (except first two and last)
        if i <= 1 || i == points.len() - 1
            || left_pts.last().map(|p| dist2(*p, left) > min_dist).unwrap_or(true)
        {
            left_pts.push(left);
            right_pts.push(right);
        }

        prev_vector = sp.vector;
    }

    // Build output polygon
    let mut outline: Vec<Vec2> = Vec::new();

    // Start cap
    let start_cap = if taper_start > 0.0 || left_pts.is_empty() || right_pts.is_empty() {
        Vec::new()
    } else if opts.start_cap {
        // Round cap
        let first_l = left_pts[0];
        let first_r = right_pts[0];
        let center = lrp(first_l, first_r, 0.5);
        let mut cap = Vec::new();
        for j in (0..=13).rev() {
            let angle = (j as f64 / 13.0) * PI;
            cap.push(rot_around(first_r, center, angle));
        }
        cap
    } else {
        vec![left_pts[0]] // flat
    };

    // End cap
    let end_cap = if taper_end > 0.0 || left_pts.is_empty() || right_pts.is_empty() {
        if let (Some(&last_l), Some(&last_r)) = (left_pts.last(), right_pts.last()) {
            vec![lrp(last_l, last_r, 0.5)]
        } else { Vec::new() }
    } else if opts.end_cap {
        let last_l = *left_pts.last().unwrap();
        let last_r = *right_pts.last().unwrap();
        let center = lrp(last_l, last_r, 0.5);
        let mut cap = Vec::new();
        for j in 0..=29 {
            let angle = -(j as f64 / 29.0) * (PI * 1.5);
            cap.push(rot_around(last_l, center, angle));
        }
        cap
    } else {
        vec![*right_pts.last().unwrap()]
    };

    // Assemble: left side → end cap → right side reversed → start cap
    outline.extend_from_slice(&left_pts);
    outline.extend(end_cap);
    right_pts.reverse();
    outline.extend_from_slice(&right_pts);
    outline.extend(start_cap);

    outline
}

fn make_dot(center: &Vec2, radius: f64) -> Vec<Vec2> {
    let mut pts = Vec::with_capacity(21);
    for i in 0..=20 {
        let angle = (i as f64 / 20.0) * PI * 2.0;
        pts.push([center[0] + radius * angle.cos(), center[1] + radius * angle.sin()]);
    }
    pts
}

// ── Public API ───────────────────────────────────────────────────────

/// Generate a stroke outline polygon from a polyline.
///
/// Input: points as `[x, y]` pairs, optional per-point pressures.
/// Output: closed polygon vertices suitable for SVG `<polygon>` or fill path.
pub fn get_stroke(points: &[[f64; 2]], pressures: Option<&[f64]>, opts: &StrokeOptions) -> Vec<[f64; 2]> {
    let stroke_points = get_stroke_points(points, pressures, opts);
    get_stroke_outline_points(&stroke_points, opts)
}

/// Convert outline polygon to an SVG path `d` attribute (closed path).
pub fn outline_to_svg_path(outline: &[[f64; 2]]) -> String {
    if outline.is_empty() {
        return String::new();
    }
    let mut d = format!("M{:.2},{:.2}", outline[0][0], outline[0][1]);
    for p in &outline[1..] {
        d.push_str(&format!(" L{:.2},{:.2}", p[0], p[1]));
    }
    d.push_str(" Z");
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_stroke() {
        let points = vec![[0.0, 0.0], [50.0, 0.0], [100.0, 0.0]];
        let opts = StrokeOptions { size: 8.0, ..Default::default() };
        let outline = get_stroke(&points, None, &opts);
        assert!(!outline.is_empty());
        // Outline should have points above and below the horizontal line
        let has_above = outline.iter().any(|p| p[1] < 0.0);
        let has_below = outline.iter().any(|p| p[1] > 0.0);
        assert!(has_above && has_below, "outline should extend above and below the center line");
    }

    #[test]
    fn organic_branch_tapers() {
        let points: Vec<[f64; 2]> = (0..=20).map(|i| [i as f64 * 5.0, 0.0]).collect();
        let opts = organic_branch(8.0);
        let outline = get_stroke(&points, None, &opts);
        assert!(!outline.is_empty());
        // The start should be wider than the end
        let start_width = outline.iter()
            .filter(|p| p[0] < 10.0)
            .map(|p| p[1].abs())
            .fold(0.0f64, f64::max);
        let end_width = outline.iter()
            .filter(|p| p[0] > 90.0)
            .map(|p| p[1].abs())
            .fold(0.0f64, f64::max);
        assert!(start_width > end_width, "organic branch should taper: start {start_width:.1} > end {end_width:.1}");
    }

    #[test]
    fn svg_path_output() {
        let points = vec![[0.0, 0.0], [100.0, 0.0]];
        let opts = StrokeOptions { size: 4.0, last: true, ..Default::default() };
        let outline = get_stroke(&points, None, &opts);
        let path = outline_to_svg_path(&outline);
        assert!(path.starts_with('M'));
        assert!(path.ends_with('Z'));
    }

    #[test]
    fn single_point_makes_dot() {
        let points = vec![[50.0, 50.0]];
        let opts = StrokeOptions { size: 10.0, ..Default::default() };
        let outline = get_stroke(&points, None, &opts);
        assert!(outline.len() > 10, "dot should have multiple vertices");
    }
}
