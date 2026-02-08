//! langchain-tools 使用示例
//!
//! 展示如何使用内置工具

use langchain_tools::{calculate, eval_expression, get_current_time, read_file, write_file};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== LangChain Tools Demo ===\n");

    // 1. 获取当前时间
    println!("1. 获取当前时间:");
    let time = get_current_time(None).await?;
    println!("   当前时间: {}\n", time);

    // 2. 简单计算
    println!("2. 计算器:");
    let result = calculate(10.0, "+".to_string(), 5.0).await?;
    println!("   10 + 5 = {}\n", result);

    // 3. 文件操作
    println!("3. 文件操作:");
    let temp_file = "demo_example.txt";
    let content = "Hello from langchain-tools!";

    // 写入文件
    let write_result = write_file(temp_file.to_string(), content.to_string()).await?;
    println!("   写入: {}", write_result);

    // 读取文件
    let read_content = read_file(temp_file.to_string()).await?;
    println!("   读取: {}", read_content);

    // 清理
    tokio::fs::remove_file(temp_file).await?;
    println!("   清理完成\n");

    // 4. 表达式计算
    println!("4. 表达式计算:");
    let expr_result = eval_expression("(2 + 3) * 4".to_string()).await?;
    println!("   (2 + 3) * 4 = {}\n", expr_result);

    println!("=== Demo 完成 ===");

    Ok(())
}
