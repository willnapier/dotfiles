/// A named style preset.
#[derive(Debug, Clone)]
pub struct Style {
    pub fill: String,
    pub stroke: String,
    pub text_color: String,
    pub opacity: i32,
    pub font_size: f64,
}

impl Style {
    /// Clinical palette: William (blue)
    pub fn william() -> Self {
        Style {
            fill: "#aed6f1".into(),
            stroke: "#2471a3".into(),
            text_color: "#1a5276".into(),
            opacity: 75,
            font_size: 14.0,
        }
    }

    /// Clinical palette: Leigh (purple)
    pub fn leigh() -> Self {
        Style {
            fill: "#d2b4de".into(),
            stroke: "#7d3c98".into(),
            text_color: "#4a235a".into(),
            opacity: 75,
            font_size: 14.0,
        }
    }

    /// Clinical palette: AI (green)
    pub fn ai() -> Self {
        Style {
            fill: "#a9dfbf".into(),
            stroke: "#1e8449".into(),
            text_color: "#1b5e20".into(),
            opacity: 75,
            font_size: 14.0,
        }
    }

    /// Clinical palette: Automated (gold)
    pub fn automated() -> Self {
        Style {
            fill: "#f9e79f".into(),
            stroke: "#d4ac0d".into(),
            text_color: "#7d6608".into(),
            opacity: 75,
            font_size: 14.0,
        }
    }

    /// Clinical palette: Urgent (red)
    pub fn urgent() -> Self {
        Style {
            fill: "#f5b7b1".into(),
            stroke: "#c0392b".into(),
            text_color: "#922b21".into(),
            opacity: 75,
            font_size: 14.0,
        }
    }

    /// Default style
    pub fn default() -> Self {
        Style {
            fill: "transparent".into(),
            stroke: "#1e1e1e".into(),
            text_color: "#1e1e1e".into(),
            opacity: 100,
            font_size: 14.0,
        }
    }

    /// Arrow style (neutral)
    pub fn arrow() -> Self {
        Style {
            fill: "transparent".into(),
            stroke: "#999999".into(),
            text_color: "#666666".into(),
            opacity: 60,
            font_size: 10.0,
        }
    }

    /// Look up a style by name.
    pub fn by_name(name: &str) -> Self {
        match name {
            "william" | "blue" => Self::william(),
            "leigh" | "purple" => Self::leigh(),
            "ai" | "green" => Self::ai(),
            "automated" | "gold" | "amber" => Self::automated(),
            "urgent" | "red" => Self::urgent(),
            "arrow" => Self::arrow(),
            _ => Self::default(),
        }
    }
}
