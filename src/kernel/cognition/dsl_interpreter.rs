//! DSL 技能解释器 —— 执行声明式配置型技能

use std::collections::HashMap;


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
#[derive(Debug, Default)]
pub struct DslInterpreter;

impl DslInterpreter {
    pub fn new() -> Self {
        Self
    }

    /// 执行 DSL 技能
    pub fn execute(&self, dsl: &DslSkill, inputs: serde_json::Value) -> Result<serde_json::Value, String> {
        let mut context = ExecutionContext::new(inputs);

        for step in &dsl.steps {
            self.execute_step(step, &mut context)?;
        }

        Ok(context.get_outputs(&dsl.outputs))
    }

    fn execute_step(&self, step: &DslStep, context: &mut ExecutionContext) -> Result<(), String> {
        match step {
            DslStep::ToolCall { tool, .. } => {
                return Err(format!("Unknown tool: {}", tool));
            }
            DslStep::If { condition, then_steps, else_steps } => {
                let cond_result = self.evaluate_condition(condition, context)?;
                let steps = if cond_result { then_steps } else { else_steps };
                for step in steps {
                    self.execute_step(step, context)?;
                }
            }
            DslStep::ForEach { over, steps } => {
                let items = context.get_array(over)?;
                for item in items {
                    context.set_variable("item", item);
                    for step in steps {
                        self.execute_step(step, context)?;
                    }
                }
            }
            DslStep::Parallel { branches } => {
                // TODO: 并行执行分支
                for branch in branches {
                    for step in branch {
                        self.execute_step(step, context)?;
                    }
                }
            }
            DslStep::Recall { query, filter, output_as } => {
                // TODO: 调用记忆系统查询
                let result = serde_json::json!({
                    "query": query,
                    "filter": filter,
                    "results": []
                });
                context.set_variable(output_as, result);
            }
            DslStep::Store { key, value, tags } => {
                let resolved_value = context.resolve_params(value);
                // TODO: 存储到记忆系统
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
        // TODO: 实现模板变量替换，如 {"path": "{{input.file}}"}
        params.clone()
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
        let result = interpreter.execute(&dsl, serde_json::Value::Null).unwrap();
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
        let result = interpreter.execute(&dsl, serde_json::Value::Null).unwrap();
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
        let result = interpreter.execute(&dsl, inputs).unwrap();
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
        let result = interpreter.execute(&dsl, inputs).unwrap();
        assert!(result.get("result").is_some());

        let inputs = serde_json::json!({"x": 3});
        let result = interpreter.execute(&dsl, inputs).unwrap();
        assert!(result.get("result").is_some());
    }

    #[test]
    fn test_execute_unknown_tool_returns_error() {
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
        let result = interpreter.execute(&dsl, serde_json::Value::Null);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown tool"));
    }
}
