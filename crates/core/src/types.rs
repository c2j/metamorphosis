//! Core types for rewrite results and rule metadata.

use ogsql_parser::ast::Statement;
use serde::{Deserialize, Serialize};

/// Safety level determines how the engine handles a rule's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SafetyLevel {
    /// Semantically equivalent rewrite — engine auto-executes.
    Safe,
    /// Semantically equivalent only when preconditions are met — engine
    /// verifies preconditions before executing.
    Conditional,
    /// Not semantically equivalent — generates suggestions only, never
    /// replaces the original statement automatically.
    Manual,
}

/// Confidence level for a rewrite result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Confidence {
    /// Single table, no subqueries, pure literal equality — deterministic.
    High,
    /// Penetrated a derived table or removed EXISTS — structural change
    /// but semantics are traceable.
    Medium,
    /// Multi-table JOIN, dynamic subqueries — result requires human review.
    Low,
}

/// Action produced by a rule after matching.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum RewriteAction {
    /// Semantically equivalent replacement for the original statement.
    Replace(Box<Statement>),
    /// Generates a derived SQL (e.g., data quality probe) that coexists
    /// with the original rather than replacing it.
    Generate {
        stmt: Box<Statement>,
        purpose: String,
        confidence: Confidence,
    },
    /// Text-only suggestion, does not produce an AST.
    Suggest { message: String, severity: Severity },
}

/// Severity level for text suggestions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

/// Result of rewriting a set of statements.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewriteResult {
    /// Rewritten statements (Safe / Conditional level).
    pub statements: Vec<Statement>,
    /// Manual-level suggestions requiring human review.
    pub suggestions: Vec<Suggestion>,
    /// Whether any rewrite occurred.
    pub changed: bool,
}

/// A single suggestion produced by a Manual-level rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub rule_id: String,
    pub rule_description: String,
    pub action: RewriteAction,
    pub confidence: Confidence,
    pub notes: Vec<String>,
}

/// Category for grouping and filtering rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum RuleCategory {
    Performance,
    DataQuality,
    Style,
    Semantic,
    Safety,
}
