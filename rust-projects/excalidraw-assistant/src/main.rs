mod elements;
mod scene;
mod builder;
mod style;
mod lint;
mod layout;
mod convert;
mod export;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use scene::Scene;
use style::Style;

#[derive(Parser)]
#[command(name = "ea", about = "Excalidraw Assistant — programmatic diagram creation")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new empty .excalidraw file
    New {
        path: PathBuf,
    },

    /// Add a rectangle with a label
    AddRect {
        file: PathBuf,
        #[arg(long)]
        label: String,
        #[arg(long, default_value = "default")]
        style: String,
        #[arg(long, default_value = "100")]
        x: f64,
        #[arg(long, default_value = "100")]
        y: f64,
        #[arg(long)]
        below: bool,
        #[arg(long, default_value = "24")]
        gap: f64,
    },

    /// Add a diamond with a label
    AddDiamond {
        file: PathBuf,
        #[arg(long)]
        label: String,
        #[arg(long, default_value = "default")]
        style: String,
        #[arg(long, default_value = "100")]
        x: f64,
        #[arg(long, default_value = "100")]
        y: f64,
        #[arg(long)]
        below: bool,
        #[arg(long, default_value = "24")]
        gap: f64,
    },

    /// Add standalone text (title, annotation)
    AddText {
        file: PathBuf,
        #[arg(long)]
        label: String,
        #[arg(long, default_value = "20")]
        size: f64,
        #[arg(long, default_value = "#1e1e1e")]
        color: String,
        #[arg(long, default_value = "100")]
        x: f64,
        #[arg(long, default_value = "50")]
        y: f64,
    },

    /// Connect two elements with an arrow
    Connect {
        file: PathBuf,
        from: String,
        to: String,
        #[arg(long, default_value = "down")]
        arrow: String,
        #[arg(long)]
        label: Option<String>,
    },

    /// Show diagram info
    Info {
        file: PathBuf,
    },

    /// Auto-layout elements in a flow
    Layout {
        file: PathBuf,
        /// Direction: down, right
        #[arg(long, default_value = "down")]
        direction: String,
        /// Gap between elements (pixels)
        #[arg(long, default_value = "24")]
        gap: f64,
    },

    /// Lint the diagram for quality issues
    Lint {
        file: PathBuf,
        /// Auto-fix violations
        #[arg(long, default_value_t = false, num_args = 0)]
        fix: bool,
    },

    /// Convert a D2 file to .excalidraw
    Convert {
        /// Input D2 file
        input: PathBuf,
        /// Output .excalidraw file
        #[arg(long)]
        output: Option<PathBuf>,
        /// Style preset (clinical, default)
        #[arg(long, default_value = "clinical")]
        style: String,
    },

    /// Export as SVG
    ExportSvg {
        file: PathBuf,
        /// Output file (defaults to stdout)
        #[arg(long)]
        output: Option<PathBuf>,
    },

    /// Export as JSON (pretty-printed)
    Export {
        file: PathBuf,
    },

    /// Convert D2 → lint+fix → export SVG (full pipeline)
    Render {
        /// Input D2 file
        input: PathBuf,
        /// Output SVG file (defaults to input with .svg extension)
        #[arg(long)]
        output: Option<PathBuf>,
        /// Style preset (clinical, default)
        #[arg(long, default_value = "clinical")]
        style: String,
        /// Open the SVG in the default browser
        #[arg(long, default_value_t = false, num_args = 0)]
        open: bool,
    },
}

/// Resolve element references. Filters to shapes only (skip text elements).
fn resolve_ref(scene: &Scene, reference: &str) -> String {
    let shapes: Vec<&elements::Element> = scene.elements.iter()
        .filter(|e| e.element_type != "text")
        .collect();

    if reference == "last" {
        shapes.last().map(|e| e.id.clone()).unwrap_or_default()
    } else if let Some(n) = reference.strip_prefix("last-") {
        let offset: usize = n.parse().unwrap_or(0);
        if offset < shapes.len() {
            shapes[shapes.len() - 1 - offset].id.clone()
        } else {
            String::new()
        }
    } else if let Ok(idx) = reference.parse::<usize>() {
        if idx < shapes.len() {
            return shapes[idx].id.clone();
        }
        reference.to_string()
    } else {
        reference.to_string()
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::New { path } => {
            let scene = Scene::new();
            scene.save(&path)?;
            println!("Created: {}", path.display());
        }

        Command::AddRect { file, label, style, x, y, below, gap } => {
            let mut scene = Scene::load(&file)?;
            let s = Style::by_name(&style);
            let (ax, ay) = if below {
                if let Some(last) = scene.elements.iter().rev().find(|e| e.element_type != "text") {
                    // Pass centre x; builder will adjust x based on actual width
                    (last.center_x(), last.bottom() + gap)
                } else { (x, y) }
            } else { (x, y) };
            let id = builder::add_rect(&mut scene, ax, ay, &label, &s, below);
            scene.save(&file)?;
            println!("{}", id);
        }

        Command::AddDiamond { file, label, style, x, y, below, gap } => {
            let mut scene = Scene::load(&file)?;
            let s = Style::by_name(&style);
            let (ax, ay) = if below {
                if let Some(last) = scene.elements.iter().rev().find(|e| e.element_type != "text") {
                    // Pass centre x; builder will adjust x based on actual width
                    (last.center_x(), last.bottom() + gap)
                } else { (x, y) }
            } else { (x, y) };
            let id = builder::add_diamond(&mut scene, ax, ay, &label, &s, below);
            scene.save(&file)?;
            println!("{}", id);
        }

        Command::AddText { file, label, size, color, x, y } => {
            let mut scene = Scene::load(&file)?;
            let id = builder::add_text(&mut scene, x, y, &label, size, &color);
            scene.save(&file)?;
            println!("{}", id);
        }

        Command::Connect { file, from, to, arrow, label } => {
            let mut scene = Scene::load(&file)?;
            let from_id = resolve_ref(&scene, &from);
            let to_id = resolve_ref(&scene, &to);
            let s = Style::arrow();
            let (from_pt, to_pt) = match arrow.as_str() {
                "down" => ([0.5, 1.0], [0.5, 0.0]),
                "up" => ([0.5, 0.0], [0.5, 1.0]),
                "right" => ([1.0, 0.5], [0.0, 0.5]),
                "left" => ([0.0, 0.5], [1.0, 0.5]),
                _ => ([0.5, 1.0], [0.5, 0.0]),
            };
            let (id, note) = builder::smart_connect(&mut scene, &from_id, &to_id, from_pt, to_pt, &s, label.as_deref());
            scene.save(&file)?;
            if let Some(n) = note {
                eprintln!("{}", n);
            }
            println!("{}", id);
        }

        Command::Layout { file, direction, gap } => {
            let mut scene = Scene::load(&file)?;
            layout::flow(&mut scene, &direction, gap);
            layout::recalculate_arrows(&mut scene);
            scene.save(&file)?;
            println!("Layout applied: {} direction, {:.0}px gap", direction, gap);
        }

        Command::Lint { file, fix } => {
            let mut scene = Scene::load(&file)?;

            if fix {
                let fixed = lint::fix(&mut scene);
                if !fixed.is_empty() {
                    scene.save(&file)?;
                    for f in &fixed {
                        println!("  Fixed: {}", f);
                    }
                }
            }

            let failures = lint::check(&scene);
            if failures.is_empty() {
                println!("✓ All checks passed ({} elements)", scene.elements.len());
            } else {
                for f in &failures {
                    println!("✗ {}", f);
                }
                std::process::exit(1);
            }
        }

        Command::Info { file } => {
            let scene = Scene::load(&file)?;
            let rects = scene.elements.iter().filter(|e| e.element_type == "rectangle").count();
            let diamonds = scene.elements.iter().filter(|e| e.element_type == "diamond").count();
            let arrows = scene.elements.iter().filter(|e| e.element_type == "arrow").count();
            println!("{}: {} elements ({} rects, {} diamonds, {} arrows)",
                file.display(), scene.elements.len(), rects, diamonds, arrows);
        }

        Command::Convert { input, output, style } => {
            let d2 = std::fs::read_to_string(&input)?;
            let scene = convert::from_d2(&d2, &style)?;
            let out_path = output.unwrap_or_else(|| input.with_extension("excalidraw"));
            scene.save(&out_path)?;
            let shapes = scene.elements.iter().filter(|e| e.element_type != "text" && e.element_type != "arrow").count();
            let arrows = scene.elements.iter().filter(|e| e.element_type == "arrow").count();
            println!("Converted: {} → {} ({} shapes, {} arrows)",
                input.display(), out_path.display(), shapes, arrows);
        }

        Command::ExportSvg { file, output } => {
            let scene = Scene::load(&file)?;
            let svg = export::to_svg(&scene);
            if let Some(out) = output {
                std::fs::write(&out, &svg)?;
                println!("Exported: {}", out.display());
            } else {
                print!("{}", svg);
            }
        }

        Command::Export { file } => {
            let scene = Scene::load(&file)?;
            println!("{}", serde_json::to_string_pretty(&scene)?);
        }

        Command::Render { input, output, style, open } => {
            // Step 1: Convert D2 → .excalidraw
            let d2 = std::fs::read_to_string(&input)?;
            let mut scene = convert::from_d2(&d2, &style)?;
            let excalidraw_path = input.with_extension("excalidraw");

            // Step 2: Lint + fix
            let fixed = lint::fix(&mut scene);
            for f in &fixed {
                println!("  Fixed: {}", f);
            }
            let failures = lint::check(&scene);
            if !failures.is_empty() {
                for f in &failures {
                    println!("✗ {}", f);
                }
            }

            scene.save(&excalidraw_path)?;
            let shapes = scene.elements.iter().filter(|e| e.element_type != "text" && e.element_type != "arrow").count();
            let arrows = scene.elements.iter().filter(|e| e.element_type == "arrow").count();

            // Step 3: Export SVG
            let svg = export::to_svg(&scene);
            let svg_path = output.unwrap_or_else(|| input.with_extension("svg"));
            std::fs::write(&svg_path, &svg)?;

            println!("Rendered: {} → {} ({} shapes, {} arrows)",
                input.display(), svg_path.display(), shapes, arrows);

            // Step 4: Optionally open
            if open {
                #[cfg(target_os = "macos")]
                std::process::Command::new("open").arg(&svg_path).spawn()?;
                #[cfg(target_os = "linux")]
                std::process::Command::new("xdg-open").arg(&svg_path).spawn()?;
            }
        }
    }

    Ok(())
}
