use crate::{
    parsers::{JsonParser, OutputParser},
    request::{ResponseFormat, ToolFunction, ToolSpec},
    state::{ChatModel, InvokeOptions},
};
use schemars::{JsonSchema, schema::RootSchema};
use serde::de::DeserializeOwned;
use std::{marker::PhantomData, sync::Arc};
use thiserror::Error;

/// Structured Output Error
#[derive(Debug, Error)]
pub enum StructuredOutputError {
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("model error: {0}")]
    Model(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("parsing error: {0}")]
    Parsing(String),
}

/// A trait that marks a struct as being capable of being used as a structured output.
pub trait StructuredOutput: JsonSchema + DeserializeOwned + Send + Sync + 'static {
    /// Returns the JSON Schema definition for this type.
    fn schema() -> RootSchema {
        schemars::schema_for!(Self)
    }

    /// Returns the JSON Output definition in the format of OpenAI ResponseFormat (JsonSchema)
    fn definition() -> ResponseFormat {
        let schema = Self::schema();
        let schema_json = serde_json::to_string(&schema).unwrap_or_else(|_| "{}".to_owned());
        ResponseFormat::json_schema(schema_json)
    }

    /// Returns the Tool definition for this structure (for Function Calling strategy)
    fn tool_definition() -> ToolSpec {
        let schema = Self::schema();
        let schema_json = serde_json::to_value(&schema).unwrap();

        ToolSpec::Function {
            function: ToolFunction {
                name: "extract_data".to_owned(),
                description: "Extract structured data based on the schema".to_owned(),
                parameters: schema_json,
            },
        }
    }
}

impl<T> StructuredOutput for T where T: JsonSchema + DeserializeOwned + Send + Sync + 'static {}

/// Strategy trait for structured output.
#[async_trait::async_trait]
pub trait StructuredOutputStrategy<T>: Send + Sync {
    /// Prepare options (e.g., set response_format, tools).
    /// Can also modify messages (e.g., inject prompt instructions).
    fn prepare(
        &self,
        options: &mut InvokeOptions,
        messages: &mut Vec<Arc<crate::message::Message>>,
    );

    /// Provide tools required by this strategy.
    fn tools(&self) -> Option<Vec<ToolSpec>> {
        None
    }

    /// Parse the model output into the target type.
    fn parse(&self, message: &crate::message::Message) -> Result<T, StructuredOutputError>;
}

/// Strategy: OpenAI JSON Schema (response_format: { type: "json_schema" })
pub struct JsonSchemaStrategy<T> {
    _marker: PhantomData<T>,
}

impl<T> JsonSchemaStrategy<T> {
    pub fn new() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<T> Default for JsonSchemaStrategy<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl<T: StructuredOutput> StructuredOutputStrategy<T> for JsonSchemaStrategy<T> {
    fn prepare(
        &self,
        options: &mut InvokeOptions,
        _messages: &mut Vec<Arc<crate::message::Message>>,
    ) {
        options.response_format = Some(T::definition());
    }

    fn parse(&self, message: &crate::message::Message) -> Result<T, StructuredOutputError> {
        let content = message.content();
        serde_json::from_str(content).map_err(|e| {
            StructuredOutputError::Parsing(format!(
                "Failed to parse JSON Schema output: {}; Content: {}",
                e, content
            ))
        })
    }
}

/// Strategy: JSON Mode (response_format: { type: "json_object" } + Prompt Injection)
pub struct JsonModeStrategy<T> {
    _marker: PhantomData<T>,
}

impl<T> JsonModeStrategy<T> {
    pub fn new() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

impl<T> Default for JsonModeStrategy<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl<T: StructuredOutput> StructuredOutputStrategy<T> for JsonModeStrategy<T> {
    fn prepare(
        &self,
        options: &mut InvokeOptions,
        messages: &mut Vec<Arc<crate::message::Message>>,
    ) {
        options.response_format = Some(ResponseFormat::json_object());

        let schema = T::schema();
        let schema_str = serde_json::to_string_pretty(&schema).unwrap_or_default();
        let instructions = format!(
            "\n\nIMPORTANT: Respond with valid JSON matching this schema:\n```json\n{}\n```",
            schema_str
        );

        let hint_msg = crate::message::Message::user(instructions);
        messages.push(Arc::new(hint_msg));
    }

    fn parse(&self, message: &crate::message::Message) -> Result<T, StructuredOutputError> {
        let content = message.content();
        // Use JsonParser to extract JSON from markdown if present
        let parser = JsonParser::<T>::new();
        parser.parse(content).map_err(|e| {
            StructuredOutputError::Parsing(format!("Failed to parse JSON Mode output: {}", e))
        })
    }
}

/// Strategy: Function Calling (Tools)
pub struct ToolCallingStrategy<T> {
    tool_spec: ToolSpec,
    _marker: PhantomData<T>,
}

impl<T: StructuredOutput> ToolCallingStrategy<T> {
    pub fn new() -> Self {
        Self {
            tool_spec: T::tool_definition(),
            _marker: PhantomData,
        }
    }
}

impl<T: StructuredOutput> Default for ToolCallingStrategy<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl<T: StructuredOutput> StructuredOutputStrategy<T> for ToolCallingStrategy<T> {
    fn prepare(
        &self,
        options: &mut InvokeOptions,
        _messages: &mut Vec<Arc<crate::message::Message>>,
    ) {
        options.tool_choice = Some(self.tool_spec.function_name().to_owned());
    }

    fn tools(&self) -> Option<Vec<ToolSpec>> {
        Some(vec![self.tool_spec.clone()])
    }

    fn parse(&self, message: &crate::message::Message) -> Result<T, StructuredOutputError> {
        if let crate::message::Message::Assistant {
            tool_calls: Some(calls),
            ..
        } = message
            && let Some(call) = calls.first()
        {
            let args = call.arguments();
            let parsed: T =
                serde_json::from_value(args).map_err(StructuredOutputError::Serialization)?;
            return Ok(parsed);
        }
        Err(StructuredOutputError::Parsing(
            "Model did not call the expected tool".into(),
        ))
    }
}

/// The method used to enforce structured output.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum StructuredOutputMethod {
    #[default]
    JsonSchema,
    JsonMode,
    FunctionCalling,
}

/// Extension trait to add structured output capabilities to any ChatModel.
#[async_trait::async_trait]
pub trait ChatModelExt: ChatModel {
    /// Invokes the model and parses the output as a structured type using the default strategy (JsonSchema).
    async fn invoke_structured<T: StructuredOutput>(
        &self,
        input: impl Into<String> + Send,
    ) -> Result<T, StructuredOutputError>
    where
        Self: Sized,
    {
        self.invoke_structured_with_strategy(input, JsonSchemaStrategy::<T>::new())
            .await
    }

    /// Invokes the model and parses the output using a specific strategy.
    async fn invoke_structured_with_strategy<T, S>(
        &self,
        input: impl Into<String> + Send,
        strategy: S,
    ) -> Result<T, StructuredOutputError>
    where
        Self: Sized,
        T: StructuredOutput,
        S: StructuredOutputStrategy<T> + Send + Sync,
    {
        let content = input.into();
        let message = crate::message::Message::user(content);
        let messages = vec![Arc::new(message)];

        let mut options = InvokeOptions::default();
        let mut final_messages = messages.clone();

        // 1. Prepare options and messages using strategy
        strategy.prepare(&mut options, &mut final_messages);

        // 2. Handle tools
        let tools = strategy.tools();
        if let Some(ref t) = tools {
            options.tools = Some(t);
        }

        // 3. Invoke model
        let completion = self
            .invoke(&final_messages, &options)
            .await
            .map_err(StructuredOutputError::Model)?;

        let last_msg = completion.messages.last().ok_or_else(|| {
            StructuredOutputError::Parsing("No message returned from model".to_owned())
        })?;

        // 4. Parse output using strategy
        strategy.parse(last_msg)
    }

    /// Invokes the model using a specific structured output method enum.
    async fn invoke_structured_with_method<T: StructuredOutput>(
        &self,
        input: impl Into<String> + Send,
        method: StructuredOutputMethod,
    ) -> Result<T, StructuredOutputError>
    where
        Self: Sized,
    {
        match method {
            StructuredOutputMethod::JsonSchema => {
                self.invoke_structured_with_strategy(input, JsonSchemaStrategy::<T>::new())
                    .await
            }
            StructuredOutputMethod::JsonMode => {
                self.invoke_structured_with_strategy(input, JsonModeStrategy::<T>::new())
                    .await
            }
            StructuredOutputMethod::FunctionCalling => {
                self.invoke_structured_with_strategy(input, ToolCallingStrategy::<T>::new())
                    .await
            }
        }
    }
}

impl<M: ChatModel> ChatModelExt for M {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Message;
    use crate::state::{ChatCompletion, StandardChatStream};
    use schemars::JsonSchema;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, JsonSchema, PartialEq)]
    struct TestData {
        name: String,
        age: u32,
    }

    struct MockModel {
        expected_method: String, // "json_schema", "json_mode", "function_calling"
        response_content: String,
    }

    #[async_trait::async_trait]
    impl ChatModel for MockModel {
        async fn invoke(
            &self,
            messages: &[Arc<Message>],
            options: &InvokeOptions<'_>,
        ) -> Result<ChatCompletion, Box<dyn std::error::Error + Send + Sync>> {
            // Verification logic based on expected_method
            match self.expected_method.as_str() {
                "json_schema" => {
                    assert!(
                        options
                            .response_format
                            .as_ref()
                            .unwrap()
                            .json_schema
                            .is_some()
                    );
                }
                "json_mode" => {
                    assert_eq!(
                        format!(
                            "{:?}",
                            options.response_format.as_ref().unwrap().format_type
                        ),
                        "JsonObject"
                    );
                    // Check if prompt was injected
                    let last_msg = messages.last().unwrap().content();
                    assert!(last_msg.contains("IMPORTANT: Respond with valid JSON"));
                }
                "function_calling" => {
                    assert!(options.tools.is_some());
                    assert_eq!(options.tool_choice.as_deref(), Some("extract_data"));
                }
                _ => {}
            }

            // Return mock response
            let msg = if self.expected_method == "function_calling" {
                Message::Assistant {
                    content: "".into(),
                    tool_calls: Some(vec![crate::message::ToolCall {
                        id: "call_1".into(),
                        type_name: "function".into(),
                        function: crate::message::FunctionCall {
                            name: "extract_data".into(),
                            arguments: serde_json::from_str(&self.response_content).unwrap(),
                        },
                    }]),
                    name: None,
                }
            } else {
                Message::assistant(self.response_content.clone())
            };

            Ok(ChatCompletion {
                messages: vec![Arc::new(msg)],
                usage: Default::default(),
            })
        }

        async fn stream(
            &self,
            _messages: &[Arc<Message>],
            _options: &InvokeOptions<'_>,
        ) -> Result<StandardChatStream, Box<dyn std::error::Error + Send + Sync>> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn test_json_schema_strategy() {
        let model = MockModel {
            expected_method: "json_schema".into(),
            response_content: r#"{"name": "Alice", "age": 30}"#.into(),
        };

        let result: TestData = model.invoke_structured("test").await.unwrap();
        assert_eq!(
            result,
            TestData {
                name: "Alice".into(),
                age: 30
            }
        );
    }

    #[tokio::test]
    async fn test_json_mode_strategy() {
        let model = MockModel {
            expected_method: "json_mode".into(),
            response_content: r#"{"name": "Bob", "age": 25}"#.into(),
        };

        let result: TestData = model
            .invoke_structured_with_method("test", StructuredOutputMethod::JsonMode)
            .await
            .unwrap();
        assert_eq!(
            result,
            TestData {
                name: "Bob".into(),
                age: 25
            }
        );
    }

    #[tokio::test]
    async fn test_function_calling_strategy() {
        let model = MockModel {
            expected_method: "function_calling".into(),
            response_content: r#"{"name": "Charlie", "age": 40}"#.into(),
        };

        // Explicitly using strategy helper
        let result: TestData = model
            .invoke_structured_with_strategy("test", ToolCallingStrategy::<TestData>::new())
            .await
            .unwrap();
        assert_eq!(
            result,
            TestData {
                name: "Charlie".into(),
                age: 40
            }
        );
    }
}
