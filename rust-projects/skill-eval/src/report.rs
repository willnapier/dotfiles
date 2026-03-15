use crate::evaluate::EvalResult;

pub fn print_scenario_results(scenario_id: &str, results: &[EvalResult]) {
    let passed = results.iter().filter(|r| r.passed).count();
    let total = results.len();

    println!("  {}: {}/{}", scenario_id, passed, total);
    for r in results {
        let icon = if r.passed { "PASS" } else { "FAIL" };
        println!("    {} {:>4} {} -- {}", icon, r.assertion_id, r.assertion_text, r.reason);
    }
    println!();
}

pub fn print_summary(results: &[EvalResult]) {
    let passed = results.iter().filter(|r| r.passed).count();
    let total = results.len();
    let pct = if total > 0 {
        (passed as f64 / total as f64) * 100.0
    } else {
        0.0
    };

    println!("=== SUMMARY ===");
    println!("  Total: {}/{} ({:.1}%)", passed, total, pct);

    let failures: Vec<_> = results.iter().filter(|r| !r.passed).collect();
    if !failures.is_empty() {
        println!();
        println!("  Failures:");
        for f in &failures {
            println!("    {} {} -- {}", f.assertion_id, f.assertion_text, f.reason);
        }
    }
}
