use crate::evaluate::{EvalOutcome, EvalResult};

pub fn print_scenario_results(scenario_id: &str, results: &[EvalResult]) {
    let applicable: Vec<_> = results.iter().filter(|r| r.is_applicable()).collect();
    let passed = applicable.iter().filter(|r| r.passed()).count();
    let na_count = results.len() - applicable.len();

    if na_count > 0 {
        println!("  {}: {}/{} ({} N/A)", scenario_id, passed, applicable.len(), na_count);
    } else {
        println!("  {}: {}/{}", scenario_id, passed, applicable.len());
    }

    for r in results {
        let icon = match r.outcome {
            EvalOutcome::Pass => "PASS",
            EvalOutcome::Fail => "FAIL",
            EvalOutcome::NotApplicable => "N/A ",
        };
        println!(
            "    {} {:>4} {} -- {}",
            icon, r.assertion_id, r.assertion_text, r.reason
        );
    }
    println!();
}

pub fn print_totals(results: &[EvalResult]) {
    let applicable: Vec<_> = results.iter().filter(|r| r.is_applicable()).collect();
    let passed = applicable.iter().filter(|r| r.passed()).count();
    let na_count = results.len() - applicable.len();
    let total = applicable.len();
    let pct = if total > 0 {
        (passed as f64 / total as f64) * 100.0
    } else {
        0.0
    };

    println!("=== SUMMARY ===");
    if na_count > 0 {
        if total > 0 {
            println!(
                "  Total: {}/{} ({:.1}%) — {} N/A",
                passed, total, pct, na_count
            );
        } else {
            println!("  Total: {}/{} — {} N/A", passed, total, na_count);
        }
    } else if total > 0 {
        println!("  Total: {}/{} ({:.1}%)", passed, total, pct);
    } else {
        println!("  Total: {}/{}", passed, total);
    }

    let failures: Vec<_> = results
        .iter()
        .filter(|r| r.outcome == EvalOutcome::Fail)
        .collect();
    if !failures.is_empty() {
        println!();
        println!("  Failures:");
        for f in &failures {
            println!("    {} {} -- {}", f.assertion_id, f.assertion_text, f.reason);
        }
    }
}
