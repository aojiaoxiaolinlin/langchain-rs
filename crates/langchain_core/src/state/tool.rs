use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc};

use crate::request::{ToolFunction, ToolSpec};

pub type ToolFuture<E> = Pin<Box<dyn Future<Output = Result<Value, E>> + Send>>;

pub type ToolFn<E> = dyn Fn(Value) -> ToolFuture<E> + Send + Sync;

pub struct RegisteredTool<E> {
    pub function: ToolFunction,
    pub handler: Box<ToolFn<E>>,
}

impl<E> RegisteredTool<E> {
    pub fn new(
        name: String,
        description: String,
        parameters: Value,
        handler: Box<ToolFn<E>>,
    ) -> Self {
        let function = ToolFunction {
            name,
            description,
            parameters,
        };
        Self { function, handler }
    }

    pub fn spec(&self) -> ToolSpec {
        ToolSpec::Function {
            function: self.function.clone(),
        }
    }
}

#[macro_export]
macro_rules! tool {
    ($name:expr, $description:expr, |$($arg:ident : $ty:ty),*| $body:expr) => {{

        #[derive(::serde::Deserialize, ::schemars::JsonSchema)]
        struct __ToolArgs {
            $( $arg: $ty ),*
        }

        $crate::state::RegisteredTool::<serde_json::Error>::from_typed(
            $name.to_string(),
            $description.to_string(),
            |args: __ToolArgs| async move {
                let __ToolArgs { $( $arg ),* } = args;
                $body
            }
        )
    }};

    ($name:expr, $description:expr, |$($arg:ident : $ty:ty),*| $body:expr, $($desc:expr),* $(,)?) => {{
        #[derive(::serde::Deserialize, ::schemars::JsonSchema)]
        struct __ToolArgs {
            $(
                #[doc = $desc]
                $arg: $ty
            ),*
        }

        $crate::state::RegisteredTool::<serde_json::Error>::from_typed(
            $name.to_string(),
            $description.to_string(),
            |args: __ToolArgs| async move {
                let __ToolArgs { $( $arg ),* } = args;
                $body
            }
        )
    }};
}

impl<E> RegisteredTool<E>
where
    E: From<serde_json::Error> + Send + Sync + 'static,
{
    pub fn from_typed<F, Fut, Args, Output>(name: String, description: String, f: F) -> Self
    where
        F: Fn(Args) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Output, E>> + Send + 'static,
        Args: DeserializeOwned + JsonSchema + Send + 'static,
        Output: Serialize + Send + 'static,
    {
        let schema = schemars::schema_for!(Args);
        let mut parameters = serde_json::to_value(schema.schema).unwrap();
        if let Value::Object(map) = &mut parameters {
            map.remove("title");
        }
        let f = Arc::new(f);
        let handler: Box<ToolFn<E>> = Box::new(move |value: Value| {
            let f = f.clone();
            Box::pin(async move {
                let args: Args = serde_json::from_value(value).map_err(E::from)?;
                let output = (f.as_ref())(args).await?;
                let value = serde_json::to_value(output).map_err(E::from)?;
                Ok(value)
            })
        });
        RegisteredTool::new(name, description, parameters, handler)
    }
}

#[cfg(test)]
mod tests {
    extern crate self as langchain_core;
    use super::*;
    use langchain_core_macro::tool;
    use schemars::JsonSchema;
    use serde::Deserialize;

    #[derive(Debug)]
    enum TestError {
        #[expect(unused)]
        Json(serde_json::Error),
    }

    impl From<serde_json::Error> for TestError {
        fn from(e: serde_json::Error) -> Self {
            TestError::Json(e)
        }
    }

    #[derive(Deserialize, JsonSchema)]
    struct AddArgs {
        a: i32,
        b: i32,
    }

    async fn add(args: AddArgs) -> Result<i32, TestError> {
        Ok(args.a + args.b)
    }

    #[test]
    fn registered_tool_from_typed_builds_schema() {
        let tool: RegisteredTool<TestError> = RegisteredTool::from_typed(
            "add".to_string(),
            "add numbers".to_string(),
            |args: AddArgs| add(args),
        );
        assert_eq!(tool.function.name, "add");
        if let Value::Object(map) = &tool.function.parameters {
            assert!(map.get("type").is_some());
            assert!(map.get("properties").is_some());
            assert!(map.get("required").is_some());
            assert!(map.get("title").is_none());
        } else {
            panic!("parameters must be object");
        }
    }

    #[test]
    fn tool_macro_builds_registered_tool_basic() {
        let tool: RegisteredTool<serde_json::Error> = tool!(
            "calc_add",
            "add numbers via llm",
            |a: i32, b: i32| Ok(a + b)
        );

        assert_eq!(tool.function.name, "calc_add");
        if let Value::Object(map) = &tool.function.parameters {
            assert!(map.get("properties").is_some());
        } else {
            panic!("parameters must be object");
        }
    }

    #[test]
    fn tool_macro_builds_registered_tool_with_descriptions() {
        let tool: RegisteredTool<serde_json::Error> = tool!(
            "calc_add2",
            "add numbers via llm 2",
            |a: i32, b: i32| Ok(a + b),
            "first number",
            "second number",
        );

        assert_eq!(tool.function.name, "calc_add2");
        if let Value::Object(map) = &tool.function.parameters {
            assert!(map.get("properties").is_some());
        } else {
            panic!("parameters must be object");
        }
    }

    #[derive(Debug)]
    enum NodeRunError {
        #[expect(unused)]
        Json(serde_json::Error),
    }

    impl From<serde_json::Error> for NodeRunError {
        fn from(e: serde_json::Error) -> Self {
            NodeRunError::Json(e)
        }
    }

    #[tool(description = "计算两个数的和", args(a = "第一个数", b = "第二个数"))]
    async fn add_attr(a: f64, b: f64) -> Result<f64, NodeRunError> {
        Ok(a + b)
    }

    #[test]
    fn tool_attribute_builds_registered_tool() {
        let tool: RegisteredTool<NodeRunError> = add_attr_tool();
        assert_eq!(tool.function.name, "add_attr");
        if let Value::Object(map) = &tool.function.parameters {
            assert!(map.get("type").is_some());
            assert!(map.get("properties").is_some());
            assert!(map.get("required").is_some());
        } else {
            panic!("parameters must be object");
        }
    }
}
