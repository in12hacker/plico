//! Pipeline execution — sequential multi-step action dispatch.

use plico::kernel::AIKernel;
use serde_json::Value;

pub(in crate::dispatch) fn execute_pipeline(args: &Value, kernel: &AIKernel) -> Result<String, String> {
    let pipeline = args.get("pipeline")
        .and_then(|p| p.as_array())
        .ok_or("pipeline must be an array")?;

    let mut results: Value = serde_json::json!({});
    let mut context: std::collections::HashMap<String, Value> = std::collections::HashMap::new();

    for (idx, step) in pipeline.iter().enumerate() {
        let step_name = step.get("step")
            .and_then(|s| s.as_str())
            .map(String::from)
            .unwrap_or_else(|| format!("step{}", idx));

        let substituted_args = substitute_pipeline_vars(step, &context)?;

        let action = substituted_args.get("action")
            .and_then(|a| a.as_str())
            .ok_or(format!("step '{}': missing action", step_name))?;

        let step_result = super::action::dispatch_plico_action(action, &substituted_args, kernel)?;

        let result_json: Value = serde_json::from_str(&step_result)
            .unwrap_or_else(|_| serde_json::json!(step_result));
        context.insert(step_name.clone(), result_json.clone());

        results[step_name] = result_json;
    }

    Ok(serde_json::to_string_pretty(&results).unwrap_or_default())
}

pub(in crate::dispatch) fn substitute_pipeline_vars(step: &Value, context: &std::collections::HashMap<String, Value>) -> Result<Value, String> {
    let step_json = serde_json::to_string(step).map_err(|e| e.to_string())?;
    let mut result = step_json.clone();

    for (key, value) in context.iter() {
        let value_str = serde_json::to_string(value).unwrap_or_default();
        result = result.replace(&format!("${}", key), &value_str);
    }

    serde_json::from_str(&result).map_err(|e| e.to_string())
}
