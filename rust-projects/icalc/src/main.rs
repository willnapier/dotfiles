mod book;
mod gen;
mod house;
mod live;

use anyhow::{bail, Result};
use book::{csv_line, markdown_table, parse_target, Book};
use clap::{Parser, Subcommand, ValueEnum};

/// IronCalc-backed spreadsheet CLI for people and assistants.
///
/// Targets: `Sheet!B31` (cell), `Sheet@label` (row whose column-A text contains
/// label, value in column B; `Sheet@label:D` for column D), `@label` (any sheet),
/// or a defined name. Overrides are `target=value` and live only in memory unless
/// `set --out` writes a new file. The source workbook is never modified.
#[derive(Parser)]
#[command(name = "icalc", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Md,
    Csv,
    Json,
}

#[derive(Subcommand)]
enum Cmd {
    /// List sheets with their used ranges
    Sheets { file: String },
    /// List defined names
    Names { file: String },
    /// Read cells (after recalculation)
    Get {
        file: String,
        /// One or more targets
        #[arg(required = true)]
        targets: Vec<String>,
        /// What-if overrides applied before reading (repeatable)
        #[arg(long = "set", value_name = "TARGET=VALUE")]
        sets: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Find rows whose column-A label contains TEXT, with their column-B value
    Find {
        file: String,
        text: String,
        #[arg(long)]
        sheet: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Dump a sheet as a table (after recalculation)
    Dump {
        file: String,
        #[arg(long)]
        sheet: String,
        #[arg(long = "set", value_name = "TARGET=VALUE")]
        sets: Vec<String>,
        #[arg(long, value_enum, default_value = "md")]
        format: Format,
        /// Show formulas instead of values
        #[arg(long)]
        formulas: bool,
        /// With --format json: numbers as JSON numbers instead of formatted strings
        #[arg(long)]
        raw: bool,
    },
    /// Recalculate every formula and compare with the values cached in the file (exit 1 on any difference or error)
    Check {
        file: String,
        #[arg(long)]
        json: bool,
    },
    /// Apply overrides and write a NEW workbook (the input is never modified)
    Set {
        file: String,
        #[arg(required = true, value_name = "TARGET=VALUE")]
        sets: Vec<String>,
        #[arg(long)]
        out: String,
    },
    /// The House Model (newest `~/Forge/Scenario - The House Model*.xlsx`): floor, ceiling, leeway, roof, chimney
    House {
        /// Workbook to read instead of the newest in ~/Forge
        #[arg(long)]
        file: Option<String>,
        /// What-if overrides: `fee=180`, `rate=5.5%`, `roof=30`, or any target=value (repeatable)
        #[arg(long = "set", value_name = "KEY=VALUE")]
        sets: Vec<String>,
        /// Compare the model's calibration with live Xero (practiceforge) and fd-budget figures
        #[arg(long)]
        live: bool,
        /// With --live: also re-run the house at the live fee and earning-week roof
        #[arg(long, requires = "live")]
        apply_live: bool,
        /// With --live: start of the Xero billing window (default: 105 days ago)
        #[arg(long)]
        live_from: Option<chrono::NaiveDate>,
        /// With --live: start of the fd-budget spend window (default: 365 days ago)
        #[arg(long)]
        spend_since: Option<chrono::NaiveDate>,
        /// Show a whole sheet instead of the house summary: lives, assumptions, rate, book, 2032
        #[arg(long)]
        sheet: Option<String>,
        /// List the alias keys usable in --set
        #[arg(long)]
        keys: bool,
        #[arg(long)]
        json: bool,
        /// Generate the workbook from the TOML spec instead of reading it (writes --out, default: the Forge file named from the spec date)
        #[arg(long)]
        gen: bool,
        /// With --gen: the TOML spec (default ~/Assistants/shared/house-model/house-model.toml)
        #[arg(long)]
        spec: Option<String>,
        /// With --gen: output .xlsx path
        #[arg(long)]
        out: Option<String>,
    },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("icalc: {e:#}");
        std::process::exit(1);
    }
}

fn print_dump(book: &Book, sheet: u32, format: Format, formulas: bool, raw: bool) -> Result<()> {
    let (header, rows) = book.dump(sheet, formulas)?;
    let (min_row, _, min_col, _) = book.dimension(sheet)?;
    let _ = min_row;
    match format {
        Format::Md => {
            let mut h = vec!["#".to_string()];
            h.extend(header);
            let r: Vec<Vec<String>> = rows.into_iter().map(|(n, mut c)| { c.insert(0, n.to_string()); c }).collect();
            print!("{}", markdown_table(&h, &r));
        }
        Format::Csv => {
            let mut h = vec!["row".to_string()];
            h.extend(header);
            println!("{}", csv_line(&h));
            for (n, mut c) in rows { c.insert(0, n.to_string()); println!("{}", csv_line(&c)); }
        }
        Format::Json => {
            let out: Vec<serde_json::Value> = rows.into_iter().map(|(n, c)| {
                let mut m = serde_json::Map::new();
                m.insert("row".into(), serde_json::json!(n));
                for (j, (h, v)) in header.iter().zip(c).enumerate() {
                    let val = if raw && !formulas {
                        match book.number(book::Pos { sheet, row: n, col: min_col + j as i32 }) { Some(x) => serde_json::json!(x), None => serde_json::Value::String(v) }
                    } else { serde_json::Value::String(v) };
                    m.insert(h.clone(), val);
                }
                serde_json::Value::Object(m)
            }).collect();
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
    }
    Ok(())
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Sheets { file } => {
            let b = Book::open(&file)?;
            for (i, name) in b.sheet_names().iter().enumerate() {
                let (r0, r1, c0, c1) = b.dimension(i as u32)?;
                println!("{i}\t{name}\t{}{r0}:{}{r1}", book::index_to_col(c0), book::index_to_col(c1));
            }
        }
        Cmd::Names { file } => {
            let b = Book::open(&file)?;
            let names = b.defined_names();
            if names.is_empty() { println!("(no defined names)"); }
            for (n, scope, f) in names {
                let s = scope.map(|i| b.sheet_names()[i as usize].clone()).unwrap_or_else(|| "workbook".into());
                println!("{n}\t{f}\t({s})");
            }
        }
        Cmd::Get { file, targets, sets, json } => {
            let mut b = Book::open(&file)?;
            b.apply_overrides(&sets)?;
            let mut reports = vec![];
            for t in &targets {
                let p = b.resolve(&parse_target(t)?)?;
                reports.push(b.report(p));
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&reports)?);
            } else {
                for r in reports {
                    let label = r.label.map(|l| format!("  [{l}]")).unwrap_or_default();
                    let raw = match &r.value { serde_json::Value::Number(n) => format!("  ({n})"), _ => String::new() };
                    let f = r.formula.map(|f| format!("  {f}")).unwrap_or_default();
                    println!("{}\t{}{}{}{}", r.r#ref, r.formatted, raw, label, f);
                }
            }
        }
        Cmd::Find { file, text, sheet, json } => {
            let mut b = Book::open(&file)?;
            b.evaluate();
            let only = match sheet { Some(s) => Some(b.sheet_index(&s)?), None => None };
            let hits = b.search_labels(&text, only)?;
            let reports: Vec<_> = hits.iter().map(|p| b.report(book::Pos { col: 2, ..*p })).collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&reports)?);
            } else if reports.is_empty() {
                bail!("no labels containing '{text}'");
            } else {
                for r in reports {
                    println!("{}\t{}\t{}", r.r#ref, r.label.unwrap_or_default(), r.formatted);
                }
            }
        }
        Cmd::Dump { file, sheet, sets, format, formulas, raw } => {
            let mut b = Book::open(&file)?;
            b.apply_overrides(&sets)?;
            let s = b.sheet_index(&sheet)?;
            print_dump(&b, s, format, formulas, raw)?;
        }
        Cmd::Check { file, json } => {
            let rep = Book::check(&file)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rep)?);
            } else {
                for m in &rep.mismatches {
                    println!("MISMATCH {}\tcached={:?}\trecalculated={:?}\t{}", m.r#ref, m.cached, m.recalculated, m.formula);
                }
                for e in &rep.errors {
                    println!("ERROR    {}\t{}", e.r#ref, e.formula);
                }
                println!("{}: {} formula cells, {} mismatches, {} errors — {}", rep.file, rep.formula_cells, rep.mismatches.len(), rep.errors.len(), if rep.ok { "OK" } else { "FAIL" });
            }
            if !rep.ok { std::process::exit(1); }
        }
        Cmd::Set { file, sets, out } => {
            if std::path::Path::new(&out) == std::path::Path::new(&file) {
                bail!("--out must differ from the input; icalc never modifies the source workbook");
            }
            let mut b = Book::open(&file)?;
            let log = b.apply_overrides(&sets)?;
            b.save_as(&out)?;
            for (r, label, before, after) in log {
                println!("{}{}: {} → {}", r, label.map(|l| format!(" ({l})")).unwrap_or_default(), before, after);
            }
            println!("wrote {out}");
        }
        Cmd::House { file, sets, live, apply_live, live_from, spend_since, sheet, keys, json, gen, spec, out } => {
            if keys {
                for (a, l) in house::ALIASES { println!("{a:18} Assumptions@{l}"); }
                return Ok(());
            }
            if gen {
                let spec = match spec { Some(s) => s, None => house::default_spec()?.to_string_lossy().to_string() };
                let out = match out { Some(o) => o, None => house::default_out(&spec)?.to_string_lossy().to_string() };
                let n = gen::generate(&spec, &out)?;
                println!("wrote {out} ({n} formula cells, no errors)");
                return Ok(());
            }
            let path = match file { Some(f) => f, None => house::default_file()?.to_string_lossy().to_string() };
            let mut b = Book::open(&path)?;
            let expanded: Vec<String> = sets.iter().map(|s| house::expand_alias_for(&b, s)).collect();
            let overrides = b.apply_overrides(&expanded)?;
            if let Some(s) = sheet {
                let name = match s.to_lowercase().as_str() { "rate" => "Rate sensitivity".to_string(), other => other.to_string() };
                let idx = b.sheet_index(&name)?;
                print_dump(&b, idx, if json { Format::Json } else { Format::Md }, false, false)?;
                return Ok(());
            }
            let h = house::read_house(&b, overrides)?;
            let d = if live { Some(house::drift(&b, live_from, spend_since)) } else { None };
            let applied = match (&d, apply_live) {
                (Some(d), true) => {
                    let lo = house::live_overrides(d)?;
                    let mut b2 = Book::open(&path)?;
                    let mut all = expanded.clone();
                    all.extend(lo.iter().map(|s| house::expand_alias_for(&b2, s)));
                    let ov = b2.apply_overrides(&all)?;
                    Some(house::read_house(&b2, ov)?)
                }
                _ => None,
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "house": h, "drift": d, "house_at_live": applied }))?);
            } else {
                print!("{}", house::render_house(&h));
                if let Some(d) = &d { print!("{}", house::render_drift(d)); }
                if let Some(a) = &applied {
                    println!("\nThe house at the live fee and earning-week roof:\n");
                    print!("{}", house::render_house(a));
                }
            }
            if let Some(d) = &d {
                if !d.failures.is_empty() { std::process::exit(2); }
            }
        }
    }
    Ok(())
}
