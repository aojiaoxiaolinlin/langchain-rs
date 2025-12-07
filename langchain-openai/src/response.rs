use serde::{Deserialize, Serialize};

use crate::message::Message;

#[derive(Serialize, Deserialize, Debug)]
pub struct ResponseBody {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    pub finish_reason: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub completion_tokens_details: TokensDetails,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TokensDetails {
    pub reasoning_tokens: u32,
}
