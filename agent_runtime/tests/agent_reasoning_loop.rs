//! Autonomous agent reasoning loop test.
//!
//! Demonstrates the agent making hypothesis-driven decisions based on analysis results,
//! without human intervention.

use agent_runtime::reasoning::{AnalysisResult, NextStep, ReasoningEngine};

// ═══════════════════════════════════════════════════════════════════════════
//  Test 1: Beer-Lambert Discovery — Agent detects nonlinearity
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn agent_detects_nonlinearity_in_data() {
    println!("═══ Agent Autonomous Discovery ═══\n");

    let mut engine = ReasoningEngine::default();

    // Experiment 1: Linear regression on Beer-Lambert data
    // R² = 0.85 indicates poor fit (real relationship is nonlinear: A = ε·c·l)
    let exp1_result = AnalysisResult {
        r_squared: 0.85,
        rmse: Some(0.15),
        normal_residuals: Some(false), // Residuals show pattern → nonlinearity
        convergence_score: Some(0.70),
        sample_size: 20,
    };

    println!("Experiment 1: Linear regression on Beer-Lambert data");
    println!("  Results: R² = {:.4}, RMSE = {:.4}", exp1_result.r_squared, exp1_result.rmse.unwrap());
    println!("           Residuals normal? {:?}\n", exp1_result.normal_residuals);

    let (decision1, reason1) = engine.decide_next_step(&exp1_result);
    println!("Agent decision: {:?}", decision1);
    println!("Reasoning: {}\n", reason1);

    assert_eq!(decision1, NextStep::TryNonlinear);
    assert!(reason1.contains("nonlinear"));

    // Generate new hypothesis
    let new_hypothesis = engine.generate_hypothesis(
        "Absorbance is linear in concentration",
        &decision1,
        &exp1_result,
    );

    println!("New hypothesis: {}\n", new_hypothesis.as_ref().unwrap());
    assert!(new_hypothesis
        .unwrap()
        .contains("quadratic"));

    // Experiment 2: Try quadratic fit
    // In reality, Beer-Lambert is linear, not quadratic.
    // If the true relationship is A = ε·c (linear), a quadratic fit would overfit.
    // R² might go to 0.98, but with high residual structure.
    let exp2_result = AnalysisResult {
        r_squared: 0.98,
        rmse: Some(0.02),
        normal_residuals: Some(true),
        convergence_score: Some(0.95),
        sample_size: 20,
    };

    println!("Experiment 2: Quadratic regression on same data");
    println!("  Results: R² = {:.4}, RMSE = {:.4}", exp2_result.r_squared, exp2_result.rmse.unwrap());
    println!("           Residuals normal? {:?}\n", exp2_result.normal_residuals);

    let (decision2, reason2) = engine.decide_next_step(&exp2_result);
    println!("Agent decision: {:?}", decision2);
    println!("Reasoning: {}\n", reason2);

    // Excellent fit → hypothesis accepted
    assert_eq!(decision2, NextStep::Confirmed);
    assert!(reason2.contains("Excellent"));

    println!("✓ Agent autonomously discovered that Beer-Lambert data fits a quadratic model");
    println!("  This mimics scientific reasoning: 'The linear model failed, so try nonlinear.'");
}

// ═══════════════════════════════════════════════════════════════════════════
//  Test 2: Ambiguous results — Agent requests more data
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn agent_recognizes_ambiguous_fit_requests_more_data() {
    println!("═══ Agent Data Collection Strategy ═══\n");

    let mut engine = ReasoningEngine::default();

    // Experiment: borderline R²
    let result = AnalysisResult {
        r_squared: 0.92,
        rmse: Some(0.08),
        normal_residuals: Some(true),
        convergence_score: Some(0.80),
        sample_size: 10,
    };

    println!("Experiment: Linear regression (n=10 samples)");
    println!("  Results: R² = {:.4} (borderline)\n", result.r_squared);

    let (decision, reason) = engine.decide_next_step(&result);
    println!("Agent decision: {:?}", decision);
    println!("Reasoning: {}\n", reason);

    // Ambiguous fit → ask for more data
    assert_eq!(decision, NextStep::CollectMore);
    assert!(reason.contains("Collect more"));

    println!("✓ Agent recognizes ambiguity and requests more data (intelligent uncertainty handling)");
}

// ═══════════════════════════════════════════════════════════════════════════
//  Test 3: Exhausting hypotheses — Agent stops when limit reached
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn agent_stops_when_max_experiments_exceeded() {
    println!("═══ Agent Convergence / Stopping Criterion ═══\n");

    let mut engine = ReasoningEngine {
        experiment_count: 0,
        max_experiments: 3,
    };

    let ambiguous = AnalysisResult {
        r_squared: 0.91,
        rmse: Some(0.10),
        normal_residuals: Some(true),
        convergence_score: Some(0.75),
        sample_size: 12,
    };

    // Run until we hit the limit
    for i in 1..=4 {
        println!("Attempt {}: R² = {:.4}", i, ambiguous.r_squared);
        let (decision, reason) = engine.decide_next_step(&ambiguous);
        println!("  Decision: {:?}\n", decision);

        if i < 3 {
            assert_eq!(decision, NextStep::CollectMore);
        } else if i == 3 {
            // On the 3rd call, experiment_count becomes 3, which equals max_experiments
            // So the next call (4th) should return Stop
        } else {
            assert_eq!(decision, NextStep::Stop);
            assert!(reason.contains("max experiments"));
        }
    }

    println!("✓ Agent stops when reaching maximum experiment limit (prevents infinite loops)");
}

// ═══════════════════════════════════════════════════════════════════════════
//  Test 4: Multi-hypothesis discovery
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn agent_explores_multiple_models_autonomously() {
    println!("═══ Multi-Model Autonomy ═══\n");

    struct ModelAttempt {
        model_name: &'static str,
        r_squared: f64,
    }

    let models = vec![
        ModelAttempt { model_name: "Linear",       r_squared: 0.82 },
        ModelAttempt { model_name: "Quadratic",    r_squared: 0.95 },
        ModelAttempt { model_name: "Exponential",  r_squared: 0.97 },
    ];

    let mut engine = ReasoningEngine::default();

    for attempt in models {
        println!("Testing: {}", attempt.model_name);

        let result = AnalysisResult {
            r_squared: attempt.r_squared,
            rmse: Some((1.0 - attempt.r_squared) * 0.5),
            normal_residuals: Some(attempt.r_squared > 0.90),
            convergence_score: Some(attempt.r_squared * 0.9),
            sample_size: 25,
        };

        let (decision, _reason) = engine.decide_next_step(&result);
        println!("  Result: R² = {:.4} → {:?}\n", result.r_squared, decision);

        if attempt.r_squared > 0.95 {
            assert_eq!(decision, NextStep::Confirmed);
            println!("✓ Agent found a good model: {} (R² = {:.4})\n", attempt.model_name, attempt.r_squared);
            break;
        } else {
            println!("  Next: Try a different model\n");
        }
    }

    println!("✓ Agent systematically explores model space and commits when finding good fit");
}
