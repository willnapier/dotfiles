//! Thin wrapper over an IronCalc `Model`: sheet/ref resolution, label lookup,
//! what-if overrides, formatted reads, sheet dumps and the cached-vs-recalc check.

use anyhow::{anyhow, bail, Context, Result};
use ironcalc::base::cell::CellValue;
use ironcalc::base::types::CellType;
use ironcalc::base::Model;
use ironcalc::export::save_to_xlsx;
use ironcalc::import::load_from_xlsx;
use serde::Serialize;

const LOCALE: &str = "en";
const TZ: &str = "UTC";
const LANG: &str = "en";

/// A resolved cell position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    pub sheet: u32,
    pub row: i32,
    pub col: i32,
}

/// One requested target before resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `Sheet!B31`
    Cell { sheet: Option<String>, col: i32, row: i32 },
    /// `Sheet@label` or `Sheet@label:C` — the row whose column-A text contains
    /// `label` (case-insensitive, unique), value taken from column `col`.
    Label { sheet: Option<String>, label: String, col: i32 },
    /// A workbook defined name.
    Name(String),
}

pub fn col_to_index(s: &str) -> Option<i32> {
    if s.is_empty() || s.len() > 3 {
        return None;
    }
    let mut n = 0i32;
    for ch in s.chars() {
        let c = ch.to_ascii_uppercase();
        if !c.is_ascii_uppercase() {
            return None;
        }
        n = n * 26 + (c as i32 - 'A' as i32 + 1);
    }
    Some(n)
}

pub fn index_to_col(mut n: i32) -> String {
    let mut s = String::new();
    while n > 0 {
        let r = (n - 1) % 26;
        s.insert(0, (b'A' + r as u8) as char);
        n = (n - 1) / 26;
    }
    s
}

fn strip_quotes(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 && ((t.starts_with('\'') && t.ends_with('\'')) || (t.starts_with('"') && t.ends_with('"'))) {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

fn parse_a1(s: &str) -> Option<(i32, i32)> {
    let s = s.trim().replace('$', "");
    let split = s.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = s.split_at(split);
    let col = col_to_index(letters)?;
    let row: i32 = digits.parse().ok()?;
    if row < 1 {
        return None;
    }
    Some((col, row))
}

/// Parse a target string. Grammar:
///   `Sheet!A1`            cell
///   `A1`                  cell on the first sheet
///   `Sheet@label`         label row in column A of Sheet, value column B
///   `Sheet@label:C`       …value column C
///   `@label`              label search across all sheets
///   `name`                a defined name
pub fn parse_target(s: &str) -> Result<Target> {
    let s = s.trim();
    if s.is_empty() {
        bail!("empty target");
    }
    if let Some(at) = s.find('@') {
        let sheet = &s[..at];
        let rest = &s[at + 1..];
        let (label, col) = match rest.rsplit_once(':') {
            Some((l, c)) if !c.is_empty() && col_to_index(c).is_some() => (l, col_to_index(c).unwrap()),
            _ => (rest, 2),
        };
        if label.trim().is_empty() {
            bail!("empty label in target '{s}'");
        }
        let sheet = if sheet.is_empty() { None } else { Some(strip_quotes(sheet)) };
        return Ok(Target::Label { sheet, label: label.trim().to_string(), col });
    }
    if let Some(bang) = s.rfind('!') {
        let sheet = strip_quotes(&s[..bang]);
        let (col, row) = parse_a1(&s[bang + 1..]).ok_or_else(|| anyhow!("bad cell reference in '{s}'"))?;
        return Ok(Target::Cell { sheet: Some(sheet), col, row });
    }
    if let Some((col, row)) = parse_a1(s) {
        return Ok(Target::Cell { sheet: None, col, row });
    }
    Ok(Target::Name(s.to_string()))
}

/// `target=value` for what-if overrides.
pub fn parse_assignment(s: &str) -> Result<(Target, String)> {
    let eq = s.find('=').ok_or_else(|| anyhow!("expected target=value, got '{s}'"))?;
    let target = parse_target(&s[..eq])?;
    let value = s[eq + 1..].to_string();
    if value.is_empty() {
        bail!("empty value in '{s}'");
    }
    Ok((target, value))
}

#[derive(Debug, Clone, Serialize)]
pub struct CellReport {
    pub r#ref: String,
    pub sheet: String,
    pub row: i32,
    pub col: String,
    pub label: Option<String>,
    pub value: serde_json::Value,
    pub formatted: String,
    pub formula: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Mismatch {
    pub r#ref: String,
    pub cached: Option<f64>,
    pub recalculated: Option<f64>,
    pub formula: String,
    pub kind: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    pub file: String,
    pub formula_cells: usize,
    pub mismatches: Vec<Mismatch>,
    pub errors: Vec<Mismatch>,
    pub ok: bool,
}

pub struct Book {
    model: Model<'static>,
    pub path: String,
}

impl Book {
    pub fn open(path: &str) -> Result<Book> {
        let model = load_from_xlsx(path, LOCALE, TZ, LANG)
            .map_err(|e| anyhow!("{e:?}"))
            .with_context(|| format!("loading {path}"))?;
        Ok(Book { model, path: path.to_string() })
    }

    pub fn from_model(model: Model<'static>, path: &str) -> Book {
        Book { model, path: path.to_string() }
    }

    pub fn evaluate(&mut self) {
        self.model.evaluate();
    }

    pub fn save_as(&self, path: &str) -> Result<()> {
        save_to_xlsx(&self.model, path).map_err(|e| anyhow!("{e:?}")).with_context(|| format!("saving {path}"))
    }

    pub fn sheet_names(&self) -> Vec<String> {
        self.model.workbook.get_worksheet_names()
    }

    pub fn sheet_index(&self, name: &str) -> Result<u32> {
        let names = self.sheet_names();
        if let Some(i) = names.iter().position(|n| n == name) {
            return Ok(i as u32);
        }
        let lower = name.to_lowercase();
        let hits: Vec<usize> = names.iter().enumerate().filter(|(_, n)| n.to_lowercase() == lower).map(|(i, _)| i).collect();
        match hits.as_slice() {
            [i] => Ok(*i as u32),
            _ => {
                let prefix: Vec<usize> = names.iter().enumerate().filter(|(_, n)| n.to_lowercase().starts_with(&lower)).map(|(i, _)| i).collect();
                match prefix.as_slice() {
                    [i] => Ok(*i as u32),
                    _ => bail!("no sheet '{name}' (sheets: {})", names.join(", ")),
                }
            }
        }
    }

    pub fn dimension(&self, sheet: u32) -> Result<(i32, i32, i32, i32)> {
        let ws = self.model.workbook.worksheet(sheet).map_err(|e| anyhow!(e))?;
        let d = ws.dimension();
        Ok((d.min_row, d.max_row, d.min_column, d.max_column))
    }

    pub fn defined_names(&self) -> Vec<(String, Option<u32>, String)> {
        self.model.get_defined_name_list()
    }

    fn text_at(&self, sheet: u32, row: i32, col: i32) -> Option<String> {
        match self.model.get_cell_value_by_index(sheet, row, col).ok()? {
            CellValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// Find the unique row in `sheet` whose column-A text contains `label`.
    /// Exact (case-insensitive) match wins; then a unique prefix; then a unique substring.
    pub fn find_label_row(&self, sheet: u32, label: &str) -> Result<i32> {
        let (min_row, max_row, _, _) = self.dimension(sheet)?;
        let needle = label.to_lowercase();
        let mut exact = vec![];
        let mut prefix = vec![];
        let mut contains = vec![];
        for row in min_row..=max_row {
            if let Some(t) = self.text_at(sheet, row, 1) {
                let tl = t.to_lowercase();
                if tl == needle {
                    exact.push(row);
                } else if tl.starts_with(&needle) {
                    prefix.push(row);
                } else if tl.contains(&needle) {
                    contains.push(row);
                }
            }
        }
        for set in [&exact, &prefix, &contains] {
            if set.len() == 1 {
                return Ok(set[0]);
            }
            if set.len() > 1 {
                let cands: Vec<String> = set.iter().map(|r| format!("A{r} '{}'", self.text_at(sheet, *r, 1).unwrap_or_default())).collect();
                bail!("label '{label}' is ambiguous on sheet '{}': {}", self.sheet_names()[sheet as usize], cands.join("; "));
            }
        }
        bail!("no row on sheet '{}' with a column-A label containing '{label}'", self.sheet_names()[sheet as usize])
    }

    /// All rows across all sheets whose column-A text contains `text`.
    pub fn search_labels(&self, text: &str, only_sheet: Option<u32>) -> Result<Vec<Pos>> {
        let needle = text.to_lowercase();
        let mut out = vec![];
        for (i, _) in self.sheet_names().iter().enumerate() {
            let sheet = i as u32;
            if let Some(s) = only_sheet {
                if s != sheet {
                    continue;
                }
            }
            let (min_row, max_row, _, _) = self.dimension(sheet)?;
            for row in min_row..=max_row {
                if let Some(t) = self.text_at(sheet, row, 1) {
                    if t.to_lowercase().contains(&needle) {
                        out.push(Pos { sheet, row, col: 1 });
                    }
                }
            }
        }
        Ok(out)
    }

    pub fn resolve(&self, t: &Target) -> Result<Pos> {
        match t {
            Target::Cell { sheet, col, row } => {
                let sheet = match sheet {
                    Some(s) => self.sheet_index(s)?,
                    None => 0,
                };
                Ok(Pos { sheet, row: *row, col: *col })
            }
            Target::Label { sheet, label, col } => {
                let sheets: Vec<u32> = match sheet {
                    Some(s) => vec![self.sheet_index(s)?],
                    None => (0..self.sheet_names().len() as u32).collect(),
                };
                let mut found = vec![];
                let mut last_err = None;
                for s in sheets {
                    match self.find_label_row(s, label) {
                        Ok(r) => found.push(Pos { sheet: s, row: r, col: *col }),
                        Err(e) => last_err = Some(e),
                    }
                }
                match found.as_slice() {
                    [p] => Ok(*p),
                    [] => Err(last_err.unwrap_or_else(|| anyhow!("label '{label}' not found"))),
                    many => bail!(
                        "label '{label}' found on several sheets: {}",
                        many.iter().map(|p| self.sheet_names()[p.sheet as usize].clone()).collect::<Vec<_>>().join(", ")
                    ),
                }
            }
            Target::Name(name) => {
                let names = self.defined_names();
                let hit = names
                    .iter()
                    .find(|(n, _, _)| n.eq_ignore_ascii_case(name))
                    .ok_or_else(|| anyhow!("'{name}' is not a cell reference, a Sheet@label, or a defined name"))?;
                match parse_target(&hit.2)? {
                    Target::Cell { sheet, col, row } => {
                        let sheet = match sheet {
                            Some(s) => self.sheet_index(&s)?,
                            None => 0,
                        };
                        Ok(Pos { sheet, row, col })
                    }
                    _ => bail!("defined name '{name}' is not a single cell ({})", hit.2),
                }
            }
        }
    }

    pub fn ref_string(&self, p: Pos) -> String {
        let name = &self.sheet_names()[p.sheet as usize];
        let quoted = if name.contains(' ') { format!("'{name}'") } else { name.clone() };
        format!("{quoted}!{}{}", index_to_col(p.col), p.row)
    }

    pub fn raw(&self, p: Pos) -> Result<CellValue> {
        self.model.get_cell_value_by_index(p.sheet, p.row, p.col).map_err(|e| anyhow!(e))
    }

    pub fn number(&self, p: Pos) -> Option<f64> {
        match self.raw(p).ok()? {
            CellValue::Number(n) => Some(n),
            _ => None,
        }
    }

    pub fn formatted(&self, p: Pos) -> String {
        self.model.get_formatted_cell_value(p.sheet, p.row, p.col).unwrap_or_default()
    }

    pub fn formula(&self, p: Pos) -> Option<String> {
        self.model.get_cell_formula(p.sheet, p.row, p.col).ok().flatten()
    }

    pub fn is_error(&self, p: Pos) -> bool {
        matches!(self.model.get_cell_type(p.sheet, p.row, p.col), Ok(CellType::ErrorValue))
    }

    pub fn label_of(&self, p: Pos) -> Option<String> {
        if p.col == 1 {
            return None;
        }
        self.text_at(p.sheet, p.row, 1)
    }

    pub fn report(&self, p: Pos) -> CellReport {
        let value = match self.raw(p).unwrap_or(CellValue::None) {
            CellValue::None => serde_json::Value::Null,
            CellValue::String(s) => serde_json::Value::String(s),
            CellValue::Number(n) => serde_json::json!(n),
            CellValue::Boolean(b) => serde_json::Value::Bool(b),
        };
        CellReport {
            r#ref: self.ref_string(p),
            sheet: self.sheet_names()[p.sheet as usize].clone(),
            row: p.row,
            col: index_to_col(p.col),
            label: self.label_of(p),
            value,
            formatted: self.formatted(p),
            formula: self.formula(p),
        }
    }

    /// Set a cell from user text: number, `=formula`, or text. Does not evaluate.
    pub fn set(&mut self, p: Pos, value: &str) -> Result<()> {
        let v = value.trim();
        let r = if let Some(f) = v.strip_prefix('=') {
            self.model.update_cell_with_formula(p.sheet, p.row, p.col, format!("={f}"))
        } else if let Ok(n) = v.replace(['£', ','], "").parse::<f64>() {
            self.model.update_cell_with_number(p.sheet, p.row, p.col, n)
        } else if let Some(pct) = v.strip_suffix('%').and_then(|s| s.parse::<f64>().ok()) {
            self.model.update_cell_with_number(p.sheet, p.row, p.col, pct / 100.0)
        } else {
            self.model.update_cell_with_text(p.sheet, p.row, p.col, v)
        };
        r.map_err(|e| anyhow!(e)).with_context(|| format!("setting {}", self.ref_string(p)))
    }

    /// Apply `target=value` overrides, then evaluate. Returns (ref, label, before, after).
    pub fn apply_overrides(&mut self, sets: &[String]) -> Result<Vec<(String, Option<String>, String, String)>> {
        let mut log = vec![];
        for s in sets {
            let (t, v) = parse_assignment(s)?;
            let p = self.resolve(&t)?;
            let before = self.formatted(p);
            self.set(p, &v)?;
            log.push((self.ref_string(p), self.label_of(p), before, v));
        }
        self.evaluate();
        for entry in log.iter_mut() {
            let p = self.resolve(&parse_target(&entry.0)?)?;
            entry.3 = self.formatted(p);
        }
        Ok(log)
    }

    /// Every formula cell in the workbook.
    pub fn formula_cells(&self) -> Vec<Pos> {
        self.model
            .get_all_cells()
            .into_iter()
            .map(|c| Pos { sheet: c.index, row: c.row, col: c.column })
            .filter(|p| self.formula(*p).is_some())
            .collect()
    }

    /// Compare the values cached in the file against a fresh IronCalc recalculation.
    pub fn check(path: &str) -> Result<CheckReport> {
        let cached = Book::open(path)?;
        let mut fresh = Book::open(path)?;
        fresh.evaluate();
        let mut mismatches = vec![];
        let mut errors = vec![];
        let cells = fresh.formula_cells();
        for p in &cells {
            let formula = fresh.formula(*p).unwrap_or_default();
            let r = fresh.ref_string(*p);
            if fresh.is_error(*p) {
                errors.push(Mismatch { r#ref: r, cached: cached.number(*p), recalculated: None, formula, kind: "error" });
                continue;
            }
            let a = cached.number(*p);
            let b = fresh.number(*p);
            match (a, b) {
                (Some(x), Some(y)) => {
                    let tol = 1e-6 * x.abs().max(1.0);
                    if (x - y).abs() > tol {
                        mismatches.push(Mismatch { r#ref: r, cached: a, recalculated: b, formula, kind: "value" });
                    }
                }
                (None, None) => {
                    // both non-numeric (text/bool): compare formatted
                    if cached.formatted(*p) != fresh.formatted(*p) {
                        mismatches.push(Mismatch { r#ref: r, cached: None, recalculated: None, formula, kind: "text" });
                    }
                }
                _ => mismatches.push(Mismatch { r#ref: r, cached: a, recalculated: b, formula, kind: "type" }),
            }
        }
        let ok = mismatches.is_empty() && errors.is_empty();
        Ok(CheckReport { file: path.to_string(), formula_cells: cells.len(), mismatches, errors, ok })
    }

    /// A sheet as rows of formatted strings (row numbers from `min_row`).
    pub fn dump(&self, sheet: u32, formulas: bool) -> Result<(Vec<String>, Vec<(i32, Vec<String>)>)> {
        let (min_row, max_row, min_col, max_col) = self.dimension(sheet)?;
        let header: Vec<String> = (min_col..=max_col).map(index_to_col).collect();
        let mut rows = vec![];
        for row in min_row..=max_row {
            let mut cells = vec![];
            let mut any = false;
            for col in min_col..=max_col {
                let p = Pos { sheet, row, col };
                let mut s = self.formatted(p);
                if formulas {
                    if let Some(f) = self.formula(p) {
                        s = f;
                    }
                }
                if !s.is_empty() {
                    any = true;
                }
                cells.push(s);
            }
            if any {
                rows.push((row, cells));
            }
        }
        Ok((header, rows))
    }
}

pub fn md_escape(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

pub fn markdown_table(header: &[String], rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    out.push_str("| ");
    out.push_str(&header.iter().map(|h| md_escape(h)).collect::<Vec<_>>().join(" | "));
    out.push_str(" |\n|");
    for _ in header {
        out.push_str("---|");
    }
    out.push('\n');
    for r in rows {
        out.push_str("| ");
        out.push_str(&r.iter().map(|c| md_escape(c)).collect::<Vec<_>>().join(" | "));
        out.push_str(" |\n");
    }
    out
}

pub fn csv_line(cells: &[String]) -> String {
    cells
        .iter()
        .map(|c| {
            if c.contains([',', '"', '\n']) {
                format!("\"{}\"", c.replace('"', "\"\""))
            } else {
                c.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Book {
        let mut m = Model::new_empty("fixture", LOCALE, TZ, LANG).unwrap();
        m.add_sheet("Inputs").unwrap();
        m.add_sheet("Rate sensitivity").unwrap();
        // Sheet 0 is the default "Sheet1"; Inputs = 1; Rate sensitivity = 2
        m.update_cell_with_text(1, 1, 1, "Assumption").unwrap();
        m.update_cell_with_text(1, 2, 1, "Realised fee per SSE (£)").unwrap();
        m.update_cell_with_number(1, 2, 2, 173.0).unwrap();
        m.update_cell_with_text(1, 3, 1, "Earning weeks per year").unwrap();
        m.update_cell_with_number(1, 3, 2, 42.0).unwrap();
        m.update_cell_with_text(1, 4, 1, "Fee (old)").unwrap();
        m.update_cell_with_number(1, 4, 2, 165.0).unwrap();
        m.update_cell_with_text(2, 1, 1, "FLOOR (sessions per week)").unwrap();
        m.update_cell_with_formula(2, 1, 2, "=100000/(Inputs!B2*Inputs!B3)".to_string()).unwrap();
        m.update_cell_with_text(2, 2, 1, "Bad").unwrap();
        m.update_cell_with_formula(2, 2, 2, "=1/0".to_string()).unwrap();
        m.evaluate();
        Book::from_model(m, "fixture.xlsx")
    }

    #[test]
    fn column_conversions_round_trip() {
        for (s, i) in [("A", 1), ("Z", 26), ("AA", 27), ("AZ", 52), ("BA", 53), ("ZZ", 702)] {
            assert_eq!(col_to_index(s), Some(i));
            assert_eq!(index_to_col(i), s);
        }
        assert_eq!(col_to_index(""), None);
        assert_eq!(col_to_index("A1"), None);
    }

    #[test]
    fn parses_targets() {
        assert_eq!(parse_target("Lives!B31").unwrap(), Target::Cell { sheet: Some("Lives".into()), col: 2, row: 31 });
        assert_eq!(parse_target("'Rate sensitivity'!$D$2").unwrap(), Target::Cell { sheet: Some("Rate sensitivity".into()), col: 4, row: 2 });
        assert_eq!(parse_target("C7").unwrap(), Target::Cell { sheet: None, col: 3, row: 7 });
        assert_eq!(parse_target("Inputs@fee").unwrap(), Target::Label { sheet: Some("Inputs".into()), label: "fee".into(), col: 2 });
        assert_eq!(parse_target("Lives@FLOOR:D").unwrap(), Target::Label { sheet: Some("Lives".into()), label: "FLOOR".into(), col: 4 });
        assert_eq!(parse_target("@Rooms: rent").unwrap(), Target::Label { sheet: None, label: "Rooms: rent".into(), col: 2 });
        assert_eq!(parse_target("fee").unwrap(), Target::Name("fee".into()));
        assert!(parse_target("Lives!").is_err());
        assert!(parse_target("Inputs@").is_err());
    }

    #[test]
    fn parses_assignments_including_formulas() {
        let (t, v) = parse_assignment("Inputs@fee=180").unwrap();
        assert_eq!(t, Target::Label { sheet: Some("Inputs".into()), label: "fee".into(), col: 2 });
        assert_eq!(v, "180");
        let (_, v) = parse_assignment("Inputs!B2==Inputs!B4*1.1").unwrap();
        assert_eq!(v, "=Inputs!B4*1.1");
        assert!(parse_assignment("Inputs!B2").is_err());
        assert!(parse_assignment("Inputs!B2=").is_err());
    }

    #[test]
    fn label_lookup_prefers_exact_then_prefix_then_unique_substring() {
        let b = fixture();
        // "fee" is inside two labels but is the unique PREFIX of "Fee (old)" -> prefix wins
        assert_eq!(b.find_label_row(1, "fee").unwrap(), 4);
        // "per" is inside two labels and a prefix of none -> ambiguous, candidates listed
        let err = b.find_label_row(1, "per").unwrap_err().to_string();
        assert!(err.contains("ambiguous") && err.contains("A2") && err.contains("A3"), "{err}");
        assert_eq!(b.find_label_row(1, "Realised fee").unwrap(), 2);
        assert_eq!(b.find_label_row(1, "fee (old)").unwrap(), 4);
        assert_eq!(b.find_label_row(1, "earning weeks").unwrap(), 3);
        assert!(b.find_label_row(1, "nothing here").is_err());
    }

    #[test]
    fn sheet_index_is_case_insensitive_with_unique_prefix() {
        let b = fixture();
        assert_eq!(b.sheet_index("Inputs").unwrap(), 1);
        assert_eq!(b.sheet_index("inputs").unwrap(), 1);
        assert_eq!(b.sheet_index("rate").unwrap(), 2);
        assert!(b.sheet_index("nope").is_err());
    }

    #[test]
    fn overrides_recalculate_and_report_before_after() {
        let mut b = fixture();
        let floor = b.resolve(&parse_target("Rate sensitivity@FLOOR").unwrap()).unwrap();
        let before = b.number(floor).unwrap();
        assert!((before - 100000.0 / (173.0 * 42.0)).abs() < 1e-9);
        let log = b.apply_overrides(&["Inputs@Realised fee=180".to_string()]).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].2, "173");
        assert_eq!(log[0].3, "180");
        let after = b.number(floor).unwrap();
        assert!((after - 100000.0 / (180.0 * 42.0)).abs() < 1e-9);
        assert!(after < before);
    }

    #[test]
    fn set_accepts_percent_and_currency_text() {
        let mut b = fixture();
        let p = Pos { sheet: 1, row: 2, col: 2 };
        b.set(p, "5.5%").unwrap();
        assert!((b.number(p).unwrap() - 0.055).abs() < 1e-12);
        b.set(p, "£1,234").unwrap();
        assert!((b.number(p).unwrap() - 1234.0).abs() < 1e-12);
        b.set(p, "hello").unwrap();
        assert!(matches!(b.raw(p).unwrap(), CellValue::String(s) if s == "hello"));
    }

    #[test]
    fn error_cells_are_detected() {
        let b = fixture();
        assert!(b.is_error(Pos { sheet: 2, row: 2, col: 2 }));
        assert!(!b.is_error(Pos { sheet: 2, row: 1, col: 2 }));
    }

    #[test]
    fn check_passes_on_a_saved_workbook_and_fails_on_a_corrupted_cache() {
        let dir = std::env::temp_dir().join(format!("icalc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fixture.xlsx");
        let mut m = Model::new_empty("fixture", LOCALE, TZ, LANG).unwrap();
        m.update_cell_with_number(0, 1, 1, 2.0).unwrap();
        m.update_cell_with_formula(0, 1, 2, "=A1*21".to_string()).unwrap();
        m.evaluate();
        save_to_xlsx(&m, path.to_str().unwrap()).unwrap();
        let rep = Book::check(path.to_str().unwrap()).unwrap();
        assert!(rep.ok, "{rep:?}");
        assert_eq!(rep.formula_cells, 1);
        // Known-red control: a workbook whose cached value disagrees with its formula.
        let mut m2 = Model::new_empty("fixture", LOCALE, TZ, LANG).unwrap();
        m2.update_cell_with_number(0, 1, 1, 2.0).unwrap();
        m2.update_cell_with_formula(0, 1, 2, "=A1*21".to_string()).unwrap();
        m2.evaluate();
        m2.update_cell_with_number(0, 1, 1, 3.0).unwrap(); // change the input but do NOT re-evaluate
        let bad = dir.join("stale.xlsx");
        save_to_xlsx(&m2, bad.to_str().unwrap()).unwrap();
        let rep = Book::check(bad.to_str().unwrap()).unwrap();
        assert!(!rep.ok);
        assert_eq!(rep.mismatches.len(), 1);
        assert_eq!(rep.mismatches[0].cached, Some(42.0));
        assert_eq!(rep.mismatches[0].recalculated, Some(63.0));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn markdown_and_csv_escape() {
        let t = markdown_table(&["A".into(), "B".into()], &[vec!["x|y".into(), "1".into()]]);
        assert!(t.contains("x\\|y"));
        assert_eq!(csv_line(&["a,b".into(), "c\"d".into(), "e".into()]), "\"a,b\",\"c\"\"d\",e");
    }
}
