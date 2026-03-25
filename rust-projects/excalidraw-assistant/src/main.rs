mod elements;
mod scene;
mod builder;
mod style;
mod lint;

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

    /// Lint the diagram for quality issues
    Lint {
        file: PathBuf,
    },

    /// Export as JSON (pretty-printed)
    Export {
        file: PathBuf,
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
            let id = builder::add_arrow(&mut scene, &from_id, &to_id, from_pt, to_pt, &s, label.as_deref());
            scene.save(&file)?;
            println!("{}", id);
        }

        Command::Lint { file } => {
            let scene = Scene::load(&file)?;
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

        Command::Export { file } => {
            let scene = Scene::load(&file)?;
            println!("{}", serde_json::to_string_pretty(&scene)?);
        }
    }

    Ok(())
}
