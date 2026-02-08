//! Web 搜索工具
//!
//! 使用 DuckDuckGo API 进行 Web 搜索。

use langchain_core::tool;
use thiserror::Error;

/// Web 搜索错误
#[derive(Debug, Error)]
pub enum WebSearchError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("No results found")]
    NoResults,

    #[error("Parse error: {0}")]
    Parse(String),
}

/// Web 搜索结果
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResult {
    pub title: String,
    pub snippet: String,
    pub url: String,
}

/// 使用 DuckDuckGo 进行 Web 搜索
#[tool(
    description = "Search the web for information using DuckDuckGo",
    args(
        query = "Search query",
        max_results = "Maximum number of results (default: 5)"
    )
)]
pub async fn search_web(
    query: String,
    max_results: Option<usize>,
) -> Result<Vec<SearchResult>, WebSearchError> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (compatible; LangChainBot/1.0)")
        .build()?;

    let url = format!(
        "https://api.duckduckgo.com/?q={}&format=json",
        urlencoding::encode(&query)
    );

    tracing::debug!("Searching DuckDuckGo: {}", url);

    let response = client.get(&url).send().await?;
    let json: serde_json::Value = response.json().await?;

    let results: Vec<SearchResult> = json["RelatedTopics"]
        .as_array()
        .ok_or(WebSearchError::NoResults)?
        .iter()
        .filter_map(|topic| {
            let text = topic["Text"].as_str()?;
            let first_url = topic["FirstURL"].as_str()?;

            // 解析标题和摘要
            let parts: Vec<&str> = text.splitn(2, " - ").collect();
            let (title, snippet) = if parts.len() == 2 {
                (parts[0].to_string(), parts[1].to_string())
            } else {
                (text.to_string(), String::new())
            };

            Some(SearchResult {
                title,
                snippet,
                url: first_url.to_string(),
            })
        })
        .take(max_results.unwrap_or(5))
        .collect();

    if results.is_empty() {
        return Err(WebSearchError::NoResults);
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // 需要网络连接
    async fn test_search_web() {
        let results = search_web("Rust programming language".to_string(), Some(3))
            .await
            .unwrap();
        assert!(!results.is_empty());
        assert!(results.len() <= 3);

        for result in results {
            println!("Title: {}", result.title);
            println!("URL: {}", result.url);
            println!("---");
        }
    }
}
