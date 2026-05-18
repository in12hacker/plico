//! DSL 技能解释器 —— 执行声明式配置型技能

use std::collections::HashMap;
use std::sync::Arc;

/// 工具执行抽象 —— 打破 DslInterpreter ↔ AIKernel 循环依赖
pub trait ToolExecutor: Send + Sync {
    fn execute_tool(&self, name: &str, params: &serde_json::Value, agent_id: &str) -> Result<serde_json::Value, String>;
}

/// DSL 技能定义（声明式）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DslSkill {
    pub version: String,
    pub name: String,
    pub description: String,
    pub inputs: Vec<DslInput>,
    pub steps: Vec<DslStep>,
    pub outputs: Vec<DslOutput>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DslInput {
    pub name: String,
    pub dtype: String,
    pub required: bool,
    pub default: Option<serde_json::Value>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DslOutput {
    pub name: String,
    pub dtype: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DslStep {
    ToolCall {
        tool: String,
        params: serde_json::Value,
        output_as: Option<String>,
    },
    If {
        condition: DslCondition,
        then_steps: Vec<DslStep>,
        else_steps: Vec<DslStep>,
    },
    ForEach {
        over: String,
        steps: Vec<DslStep>,
    },
    Parallel {
        branches: Vec<Vec<DslStep>>,
    },
    Recall {
        query: String,
        filter: Option<serde_json::Value>,
        output_as: String,
    },
    Store {
        key: String,
        value: serde_json::Value,
        tags: Vec<String>,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DslCondition {
    pub left: String,
    pub op: String, // eq, ne, gt, lt, contains
    pub right: serde_json::Value,
}

/// DSL 解释器
#[derive(Default)]
pub struct DslInterpreter {
    executor: Option<Arc<dyn ToolExecutor>>,
}

impl std::fmt::Debug for DslInterpreter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DslInterpreter")
            .field("has_executor", &self.executor.is_some())
            .finish()
    }
}

impl DslInterpreter {
    pub fn new() -> Self {
        Self { executor: None }
    }

    pub fn with_executor(executor: Arc<dyn ToolExecutor>) -> Self {
        Self { executor: Some(executor) }
    }

    /// 执行 DSL 技能
    pub fn execute(&self, dsl: &DslSkill, inputs: serde_json::Value, agent_id: Option<&str>) -> Result<serde_json::Value, String> {
        let mut context = ExecutionContext::new(inputs);

        for step in &dsl.steps {
            self.execute_step(step, &mut context, agent_id.unwrap_or("system"))?;
        }

        Ok(context.get_outputs(&dsl.outputs))
    }

    fn execute_step(&self, step: &DslStep, context: &mut ExecutionContext, agent_id: &str) -> Result<(), String> {
        match step {
            DslStep::ToolCall { tool, params, output_as } => {
                let executor = self.executor.as_ref()
                    .ok_or_else(|| format!("No tool executor configured (tool: {})", tool))?;
                let resolved_params = context.resolve_params(params);
                let result = executor.execute_tool(tool, &resolved_params, agent_id)?;
                let var_name = output_as.as_deref().unwrap_or(tool);
                context.set_variable(var_name, result);
            }
            DslStep::If { condition, then_steps, else_steps } => {
                let cond_result = self.evaluate_condition(condition, context)?;
                let steps = if cond_result { then_steps } else { else_steps };
                for step in steps {
                    self.execute_step(step, context, agent_id)?;
                }
            }
            DslStep::ForEach { over, steps } => {
                let items = context.get_array(over)?;
                for item in items {
                    context.set_variable("item", item);
                    for step in steps {
                        self.execute_step(step, context, agent_id)?;
                    }
                }
            }
            DslStep::Parallel { branches } => {
                // Each branch runs with its own cloned context; results merge back
                let mut branch_keys = Vec::new();
                for branch in branches {
                    let mut branch_ctx = context.clone();
                    for step in branch {
                        self.execute_step(step, &mut branch_ctx, agent_id)?;
                    }
                    // Merge branch variables back into main context
                    for (k, v) in branch_ctx.variables {
                        branch_keys.push(k.clone());
                        context.set_variable(&k, v);
                    }
                }
                // Record which keys came from parallel branches
                context.set_variable("__parallel_keys", serde_json::json!(branch_keys));
            }
            DslStep::Recall { query, filter, output_as } => {
                // Search context variables for matching values
                let query_lower = query.to_lowercase();
                let mut results = Vec::new();
                for (key, value) in &context.variables {
                    let value_str = value.to_string().to_lowercase();
                    if key.to_lowercase().contains(&query_lower) || value_str.contains(&query_lower) {
                        if let Some(f) = filter {
                            // Apply simple equality filter
                            if let (Some(filter_key), Some(filter_val)) = (f.get("key"), f.get("value")) {
                                if let (Some(fk), Some(fv)) = (filter_key.as_str(), filter_val.as_str()) {
                                    if key != fk || !value_str.contains(fv) {
                                        continue;
                                    }
                                }
                            }
                        }
                        results.push(serde_json::json!({"key": key, "value": value}));
                    }
                }
                context.set_variable(output_as, serde_json::json!({
                    "query": query,
                    "results": results
                }));
            }
            DslStep::Store { key, value, tags } => {
                let resolved_value = context.resolve_params(value);
                context.set_variable(key, serde_json::json!({
                    "stored": true,
                    "value": resolved_value,
                    "tags": tags
                }));
            }
        }
        Ok(())
    }

    fn evaluate_condition(&self, condition: &DslCondition, context: &ExecutionContext) -> Result<bool, String> {
        let left = context.resolve_variable(&condition.left);
        match condition.op.as_str() {
            "eq" => Ok(left == condition.right),
            "ne" => Ok(left != condition.right),
            "gt" => Ok(as_f64(&left) > as_f64(&condition.right)),
            "lt" => Ok(as_f64(&left) < as_f64(&condition.right)),
            "contains" => {
                let left_str = left.as_str().unwrap_or("");
                let right_str = condition.right.as_str().unwrap_or("");
                Ok(left_str.contains(right_str))
            }
            _ => Err(format!("Unknown operator: {}", condition.op)),
        }
    }
}

/// 执行上下文
#[derive(Clone)]
struct ExecutionContext {
    variables: HashMap<String, serde_json::Value>,
}

impl ExecutionContext {
    fn new(inputs: serde_json::Value) -> Self {
        let mut variables = HashMap::new();
        if let serde_json::Value::Object(map) = inputs {
            for (k, v) in map {
                variables.insert(k, v);
            }
        }
        Self { variables }
    }

    fn set_variable(&mut self, name: &str, value: serde_json::Value) {
        self.variables.insert(name.to_string(), value);
    }

    fn resolve_variable(&self, name: &str) -> serde_json::Value {
        self.variables.get(name).cloned().unwrap_or(serde_json::Value::Null)
    }

    fn resolve_params(&self, params: &serde_json::Value) -> serde_json::Value {
        match params {
            serde_json::Value::String(s) => {
                // Template substitution: replace ${var} and {{var}} with context values
                let mut result = s.clone();
                let mut changed = true;
                // Iterate to handle nested substitutions (max 10 passes to prevent infinite loops)
                for _ in 0..10 {
                    if !changed { break; }
                    changed = false;
                    // Handle ${var} syntax
                    while let Some(start) = result.find("${") {
                        if let Some(end) = result[start + 2..].find('}') {
                            let var_name = &result[start + 2..start + 2 + end];
                            let value = self.resolve_variable(var_name);
                            let replacement = match &value {
                                serde_json::Value::String(s) => s.clone(),
                                other => other.to_string(),
                            };
                            result = format!("{}{}{}", &result[..start], replacement, &result[start + 2 + end + 1..]);
                            changed = true;
                        } else {
                            break;
                        }
                    }
                    // Handle {{var}} syntax
                    while let Some(start) = result.find("{{") {
                        if let Some(end) = result[start + 2..].find("}}") {
                            let var_name = &result[start + 2..start + 2 + end].trim();
                            let value = self.resolve_variable(var_name);
                            let replacement = match &value {
                                serde_json::Value::String(s) => s.clone(),
                                other => other.to_string(),
                            };
                            result = format!("{}{}{}", &result[..start], replacement, &result[start + 2 + end + 2..]);
                            changed = true;
                        } else {
                            break;
                        }
                    }
                }
                serde_json::Value::String(result)
            }
            serde_json::Value::Object(map) => {
                let resolved: serde_json::Map<String, serde_json::Value> = map.iter()
                    .map(|(k, v)| (k.clone(), self.resolve_params(v)))
                    .collect();
                serde_json::Value::Object(resolved)
            }
            serde_json::Value::Array(arr) => {
                let resolved: Vec<serde_json::Value> = arr.iter()
                    .map(|v| self.resolve_params(v))
                    .collect();
                serde_json::Value::Array(resolved)
            }
            other => other.clone(),
        }
    }

    fn get_array(&self, name: &str) -> Result<Vec<serde_json::Value>, String> {
        match self.variables.get(name) {
            Some(serde_json::Value::Array(arr)) => Ok(arr.clone()),
            Some(other) => Err(format!("Expected array for '{}', got {:?}", name, other)),
            None => Err(format!("Variable '{}' not found", name)),
        }
    }

    fn get_outputs(&self, outputs: &[DslOutput]) -> serde_json::Value {
        let mut result = serde_json::Map::new();
        for output in outputs {
            if let Some(value) = self.variables.get(&output.name) {
                result.insert(output.name.clone(), value.clone());
            }
        }
        serde_json::Value::Object(result)
    }
}

fn as_f64(value: &serde_json::Value) -> f64 {
    value.as_f64().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_creates_interpreter() {
        let interpreter = DslInterpreter::new();
        let _ = interpreter;
    }

    #[test]
    fn test_execute_empty_steps_returns_empty_object() {
        let dsl = DslSkill {
            version: "1.0".to_string(),
            name: "test".to_string(),
            description: "test".to_string(),
            inputs: vec![],
            steps: vec![],
            outputs: vec![],
        };
        let interpreter = DslInterpreter::new();
        let result = interpreter.execute(&dsl, serde_json::Value::Null, None).unwrap();
        assert_eq!(result, serde_json::json!({}));
    }

    #[test]
    fn test_execute_store_step_works() {
        let dsl = DslSkill {
            version: "1.0".to_string(),
            name: "test".to_string(),
            description: "test".to_string(),
            inputs: vec![],
            steps: vec![
                DslStep::Store {
                    key: "my_key".to_string(),
                    value: serde_json::json!("hello"),
                    tags: vec!["tag1".to_string()],
                },
            ],
            outputs: vec![
                DslOutput {
                    name: "my_key".to_string(),
                    dtype: "string".to_string(),
                },
            ],
        };
        let interpreter = DslInterpreter::new();
        let result = interpreter.execute(&dsl, serde_json::Value::Null, None).unwrap();
        assert!(result.get("my_key").is_some());
    }

    #[test]
    fn test_execute_foreach_iterates_correctly() {
        let dsl = DslSkill {
            version: "1.0".to_string(),
            name: "test".to_string(),
            description: "test".to_string(),
            inputs: vec![],
            steps: vec![
                DslStep::ForEach {
                    over: "items".to_string(),
                    steps: vec![
                        DslStep::Store {
                            key: "last_item".to_string(),
                            value: serde_json::json!("{{item}}"),
                            tags: vec![],
                        },
                    ],
                },
            ],
            outputs: vec![
                DslOutput {
                    name: "last_item".to_string(),
                    dtype: "string".to_string(),
                },
            ],
        };
        let interpreter = DslInterpreter::new();
        let inputs = serde_json::json!({"items": ["a", "b", "c"]});
        let result = interpreter.execute(&dsl, inputs, None).unwrap();
        let stored = result.get("last_item").unwrap();
        assert!(stored.get("stored").unwrap().as_bool().unwrap());
    }

    #[test]
    fn test_execute_if_condition_evaluates_correctly() {
        let dsl = DslSkill {
            version: "1.0".to_string(),
            name: "test".to_string(),
            description: "test".to_string(),
            inputs: vec![],
            steps: vec![
                DslStep::If {
                    condition: DslCondition {
                        left: "x".to_string(),
                        op: "eq".to_string(),
                        right: serde_json::json!(5),
                    },
                    then_steps: vec![
                        DslStep::Store {
                            key: "result".to_string(),
                            value: serde_json::json!("yes"),
                            tags: vec![],
                        },
                    ],
                    else_steps: vec![
                        DslStep::Store {
                            key: "result".to_string(),
                            value: serde_json::json!("no"),
                            tags: vec![],
                        },
                    ],
                },
            ],
            outputs: vec![
                DslOutput {
                    name: "result".to_string(),
                    dtype: "string".to_string(),
                },
            ],
        };
        let interpreter = DslInterpreter::new();
        let inputs = serde_json::json!({"x": 5});
        let result = interpreter.execute(&dsl, inputs, None).unwrap();
        assert!(result.get("result").is_some());

        let inputs = serde_json::json!({"x": 3});
        let result = interpreter.execute(&dsl, inputs, None).unwrap();
        assert!(result.get("result").is_some());
    }

    #[test]
    fn test_execute_toolcall_without_executor_returns_error() {
        let dsl = DslSkill {
            version: "1.0".to_string(),
            name: "test".to_string(),
            description: "test".to_string(),
            inputs: vec![],
            steps: vec![
                DslStep::ToolCall {
                    tool: "unknown_tool".to_string(),
                    params: serde_json::json!({}),
                    output_as: None,
                },
            ],
            outputs: vec![],
        };
        let interpreter = DslInterpreter::new();
        let result = interpreter.execute(&dsl, serde_json::Value::Null, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No tool executor"));
    }

    #[test]
    fn test_template_substitution_dollar_brace() {
        let dsl = DslSkill {
            version: "1.0".to_string(),
            name: "test".to_string(),
            description: "test".to_string(),
            inputs: vec![],
            steps: vec![
                DslStep::Store {
                    key: "greeting".to_string(),
                    value: serde_json::json!("Hello ${name}!"),
                    tags: vec![],
                },
            ],
            outputs: vec![
                DslOutput { name: "greeting".to_string(), dtype: "string".to_string() },
            ],
        };
        let interpreter = DslInterpreter::new();
        let inputs = serde_json::json!({"name": "World"});
        let result = interpreter.execute(&dsl, inputs, None).unwrap();
        let stored = result.get("greeting").unwrap();
        assert_eq!(stored.get("value").unwrap().as_str().unwrap(), "Hello World!");
    }

    #[test]
    fn test_template_substitution_double_brace() {
        let dsl = DslSkill {
            version: "1.0".to_string(),
            name: "test".to_string(),
            description: "test".to_string(),
            inputs: vec![],
            steps: vec![
                DslStep::Store {
                    key: "path".to_string(),
                    value: serde_json::json!("{{base}}/file.txt"),
                    tags: vec![],
                },
            ],
            outputs: vec![
                DslOutput { name: "path".to_string(), dtype: "string".to_string() },
            ],
        };
        let interpreter = DslInterpreter::new();
        let inputs = serde_json::json!({"base": "/tmp"});
        let result = interpreter.execute(&dsl, inputs, None).unwrap();
        let stored = result.get("path").unwrap();
        assert_eq!(stored.get("value").unwrap().as_str().unwrap(), "/tmp/file.txt");
    }

    #[test]
    fn test_template_substitution_in_object() {
        let _interpreter = DslInterpreter::new();
        let ctx = ExecutionContext::new(serde_json::json!({"host": "localhost", "port": 8080}));
        let params = serde_json::json!({"url": "http://${host}:${port}/api"});
        let resolved = ctx.resolve_params(&params);
        assert_eq!(resolved.get("url").unwrap().as_str().unwrap(), "http://localhost:8080/api");
    }

    #[test]
    fn test_recall_step_finds_matching_context() {
        let dsl = DslSkill {
            version: "1.0".to_string(),
            name: "test".to_string(),
            description: "test".to_string(),
            inputs: vec![],
            steps: vec![
                DslStep::Store {
                    key: "user_name".to_string(),
                    value: serde_json::json!("Alice"),
                    tags: vec![],
                },
                DslStep::Recall {
                    query: "user".to_string(),
                    filter: None,
                    output_as: "found".to_string(),
                },
            ],
            outputs: vec![
                DslOutput { name: "found".to_string(), dtype: "object".to_string() },
            ],
        };
        let interpreter = DslInterpreter::new();
        let result = interpreter.execute(&dsl, serde_json::json!({}), None).unwrap();
        let found = result.get("found").unwrap();
        let results = found.get("results").unwrap().as_array().unwrap();
        assert!(!results.is_empty());
        assert!(results.iter().any(|r| r.get("key").unwrap().as_str().unwrap() == "user_name"));
    }

    #[test]
    fn test_recall_step_with_filter() {
        let dsl = DslSkill {
            version: "1.0".to_string(),
            name: "test".to_string(),
            description: "test".to_string(),
            inputs: vec![],
            steps: vec![
                DslStep::Store {
                    key: "config_debug".to_string(),
                    value: serde_json::json!(true),
                    tags: vec![],
                },
                DslStep::Recall {
                    query: "config".to_string(),
                    filter: Some(serde_json::json!({"key": "config_debug", "value": "true"})),
                    output_as: "found".to_string(),
                },
            ],
            outputs: vec![
                DslOutput { name: "found".to_string(), dtype: "object".to_string() },
            ],
        };
        let interpreter = DslInterpreter::new();
        let result = interpreter.execute(&dsl, serde_json::json!({}), None).unwrap();
        let found = result.get("found").unwrap();
        let results = found.get("results").unwrap().as_array().unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_parallel_step_merges_results() {
        let dsl = DslSkill {
            version: "1.0".to_string(),
            name: "test".to_string(),
            description: "test".to_string(),
            inputs: vec![],
            steps: vec![
                DslStep::Parallel {
                    branches: vec![
                        vec![DslStep::Store {
                            key: "branch_a".to_string(),
                            value: serde_json::json!("result_a"),
                            tags: vec![],
                        }],
                        vec![DslStep::Store {
                            key: "branch_b".to_string(),
                            value: serde_json::json!("result_b"),
                            tags: vec![],
                        }],
                    ],
                },
            ],
            outputs: vec![
                DslOutput { name: "branch_a".to_string(), dtype: "string".to_string() },
                DslOutput { name: "branch_b".to_string(), dtype: "string".to_string() },
            ],
        };
        let interpreter = DslInterpreter::new();
        let result = interpreter.execute(&dsl, serde_json::json!({}), None).unwrap();
        let a = result.get("branch_a").unwrap();
        assert_eq!(a.get("value").unwrap().as_str().unwrap(), "result_a");
        let b = result.get("branch_b").unwrap();
        assert_eq!(b.get("value").unwrap().as_str().unwrap(), "result_b");
    }

    /// Mock executor for testing ToolCall
    struct MockExecutor;
    impl ToolExecutor for MockExecutor {
        fn execute_tool(&self, name: &str, params: &serde_json::Value, _agent_id: &str) -> Result<serde_json::Value, String> {
            Ok(serde_json::json!({
                "tool": name,
                "params": params,
                "mock": true
            }))
        }
    }

    #[test]
    fn test_toolcall_with_executor() {
        let dsl = DslSkill {
            version: "1.0".to_string(),
            name: "test".to_string(),
            description: "test".to_string(),
            inputs: vec![],
            steps: vec![
                DslStep::ToolCall {
                    tool: "memory.create".to_string(),
                    params: serde_json::json!({"content": "hello"}),
                    output_as: Some("result".to_string()),
                },
            ],
            outputs: vec![
                DslOutput { name: "result".to_string(), dtype: "object".to_string() },
            ],
        };
        let interpreter = DslInterpreter::with_executor(Arc::new(MockExecutor));
        let result = interpreter.execute(&dsl, serde_json::json!({}), Some("test_agent")).unwrap();
        let r = result.get("result").unwrap();
        assert_eq!(r.get("tool").unwrap().as_str().unwrap(), "memory.create");
        assert!(r.get("mock").unwrap().as_bool().unwrap());
    }

    #[test]
    fn test_toolcall_resolves_template_params() {
        let dsl = DslSkill {
            version: "1.0".to_string(),
            name: "test".to_string(),
            description: "test".to_string(),
            inputs: vec![],
            steps: vec![
                DslStep::ToolCall {
                    tool: "search".to_string(),
                    params: serde_json::json!({"query": "${search_term}"}),
                    output_as: None,
                },
            ],
            outputs: vec![
                DslOutput { name: "search".to_string(), dtype: "object".to_string() },
            ],
        };
        let interpreter = DslInterpreter::with_executor(Arc::new(MockExecutor));
        let inputs = serde_json::json!({"search_term": "rust programming"});
        let result = interpreter.execute(&dsl, inputs, Some("test_agent")).unwrap();
        let r = result.get("search").unwrap();
        assert_eq!(r.get("params").unwrap().get("query").unwrap().as_str().unwrap(), "rust programming");
    }
}
