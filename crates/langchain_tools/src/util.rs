//! 实用工具
//!
//! 提供常用的实用工具，如获取当前时间、计算器等。

use langchain_core::tool;
use thiserror::Error;

/// 实用工具错误
#[derive(Debug, Error)]
pub enum UtilError {
    #[error("Calculation error: {0}")]
    Calculation(String),

    #[error("Parse error: {0}")]
    Parse(String),
}

/// 获取当前时间
#[tool(
    description = "Get the current date and time",
    args(format = "Format string (optional, defaults to ISO 8601)")
)]
pub async fn get_current_time(format: Option<String>) -> Result<String, UtilError> {
    let now = chrono::Utc::now();

    let formatted = match format.as_deref() {
        Some("%Y-%m-%d %H:%M:%S") | None => now.format("%Y-%m-%d %H:%M:%S").to_string(),
        Some("%Y-%m-%d") => now.format("%Y-%m-%d").to_string(),
        Some("%H:%M:%S") => now.format("%H:%M:%S").to_string(),
        Some(f) => {
            return Err(UtilError::Parse(format!("Unsupported format: {}", f)));
        }
    };

    Ok(formatted)
}

/// 简单计算器
#[tool(
    description = "Perform basic arithmetic calculations",
    args(a = "First number", op = "Operator (+, -, *, /)", b = "Second number")
)]
pub async fn calculate(a: f64, op: String, b: f64) -> Result<f64, UtilError> {
    let result = match op.as_str() {
        "+" => a + b,
        "-" => a - b,
        "*" => a * b,
        "/" => {
            if b == 0.0 {
                return Err(UtilError::Calculation("Division by zero".to_string()));
            }
            a / b
        }
        _ => {
            return Err(UtilError::Calculation(format!("Unknown operator: {}", op)));
        }
    };

    Ok(result)
}

/// 计算表达式（简单的四则运算）
#[tool(
    description = "Evaluate a mathematical expression (supports +, -, *, /, parentheses)",
    args(expression = "Mathematical expression to evaluate")
)]
pub async fn eval_expression(expression: String) -> Result<f64, UtilError> {
    // 简单的表达式解析和计算
    // 注意：这是一个简化的实现，仅用于演示
    // 生产环境应使用更安全的表达式解析库

    let result = eval_with_validation(&expression)?;
    Ok(result)
}

/// 简单的表达式计算器（内部使用）
fn eval_with_validation(expr: &str) -> Result<f64, UtilError> {
    // 移除所有空格
    let expr = expr.replace(" ", "");

    // 验证表达式只包含合法字符
    for c in expr.chars() {
        let is_valid = c.is_ascii_digit()
            || c == '.'
            || c == '+'
            || c == '-'
            || c == '*'
            || c == '/'
            || c == '('
            || c == ')';
        if !is_valid {
            return Err(UtilError::Calculation(format!("Invalid character: {}", c)));
        }
    }

    // 使用简单的表达式求值
    let result = parse_expression_full(&expr)?;
    Ok(result)
}

/// 解析表达式（递归下降解析器）- 完整版本
fn parse_expression_full(expr: &str) -> Result<f64, UtilError> {
    let (left, rest) = parse_term_with_rest(expr)?;

    if !rest.is_empty() {
        let op = rest.chars().next().unwrap();
        if op == '+' || op == '-' {
            let right = parse_expression_full(&rest[1..])?;
            return Ok(if op == '+' {
                left + right
            } else {
                left - right
            });
        }
    }

    Ok(left)
}

/// 解析项（处理乘除）- 带剩余返回
fn parse_term_with_rest(expr: &str) -> Result<(f64, &str), UtilError> {
    let (mut left, mut rest) = parse_factor_with_rest(expr)?;

    while !rest.is_empty() {
        let op = rest.chars().next().unwrap();
        if op == '*' || op == '/' {
            let (right, new_rest) = parse_factor_with_rest(&rest[1..])?;
            left = if op == '*' {
                left * right
            } else {
                if right == 0.0 {
                    return Err(UtilError::Calculation("Division by zero".to_string()));
                }
                left / right
            };
            rest = new_rest;
        } else {
            break;
        }
    }

    Ok((left, rest))
}

/// 解析因子（处理数字和括号）- 带剩余返回
fn parse_factor_with_rest(expr: &str) -> Result<(f64, &str), UtilError> {
    let expr = expr.trim_start();

    if expr.is_empty() {
        return Err(UtilError::Calculation(
            "Unexpected end of expression".to_string(),
        ));
    }

    // 处理括号
    if expr.starts_with('(') {
        let closing = expr
            .find(')')
            .ok_or_else(|| UtilError::Calculation("Unclosed parenthesis".to_string()))?;

        let inner = &expr[1..closing];
        let value = parse_expression_full(inner)?;

        return Ok((value, &expr[closing + 1..]));
    }

    // 解析数字
    let end = expr
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(expr.len());

    let num_str = &expr[..end];
    let value = num_str
        .parse::<f64>()
        .map_err(|_| UtilError::Calculation(format!("Invalid number: {}", num_str)))?;

    Ok((value, &expr[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_calculate() {
        // 测试基本运算
        assert_eq!(calculate(10.0, "+".to_string(), 5.0).await.unwrap(), 15.0);
        assert_eq!(calculate(10.0, "-".to_string(), 5.0).await.unwrap(), 5.0);
        assert_eq!(calculate(10.0, "*".to_string(), 5.0).await.unwrap(), 50.0);
        assert_eq!(calculate(10.0, "/".to_string(), 5.0).await.unwrap(), 2.0);
    }

    #[tokio::test]
    async fn test_eval_expression() {
        // 测试表达式计算
        assert_eq!(eval_expression("1 + 2".to_string()).await.unwrap(), 3.0);
        assert_eq!(
            eval_expression("2 * 3 + 4".to_string()).await.unwrap(),
            10.0
        );
        assert_eq!(
            eval_expression("(1 + 2) * 3".to_string()).await.unwrap(),
            9.0
        );
        assert_eq!(
            eval_expression("10 / 2 - 1".to_string()).await.unwrap(),
            4.0
        );
    }

    #[tokio::test]
    async fn test_get_current_time() {
        // 测试获取当前时间
        let time = get_current_time(None).await.unwrap();
        assert!(!time.is_empty());

        let time_formatted = get_current_time(Some("%Y-%m-%d".to_string()))
            .await
            .unwrap();
        // 简单检查格式：应该包含 3 个 '-'
        assert_eq!(time_formatted.chars().filter(|&c| c == '-').count(), 2);
        assert!(time_formatted.len() == 10); // YYYY-MM-DD 格式长度
    }
}
