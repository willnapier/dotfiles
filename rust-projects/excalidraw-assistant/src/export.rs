use crate::scene::Scene;

/// Export a scene to SVG with Nunito font and consistent arrowhead markers.
pub fn to_svg(scene: &Scene) -> String {
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

    // Render elements in order (z-order = array order)
    for el in &scene.elements {
        if el.is_deleted {
            continue;
        }

        match el.element_type.as_str() {
            "rectangle" => {
                let rx = if el.roundness.is_some() { 8 } else { 0 };
                svg.push_str(&format!(
                    r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{}" stroke="{}" stroke-width="{}" rx="{}" opacity="{}"/>
"#,
                    el.x, el.y, el.width, el.height,
                    el.background_color, el.stroke_color, el.stroke_width,
                    rx, el.opacity as f64 / 100.0
                ));
            }

            "diamond" => {
                let cx = el.x + el.width / 2.0;
                let cy = el.y + el.height / 2.0;
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

            "text" => {
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

                    if lines.len() == 1 {
                        let ty = el.y + el.height / 2.0;
                        svg.push_str(&format!(
                            r#"<text x="{:.0}" y="{:.0}" text-anchor="{}" dominant-baseline="central" font-size="{}" fill="{}" opacity="{}">{}</text>
"#,
                            tx, ty, svg_anchor, el.font_size, el.stroke_color,
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

            "arrow" => {
                if let Some(ref points) = el.points {
                    if points.len() == 2 {
                        // Simple line
                        svg.push_str(&format!(
                            r#"<line x1="{:.0}" y1="{:.0}" x2="{:.0}" y2="{:.0}" stroke="{}" stroke-width="{}" opacity="{}" marker-end="url(#ah)"/>
"#,
                            el.x + points[0][0], el.y + points[0][1],
                            el.x + points[1][0], el.y + points[1][1],
                            el.stroke_color, el.stroke_width,
                            el.opacity as f64 / 100.0
                        ));
                    } else {
                        // Multi-point polyline
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
