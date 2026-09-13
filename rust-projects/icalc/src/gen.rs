//! Generator: builds the House Model workbook from a TOML spec with IronCalc.
//! The spec (`house-model.toml`) holds every number and note; this module holds
//! the structure and formulas. Every Assumptions key becomes a workbook defined
//! name (`fee`, `weeks`, …) so formulas read `=B30/(fee*weeks)` and tools can
//! address inputs by name. Row layout matches the 12 Sep 2026 Python generator.

use anyhow::{anyhow, bail, Context, Result};
use ironcalc::base::types::{Alignment, Style};
use ironcalc::base::Model;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct Spec {
    pub meta: Meta,
    #[serde(rename = "assumption")]
    pub assumptions: Vec<Assumption>,
    #[serde(rename = "book")]
    pub book: Vec<BookRow>,
}

#[derive(Debug, Deserialize)]
pub struct Meta {
    pub title: String,
    pub date: String,
    #[serde(default)]
    pub calibration: String,
}

#[derive(Debug, Deserialize)]
pub struct Assumption {
    pub header: Option<String>,
    pub key: Option<String>,
    pub label: Option<String>,
    pub value: Option<toml::Value>,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Deserialize)]
pub struct BookRow {
    pub day: String,
    pub sessions: f64,
    pub remote_always: f64,
    #[serde(default)]
    pub note: String,
}

pub fn load_spec(path: &str) -> Result<Spec> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
    let spec: Spec = toml::from_str(&text).with_context(|| format!("parsing {path}"))?;
    for a in &spec.assumptions {
        match (&a.header, &a.key, &a.label, &a.value) {
            (Some(_), None, None, None) => {}
            (None, Some(k), Some(_), Some(_)) => {
                if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || k.chars().next().unwrap().is_ascii_digit() {
                    bail!("assumption key '{k}' must be an identifier (letters, digits, _)");
                }
            }
            _ => bail!("each [[assumption]] needs either `header` or all of `key`, `label`, `value` (got {:?})", a),
        }
    }
    Ok(spec)
}

const S_READ: u32 = 0;
const S_ASSUM: u32 = 1;
const S_LIVES: u32 = 2;
const S_RATE: u32 = 3;
const S_BOOK: u32 = 4;
const S_2032: u32 = 5;
const SHEETS: [&str; 6] = ["Read me", "Assumptions", "Lives", "Rate sensitivity", "Book", "2032"];

/// Column widths in pixels (the Python generator's cm widths at ~38 px/cm).
const W_LABEL: f64 = 435.0; // 11.5cm
const W_VALUE: f64 = 144.0; // 3.8cm
const W_NOTE: f64 = 605.0; // 16cm
const W_LIVES: f64 = 197.0; // 5.2cm
const W_RATE: f64 = 166.0; // 4.4cm

const FMT_INT: &str = "#,##0";
const FMT_1DP: &str = "0.0";
const FMT_PCT: &str = "0.00%";
const SESSION_WORDS: [&str; 12] = ["Pence", "FLOOR", "CEILING", "LEEWAY", "Floor", "Ceiling", "Sessions", "sessions", "margin", "Roof", "Book after", "Average over"];

/// A cell value in a generated row.
#[derive(Clone, Debug)]
pub enum V {
    Empty,
    Num(f64),
    Text(String),
    Formula(String),
}

impl From<&str> for V {
    fn from(s: &str) -> V {
        if s.is_empty() {
            V::Empty
        } else if let Some(f) = s.strip_prefix('=') {
            V::Formula(f.to_string())
        } else {
            V::Text(s.to_string())
        }
    }
}
impl From<String> for V {
    fn from(s: String) -> V {
        V::from(s.as_str())
    }
}
impl From<f64> for V {
    fn from(n: f64) -> V {
        V::Num(n)
    }
}
impl From<i32> for V {
    fn from(n: i32) -> V {
        V::Num(n as f64)
    }
}

fn style(num_fmt: Option<&str>, bold: bool, wrap: bool) -> Style {
    let mut st = Style::default();
    if let Some(f) = num_fmt {
        st.num_fmt = f.to_string();
    }
    st.font.b = bold;
    if wrap {
        st.alignment = Some(Alignment { wrap_text: true, vertical: ironcalc::base::types::VerticalAlignment::Top, ..Default::default() });
    }
    st
}

struct Gen<'a> {
    m: Model<'a>,
}

impl<'a> Gen<'a> {
    fn put(&mut self, sheet: u32, row: i32, col: i32, v: &V) -> Result<()> {
        let r = match v {
            V::Empty => return Ok(()),
            V::Num(n) => self.m.update_cell_with_number(sheet, row, col, *n),
            V::Text(t) => self.m.update_cell_with_text(sheet, row, col, t),
            V::Formula(f) => self.m.update_cell_with_formula(sheet, row, col, format!("={f}")),
        };
        r.map_err(|e| anyhow!("{}!{}{}: {e}", SHEETS[sheet as usize], crate::book::index_to_col(col), row))
    }

    fn style(&mut self, sheet: u32, row: i32, col: i32, st: &Style) -> Result<()> {
        self.m.set_cell_style(sheet, row, col, st).map_err(|e| anyhow!("style {}!{}{}: {e}", SHEETS[sheet as usize], crate::book::index_to_col(col), row))
    }

    /// Write a row; `fmt(col_index)` gives the number format per data column.
    fn row(&mut self, sheet: u32, row: i32, cells: &[V], fmt: &dyn Fn(usize, &V) -> Option<&'static str>) -> Result<()> {
        for (i, v) in cells.iter().enumerate() {
            let col = i as i32 + 1;
            self.put(sheet, row, col, v)?;
            if let Some(f) = fmt(i, v) {
                if !matches!(v, V::Empty | V::Text(_)) {
                    self.style(sheet, row, col, &style(Some(f), false, false))?;
                }
            }
        }
        Ok(())
    }

    fn header_row(&mut self, sheet: u32, cells: &[&str]) -> Result<()> {
        for (i, t) in cells.iter().enumerate() {
            let col = i as i32 + 1;
            self.put(sheet, 1, col, &V::from(*t))?;
            self.style(sheet, 1, col, &style(None, true, true))?;
        }
        Ok(())
    }
}

/// Number format for a generic model row, by the Python generator's rule:
/// percent for "Mortgage rate", one decimal for session rows, else integers.
fn row_fmt(label: &str) -> &'static str {
    if label.starts_with("Mortgage rate") {
        FMT_PCT
    } else if SESSION_WORDS.iter().any(|w| label.contains(w)) {
        FMT_1DP
    } else {
        FMT_INT
    }
}

pub fn build(spec: &Spec) -> Result<Model<'static>> {
    let mut m = Model::new_empty("house-model", "en", "UTC", "en").map_err(|e| anyhow!(e))?;
    m.rename_sheet("Sheet1", SHEETS[0]).map_err(|e| anyhow!(e))?;
    for name in &SHEETS[1..] {
        m.add_sheet(name).map_err(|e| anyhow!(e))?;
    }
    let mut g = Gen { m };

    // ---- Assumptions rows and defined names (names first, so formulas parse) ----
    let mut arow: HashMap<String, i32> = HashMap::new();
    let mut r = 2;
    for a in &spec.assumptions {
        if let Some(k) = &a.key {
            arow.insert(k.clone(), r);
        }
        r += 1;
    }
    for (k, row) in &arow {
        g.m.new_defined_name(k, None, &format!("Assumptions!$B${row}")).map_err(|e| anyhow!("defined name '{k}': {e}"))?;
    }
    // Structural names owned by the generator (documented in icalc.md).
    g.m.new_defined_name("lives_pension_take", None, "Lives!$B$41").map_err(|e| anyhow!(e))?;
    g.m.new_defined_name("book_roof", None, "Book!$B$8").map_err(|e| anyhow!(e))?;
    g.m.new_defined_name("book_remote_only", None, "Book!$B$14").map_err(|e| anyhow!(e))?;
    let need = |k: &str| -> Result<()> { if arow.contains_key(k) { Ok(()) } else { bail!("spec is missing assumption key '{k}'") } };
    for k in ["fee","weeks","remote_weeks","remote_frac","roof","salary","rent_3days","leigh","motor","other_costs","pension_allowance","pa","basic_band","div_allow","div_basic","div_higher","cap","ct_marg","ct_adj","ct_small","ns_basic","ns_higher","base_life","flat_running","ct_premium","premium_share","london_transport","london_food","london_cash","trips_if_sold","heating_if_sold","nights","room","train","parking","forge_bal","flat_bal","rate","rate_now","jenny_share","capital","proceeds","shares","tsla","fx","sjp","sjp_g","tsla_g","years","chimney_in","draw_pct","state_pension","clear_flat","lsa"] {
        need(k)?;
    }

    // ---- Read me ----
    let readme = [
        format!("{}, {}", spec.meta.title, spec.meta.date),
        String::new(),
        "Change numbers on the Assumptions sheet only. Every other sheet is formulas. Each input also has a workbook name (fee, weeks, rate, …): see Formulas → Manage Names, or `icalc names`.".into(),
        "Lives: the three futures side by side (keep the flat / sell and lodge / Somerset only), plus keep-the-flat at today's rate for Jan–May 2027, and the present (three London days, today's rate) to December 2026.".into(),
        "Rate sensitivity: keep-the-flat at each mortgage rate.".into(),
        "Book: sessions by day, the roof, the chimney, and what a remote-only week keeps.".into(),
        "2032: the pot at each Tesla price, and the floor and ceiling you would work with the flat kept and no London clinic, state pension in.".into(),
        String::new(),
        "The house: FLOOR = sessions that fund the living and the taxes. CEILING = sessions at which drawable profit reaches the £100k line. LEEWAY = ceiling − floor = the spendable room. ROOF = the book you actually carry. CHIMNEY = sessions above the ceiling, to the pension.".into(),
        spec.meta.calibration.clone(),
        "Source of truth: house-model.toml (inputs) + icalc (structure), regenerated with `icalc house gen`. Edits made here are what-ifs; carry them back to the TOML to keep them.".into(),
    ];
    for (i, line) in readme.iter().enumerate() {
        g.put(S_READ, i as i32 + 1, 1, &V::from(line.as_str()))?;
    }
    g.m.set_column_width(S_READ, 1, W_NOTE).map_err(|e| anyhow!(e))?;

    // ---- Assumptions ----
    g.header_row(S_ASSUM, &["Assumption", "Value", "Source / note"])?;
    let mut r = 2;
    for a in &spec.assumptions {
        if let Some(h) = &a.header {
            g.put(S_ASSUM, r, 1, &V::Text(format!("— {h} —")))?;
            g.put(S_ASSUM, r, 3, &V::from(a.note.as_str()))?;
        } else {
            g.put(S_ASSUM, r, 1, &V::from(a.label.clone().unwrap_or_default()))?;
            let v = match a.value.as_ref().unwrap() {
                toml::Value::Integer(i) => V::Num(*i as f64),
                toml::Value::Float(f) => V::Num(*f),
                toml::Value::String(s) => V::from(s.as_str()),
                other => bail!("assumption '{}': unsupported value {other:?}", a.key.clone().unwrap_or_default()),
            };
            g.put(S_ASSUM, r, 2, &v)?;
            g.put(S_ASSUM, r, 3, &V::from(a.note.as_str()))?;
        }
        g.style(S_ASSUM, r, 3, &style(None, false, true))?;
        r += 1;
    }
    for (c, w) in [(1, W_LABEL), (2, W_VALUE), (3, W_NOTE)] {
        g.m.set_column_width(S_ASSUM, c, w).map_err(|e| anyhow!(e))?;
    }

    // ---- Book ----
    g.header_row(S_BOOK, &["Day", "Sessions (SSE)", "Of which remote-always", "Note"])?;
    let mut r = 2;
    for b in &spec.book {
        g.row(S_BOOK, r, &[V::from(b.day.as_str()), V::Num(b.sessions), V::Num(b.remote_always), V::from(b.note.as_str())], &|_, _| Some(FMT_INT))?;
        r += 1;
    }
    if r != 7 {
        bail!("the Book expects exactly 5 day rows (Monday–Friday); got {}", spec.book.len());
    }
    let book_rows: Vec<[V; 4]> = vec![
        [V::from("Itemised book (sessions per full week)"), V::from("=SUM(B2:B6)"), V::from("=SUM(C2:C6)"), V::from("Total itemised / remote-always. The planning roof on Assumptions is what the model relies on")],
        [V::from("London sessions (Tue + Wed)"), V::from("=B3+B4"), V::from("=C3+C4"), V::Empty],
        [V::from("Somerset-day sessions (Mon + Thu + Fri)"), V::from("=B2+B5+B6"), V::from("=C2+C5+C6"), V::Empty],
        [V::from("Average over the year incl. remote-only weeks"), V::from("=(B8*(weeks-remote_weeks)+B8*remote_frac*remote_weeks)/weeks"), V::Empty, V::from("The four remote weeks cost about one session a week on the average")],
        [V::from("Remote-only week: sessions kept, no in-person converts"), V::from("=C8"), V::from("=C8/B8"), V::from("share of a normal week")],
        [V::from("Remote-only week: kept if half the in-person clients accept a one-off remote"), V::from("=C8+0.5*(B8-C8)"), V::from("=(C8+0.5*(B8-C8))/B8"), V::Empty],
        [V::from("Book after leaving London (remote-always only)"), V::from("=C8"), V::Empty, V::from("Add any in-person clients who convert")],
    ];
    for (i, cells) in book_rows.iter().enumerate() {
        let row = 8 + i as i32;
        let label = match &cells[0] { V::Text(t) => t.clone(), _ => String::new() };
        let f = row_fmt(&label);
        g.row(S_BOOK, row, cells, &|_, _| Some(f))?;
    }
    for (c, w) in [(1, W_LABEL), (2, W_LIVES), (3, W_LIVES), (4, W_NOTE)] {
        g.m.set_column_width(S_BOOK, c, w).map_err(|e| anyhow!(e))?;
    }

    // ---- Lives (columns B..F) ----
    let cols = ["B", "C", "D", "E", "F"];
    let each = |f: &dyn Fn(&str) -> String| -> Vec<V> { cols.iter().map(|c| V::from(f(c))).collect() };
    let mut lives: Vec<(String, Vec<V>)> = vec![];
    let mut push = |label: &str, cells: Vec<V>| lives.push((label.to_string(), cells));
    let head = |s: &str| vec![V::from(s); 0];
    let _ = head;
    push("", vec!["Keep the flat", "Sell, lodge, keep London", "Somerset only, flat sold", "Keep the flat, today's rate (Jan–May 2027)", "Now: three days, today's rate (to Dec 2026)"].into_iter().map(V::from).collect()); // 1
    push("London clinic days", vec![2, 2, 0, 2, 3].into_iter().map(V::from).collect()); // 2
    push("Mortgage rate", vec!["=rate", "=rate", "=rate", "=rate_now", "=rate_now"].into_iter().map(V::from).collect()); // 3
    push("Flat kept? (1/0)", vec![1, 0, 0, 1, 1].into_iter().map(V::from).collect()); // 4
    push("London nights lodged per London week", vec!["0", "=nights", "0", "0", "0"].into_iter().map(|s| if s == "0" { V::Num(0.0) } else { V::from(s) }).collect()); // 5
    push("Jenny's share applies? (1 = yes; 0 until Jan 2027)", vec![1, 1, 1, 1, 0].into_iter().map(V::from).collect()); // 6
    push("Employer pension contribution (£/yr) — stops at the June 2027 remortgage", vec![0, 0, 0, 15000, 15000].into_iter().map(V::from).collect()); // 7
    push("— COMPANY —", vec![]); // 8
    push("Rooms (rent scaled by London days)", each(&|c| format!("=rent_3days*{c}2/3"))); // 9
    push("Other company costs incl. salary and Leigh", each(&|_| "=salary+leigh+motor+other_costs".into())); // 10
    push("Company costs, total", each(&|c| format!("={c}9+{c}10+{c}7"))); // 11
    push("— HOUSEHOLD MORTGAGE —", vec![]); // 12
    push("Balances refinanced", each(&|c| format!("=forge_bal+{c}4*flat_bal-(1-{c}4)*proceeds"))); // 13
    push("Household interest", each(&|c| format!("={c}13*{c}3"))); // 14
    push("Jenny's share of interest + capital", each(&|c| format!("=({c}14+capital)*jenny_share*{c}6"))); // 15
    push("Will's share of interest + capital", each(&|c| format!("=({c}14+capital)*(1-jenny_share*{c}6)"))); // 16
    push("— WILL'S LIVING —", vec![]); // 17
    push("Somerset life", each(&|_| "=base_life".into())); // 18
    push("Flat running costs + Will's share of the council-tax premium", each(&|c| format!("={c}4*(flat_running+ct_premium*premium_share)"))); // 19
    push("London transport, food, cash (only with London days)", each(&|c| format!("=IF({c}2>0,london_transport+london_food+london_cash,0)"))); // 20
    push("Lodging: nights × rate × London weeks", each(&|c| format!("={c}5*room*(weeks-remote_weeks)"))); // 21
    push("Train + parking (lodging life only)", each(&|c| format!("=IF({c}5>0,train+parking,0)"))); // 22
    push("Voluntary London trips + extra heating (no flat)", each(&|c| format!("=(1-{c}4)*(heating_if_sold+IF({c}2>0,0,trips_if_sold))"))); // 23
    push("Mortgage: Will's share + capital", each(&|c| format!("={c}16"))); // 24
    push("Will's living, net", each(&|c| format!("=SUM({c}18:{c}24)"))); // 25
    push("— TAX AND SESSIONS —", vec![]); // 26
    // dividends needed for the target net: closed forms for the higher and basic bands (see the Python original)
    let hb = |c: &str| format!("({c}25-salary+div_basic*(basic_band-div_allow)-div_higher*(pa-salary+basic_band))/(1-div_higher)");
    let bb = |c: &str| format!("({c}25-salary-div_basic*(pa-salary+div_allow))/(1-div_basic)");
    push("Dividends needed (gross)", each(&|c| format!("=IF({}-(pa-salary)>basic_band,{},{})", hb(c), hb(c), bb(c)))); // 27
    push("Gross draw needed (salary + dividends)", each(&|c| format!("=salary+{c}27"))); // 28
    push("Pre-tax profit needed", each(&|c| format!("=IF(({c}27-ct_adj)/(1-ct_marg)>50000,({c}27-ct_adj)/(1-ct_marg),{c}27/(1-ct_small))"))); // 29
    push("Revenue needed", each(&|c| format!("={c}29+{c}11"))); // 30
    push("FLOOR (sessions per week)", each(&|c| format!("={c}30/(fee*weeks)"))); // 31
    push("Profit at the draw cap", each(&|_| "=((cap-salary)-ct_adj)/(1-ct_marg)".into())); // 32
    push("CEILING (sessions per week at the £100k line)", each(&|c| format!("=({c}32+{c}11)/(fee*weeks)"))); // 33
    push("LEEWAY (ceiling − floor)", each(&|c| format!("={c}33-{c}31"))); // 34
    push("Net in hand at the cap", each(&|_| "=salary+(cap-salary)-(div_basic*(basic_band-div_allow)+div_higher*((cap-salary)-(pa-salary)-basic_band))".into())); // 35
    push("Spendable room under the cap (£)", each(&|c| format!("={c}35-{c}25"))); // 36
    push("— THE \"HOUSE\" —", vec![]); // 37
    push("Roof (planning roof from Assumptions; Somerset-only = remote-always book)", vec!["=roof", "=roof", "=book_remote_only", "=roof", "=roof"].into_iter().map(V::from).collect()); // 38
    push("Sessions above the ceiling (chimney)", each(&|c| format!("=MAX(0,{c}38-{c}33)"))); // 39
    push("Chimney, pre-tax (£/yr)", each(&|c| format!("={c}39*fee*weeks"))); // 40
    push("Of which the pension can take", each(&|c| format!("=MIN({c}40,pension_allowance)"))); // 41
    push("Retained in the company after CT", each(&|c| format!("=({c}40-{c}41)*(1-ct_marg)"))); // 42
    push("Solvency margin (roof − floor)", each(&|c| format!("={c}38-{c}31"))); // 43
    push("— JENNY —", vec![]); // 44
    push("Jenny's share of mortgage payments (£/yr)", each(&|c| format!("={c}15"))); // 45
    push("— DRAWING INTO THE TAPER (everything at the roof drawn, no pension) —", vec![]); // 46
    push("Revenue at the roof", each(&|c| format!("={c}38*fee*weeks"))); // 47
    push("Pre-tax profit at the roof", each(&|c| format!("={c}47-{c}11+{c}7"))); // 48
    push("Corporation tax", each(&|c| format!("=IF({c}48>50000,ct_marg*{c}48-ct_adj,ct_small*{c}48)"))); // 49
    push("Gross draw if all is drawn (salary + all dividends)", each(&|c| format!("=salary+{c}48-{c}49"))); // 50
    push("Personal allowance after the taper", each(&|c| format!("=MAX(0,pa-MAX(0,{c}50-100000)/2)"))); // 51
    push("Taxable salary", each(&|c| format!("=MAX(0,salary-{c}51)"))); // 52
    push("Taxable dividends (after any allowance left)", each(&|c| format!("=MAX(0,({c}50-salary)-MAX(0,{c}51-salary))"))); // 53
    push("Dividends in the basic band", each(&|c| format!("=MIN(MAX(0,{c}53-div_allow),MAX(0,basic_band-{c}52))"))); // 54
    push("Dividends in the higher band", each(&|c| format!("=MIN(MAX(0,{c}53-div_allow)-{c}54,MAX(0,125140-MAX(basic_band,{c}52+{c}54)))"))); // 55
    push("Dividends at the additional rate", each(&|c| format!("=MAX(0,{c}53-div_allow)-{c}54-{c}55"))); // 56
    push("Personal tax", each(&|c| format!("=ns_basic*{c}52+div_basic*{c}54+div_higher*{c}55+0.3935*{c}56"))); // 57
    push("NET IN HAND, everything drawn", each(&|c| format!("={c}50-{c}57"))); // 58
    push("Net in hand at the cap (for comparison)", each(&|c| format!("={c}35"))); // 59
    push("Extra spendable from drawing past the cap", each(&|c| format!("={c}58-{c}59"))); // 60
    push("Pension forgone to get it (pre-tax)", each(&|c| format!("={c}40"))); // 61
    push("Pence kept per £ drawn past the cap", each(&|c| format!("=IF({c}50>cap,{c}60/({c}50-cap),0)"))); // 62
    push("Room above the careful life if everything is drawn (£)", each(&|c| format!("={c}58-{c}25"))); // 63
    if lives.len() != 63 {
        bail!("Lives layout drifted: {} rows, expected 63", lives.len());
    }
    if lives[40].0 != "Of which the pension can take" {
        bail!("Lives row 41 is no longer 'Of which the pension can take'; the lives_pension_take name would be wrong");
    }
    for (i, (label, cells)) in lives.iter().enumerate() {
        let row = i as i32 + 1;
        if row == 1 {
            let mut h = vec![""];
            let owned: Vec<String> = cells.iter().map(|v| match v { V::Text(t) => t.clone(), _ => String::new() }).collect();
            h.extend(owned.iter().map(|s| s.as_str()));
            g.header_row(S_LIVES, &h)?;
            continue;
        }
        g.put(S_LIVES, row, 1, &V::from(label.as_str()))?;
        let f = row_fmt(label);
        for (j, v) in cells.iter().enumerate() {
            let col = j as i32 + 2;
            g.put(S_LIVES, row, col, v)?;
            if !matches!(v, V::Empty | V::Text(_)) {
                g.style(S_LIVES, row, col, &style(Some(f), false, false))?;
            }
        }
    }
    g.m.set_column_width(S_LIVES, 1, W_LABEL).map_err(|e| anyhow!(e))?;
    for c in 2..=6 {
        g.m.set_column_width(S_LIVES, c, W_LIVES).map_err(|e| anyhow!(e))?;
    }

    // ---- Rate sensitivity (keep the flat) ----
    g.header_row(S_RATE, &["Rate", "Household interest", "Will's living", "Floor", "Ceiling", "Leeway", "Room under cap (£)", "Jenny's share (incl. capital)"])?;
    for (i, rt) in [0.026, 0.04, 0.045, 0.05, 0.055, 0.06].iter().enumerate() {
        let n = i as i32 + 2;
        let living = format!("=base_life+flat_running+ct_premium*premium_share+london_transport+london_food+london_cash+(B{n}+capital)*(1-jenny_share)");
        let d = format!("(C{n}-salary+div_basic*(basic_band-div_allow)-div_higher*(pa-salary+basic_band))/(1-div_higher)");
        let p = format!("(({d})-ct_adj)/(1-ct_marg)");
        let c = "(rent_3days*2/3+salary+leigh+motor+other_costs)";
        let floor = format!("=(({p})+{c})/(fee*weeks)");
        let ceil = format!("=(((cap-salary)-ct_adj)/(1-ct_marg)+{c})/(fee*weeks)");
        let cells = [V::Num(*rt), V::from(format!("=(forge_bal+flat_bal)*A{n}")), V::from(living), V::from(floor), V::from(ceil), V::from(format!("=E{n}-D{n}")), V::from(format!("=Lives!B35-C{n}")), V::from(format!("=(B{n}+capital)*jenny_share"))];
        g.row(S_RATE, n, &cells, &|i, _| Some(match i { 0 => FMT_PCT, 3 | 4 | 5 => FMT_1DP, _ => FMT_INT }))?;
    }
    for c in 1..=8 {
        g.m.set_column_width(S_RATE, c, W_RATE).map_err(|e| anyhow!(e))?;
    }

    // ---- 2032 (flat kept, no London work, state pension in) ----
    g.header_row(S_2032, &["Tesla price 2032 ($)", "Tesla (£)", "Non-Tesla sleeve (£)", "Chimney accumulated (£)", "Pot (£)", "Draw (£)", "Living 2032 (£)", "FLOOR you work", "CEILING you work", "LEEWAY", "Floor with no draw", "Pension covers (sessions)", "Lump to the flat (£)", "Pot after the lump (£)"])?;
    let prices: [V; 8] = [V::Num(0.0), V::from("=tsla/2"), V::from("=tsla"), V::from("=tsla*(1+tsla_g)^years"), V::Num(750.0), V::Num(1000.0), V::Num(1500.0), V::Num(2000.0)];
    let sessions = |n: i32, draw: &str| -> (String, String) {
        let ns = format!("(state_pension+{draw}+salary)");
        let taxable = format!("MAX(0,{ns}-pa)");
        let tax_ns = format!("(ns_basic*MIN({taxable},basic_band)+ns_higher*MAX({taxable}-basic_band,0))");
        let net_ns = format!("({ns}-{tax_ns})");
        let rem = format!("MAX(0,basic_band-{taxable})");
        let target = format!("MAX(0,G{n}-{net_ns})");
        let d_basic = format!("(div_allow+({target}-div_allow)/(1-div_basic))");
        let d_high = format!("(div_allow+{rem}+({target}-div_allow-{rem}*(1-div_basic))/(1-div_higher))");
        let d = format!("IF({target}<=div_allow,{target},IF({target}<=div_allow+{rem}*(1-div_basic),{d_basic},{d_high}))");
        let p = format!("IF(({d}-ct_adj)/(1-ct_marg)>50000,({d}-ct_adj)/(1-ct_marg),{d}/(1-ct_small))");
        let c = "(salary+leigh+motor+other_costs)";
        let floor = format!("=IF({target}<=0,0,(({p})+{c})/(fee*weeks))");
        let room = format!("MAX(0,cap-{ns})");
        let pcap = format!("IF(({room}-ct_adj)/(1-ct_marg)>50000,({room}-ct_adj)/(1-ct_marg),{room}/(1-ct_small))");
        let ceil = format!("=(({pcap})+{c})/(fee*weeks)");
        (floor, ceil)
    };
    for (i, p) in prices.iter().enumerate() {
        let n = i as i32 + 2;
        let (fl, ce) = sessions(n, &format!("F{n}"));
        let (fl0, _) = sessions(n, "0");
        let living = format!("=base_life+flat_running+ct_premium*premium_share+trips_if_sold+heating_if_sold+((forge_bal+(1-IF(E{n}>=clear_flat,1,0))*flat_bal)*rate+capital)*(1-jenny_share)");
        let cells = [
            p.clone(),
            V::from(format!("=A{n}*shares*fx")),
            V::from("=sjp*(1+sjp_g)^years"),
            V::from("=chimney_in*((1+sjp_g)^5-1)/sjp_g"),
            V::from(format!("=B{n}+C{n}+D{n}")),
            V::from(format!("=N{n}*draw_pct")),
            V::from(living),
            V::from(fl),
            V::from(ce),
            V::from(format!("=I{n}-H{n}")),
            V::from(fl0),
            V::from(format!("=K{n}-H{n}")),
            V::from(format!("=IF(E{n}>=clear_flat,flat_bal,0)")),
            V::from(format!("=E{n}-M{n}")),
        ];
        g.row(S_2032, n, &cells, &|i, _| Some(if (7..=11).contains(&i) { FMT_1DP } else { FMT_INT }))?;
    }
    let key: [(&str, &str); 10] = [
        ("KEY", ""),
        ("Tesla price", "A price in 2032; the shares are 1,310 × price × the £/$ rate. Rows: zero, half today's, today's, today's grown at the base-case rate, then $750 / $1,000 / $1,500 / $2,000."),
        ("Non-Tesla sleeve", "The £177k already in the SJP sleeve, grown at the sleeve rate to June 2032. The contributions that will also go into this sleeve are shown separately in the next column, so the two together are the non-Tesla sleeve in 2032."),
        ("Chimney accumulated", "Five year-end contributions (Aug 2027 to Aug 2031) of the yearly chimney figure on Assumptions, paid into the non-Tesla sleeve and each grown at the sleeve rate to 2032. The yearly figure is derived from the planning roof: sessions above the ceiling × fee × weeks, capped at the annual allowance (32 → about £60k). Nothing new goes into Tesla."),
        ("Pot", "Tesla + sleeve + chimney."),
        ("Lump to the flat / Pot after the lump", "If the pot reaches the threshold on Assumptions (default £2m), £200k clears the flat's loan and the draw is taken on what is left."),
        ("Draw", "The slider on Assumptions (default 4%) × the pot after the lump. Taxed as income together with the state pension and salary."),
        ("Living 2032", "The keep-the-flat life without commuting, plus voluntary London trips, minus Will's share of the flat's interest where the lump has cleared it."),
        ("FLOOR you work", "Sessions still needed after the state pension and the draw have paid their part. Rooms released; Leigh, motor, salary and the small lines still carried."),
        ("CEILING you work", "Sessions at which state pension + draw + salary + dividends reach the £100k line. At large pots the draw alone crosses it and the ceiling shown is just the company costs: turn the slider down to see the real room."),
    ];
    // row 10 blank, key from row 11 (matches the Python layout: 8 data rows, a blank, KEY, 10 explanations)
    let mut row = 11;
    for (k, text) in key {
        g.put(S_2032, row, 1, &V::from(k))?;
        g.put(S_2032, row, 2, &V::from(text))?;
        row += 1;
    }
    g.put(S_2032, row, 1, &V::from("Floor with no draw / Pension covers"))?;
    g.put(S_2032, row, 2, &V::from("The floor if nothing is drawn, and the difference between that and the floor you work, i.e. how many sessions the pension is paying for."))?;
    g.m.set_column_width(S_2032, 1, W_LIVES).map_err(|e| anyhow!(e))?;
    for c in 2..=14 {
        g.m.set_column_width(S_2032, c, W_RATE).map_err(|e| anyhow!(e))?;
    }

    g.m.evaluate();
    Ok(g.m)
}

/// Build, refuse on any error cell, and write the workbook.
pub fn generate(spec_path: &str, out: &str) -> Result<usize> {
    let spec = load_spec(spec_path)?;
    let model = build(&spec)?;
    let book = crate::book::Book::from_model(model, out);
    let mut errors = vec![];
    for p in book.formula_cells() {
        if book.is_error(p) {
            errors.push(format!("{} = {}", book.ref_string(p), book.formula(p).unwrap_or_default()));
        }
    }
    if !errors.is_empty() {
        bail!("refusing to write: {} formula cell(s) evaluate to an error:\n  {}", errors.len(), errors.join("\n  "));
    }
    let n = book.formula_cells().len();
    book.save_as(out)?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINI: &str = r#"
[meta]
title = "t"
date = "2026-01-01"
[[assumption]]
header = "H"
note = "n"
[[assumption]]
key = "fee"
label = "Fee"
value = 100
note = ""
[[book]]
day = "Monday"
sessions = 1
remote_always = 1
note = ""
"#;

    #[test]
    fn spec_validation_rejects_bad_keys_and_shapes() {
        let ok: Spec = toml::from_str(MINI).unwrap();
        assert_eq!(ok.assumptions.len(), 2);
        let bad = MINI.replace("key = \"fee\"", "key = \"3fee\"");
        let dir = std::env::temp_dir().join(format!("icalc-gen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bad.toml");
        std::fs::write(&p, bad).unwrap();
        let e = load_spec(p.to_str().unwrap()).unwrap_err().to_string();
        assert!(e.contains("identifier"), "{e}");
        let shape = MINI.replace("label = \"Fee\"\n", "");
        std::fs::write(&p, shape).unwrap();
        let e = load_spec(p.to_str().unwrap()).unwrap_err().to_string();
        assert!(e.contains("needs either"), "{e}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_refuses_a_spec_missing_required_keys() {
        let spec: Spec = toml::from_str(MINI).unwrap();
        let e = match build(&spec) { Err(e) => e.to_string(), Ok(_) => panic!("built without required keys") };
        assert!(e.contains("missing assumption key"), "{e}");
    }

    #[test]
    fn row_formats_follow_the_label_rule() {
        assert_eq!(row_fmt("Mortgage rate"), FMT_PCT);
        assert_eq!(row_fmt("FLOOR (sessions per week)"), FMT_1DP);
        assert_eq!(row_fmt("Solvency margin (roof − floor)"), FMT_1DP);
        assert_eq!(row_fmt("Will's living, net"), FMT_INT);
    }
}
