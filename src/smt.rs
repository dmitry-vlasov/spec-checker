//! SMT solver integration via subprocess.
//!
//! Generates SMT-LIB2 queries and invokes `z3` (or another solver) as a
//! subprocess. If no solver is available, degrades gracefully.
//!
//! This is the middle tier of the verification cascade:
//!   syntactic → **SMT** → LLM

use std::io::Write;
use std::process::{Command, Stdio};

// ─── Solver Detection ────────────────────────────────────────────────────────

/// Check if an SMT solver (z3) is available on the system.
pub fn solver_available() -> bool {
    Command::new("z3")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Result of an SMT query
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmtResult {
    /// The formula is satisfiable (constraints are consistent)
    Sat,
    /// The formula is unsatisfiable (constraints are contradictory)
    Unsat,
    /// The solver could not decide
    Unknown,
    /// The solver is not available or the query failed
    Error(String),
}

// ─── SMT-LIB2 Generation ────────────────────────────────────────────────────

/// A simple SMT-LIB2 assertion builder.
///
/// Supports integer/boolean constraints relevant to spec checking:
/// - Numeric bounds (e.g., "count >= 0")
/// - Equality constraints
/// - State ownership (exclusivity via disjointness)
/// - Logical combinations (and, or, not, implies)
#[derive(Debug, Clone, Default)]
pub struct SmtQuery {
    /// SMT-LIB2 declarations
    declarations: Vec<String>,
    /// SMT-LIB2 assertions
    assertions: Vec<String>,
}

impl SmtQuery {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare an integer variable
    #[allow(dead_code)]
    pub fn declare_int(&mut self, name: &str) {
        self.declarations
            .push(format!("(declare-const {} Int)", name));
    }

    /// Declare a boolean variable
    pub fn declare_bool(&mut self, name: &str) {
        self.declarations
            .push(format!("(declare-const {} Bool)", name));
    }

    /// Add a raw SMT-LIB2 assertion
    pub fn assert_raw(&mut self, smt_expr: &str) {
        self.assertions
            .push(format!("(assert {})", smt_expr));
    }

    /// Assert that two expressions are equal
    #[allow(dead_code)]
    pub fn assert_eq(&mut self, a: &str, b: &str) {
        self.assert_raw(&format!("(= {} {})", a, b));
    }

    /// Assert that a >= b
    #[allow(dead_code)]
    pub fn assert_ge(&mut self, a: &str, b: &str) {
        self.assert_raw(&format!("(>= {} {})", a, b));
    }

    /// Assert that a > b
    #[allow(dead_code)]
    pub fn assert_gt(&mut self, a: &str, b: &str) {
        self.assert_raw(&format!("(> {} {})", a, b));
    }

    /// Assert that a != b
    #[allow(dead_code)]
    pub fn assert_ne(&mut self, a: &str, b: &str) {
        self.assert_raw(&format!("(not (= {} {}))", a, b));
    }

    /// Assert implication: if `premise` then `conclusion`
    #[allow(dead_code)]
    pub fn assert_implies(&mut self, premise: &str, conclusion: &str) {
        self.assert_raw(&format!("(=> {} {})", premise, conclusion));
    }

    /// Generate the full SMT-LIB2 query string
    pub fn to_smtlib2(&self) -> String {
        let mut query = String::new();
        query.push_str("(set-logic ALL)\n");

        for decl in &self.declarations {
            query.push_str(decl);
            query.push('\n');
        }

        for assertion in &self.assertions {
            query.push_str(assertion);
            query.push('\n');
        }

        query.push_str("(check-sat)\n");
        query
    }

    /// Check satisfiability: returns Sat if all assertions can hold simultaneously.
    /// To check if something is *always true*, negate it and check for Unsat.
    pub fn check_sat(&self) -> SmtResult {
        run_solver(&self.to_smtlib2())
    }
}

// ─── Solver Invocation ───────────────────────────────────────────────────────

/// Run an SMT-LIB2 query through z3 and return the result.
fn run_solver(query: &str) -> SmtResult {
    let mut child = match Command::new("z3")
        .arg("-in")       // read from stdin
        .arg("-smt2")     // SMT-LIB2 format
        .arg("-t:5000")   // 5 second timeout
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => return SmtResult::Error(format!("Failed to spawn z3: {}", e)),
    };

    // Write query to stdin
    if let Some(ref mut stdin) = child.stdin {
        if let Err(e) = stdin.write_all(query.as_bytes()) {
            return SmtResult::Error(format!("Failed to write to z3 stdin: {}", e));
        }
    }
    // Drop stdin to close the pipe
    drop(child.stdin.take());

    // Read output
    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return SmtResult::Error(format!("Failed to read z3 output: {}", e)),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let result = stdout.trim();

    match result {
        "sat" => SmtResult::Sat,
        "unsat" => SmtResult::Unsat,
        "unknown" => SmtResult::Unknown,
        "timeout" => SmtResult::Unknown,
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.is_empty() {
                SmtResult::Error(format!("Unexpected z3 output: {}", result))
            } else {
                SmtResult::Error(format!("z3 error: {}", stderr.trim()))
            }
        }
    }
}

// ─── High-Level Constraint Checking ──────────────────────────────────────────

/// Check if a set of state ownership claims are mutually exclusive.
///
/// Given a list of (module_name, state_var) pairs, verifies that no state
/// variable is owned by more than one module.
/// Returns Ok(()) if consistent, Err(msg) if conflicting.
pub fn check_state_ownership_consistency(
    claims: &[(&str, &str)],
) -> Result<(), String> {
    if !solver_available() {
        return Err("z3 not available".to_string());
    }

    let mut query = SmtQuery::new();

    // For each state variable, create a boolean "owned_by_MODULE" variable
    // and assert that at most one can be true
    let mut vars_by_state: std::collections::HashMap<&str, Vec<String>> =
        std::collections::HashMap::new();

    for (module, state_var) in claims {
        let var_name = format!("owns_{}_{}", sanitize_smt_name(module), sanitize_smt_name(state_var));
        query.declare_bool(&var_name);
        query.assert_raw(&var_name); // claim is true
        vars_by_state
            .entry(state_var)
            .or_default()
            .push(var_name);
    }

    // For each state var with multiple claimants, assert mutual exclusion
    for (_state_var, vars) in &vars_by_state {
        if vars.len() > 1 {
            // At-most-one constraint: for each pair, not both true
            for i in 0..vars.len() {
                for j in (i + 1)..vars.len() {
                    query.assert_raw(&format!(
                        "(not (and {} {}))",
                        vars[i], vars[j]
                    ));
                }
            }
        }
    }

    match query.check_sat() {
        SmtResult::Sat => Ok(()),
        SmtResult::Unsat => Err("State ownership conflict detected (SMT-verified)".to_string()),
        SmtResult::Unknown => Err("SMT solver could not decide".to_string()),
        SmtResult::Error(e) => Err(format!("SMT error: {}", e)),
    }
}

/// Check if a numeric invariant is satisfiable.
///
/// Takes a list of (variable_name, lower_bound, upper_bound) constraints
/// and checks if they can all hold simultaneously.
#[allow(dead_code)]
pub fn check_numeric_constraints(
    constraints: &[(&str, Option<i64>, Option<i64>)],
) -> SmtResult {
    if !solver_available() {
        return SmtResult::Error("z3 not available".to_string());
    }

    let mut query = SmtQuery::new();

    for (name, lower, upper) in constraints {
        let smt_name = sanitize_smt_name(name);
        query.declare_int(&smt_name);

        if let Some(lb) = lower {
            query.assert_ge(&smt_name, &lb.to_string());
        }
        if let Some(ub) = upper {
            query.assert_raw(&format!("(<= {} {})", smt_name, ub));
        }
    }

    query.check_sat()
}

/// Check if a logical implication holds: does `premises` imply `conclusion`?
///
/// Returns true if the implication is valid (i.e., negation is unsat).
#[allow(dead_code)]
pub fn check_implication(
    premises: &[&str],
    conclusion: &str,
    declarations: &[(&str, &str)], // (name, sort)
) -> SmtResult {
    if !solver_available() {
        return SmtResult::Error("z3 not available".to_string());
    }

    let mut query = SmtQuery::new();

    for (name, sort) in declarations {
        let smt_name = sanitize_smt_name(name);
        match *sort {
            "Int" => query.declare_int(&smt_name),
            "Bool" => query.declare_bool(&smt_name),
            _ => query.declarations.push(format!("(declare-const {} {})", smt_name, sort)),
        }
    }

    // Assert all premises
    for premise in premises {
        query.assert_raw(premise);
    }

    // Assert negation of conclusion — if unsat, the implication holds
    query.assert_raw(&format!("(not {})", conclusion));

    match query.check_sat() {
        SmtResult::Unsat => SmtResult::Unsat, // negation unsat = implication valid
        other => other,
    }
}

/// Sanitize a name for use in SMT-LIB2 (replace non-alphanumeric chars)
fn sanitize_smt_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_z3() -> bool {
        solver_available()
    }

    #[test]
    fn test_smt_query_generation() {
        let mut q = SmtQuery::new();
        q.declare_int("x");
        q.declare_int("y");
        q.assert_ge("x", "0");
        q.assert_gt("y", "x");

        let smtlib = q.to_smtlib2();
        assert!(smtlib.contains("(declare-const x Int)"));
        assert!(smtlib.contains("(declare-const y Int)"));
        assert!(smtlib.contains("(assert (>= x 0))"));
        assert!(smtlib.contains("(assert (> y x))"));
        assert!(smtlib.contains("(check-sat)"));
    }

    #[test]
    fn test_smt_query_eq() {
        let mut q = SmtQuery::new();
        q.declare_int("a");
        q.declare_int("b");
        q.assert_eq("a", "b");

        let smtlib = q.to_smtlib2();
        assert!(smtlib.contains("(assert (= a b))"));
    }

    #[test]
    fn test_smt_query_implies() {
        let mut q = SmtQuery::new();
        q.declare_bool("p");
        q.declare_bool("q");
        q.assert_implies("p", "q");

        let smtlib = q.to_smtlib2();
        assert!(smtlib.contains("(assert (=> p q))"));
    }

    #[test]
    fn test_sanitize_smt_name() {
        assert_eq!(sanitize_smt_name("hello_world"), "hello_world");
        assert_eq!(sanitize_smt_name("my.module"), "my_module");
        assert_eq!(sanitize_smt_name("src/checker.rs"), "src_checker_rs");
    }

    #[test]
    fn test_smt_sat_simple() {
        if !has_z3() {
            eprintln!("  [skip] z3 not available");
            return;
        }

        let mut q = SmtQuery::new();
        q.declare_int("x");
        q.assert_ge("x", "0");
        q.assert_raw("(<= x 10)");

        assert_eq!(q.check_sat(), SmtResult::Sat);
    }

    #[test]
    fn test_smt_unsat_simple() {
        if !has_z3() {
            eprintln!("  [skip] z3 not available");
            return;
        }

        let mut q = SmtQuery::new();
        q.declare_int("x");
        q.assert_gt("x", "10");
        q.assert_raw("(< x 5)");

        assert_eq!(q.check_sat(), SmtResult::Unsat);
    }

    #[test]
    fn test_smt_state_ownership_consistent() {
        if !has_z3() {
            eprintln!("  [skip] z3 not available");
            return;
        }

        // Different modules own different state — should be consistent
        let claims = vec![
            ("bridge", "balances"),
            ("vault", "reserves"),
        ];
        assert!(check_state_ownership_consistency(&claims).is_ok());
    }

    #[test]
    fn test_smt_state_ownership_conflict() {
        if !has_z3() {
            eprintln!("  [skip] z3 not available");
            return;
        }

        // Two modules claim the same state — conflict
        let claims = vec![
            ("bridge", "balances"),
            ("vault", "balances"),
        ];
        assert!(check_state_ownership_consistency(&claims).is_err());
    }

    #[test]
    fn test_smt_numeric_constraints_sat() {
        if !has_z3() {
            eprintln!("  [skip] z3 not available");
            return;
        }

        let constraints = vec![
            ("count", Some(0), Some(100)),
            ("index", Some(0), None),
        ];
        assert_eq!(check_numeric_constraints(&constraints), SmtResult::Sat);
    }

    #[test]
    fn test_smt_numeric_constraints_unsat() {
        if !has_z3() {
            eprintln!("  [skip] z3 not available");
            return;
        }

        // count >= 10 AND count <= 5 — impossible
        let constraints = vec![
            ("count", Some(10), Some(5)),
        ];
        assert_eq!(check_numeric_constraints(&constraints), SmtResult::Unsat);
    }

    #[test]
    fn test_smt_implication_valid() {
        if !has_z3() {
            eprintln!("  [skip] z3 not available");
            return;
        }

        // x > 5 implies x > 3 — should be valid (negation unsat)
        let result = check_implication(
            &["(> x 5)"],
            "(> x 3)",
            &[("x", "Int")],
        );
        assert_eq!(result, SmtResult::Unsat, "x > 5 should imply x > 3");
    }

    #[test]
    fn test_smt_implication_invalid() {
        if !has_z3() {
            eprintln!("  [skip] z3 not available");
            return;
        }

        // x > 3 does NOT imply x > 5 — should be sat (counterexample: x=4)
        let result = check_implication(
            &["(> x 3)"],
            "(> x 5)",
            &[("x", "Int")],
        );
        assert_eq!(result, SmtResult::Sat, "x > 3 should not imply x > 5");
    }
}
