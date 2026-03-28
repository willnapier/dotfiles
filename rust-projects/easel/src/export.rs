use crate::freehand;
use crate::rough;
use crate::scene::Scene;
use crate::visual_style::VisualStyle;

/// Export a scene to SVG with Nunito font and consistent arrowhead markers.
/// When `style` is None or clean, uses geometric primitives (current behaviour).
/// When a non-clean style is provided, renders shapes with rough/hand-drawn paths.
pub fn to_svg(scene: &Scene) -> String {
    to_svg_styled(scene, None)
}

/// Export a scene to SVG with an optional visual style for hand-drawn rendering.
pub fn to_svg_styled(scene: &Scene, style: Option<&VisualStyle>) -> String {
    let is_rough = style.map_or(false, |s| !s.is_clean());

    // Calculate viewBox from element bounds
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;

    for el in &scene.elements {
        if el.is_deleted {
            continue;
        }
        min_x = min_x.min(el.x);
        min_y = min_y.min(el.y);
        max_x = max_x.max(el.x + el.width.abs());
        max_y = max_y.max(el.y + el.height.abs());

        // Account for arrow endpoints
        if let Some(ref pts) = el.points {
            for p in pts {
                max_x = max_x.max(el.x + p[0]);
                max_y = max_y.max(el.y + p[1]);
                min_x = min_x.min(el.x + p[0]);
                min_y = min_y.min(el.y + p[1]);
            }
        }
    }

    let pad = 30.0;
    let vx = (min_x - pad).floor();
    let vy = (min_y - pad).floor();
    let vw = (max_x - min_x + pad * 2.0).ceil();
    let vh = (max_y - min_y + pad * 2.0).ceil();

    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vx} {vy} {vw} {vh}" width="{vw}">
<defs>
  <marker id="ah" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
    <path d="M2 1L8 5L2 9" fill="none" stroke="context-stroke" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
  </marker>
</defs>
<style>
  @import url('https://fonts.googleapis.com/css2?family=Nunito:wght@400;500;600;700&amp;display=swap');
  text {{ font-family: 'Nunito', sans-serif; }}
</style>
<rect x="{vx}" y="{vy}" width="{vw}" height="{vh}" fill="white"/>
"#
    );

    // Element index used to vary seeds per element for deterministic but unique wobble
    let mut el_idx: u64 = 0;

    // Render elements in order (z-order = array order)
    for el in &scene.elements {
        if el.is_deleted {
            continue;
        }
        el_idx += 1;

        let el_seed = style.map_or(0, |s| s.seed.wrapping_add(el_idx * 97));

        match el.element_type.as_str() {
            "rectangle" => {
                if is_rough {
                    let vs = style.unwrap();
                    // Rough rectangle outline
                    let path_d = rough::rough_rect(el.x, el.y, el.width, el.height, vs.roughness, el_seed);
                    // Fill: either hachure or solid
                    if vs.hachure && el.background_color != "transparent" {
                        // Solid fill underneath for background color
                        let rx = if el.roundness.is_some() { 8 } else { 0 };
                        svg.push_str(&format!(
                            r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}" stroke="none" rx="{}" opacity="{}"/>
"#,
                            el.x, el.y, el.width, el.height,
                            el.background_color, rx, el.opacity as f64 / 100.0
                        ));
                        // Hachure fill overlay
                        let hachure_d = rough::hachure_fill(
                            el.x, el.y, el.width, el.height,
                            vs.hachure_angle.to_radians(), vs.hachure_gap,
                            vs.roughness, el_seed.wrapping_add(7),
                        );
                        if !hachure_d.is_empty() {
                            svg.push_str(&format!(
                                r#"<path d="{}" fill="none" stroke="{}" stroke-width="1" opacity="{}"/>
"#,
                                hachure_d, el.stroke_color,
                                (el.opacity as f64 / 100.0) * 0.5
                            ));
                        }
                    } else if el.background_color != "transparent" {
                        // Solid fill with rough edges (use the rough path as a filled shape)
                        let fill_path = rough::rough_rect(el.x, el.y, el.width, el.height, vs.roughness * 0.3, el_seed.wrapping_add(3));
                        svg.push_str(&format!(
                            r#"<path d="{}" fill="{}" stroke="none" opacity="{}"/>
"#,
                            fill_path, el.background_color,
                            el.opacity as f64 / 100.0
                        ));
                    }
                    // Rough outline stroke
                    svg.push_str(&format!(
                        r#"<path d="{}" fill="none" stroke="{}" stroke-width="{}" opacity="{}"/>
"#,
                        path_d, el.stroke_color, el.stroke_width,
                        el.opacity as f64 / 100.0
                    ));
                } else {
                    let rx = if el.roundness.is_some() { 8 } else { 0 };
                    svg.push_str(&format!(
                        r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}" stroke="{}" stroke-width="{}" rx="{}" opacity="{}"/>
"#,
                        el.x, el.y, el.width, el.height,
                        el.background_color, el.stroke_color, el.stroke_width,
                        rx, el.opacity as f64 / 100.0
                    ));
                }
            }

            "ellipse" => {
                let cx = el.x + el.width / 2.0;
                let cy = el.y + el.height / 2.0;
                let rx = el.width / 2.0;
                let ry = el.height / 2.0;

                if is_rough {
                    let vs = style.unwrap();
                    // Fill
                    if vs.hachure && el.background_color != "transparent" {
                        // Solid background
                        svg.push_str(&format!(
                            r#"<ellipse cx="{:.0}" cy="{:.0}" rx="{:.0}" ry="{:.0}" fill="{}" stroke="none" opacity="{}"/>
"#,
                            cx, cy, rx, ry,
                            el.background_color, el.opacity as f64 / 100.0
                        ));
                        // Hachure (use bounding rect)
                        let hachure_d = rough::hachure_fill(
                            el.x, el.y, el.width, el.height,
                            vs.hachure_angle.to_radians(), vs.hachure_gap,
                            vs.roughness, el_seed.wrapping_add(7),
                        );
                        if !hachure_d.is_empty() {
                            // Clip to ellipse
                            let clip_id = format!("ec{}", el_idx);
                            svg.push_str(&format!(
                                r#"<clipPath id="{}"><ellipse cx="{:.0}" cy="{:.0}" rx="{:.0}" ry="{:.0}"/></clipPath>
"#,
                                clip_id, cx, cy, rx, ry
                            ));
                            svg.push_str(&format!(
                                r#"<path d="{}" fill="none" stroke="{}" stroke-width="1" opacity="{}" clip-path="url(#{})"/>
"#,
                                hachure_d, el.stroke_color,
                                (el.opacity as f64 / 100.0) * 0.5, clip_id
                            ));
                        }
                    } else if el.background_color != "transparent" {
                        let fill_path = rough::rough_ellipse(cx, cy, rx, ry, vs.roughness * 0.3, el_seed.wrapping_add(3));
                        svg.push_str(&format!(
                            r#"<path d="{}" fill="{}" stroke="none" opacity="{}"/>
"#,
                            fill_path, el.background_color,
                            el.opacity as f64 / 100.0
                        ));
                    }
                    // Rough outline
                    let path_d = rough::rough_ellipse(cx, cy, rx, ry, vs.roughness, el_seed);
                    svg.push_str(&format!(
                        r#"<path d="{}" fill="none" stroke="{}" stroke-width="{}" opacity="{}"/>
"#,
                        path_d, el.stroke_color, el.stroke_width,
                        el.opacity as f64 / 100.0
                    ));
                } else {
                    svg.push_str(&format!(
                        r#"<ellipse cx="{:.0}" cy="{:.0}" rx="{:.0}" ry="{:.0}" fill="{}" stroke="{}" stroke-width="{}" opacity="{}"/>
"#,
                        cx, cy, rx, ry,
                        el.background_color, el.stroke_color, el.stroke_width,
                        el.opacity as f64 / 100.0
                    ));
                }
            }

            "diamond" => {
                let cx = el.x + el.width / 2.0;
                let cy = el.y + el.height / 2.0;

                if is_rough {
                    let vs = style.unwrap();
                    // Diamond as 4 rough lines
                    let corners = [
                        (cx, el.y),
                        (el.x + el.width, cy),
                        (cx, el.y + el.height),
                        (el.x, cy),
                    ];

                    // Fill
                    if vs.hachure && el.background_color != "transparent" {
                        svg.push_str(&format!(
                            r#"<polygon points="{:.0},{:.0} {:.0},{:.0} {:.0},{:.0} {:.0},{:.0}" fill="{}" stroke="none" opacity="{}"/>
"#,
                            corners[0].0, corners[0].1,
                            corners[1].0, corners[1].1,
                            corners[2].0, corners[2].1,
                            corners[3].0, corners[3].1,
                            el.background_color, el.opacity as f64 / 100.0
                        ));
                        let hachure_d = rough::hachure_fill(
                            el.x, el.y, el.width, el.height,
                            vs.hachure_angle.to_radians(), vs.hachure_gap,
                            vs.roughness, el_seed.wrapping_add(7),
                        );
                        if !hachure_d.is_empty() {
                            let clip_id = format!("dc{}", el_idx);
                            svg.push_str(&format!(
                                r#"<clipPath id="{}"><polygon points="{:.0},{:.0} {:.0},{:.0} {:.0},{:.0} {:.0},{:.0}"/></clipPath>
"#,
                                clip_id,
                                corners[0].0, corners[0].1,
                                corners[1].0, corners[1].1,
                                corners[2].0, corners[2].1,
                                corners[3].0, corners[3].1,
                            ));
                            svg.push_str(&format!(
                                r#"<path d="{}" fill="none" stroke="{}" stroke-width="1" opacity="{}" clip-path="url(#{})"/>
"#,
                                hachure_d, el.stroke_color,
                                (el.opacity as f64 / 100.0) * 0.5, clip_id
                            ));
                        }
                    } else if el.background_color != "transparent" {
                        svg.push_str(&format!(
                            r#"<polygon points="{:.0},{:.0} {:.0},{:.0} {:.0},{:.0} {:.0},{:.0}" fill="{}" stroke="none" opacity="{}"/>
"#,
                            corners[0].0, corners[0].1,
                            corners[1].0, corners[1].1,
                            corners[2].0, corners[2].1,
                            corners[3].0, corners[3].1,
                            el.background_color, el.opacity as f64 / 100.0
                        ));
                    }

                    // Rough outline edges
                    let mut outline_d = String::new();
                    for i in 0..4 {
                        let (x1, y1) = corners[i];
                        let (x2, y2) = corners[(i + 1) % 4];
                        let edge_seed = el_seed.wrapping_add(i as u64 * 17);
                        let pts = rough::rough_line(x1, y1, x2, y2, vs.roughness, edge_seed);
                        for (j, p) in pts.iter().enumerate() {
                            if i == 0 && j == 0 {
                                outline_d.push_str(&format!("M{:.2},{:.2}", p[0], p[1]));
                            } else {
                                outline_d.push_str(&format!(" L{:.2},{:.2}", p[0], p[1]));
                            }
                        }
                    }
                    outline_d.push_str(" Z");
                    svg.push_str(&format!(
                        r#"<path d="{}" fill="none" stroke="{}" stroke-width="{}" opacity="{}"/>
"#,
                        outline_d, el.stroke_color, el.stroke_width,
                        el.opacity as f64 / 100.0
                    ));
                } else {
                    svg.push_str(&format!(
                        r#"<polygon points="{:.0},{:.0} {:.0},{:.0} {:.0},{:.0} {:.0},{:.0}" fill="{}" stroke="{}" stroke-width="{}" opacity="{}"/>
"#,
                        cx, el.y,
                        el.x + el.width, cy,
                        cx, el.y + el.height,
                        el.x, cy,
                        el.background_color, el.stroke_color, el.stroke_width,
                        el.opacity as f64 / 100.0
                    ));
                }
            }

            "text" => {
                // Check if this text should follow a branch path
                let on_branch = el.custom_data.as_ref()
                    .and_then(|cd| cd.get("onBranch"))
                    .and_then(|v| v.as_str());

                if let (Some(branch_id), Some(text)) = (on_branch, &el.text) {
                    // Text on path — use SVG textPath along the branch center-line
                    // Shift text above the branch surface with dy
                    let href = format!("#cl-{}", branch_id);

                    // Simple fixed offset — same distance from junction on all sides.
                    // 65px for L2, 45px for L1.
                    let offset = if el.font_size < 16.0 { 65.0 } else { 45.0 };
                    // Read branch size from the arrow's customData to compute vertical offset
                    let branch_half = el.custom_data.as_ref()
                        .and_then(|cd| cd.get("onBranch"))
                        .and_then(|_| {
                            // Find the arrow element to get its stroke size
                            scene.elements.iter()
                                .find(|a| a.id == branch_id)
                                .and_then(|a| a.custom_data.as_ref())
                                .and_then(|cd| cd.get("strokeOptions"))
                                .and_then(|so| so.get("startSize"))
                                .and_then(|v| v.as_f64())
                        })
                        .unwrap_or(8.0) / 2.0;
                    let dy = -(branch_half + 3.0); // sit just above the branch edge

                    svg.push_str(&format!(
                        "<text font-size=\"{}\" fill=\"{}\" font-family=\"'Nunito', sans-serif\" font-weight=\"600\" dy=\"{:.1}\"><textPath href=\"{}\" startOffset=\"{:.0}\">{}</textPath></text>\n",
                        el.font_size, el.stroke_color,
                        dy, href, offset,
                        xml_escape(text)
                    ));
                    continue;
                }

                // Regular text rendering
                let anchor = el.text_align.as_deref().unwrap_or("center");
                let svg_anchor = match anchor {
                    "center" => "middle",
                    "right" => "end",
                    _ => "start",
                };

                if let Some(ref text) = el.text {
                    let lines: Vec<&str> = text.split('\n').collect();
                    let line_height = el.font_size * 1.3;

                    let tx = if svg_anchor == "middle" {
                        el.x + el.width / 2.0
                    } else {
                        el.x
                    };

                    // Rotation transform for text on branches
                    let rot_attr = if let Some(angle) = el.angle {
                        if angle.abs() > 0.01 {
                            let deg = angle.to_degrees();
                            let cx = el.x + el.width / 2.0;
                            let cy = el.y + el.height / 2.0;
                            format!(r#" transform="rotate({:.1},{:.0},{:.0})""#, deg, cx, cy)
                        } else {
                            String::new()
                        }
                    } else {
                        String::new()
                    };

                    if lines.len() == 1 {
                        // Position text ABOVE the branch (offset up by font size)
                        let ty = el.y + el.height / 2.0 - if el.angle.is_some() { el.font_size * 0.3 } else { 0.0 };
                        svg.push_str(&format!(
                            r#"<text x="{:.0}" y="{:.0}" text-anchor="{}" dominant-baseline="central" font-size="{}" fill="{}"{} opacity="{}">{}</text>
"#,
                            tx, ty, svg_anchor, el.font_size, el.stroke_color,
                            rot_attr,
                            el.opacity as f64 / 100.0,
                            xml_escape(lines[0])
                        ));
                    } else {
                        let start_y = el.y + el.height / 2.0 - (lines.len() as f64 - 1.0) * line_height / 2.0;
                        svg.push_str(&format!(
                            r#"<text font-size="{}" fill="{}" text-anchor="{}" dominant-baseline="central" opacity="{}">"#,
                            el.font_size, el.stroke_color, svg_anchor,
                            el.opacity as f64 / 100.0
                        ));
                        for (i, line) in lines.iter().enumerate() {
                            let ly = start_y + i as f64 * line_height;
                            svg.push_str(&format!(
                                r#"<tspan x="{:.0}" y="{:.0}">{}</tspan>"#,
                                tx, ly, xml_escape(line)
                            ));
                        }
                        svg.push_str("</text>\n");
                    }
                }
            }

            "line" => {
                if let Some(ref points) = el.points {
                    if points.len() >= 2 {
                        if is_rough {
                            let vs = style.unwrap();
                            // Render line segments with rough perturbation
                            let mut path_d = String::new();
                            for i in 0..points.len() - 1 {
                                let seg_seed = el_seed.wrapping_add(i as u64 * 23);
                                let pts = rough::rough_line(
                                    el.x + points[i][0], el.y + points[i][1],
                                    el.x + points[i + 1][0], el.y + points[i + 1][1],
                                    vs.roughness, seg_seed,
                                );
                                for (j, p) in pts.iter().enumerate() {
                                    if i == 0 && j == 0 {
                                        path_d.push_str(&format!("M{:.2},{:.2}", p[0], p[1]));
                                    } else {
                                        path_d.push_str(&format!(" L{:.2},{:.2}", p[0], p[1]));
                                    }
                                }
                            }
                            svg.push_str(&format!(
                                r#"<path d="{}" fill="none" stroke="{}" stroke-width="{}" opacity="{}" stroke-linecap="round" stroke-linejoin="round"/>
"#,
                                path_d, el.stroke_color, el.stroke_width,
                                el.opacity as f64 / 100.0
                            ));
                        } else {
                            let pts: Vec<String> = points.iter()
                                .map(|p| format!("{:.0},{:.0}", el.x + p[0], el.y + p[1]))
                                .collect();
                            svg.push_str(&format!(
                                r#"<polyline points="{}" fill="none" stroke="{}" stroke-width="{}" opacity="{}" stroke-linecap="round" stroke-linejoin="round"/>
"#,
                                pts.join(" "),
                                el.stroke_color, el.stroke_width,
                                el.opacity as f64 / 100.0
                            ));
                        }
                    }
                }
            }

            "arrow" => {
                if let Some(ref points) = el.points {
                    // Check for organic stroke rendering via customData
                    let is_organic = el.custom_data.as_ref()
                        .and_then(|cd| cd.get("strokeOptions"))
                        .is_some();

                    if is_organic {
                        // Read depth-based sizes from customData
                        let cd = el.custom_data.as_ref().and_then(|c| c.get("strokeOptions"));
                        let start_size = cd.and_then(|s| s.get("startSize")).and_then(|v| v.as_f64()).unwrap_or(10.0);
                        let end_size = cd.and_then(|s| s.get("endSize")).and_then(|v| v.as_f64()).unwrap_or(4.0);

                        // Sample cubic Bezier from the 4 arrow control points
                        let abs_pts: Vec<[f64; 2]> = if points.len() == 4 {
                            // Cubic Bezier: sample into dense points for freehand
                            let p0 = [el.x + points[0][0], el.y + points[0][1]];
                            let p1 = [el.x + points[1][0], el.y + points[1][1]];
                            let p2 = [el.x + points[2][0], el.y + points[2][1]];
                            let p3 = [el.x + points[3][0], el.y + points[3][1]];
                            (0..=48).map(|i| {
                                let t = i as f64 / 48.0;
                                let u = 1.0 - t;
                                let u2 = u * u;
                                let t2 = t * t;
                                [
                                    u2 * u * p0[0] + 3.0 * u2 * t * p1[0] + 3.0 * u * t2 * p2[0] + t2 * t * p3[0],
                                    u2 * u * p0[1] + 3.0 * u2 * t * p1[1] + 3.0 * u * t2 * p2[1] + t2 * t * p3[1],
                                ]
                            }).collect()
                        } else {
                            points.iter().map(|p| [el.x + p[0], el.y + p[1]]).collect()
                        };

                        // For organic connectors with rough style: gently wander the center path
                        let final_pts = if is_rough {
                            let vs = style.unwrap();
                            rough::wander_path(&abs_pts, vs.stroke_jitter * 2.0, el_seed.wrapping_add(11))
                        } else {
                            abs_pts
                        };

                        // Generate pressure array: taper from start_size to end_size
                        let n = final_pts.len();
                        let pressures: Vec<f64> = (0..n).map(|i| {
                            let t = i as f64 / (n - 1).max(1) as f64;
                            let target_radius = start_size * (1.0 - t) + end_size * t;
                            (target_radius / start_size).clamp(0.05, 1.0)
                        }).collect();

                        let opts = freehand::StrokeOptions {
                            size: start_size,
                            thinning: 0.6,
                            smoothing: 0.7,
                            streamline: 0.5,
                            simulate_pressure: false,
                            start_taper: 0.0,
                            start_cap: false,
                            end_taper: 0.0,
                            end_cap: false,
                            last: true,
                            ..Default::default()
                        };
                        let outline = freehand::get_stroke(&final_pts, Some(&pressures), &opts);
                        if !outline.is_empty() {
                            let path_d = freehand::outline_to_svg_path(&outline);
                            svg.push_str(&format!(
                                r#"<path d="{}" fill="{}" stroke="none" opacity="{}"/>
"#,
                                path_d, el.stroke_color,
                                el.opacity as f64 / 100.0
                            ));
                        }
                        // Emit hidden center-line path for textPath references.
                        // Reverse path for left-going branches so text reads left-to-right.
                        let goes_left = final_pts.last().map(|p| p[0] < final_pts[0][0]).unwrap_or(false);
                        let cl_pts: Vec<&[f64; 2]> = if goes_left {
                            final_pts.iter().rev().collect()
                        } else {
                            final_pts.iter().collect()
                        };
                        let mut cl_d = format!("M{:.1},{:.1}", cl_pts[0][0], cl_pts[0][1]);
                        for p in &cl_pts[1..] {
                            cl_d.push_str(&format!(" L{:.1},{:.1}", p[0], p[1]));
                        }
                        svg.push_str(&format!(
                            r#"<path id="cl-{}" d="{}" fill="none" stroke="none"/>
"#,
                            el.id, cl_d
                        ));
                    } else if is_rough && style.map_or(false, |s| s.connector_rough) {
                        // Non-organic arrow with rough style: use rough lines
                        let vs = style.unwrap();
                        if points.len() == 2 {
                            let pts = rough::rough_line(
                                el.x + points[0][0], el.y + points[0][1],
                                el.x + points[1][0], el.y + points[1][1],
                                vs.roughness * 0.7, el_seed,
                            );
                            let mut path_d = String::new();
                            for (j, p) in pts.iter().enumerate() {
                                if j == 0 {
                                    path_d.push_str(&format!("M{:.2},{:.2}", p[0], p[1]));
                                } else {
                                    path_d.push_str(&format!(" L{:.2},{:.2}", p[0], p[1]));
                                }
                            }
                            svg.push_str(&format!(
                                r#"<path d="{}" fill="none" stroke="{}" stroke-width="{}" opacity="{}" marker-end="url(#ah)"/>
"#,
                                path_d, el.stroke_color, el.stroke_width,
                                el.opacity as f64 / 100.0
                            ));
                        } else {
                            // Multi-point: rough each segment
                            let mut path_d = String::new();
                            for i in 0..points.len() - 1 {
                                let seg_seed = el_seed.wrapping_add(i as u64 * 29);
                                let pts = rough::rough_line(
                                    el.x + points[i][0], el.y + points[i][1],
                                    el.x + points[i + 1][0], el.y + points[i + 1][1],
                                    vs.roughness * 0.7, seg_seed,
                                );
                                for (j, p) in pts.iter().enumerate() {
                                    if i == 0 && j == 0 {
                                        path_d.push_str(&format!("M{:.2},{:.2}", p[0], p[1]));
                                    } else {
                                        path_d.push_str(&format!(" L{:.2},{:.2}", p[0], p[1]));
                                    }
                                }
                            }
                            svg.push_str(&format!(
                                r#"<path d="{}" fill="none" stroke="{}" stroke-width="{}" opacity="{}" marker-end="url(#ah)"/>
"#,
                                path_d, el.stroke_color, el.stroke_width,
                                el.opacity as f64 / 100.0
                            ));
                        }
                    } else if points.len() == 2 {
                        // Simple line (clean)
                        svg.push_str(&format!(
                            r#"<line x1="{:.0}" y1="{:.0}" x2="{:.0}" y2="{:.0}" stroke="{}" stroke-width="{}" opacity="{}" marker-end="url(#ah)"/>
"#,
                            el.x + points[0][0], el.y + points[0][1],
                            el.x + points[1][0], el.y + points[1][1],
                            el.stroke_color, el.stroke_width,
                            el.opacity as f64 / 100.0
                        ));
                    } else {
                        // Multi-point polyline (clean)
                        let pts: Vec<String> = points.iter()
                            .map(|p| format!("{:.0},{:.0}", el.x + p[0], el.y + p[1]))
                            .collect();
                        svg.push_str(&format!(
                            r#"<polyline points="{}" fill="none" stroke="{}" stroke-width="{}" opacity="{}" marker-end="url(#ah)"/>
"#,
                            pts.join(" "),
                            el.stroke_color, el.stroke_width,
                            el.opacity as f64 / 100.0
                        ));
                    }

                    // Render arrow label at midpoint (always clean text)
                    if let Some(ref label) = el.label {
                        let mid_idx = points.len() / 2;
                        let (mx, my) = if points.len() % 2 == 0 {
                            let p1 = &points[mid_idx - 1];
                            let p2 = &points[mid_idx];
                            ((el.x + p1[0] + el.x + p2[0]) / 2.0,
                             (el.y + p1[1] + el.y + p2[1]) / 2.0)
                        } else {
                            (el.x + points[mid_idx][0], el.y + points[mid_idx][1])
                        };
                        let label_color = "#666666";
                        svg.push_str(&format!(
                            "<text x=\"{:.0}\" y=\"{:.0}\" text-anchor=\"middle\" dominant-baseline=\"central\" font-size=\"{}\" fill=\"{}\" opacity=\"0.8\"><tspan dx=\"12\" dy=\"-8\">{}</tspan></text>\n",
                            mx, my, label.font_size, label_color, xml_escape(&label.text)
                        ));
                    }
                }
            }

            _ => {} // skip unknown types
        }
    }

    svg.push_str("</svg>\n");
    svg
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
