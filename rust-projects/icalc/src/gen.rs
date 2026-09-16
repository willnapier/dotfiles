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
const S_THR: u32 = 6;
const SHEETS: [&str; 7] = ["Read me", "Assumptions", "Lives", "Rate sensitivity", "Book", "2032", "Thresholds"];

/// Column widths in pixels (the Python generator's cm widths at ~38 px/cm).
const W_LABEL: f64 = 435.0; // 11.5cm
const W_VALUE: f64 = 144.0; // 3.8cm
const W_NOTE: f64 = 605.0; // 16cm
const W_LIVES: f64 = 197.0; // 5.2cm
const W_BOOK_LABEL: f64 = 720.0; // wide enough for the longest Book label in full
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

// ---- Tax engine expressions (IronCalc syntax, names from Assumptions) ----
// Non-savings income (salary, pension, state pension) is taxed first; dividends
// stack on top. The £100k taper, the £125,140 additional-rate line and the
// £50k corporation-tax step are structural constants of the tax system and are
// written as literals here, as they are on Lives.

/// Personal allowance after the taper, for total gross income `total`.
fn pa_after_taper(total: &str) -> String {
    format!("MAX(0,pa-MAX(0,{total}-100000)/2)")
}

/// Income tax on taxable non-savings income `t`: basic, higher, additional above £125,140.
fn ns_tax(t: &str) -> String {
    format!("(ns_basic*MIN({t},basic_band)+ns_higher*MIN(MAX(0,{t}-basic_band),125140-basic_band)+ns_addl*MAX(0,{t}-125140))")
}

/// Dividends taxed in the basic band, for taxable dividends `td` above taxable
/// non-savings `tns`. The £500 allowance is a nil rate that still occupies band
/// space (the treatment that reproduces the verified £99,200 → £21,491).
fn div_basic_band(td: &str, tns: &str) -> String {
    format!("MIN(MAX(0,{td}-div_allow),MAX(0,basic_band-{tns}-MIN({td},div_allow)))")
}

/// Dividends taxed in the higher band, given the basic-band slice `db`.
fn div_higher_band(td: &str, tns: &str, db: &str) -> String {
    format!("MIN(MAX(0,{td}-div_allow)-{db},MAX(0,125140-MAX(basic_band,{tns}+MIN({td},div_allow)+{db})))")
}

/// Net in hand for non-savings income `ns` with dividends `dv` stacked on top,
/// as one expression: the same rules as the visible engine columns on Thresholds.
fn net_in_hand(ns: &str, dv: &str) -> String {
    let total = format!("({ns}+{dv})");
    let pa2 = pa_after_taper(&total);
    let tns = format!("MAX(0,{ns}-{pa2})");
    let td = format!("MAX(0,{dv}-MAX(0,{pa2}-{ns}))");
    let db = div_basic_band(&td, &tns);
    let dh = div_higher_band(&td, &tns, &db);
    let da = format!("(MAX(0,{td}-div_allow)-{db}-{dh})");
    format!("({total}-{}-(div_basic*{db}+div_higher*{dh}+div_addl*{da}))", ns_tax(&tns))
}

/// Gross non-savings income that leaves net `n` in hand when nothing else is
/// taxed: the exact piecewise inverse of `ns_tax` with the taper (marginal 60%
/// between £100k and £125,140) and the additional rate above.
fn earnings_gross_for_net(n: &str) -> String {
    let n2 = "(pa+(1-ns_basic)*basic_band)";
    let n3 = "(100000-ns_basic*basic_band-ns_higher*(100000-pa-basic_band))";
    let n4 = "(125140-ns_basic*basic_band-ns_higher*(125140-basic_band))";
    format!("IF({n}<=pa,{n},IF({n}<={n2},pa+({n}-pa)/(1-ns_basic),IF({n}<={n3},pa+basic_band+({n}-{n2})/(1-ns_higher),IF({n}<={n4},100000+({n}-{n3})/(1-1.5*ns_higher),125140+({n}-{n4})/(1-ns_addl)))))")
}

/// Sessions needed (floor) and the ceiling when a pension `draw` and state
/// pension `sp` arrive alongside the salary, the living target is `living`, and
/// the company costs are `costs`. Shared by the 2032 and Thresholds sheets.
fn floor_and_ceiling(living: &str, draw: &str, sp: &str, costs: &str) -> (String, String) {
    let ns = format!("({sp}+{draw}+salary)");
    let taxable = format!("MAX(0,{ns}-pa)");
    let tax_ns = format!("(ns_basic*MIN({taxable},basic_band)+ns_higher*MAX({taxable}-basic_band,0))");
    let net_ns = format!("({ns}-{tax_ns})");
    let rem = format!("MAX(0,basic_band-{taxable})");
    let target = format!("MAX(0,{living}-{net_ns})");
    let d_basic = format!("(div_allow+({target}-div_allow)/(1-div_basic))");
    let d_high = format!("(div_allow+{rem}+({target}-div_allow-{rem}*(1-div_basic))/(1-div_higher))");
    let d = format!("IF({target}<=div_allow,{target},IF({target}<=div_allow+{rem}*(1-div_basic),{d_basic},{d_high}))");
    let p = format!("IF(({d}-ct_adj)/(1-ct_marg)>50000,({d}-ct_adj)/(1-ct_marg),{d}/(1-ct_small))");
    let floor = format!("=IF({target}<=0,0,(({p})+{costs})/(fee*weeks))");
    let room = format!("MAX(0,cap-{ns})");
    let pcap = format!("IF(({room}-ct_adj)/(1-ct_marg)>50000,({room}-ct_adj)/(1-ct_marg),{room}/(1-ct_small))");
    let ceil = format!("=(({pcap})+{costs})/(fee*weeks)");
    (floor, ceil)
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
    for k in ["fee","weeks","remote_weeks","remote_frac","roof","salary","rent_3days","leigh","motor","other_costs","pension_allowance","pa","basic_band","div_allow","div_basic","div_higher","div_addl","cap","ct_marg","ct_adj","ct_small","ns_basic","ns_higher","ns_addl","base_life","flat_running","ct_premium","premium_share","london_transport","london_food","london_cash","trips_if_sold","heating_if_sold","nights","room","train","parking","forge_bal","flat_bal","rate","rate_now","jenny_share","capital","proceeds","shares","tsla","fx","sjp","sjp_g","tsla_g","years","chimney_in","draw_pct","state_pension","clear_flat","lsa","keep_sessions","target_lo","target_hi","draw_rate_safe","draw_rate_high","years_h","chimney_first","sp_years","thr_price_sold","thr_price_kept","lump_flat","lump_disc","side_door_cash","side_door_from","side_door_years","mpaa"] {
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
        "Thresholds: what the pot must reach for the practice to become optional — row by row (the ceiling, twenty sessions, no work; flat kept or sold), the draw needed, the pot at three draw rates, and the Tesla price that reaches it at the horizon; then the sessions still needed at a given Tesla price.".into(),
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
        [V::from("Average over the earning weeks (not the calendar year), remote-only weeks at their reduced strength"), V::from("=(B8*(weeks-remote_weeks)+B8*remote_frac*remote_weeks)/weeks"), V::Empty, V::from("The four remote weeks cost about one session a week on the average")],
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
    for (c, w) in [(1, W_BOOK_LABEL), (2, W_LIVES), (3, W_LIVES), (4, W_NOTE)] {
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
    push("Dividends in the basic band", each(&|c| format!("={}", div_basic_band(&format!("{c}53"), &format!("{c}52"))))); // 54
    push("Dividends in the higher band", each(&|c| format!("={}", div_higher_band(&format!("{c}53"), &format!("{c}52"), &format!("{c}54"))))); // 55
    push("Dividends at the additional rate", each(&|c| format!("=MAX(0,{c}53-div_allow)-{c}54-{c}55"))); // 56
    push("Personal tax", each(&|c| format!("=ns_basic*{c}52+div_basic*{c}54+div_higher*{c}55+div_addl*{c}56"))); // 57
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
    let sessions = |n: i32, draw: &str| -> (String, String) { floor_and_ceiling(&format!("G{n}"), draw, "state_pension", "(salary+leigh+motor+other_costs)") };
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

    // ---- Thresholds (16 Sep 2026: what the pot must reach for the practice to become optional) ----
    build_thresholds(&mut g)?;

    g.m.evaluate();
    Ok(g.m)
}

/// Ordered columns of one Thresholds block: a stable key per column so the
/// formulas name columns (`[living]`) instead of letters, and the header text.
struct Cols {
    keys: Vec<&'static str>,
    heads: Vec<&'static str>,
    letters: HashMap<&'static str, String>,
}

impl Cols {
    fn new(spec: &[(&'static str, &'static str)]) -> Result<Cols> {
        let mut letters = HashMap::new();
        for (i, (k, _)) in spec.iter().enumerate() {
            if letters.insert(*k, crate::book::index_to_col(i as i32 + 1)).is_some() {
                bail!("duplicate Thresholds column key '{k}'");
            }
        }
        Ok(Cols { keys: spec.iter().map(|(k, _)| *k).collect(), heads: spec.iter().map(|(_, h)| *h).collect(), letters })
    }

    /// `[key]` → `LETTERn` for row `n`. Refuses an unknown key (a stray `[`).
    fn sub(&self, template: &str, n: i32) -> Result<String> {
        let mut s = template.to_string();
        for (k, l) in &self.letters {
            s = s.replace(&format!("[{k}]"), &format!("{l}{n}"));
        }
        if s.contains('[') {
            bail!("Thresholds formula still has an unresolved column key: {s}");
        }
        Ok(s)
    }

    /// A cell reference into another block's row.
    fn at(&self, key: &str, n: i32) -> Result<String> {
        self.letters.get(key).map(|l| format!("{l}{n}")).with_context(|| format!("no Thresholds column '{key}'"))
    }
}

/// The pension income that lifts net in hand to `living` (or reaches the
/// target), given the practice position: `short` = shortfall cell, `ns0` =
/// non-savings income before any draw, `divs` = dividends. Exact for pension
/// alone (the earnings inverse); shortfall ÷ marginal keep-rate when dividends
/// are stacked, where a pension pound is taxed at its own rate and pushes a
/// dividend pound up a band.
fn draw_needed(living: &str, short: &str, ns0: &str, divs: &str) -> String {
    let tns0 = format!("MAX(0,{ns0}-pa)");
    let keep = format!("(1-IF({tns0}<basic_band,ns_basic+IF(MAX(0,{divs}-div_allow)>basic_band-{tns0},div_higher-div_basic,0),ns_higher))");
    format!(
        "IF([target]>0,MAX(0,[target]-[gross]-[sp]),IF({short}<=0,0,IF({divs}=0,MAX(0,{}-{ns0}),{short}/{keep})))",
        earnings_gross_for_net(living)
    )
}

const MPAA_TEXT: &str = "\"tripped: chimney capped at the MPAA\",\"not tripped\"";

/// Block A (rows 2–8): sessions given → the draw that fills the gap to the
/// living or to an income target → the pot at three draw rates → the Tesla
/// price that reaches it at the horizon; then the £2m lump rule (both branches)
/// and the MPAA flag. Block B (rows 11–12): a Tesla price given → the pot → the
/// sessions still needed, lump rule applied. Block C (rows 15–16): the tax-free
/// side door. KEY below. Everything reads the Assumptions names and Lives rows
/// 25 (living) and 33 (ceiling), so the table moves with the inputs.
fn build_thresholds(g: &mut Gen) -> Result<()> {
    // ---- Block A ----
    let a = Cols::new(&[
        ("scenario", "Scenario"), ("flat", "Flat kept (1/0)"), ("days", "London clinic days"), ("sessions", "Sessions per week"), ("roof", "Roof worked until the horizon (1/0)"), ("target", "Target gross income (£; 0 = fund the living only)"),
        ("living", "Living, net (£)"), ("gross", "Practice gross: salary + dividends (£)"), ("net_alone", "Net in hand from the practice alone (£)"), ("short", "Shortfall against the living (£)"), ("draw", "Taxable draw needed (£/yr)"), ("net", "Net in hand with the draw (£)"), ("spare", "Spare over the living (£)"),
        ("pot_s", "Pot at the safe rate (£)"), ("pot_m", "Pot at the slider rate (£)"), ("pot_h", "Pot at the high rate (£)"), ("tsla_s", "Tesla at the horizon, safe rate ($)"), ("tsla_m", "Tesla, slider rate ($)"), ("tsla_h", "Tesla, high rate ($)"),
        ("fire_s", "£2m lump rule fires at the safe rate (1/0)"), ("fire_m", "…at the slider rate (1/0)"), ("fire_h", "…at the high rate (1/0)"), ("living_l", "Living after the lump (£)"), ("draw_l", "Taxable draw if the lump is taken (£/yr)"), ("net_l", "Net in hand if the lump is taken (£)"), ("spare_l", "Spare over the living if the lump is taken (£)"),
        ("potl_s", "Pot with the rule, safe rate (£, incl. the lump where it fires)"), ("potl_m", "Pot with the rule, slider rate (£)"), ("potl_h", "Pot with the rule, high rate (£)"), ("tslal_s", "Tesla with the rule, safe rate ($)"), ("tslal_m", "Tesla with the rule, slider rate ($)"), ("tslal_h", "Tesla with the rule, high rate ($)"),
        ("mpaa", "MPAA"), ("chimney_t", "Chimney once tripped (£/yr)"),
        ("costs", "Company costs (£)"), ("revenue", "Revenue (£)"), ("ceiling", "Ceiling at this cost base (sessions)"), ("chimney", "Chimney to the pension (£/yr)"), ("profit", "Pre-tax profit (£)"), ("ct", "Corporation tax (£)"), ("divs", "Dividends (£)"), ("sp", "State pension at the horizon (£)"),
        ("ns", "Non-savings income with the draw (£)"), ("total", "Total gross income (£)"), ("pa2", "Personal allowance after the taper (£)"), ("tns", "Taxable non-savings income (£)"), ("nstax", "Income tax on it (£)"), ("td", "Taxable dividends (£)"), ("db", "Dividends in the basic band (£)"), ("dh", "Dividends in the higher band (£)"), ("da", "Dividends at the additional rate (£)"), ("dtax", "Dividend tax (£)"),
        ("sleeve", "Sleeve at the horizon (£)"), ("chim_acc", "Chimney accumulated at the horizon (£)"),
    ])?;
    g.header_row(S_THR, &a.heads)?;
    // label, flat kept, London days, sessions, roof worked to the horizon, target gross income
    let rows_a: [(&str, i32, i32, &str, i32, &str); 7] = [
        ("1. Now: the ceiling, flat kept, two London days", 1, 2, "=Lives!$B$33", 1, "0"),
        ("2. Twenty, enough to live: the taxable draw that closes the shortfall", 1, 2, "=keep_sessions", 0, "0"),
        ("3. Twenty + top-up to the lower income target", 1, 2, "=keep_sessions", 0, "=target_lo"),
        ("4. Twenty + the upper income target", 1, 2, "=keep_sessions", 0, "=target_hi"),
        ("5. No work, flat kept for visits: fund the living (≈ the £100k line)", 1, 0, "0", 1, "0"),
        ("6. No work, upper target, flat kept for visits", 1, 0, "0", 1, "=target_hi"),
        ("7. No work, Somerset only, flat sold", 0, 0, "0", 1, "0"),
    ];
    let ns0 = "(IF([sessions]>0,salary,0)+[sp])";
    let living_cut = "[flat]*flat_bal*rate*(1-jenny_share)";
    let chim = "MIN(MAX(0,[sessions]-[ceiling])*fee*weeks,pension_allowance)";
    let short_l = "MAX(0,[living_l]-[net_alone])";
    let one_dp: [&str; 2] = ["sessions", "ceiling"];
    for (i, (label, flat, days, sessions, roof, target)) in rows_a.iter().enumerate() {
        let n = i as i32 + 2;
        let num = |s: &str| if let Ok(x) = s.parse::<f64>() { V::Num(x) } else { V::from(s) };
        let mut cells: Vec<V> = vec![V::from(*label), V::Num(*flat as f64), V::Num(*days as f64), num(sessions), V::Num(*roof as f64), num(target)];
        let f: Vec<(&str, String)> = vec![
            ("living", "=IF([flat]=1,Lives!$B$25,Lives!$D$25)".into()),
            ("gross", "=IF([sessions]>0,salary+[divs],0)".into()),
            ("net_alone", format!("={}", net_in_hand(ns0, "[divs]"))),
            ("short", "=MAX(0,[living]-[net_alone])".into()),
            ("draw", format!("={}", draw_needed("[living]", "[short]", ns0, "[divs]"))),
            ("net", "=[total]-[nstax]-[dtax]".into()),
            ("spare", "=[net]-[living]".into()),
            ("pot_s", "=[draw]/draw_rate_safe".into()),
            ("pot_m", "=[draw]/draw_pct".into()),
            ("pot_h", "=[draw]/draw_rate_high".into()),
            ("tsla_s", "=MAX(0,[pot_s]-[sleeve]-[chim_acc])/(shares*fx)".into()),
            ("tsla_m", "=MAX(0,[pot_m]-[sleeve]-[chim_acc])/(shares*fx)".into()),
            ("tsla_h", "=MAX(0,[pot_h]-[sleeve]-[chim_acc])/(shares*fx)".into()),
            ("fire_s", "=IF(AND([flat]=1,[pot_s]>=clear_flat),1,0)".into()),
            ("fire_m", "=IF(AND([flat]=1,[pot_m]>=clear_flat),1,0)".into()),
            ("fire_h", "=IF(AND([flat]=1,[pot_h]>=clear_flat),1,0)".into()),
            ("living_l", format!("=[living]-{living_cut}")),
            ("draw_l", format!("={}", draw_needed("[living_l]", short_l, ns0, "[divs]"))),
            ("net_l", format!("={}", net_in_hand(&format!("({ns0}+[draw_l])"), "[divs]"))),
            ("spare_l", "=[net_l]-[living_l]".into()),
            ("potl_s", "=IF([fire_s]=1,[draw_l]/draw_rate_safe+lump_flat,[pot_s])".into()),
            ("potl_m", "=IF([fire_m]=1,[draw_l]/draw_pct+lump_flat,[pot_m])".into()),
            ("potl_h", "=IF([fire_h]=1,[draw_l]/draw_rate_high+lump_flat,[pot_h])".into()),
            ("tslal_s", "=MAX(0,[potl_s]-[sleeve]-[chim_acc])/(shares*fx)".into()),
            ("tslal_m", "=MAX(0,[potl_m]-[sleeve]-[chim_acc])/(shares*fx)".into()),
            ("tslal_h", "=MAX(0,[potl_h]-[sleeve]-[chim_acc])/(shares*fx)".into()),
            ("mpaa", format!("=IF([draw]>0,{MPAA_TEXT})")),
            ("chimney_t", "=IF([draw]>0,MIN([chimney],mpaa),[chimney])".into()),
            ("costs", "=IF([sessions]>0,rent_3days*[days]/3+salary+leigh+motor+other_costs,0)".into()),
            ("revenue", "=[sessions]*fee*weeks".into()),
            ("ceiling", "=IF([sessions]>0,(((cap-salary)-ct_adj)/(1-ct_marg)+[costs])/(fee*weeks),0)".into()),
            ("chimney", format!("={chim}")),
            ("profit", "=[revenue]-[costs]-[chimney]".into()),
            ("ct", "=IF([profit]>50000,ct_marg*[profit]-ct_adj,ct_small*MAX(0,[profit]))".into()),
            ("divs", "=IF([sessions]>0,[profit]-[ct],0)".into()),
            ("sp", "=IF(years_h>=sp_years,state_pension,0)".into()),
            ("ns", "=IF([sessions]>0,salary,0)+[sp]+[draw]".into()),
            ("total", "=[gross]+[sp]+[draw]".into()),
            ("pa2", format!("={}", pa_after_taper("[total]"))),
            ("tns", "=MAX(0,[ns]-[pa2])".into()),
            ("nstax", format!("={}", ns_tax("[tns]"))),
            ("td", "=MAX(0,[divs]-MAX(0,[pa2]-[ns]))".into()),
            ("db", format!("={}", div_basic_band("[td]", "[tns]"))),
            ("dh", format!("={}", div_higher_band("[td]", "[tns]", "[db]"))),
            ("da", "=MAX(0,[td]-div_allow)-[db]-[dh]".into()),
            ("dtax", "=div_basic*[db]+div_higher*[dh]+div_addl*[da]".into()),
            ("sleeve", "=sjp*(1+sjp_g)^years_h".into()),
            ("chim_acc", "=[roof]*chimney_in*((1+sjp_g)^MAX(0,INT(years_h-chimney_first)+1)-1)/sjp_g".into()),
        ];
        for (j, (k, formula)) in f.iter().enumerate() {
            if a.keys[cells.len()] != *k {
                bail!("Thresholds block A column {} is '{}' but formula {} is for '{k}'", cells.len() + 1, a.keys[cells.len()], j);
            }
            cells.push(V::from(a.sub(formula, n)?));
        }
        if cells.len() != a.keys.len() {
            bail!("Thresholds row {n} has {} cells for {} columns", cells.len(), a.keys.len());
        }
        let keys = &a.keys;
        g.row(S_THR, n, &cells, &|i, _| Some(if one_dp.contains(&keys[i]) { FMT_1DP } else { FMT_INT }))?;
    }

    // ---- Block B: a Tesla price given → the sessions still needed ----
    let b = Cols::new(&[
        ("scenario", "Scenario"), ("flat", "Flat kept (1/0)"), ("days", "London clinic days"), ("roof", "Roof worked until the horizon (1/0)"), ("price", "Tesla price at the horizon ($)"), ("living", "Living, net (£)"),
        ("sess_s", "Sessions needed, safe rate"), ("sess_m", "Sessions needed, slider rate"), ("sess_h", "Sessions needed, high rate"),
        ("fire", "£2m lump rule fires (1/0)"), ("living_l", "Living with the rule (£)"), ("pot_l", "Pot with the rule (£, after the lump where it fires)"), ("sessl_s", "Sessions with the rule, safe rate"), ("sessl_m", "Sessions with the rule, slider rate"), ("sessl_h", "Sessions with the rule, high rate"), ("mpaa", "MPAA"),
        ("tsla_gbp", "Tesla (£)"), ("sleeve", "Sleeve at the horizon (£)"), ("chim_acc", "Chimney accumulated at the horizon (£)"), ("pot", "Pot (£)"), ("draw_m", "Draw at the slider rate (£)"), ("sp", "State pension at the horizon (£)"), ("costs", "Company costs, no rooms (£)"), ("ceiling", "Ceiling at this cost base (sessions)"),
    ])?;
    for (i, t) in b.heads.iter().enumerate() {
        g.put(S_THR, 10, i as i32 + 1, &V::from(*t))?;
        g.style(S_THR, 10, i as i32 + 1, &style(None, true, true))?;
    }
    let rows_b: [(&str, i32, i32, i32, &str); 2] = [
        ("8. Worst case: flat sold, Somerset work only, Tesla at the worst-case price", 0, 0, 1, "=thr_price_sold"),
        ("9. Flat kept for visits, Somerset work only, Tesla at the kept price", 1, 0, 1, "=thr_price_kept"),
    ];
    let one_dp_b: [&str; 7] = ["sess_s", "sess_m", "sess_h", "sessl_s", "sessl_m", "sessl_h", "ceiling"];
    for (i, (label, flat, days, roof, price)) in rows_b.iter().enumerate() {
        let n = i as i32 + 11;
        let sess = |living: &str, pot: &str, rate: &str| floor_and_ceiling(living, &format!("({pot}*{rate})"), "[sp]", "[costs]");
        let mut cells: Vec<V> = vec![V::from(*label), V::Num(*flat as f64), V::Num(*days as f64), V::Num(*roof as f64), V::from(*price)];
        let f: Vec<(&str, String)> = vec![
            ("living", "=IF([flat]=1,Lives!$B$25,Lives!$D$25)".into()),
            ("sess_s", sess("[living]", "[pot]", "draw_rate_safe").0),
            ("sess_m", sess("[living]", "[pot]", "draw_pct").0),
            ("sess_h", sess("[living]", "[pot]", "draw_rate_high").0),
            ("fire", "=IF(AND([flat]=1,[pot]>=clear_flat),1,0)".into()),
            ("living_l", "=[living]-[fire]*flat_bal*rate*(1-jenny_share)".into()),
            ("pot_l", "=[pot]-[fire]*lump_flat".into()),
            ("sessl_s", sess("[living_l]", "[pot_l]", "draw_rate_safe").0),
            ("sessl_m", sess("[living_l]", "[pot_l]", "draw_pct").0),
            ("sessl_h", sess("[living_l]", "[pot_l]", "draw_rate_high").0),
            ("mpaa", format!("=IF([draw_m]>0,{MPAA_TEXT})")),
            ("tsla_gbp", "=[price]*shares*fx".into()),
            ("sleeve", "=sjp*(1+sjp_g)^years_h".into()),
            ("chim_acc", "=[roof]*chimney_in*((1+sjp_g)^MAX(0,INT(years_h-chimney_first)+1)-1)/sjp_g".into()),
            ("pot", "=[tsla_gbp]+[sleeve]+[chim_acc]".into()),
            ("draw_m", "=[pot]*draw_pct".into()),
            ("sp", "=IF(years_h>=sp_years,state_pension,0)".into()),
            ("costs", "=rent_3days*[days]/3+salary+leigh+motor+other_costs".into()),
            ("ceiling", sess("[living]", "[pot]", "draw_pct").1),
        ];
        for (k, formula) in &f {
            if b.keys[cells.len()] != *k {
                bail!("Thresholds block B column {} is '{}' but the formula is for '{k}'", cells.len() + 1, b.keys[cells.len()]);
            }
            cells.push(V::from(b.sub(formula, n)?));
        }
        if cells.len() != b.keys.len() {
            bail!("Thresholds row {n} has {} cells for {} columns", cells.len(), b.keys.len());
        }
        let keys = &b.keys;
        g.row(S_THR, n, &cells, &|i, _| Some(if one_dp_b.contains(&keys[i]) { FMT_1DP } else { FMT_INT }))?;
    }

    // ---- Block C: the tax-free side door (row 2b), from the remortgage and from now ----
    let c = Cols::new(&[
        ("scenario", "Scenario"), ("sessions", "Sessions per week"), ("from", "Side door starts (years from the model date)"), ("years", "Years of tax-free cash to the state pension"), ("cash", "Tax-free cash per year (£)"),
        ("living", "Living, net (£)"), ("net_alone", "Net in hand from the practice alone (£)"), ("short", "Shortfall against the living (£)"), ("net", "Net in hand with the cash (£)"), ("spare", "Spare over the living (£)"),
        ("consumed", "LSA consumed (£)"), ("remaining", "LSA remaining (£)"), ("vs_flat", "LSA remaining vs the flat slice"), ("disc_left", "Discretionary tax-free cash left after the side door (£)"), ("cryst", "Crystallised per year (£; 25% tax-free, 75% to drawdown untouched)"), ("pot_lsa", "Pot needed later for the full LSA (£)"),
        ("draw", "Taxable draw (£/yr)"), ("mpaa", "MPAA"), ("pots", "Pot needed"),
    ])?;
    for (i, t) in c.heads.iter().enumerate() {
        g.put(S_THR, 14, i as i32 + 1, &V::from(*t))?;
        g.style(S_THR, 14, i as i32 + 1, &style(None, true, true))?;
    }
    let twenty = 3; // block A row 2 ("Twenty, enough to live")
    let rows_c: [(&str, &str); 2] = [
        ("2b. Twenty + the side door from the remortgage (June 2027)", "=side_door_from"),
        ("2c. Twenty + the side door from now", "0"),
    ];
    for (i, (label, from)) in rows_c.iter().enumerate() {
        let n = i as i32 + 15;
        let mut cells: Vec<V> = vec![V::from(*label), V::from(format!("={}", a.at("sessions", twenty)?))];
        let f: Vec<(&str, String)> = vec![
            ("from", if *from == "0" { "0".into() } else { from.to_string() }),
            ("years", "=MAX(0,sp_years-[from])".into()),
            ("cash", "=side_door_cash".into()),
            ("living", format!("={}", a.at("living", twenty)?)),
            ("net_alone", format!("={}", a.at("net_alone", twenty)?)),
            ("short", format!("={}", a.at("short", twenty)?)),
            ("net", "=[net_alone]+[cash]".into()),
            ("spare", "=[net]-[living]".into()),
            ("consumed", "=[cash]*[years]".into()),
            ("remaining", "=lsa-[consumed]".into()),
            ("vs_flat", "=IF([remaining]>=lump_flat,\"OK: still covers the flat slice\",\"SHORT of the flat slice\")".into()),
            ("disc_left", "=lump_disc-[consumed]".into()),
            ("cryst", "=[cash]/0.25".into()),
            ("pot_lsa", "=lsa/0.25".into()),
            ("draw", "0".into()),
            ("mpaa", "not tripped (tax-free cash by partial crystallisation)".into()),
            ("pots", "any pot suffices".into()),
        ];
        for (k, formula) in &f {
            if c.keys[cells.len()] != *k {
                bail!("Thresholds block C column {} is '{}' but the formula is for '{k}'", cells.len() + 1, c.keys[cells.len()]);
            }
            let v = if formula == "0" { V::Num(0.0) } else { V::from(c.sub(formula, n)?) };
            cells.push(v);
        }
        if cells.len() != c.keys.len() {
            bail!("Thresholds row {n} has {} cells for {} columns", cells.len(), c.keys.len());
        }
        let keys = &c.keys;
        g.row(S_THR, n, &cells, &|i, _| Some(match keys[i] { "sessions" => FMT_1DP, "from" | "years" => "0.00", _ => FMT_INT }))?;
    }

    let key: [(&str, &str); 16] = [
        ("KEY", ""),
        ("Rows 1–7", "Sessions given. The living is Lives row 25: keep-the-flat (column B, which carries the London lines even with no clinic, as the visits) or Somerset-only with the flat sold (column D). Company costs are rooms scaled by London days plus salary, Leigh, motor and the small lines; nothing when no sessions are worked."),
        ("Taxable draw needed", "With a target: target − practice gross − state pension. Without: the pension income that lifts net in hand to the living — exact for pension alone (the piecewise inverse of income tax with the taper), and shortfall ÷ marginal keep-rate when dividends are stacked (a pension pound is taxed at its own rate and pushes a dividend pound up a band). 'Spare over the living' shows the residual either way."),
        ("Tax", "Pension income is taxed as earnings on top of the salary: personal allowance tapered above £100k (gone at £125,140), basic / higher / additional rates; dividends stack on top at their three rates, the £500 allowance occupying band space. Corporation tax at the small rate below £50k, else the marginal rate less the adjustment."),
        ("Chimney", "Zero when the sessions worked are below the ceiling at that cost base (the twenty rows). Where the roof is worked until the horizon (1 in column E), the yearly chimney contribution from Assumptions is paid once a year from the first contribution to the horizon and grown at the sleeve rate, as on 2032."),
        ("Pot", "Draw needed ÷ draw rate, at the safe rate, the slider (draw_pct) and the high rate. Zero means the pot is untouched."),
        ("Tesla at the horizon", "The price at which Tesla (shares × price × £/$) plus the sleeve and chimney at the horizon equals the pot; zero when the sleeve and chimney alone reach it."),
        ("£2m lump rule", "Where the flat is kept and the pot needed at a rate reaches clear_flat, lump_flat of tax-free cash clears the flat's loan: the living falls by Will's share of the flat's interest, the draw is recomputed on the reduced living (rows funding the living) or kept (rows with a target), and the pot must include the lump. Both branches are shown: 'fires' per rate, the 'if the lump is taken' columns unconditionally, and 'with the rule' picking the branch per rate. A row with the flat sold never fires."),
        ("MPAA", "The first taxable draw trips the money-purchase annual allowance: 'Chimney once tripped' caps the yearly chimney at mpaa. It is shown beside the profit chimney rather than fed into it, so the sheet has no circular reference; in the rows shown the chimney is already shut below the ceiling. Tax-free cash by partial crystallisation does not trip it; an UFPLS would."),
        ("Rows 8–9", "A Tesla price given: the pot it makes, the draw at each rate, and the sessions still needed (the 2032 sheet's floor logic, no rooms, state pension in only past Aug 2032); then the same with the lump rule applied where the pot reaches clear_flat."),
        ("Row 2b / 2c", "The side door: the twenty-session shortfall covered by side_door_cash of tax-free cash a year, untaxed and outside the £100k line, from side_door_from (June 2027) or from now, until the state pension replaces it. LSA consumed comes out of the discretionary slice (lump_disc), so 'LSA remaining vs the flat slice' must stay OK for the flat's lump to remain available; the pot must later reach lsa ÷ 25% for the full LSA. Any pot suffices for the side door itself."),
        ("Horizon", "years_h on Assumptions: 3.25 = end-2029; 5.75 = June 2032. The state pension is added only when the horizon reaches sp_years (Aug 2032)."),
        ("Not modelled", "The lump as income (spending the flat slice as tax-free income instead of capital — open for Tom Wood); PCLS recycling; the foregone growth on tax-free cash taken early."),
        ("Source", "Scenario — The Pot Thresholds (16 September 2026), Table 1 and row 2b; WILLIAM-FINANCIAL-PLANNING-CONTEXT.md § 16 Sep 2026."),
        ("Read it with", "icalc house --sheet thresholds (block A columns A–S are the table, T–AH the lump rule and MPAA, AI onwards the working)."),
        ("Cells", "Block A rows 2–8, block B rows 11–12, block C rows 15–16; this key from row 18."),
    ];
    let mut row = 18;
    for (k, text) in key {
        g.put(S_THR, row, 1, &V::from(k))?;
        g.put(S_THR, row, 2, &V::from(text))?;
        row += 1;
    }
    g.m.set_column_width(S_THR, 1, W_LABEL).map_err(|e| anyhow!(e))?;
    for col in 2..=(a.keys.len() as i32) {
        g.m.set_column_width(S_THR, col, W_RATE).map_err(|e| anyhow!(e))?;
    }
    Ok(())
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

    /// A model with the 2026/27 tax constants as names, so the engine
    /// expressions can be evaluated on their own.
    fn tax_model() -> crate::book::Book {
        let mut m = Model::new_empty("tax", "en", "UTC", "en").unwrap();
        let consts = [("pa", 12570.0), ("basic_band", 37700.0), ("ns_basic", 0.2), ("ns_higher", 0.4), ("ns_addl", 0.45), ("div_allow", 500.0), ("div_basic", 0.1075), ("div_higher", 0.3575), ("div_addl", 0.3935)];
        for (i, (k, v)) in consts.iter().enumerate() {
            let row = i as i32 + 1;
            m.update_cell_with_number(0, row, 2, *v).unwrap();
            m.new_defined_name(k, None, &format!("Sheet1!$B${row}")).unwrap();
        }
        crate::book::Book::from_model(m, "tax")
    }

    fn eval(book: &mut crate::book::Book, row: i32, formula: &str) -> f64 {
        book.model.update_cell_with_formula(0, row, 4, format!("={formula}")).unwrap();
        book.evaluate();
        book.number(crate::book::Pos { sheet: 0, row, col: 4 }).unwrap_or_else(|| panic!("row {row} is not a number: {}", book.formatted(crate::book::Pos { sheet: 0, row, col: 4 })))
    }

    #[test]
    fn net_in_hand_reproduces_the_verified_draw_at_the_cap() {
        // £12k salary + £87,200 dividends → £21,491 tax (verified 3 Aug 2026).
        let mut b = tax_model();
        let net = eval(&mut b, 20, &net_in_hand("12000", "87200"));
        assert!((net - (99200.0 - 21491.0)).abs() < 1.0, "net {net}");
        // Pension alone at £100k: PA intact at exactly the line.
        let net = eval(&mut b, 21, &net_in_hand("100000", "0"));
        assert!((net - 72568.0).abs() < 1.0, "net {net}");
        // £150k of pension: PA gone, additional rate above £125,140.
        let net = eval(&mut b, 22, &net_in_hand("150000", "0"));
        assert!((net - 96297.0).abs() < 1.0, "net {net}");
    }

    #[test]
    fn earnings_inverse_round_trips_through_every_region() {
        let mut b = tax_model();
        for (i, target) in [8000.0, 30000.0, 47965.0, 60000.0, 72000.0, 78000.0, 85000.0, 120000.0].iter().enumerate() {
            let row = 30 + i as i32;
            let gross = eval(&mut b, row, &earnings_gross_for_net(&target.to_string()));
            let net = eval(&mut b, row + 20, &net_in_hand(&gross.to_string(), "0"));
            assert!((net - target).abs() < 0.5, "target {target}: gross {gross} nets {net}");
        }
        // The Somerset-only row: £47,965 net needs ~£59k of pension.
        let gross = eval(&mut b, 60, &earnings_gross_for_net("47965"));
        assert!((gross - 58995.0).abs() < 1.0, "gross {gross}");
    }

    #[test]
    fn engine_expressions_fit_a_spreadsheet_cell() {
        assert!(net_in_hand("(IF(D2>0,salary,0)+AA2)", "Z2").len() < 8000);
        assert!(floor_and_ceiling("F11", "(M11*draw_pct)", "O11", "P11").0.len() < 8000);
    }

    #[test]
    fn row_formats_follow_the_label_rule() {
        assert_eq!(row_fmt("Mortgage rate"), FMT_PCT);
        assert_eq!(row_fmt("FLOOR (sessions per week)"), FMT_1DP);
        assert_eq!(row_fmt("Solvency margin (roof − floor)"), FMT_1DP);
        assert_eq!(row_fmt("Will's living, net"), FMT_INT);
    }
}
