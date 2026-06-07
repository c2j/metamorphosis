use crate::environment::Environment;

#[derive(Debug, thiserror::Error)]
pub enum ConstraintError {
    #[error("invalid constraint format")]
    InvalidFormat,
    #[error("unsupported operator: {0}")]
    UnsupportedOperator(String),
}

pub fn apply_constraints(
    constraints: &serde_json::Value,
    env: &mut Environment,
) -> Result<(), ConstraintError> {
    match constraints {
        serde_json::Value::Array(arr) => {
            for c in arr {
                apply_single(c, env)?;
            }
            Ok(())
        }
        serde_json::Value::Null | serde_json::Value::Bool(false) => Ok(()),
        _ => Err(ConstraintError::InvalidFormat),
    }
}

fn apply_single(
    constraint: &serde_json::Value,
    _env: &mut Environment,
) -> Result<(), ConstraintError> {
    let obj = constraint.as_object().ok_or(ConstraintError::InvalidFormat)?;
    // Constraints are applied during database creation for now.
    // Full implementation will parse PK, FK, NOT NULL, range constraints.
    let _ = obj;
    Ok(())
}
