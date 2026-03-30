//! Visual style presets for SVG export.
//! Controls how shapes and connectors look: clean (geometric), subtle (slight wobble),
//! or sketchy (hand-drawn with optional hachure fills).

/// Visual style configuration for SVG rendering.
#[derive(Debug, Clone)]
pub struct VisualStyle {
    /// Overall roughness: 0.0 = none, 1.0 = subtle, 3.0 = very sketchy.
    pub roughness: f64,
    /// Whether to use hachure (cross-hatch) fills instead of solid fills.
    pub hachure: bool,
    /// Angle of hachure strokes in degrees.
    pub hachure_angle: f64,
    /// Gap between hachure strokes in pixels.
    pub hachure_gap: f64,
    /// Extra jitter applied to connector/arrow strokes.
    pub stroke_jitter: f64,
    /// Whether connectors get rough treatment.
    pub connector_rough: bool,
    /// Seed for deterministic randomness.
    pub seed: u64,
}

impl VisualStyle {
    /// Clean geometric output (current default behaviour, no perturbation).
    pub fn clean() -> Self {
        VisualStyle {
            roughness: 0.0,
            hachure: false,
            hachure_angle: -41.0,
            hachure_gap: 8.0,
            stroke_jitter: 0.0,
            connector_rough: false,
            seed: 42,
        }
    }

    /// Subtle hand-drawn look: slight wobble on outlines and connectors,
    /// solid fills with rough edges.
    pub fn subtle() -> Self {
        VisualStyle {
            roughness: 1.0,
            hachure: false,
            hachure_angle: -41.0,
            hachure_gap: 8.0,
            stroke_jitter: 2.0,
            connector_rough: true,
            seed: 42,
        }
    }

    /// Sketchy hand-drawn look: more jitter, optionally hachure fills.
    pub fn sketchy() -> Self {
        VisualStyle {
            roughness: 0.5,
            hachure: true,
            hachure_angle: -41.0,
            hachure_gap: 8.0,
            stroke_jitter: 0.6,
            connector_rough: true,
            seed: 42,
        }
    }

    /// Look up a preset by name. Returns None for unknown names.
    pub fn by_name(name: &str) -> Option<Self> {
        match name {
            "clean" => Some(Self::clean()),
            "subtle" => Some(Self::subtle()),
            "sketchy" => Some(Self::sketchy()),
            _ => None,
        }
    }

    /// Returns true if this is the clean style (no perturbation needed).
    pub fn is_clean(&self) -> bool {
        self.roughness <= 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_names() {
        assert!(VisualStyle::by_name("clean").unwrap().is_clean());
        assert!(!VisualStyle::by_name("subtle").unwrap().is_clean());
        assert!(!VisualStyle::by_name("sketchy").unwrap().is_clean());
        assert!(VisualStyle::by_name("unknown").is_none());
    }

    #[test]
    fn sketchy_has_hachure() {
        let s = VisualStyle::sketchy();
        assert!(s.hachure);
        assert!(s.roughness > 0.3);
    }

    #[test]
    fn subtle_no_hachure() {
        let s = VisualStyle::subtle();
        assert!(!s.hachure);
        assert!(s.roughness > 0.0);
    }
}
