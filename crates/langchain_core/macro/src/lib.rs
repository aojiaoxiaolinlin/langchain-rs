use std::collections::BTreeMap;

use darling::ast::NestedMeta;
use darling::{Error, FromMeta};
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Expr, ItemFn, Lit, Meta, parse_macro_input, spanned::Spanned};

#[derive(Debug, FromMeta)]
struct ToolAttrArgs {
    #[darling(default)]
    name: Option<String>,
    description: String,
    #[darling(default)]
    args: ArgsMeta,
}

#[derive(Debug, Default)]
struct ArgsMeta {
    docs: BTreeMap<String, String>,
}

impl FromMeta for ArgsMeta {
    fn from_meta(item: &Meta) -> darling::Result<Self> {
        match item {
            Meta::List(list) => {
                let nested = NestedMeta::parse_meta_list(list.tokens.clone())?;
                let mut docs = BTreeMap::new();

                for nm in nested {
                    match nm {
                        NestedMeta::Meta(Meta::NameValue(nv)) => {
                            if let Some(ident) = nv.path.get_ident() {
                                if let Expr::Lit(expr_lit) = &nv.value
                                    && let Lit::Str(s) = &expr_lit.lit
                                {
                                    docs.insert(ident.to_string(), s.value());
                                    continue;
                                }

                                return Err(Error::custom("args values must be string literals")
                                    .with_span(&nv.value));
                            } else {
                                return Err(Error::custom("args keys must be identifiers")
                                    .with_span(&nv.path));
                            }
                        }
                        other => {
                            return Err(Error::custom(
                                "args entries must be `name = \"...\"` pairs",
                            )
                            .with_span(&other));
                        }
                    }
                }

                Ok(ArgsMeta { docs })
            }
            _ => Err(Error::custom("args must be a list").with_span(item)),
        }
    }

    fn from_none() -> Option<Self> {
        Some(ArgsMeta::default())
    }
}

#[proc_macro_attribute]
pub fn tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    let meta_list = match NestedMeta::parse_meta_list(attr.into()) {
        Ok(list) => list,
        Err(e) => return e.to_compile_error().into(),
    };
    let parsed = match ToolAttrArgs::from_list(&meta_list) {
        Ok(v) => v,
        Err(e) => return e.write_errors().into(),
    };
    let func = parse_macro_input!(item as ItemFn);

    let name_override = parsed.name.clone();
    let description = parsed.description;
    let arg_docs = parsed.args.docs;

    // 2. 分析原函数：名字、参数列表、返回类型
    let fn_name = &func.sig.ident;
    let tool_name = name_override.clone().unwrap_or_else(|| fn_name.to_string());
    let args_struct_ident = format_ident!("{}Args", to_camel_case(&tool_name));
    let tool_fn_ident = format_ident!("{}_tool", fn_name);

    // 参数列表
    let mut arg_fields = Vec::new();
    let mut arg_bindings = Vec::new();
    let mut arg_pats = Vec::new();

    for input in &func.sig.inputs {
        if let syn::FnArg::Typed(pat_type) = input {
            let ident = match &*pat_type.pat {
                syn::Pat::Ident(pi) => &pi.ident,
                _ => {
                    return syn::Error::new(
                        pat_type.span(),
                        "tool: only simple ident arguments are supported",
                    )
                    .to_compile_error()
                    .into();
                }
            };
            let ty = &*pat_type.ty;
            let doc = arg_docs.get(&ident.to_string()).cloned();

            // 生成 struct 字段
            let field = if let Some(doc_str) = doc {
                quote! {
                    #[doc = #doc_str]
                    #ident: #ty
                }
            } else {
                quote! {
                    #ident: #ty
                }
            };

            arg_fields.push(field);
            arg_bindings.push(ident);
            arg_pats.push(quote! { #ident });
        }
    }

    // 3. 提取返回类型中的错误类型 E（Result<_, E>）
    let output = &func.sig.output;
    let err_ty = match output {
        syn::ReturnType::Type(_, ty) => {
            // 期待形如 Result<T, E>
            if let syn::Type::Path(tp) = ty.as_ref() {
                if let Some(seg) = tp.path.segments.last() {
                    if seg.ident == "Result" {
                        if let syn::PathArguments::AngleBracketed(ab) = &seg.arguments {
                            if ab.args.len() == 2 {
                                if let syn::GenericArgument::Type(e_ty) = &ab.args[1] {
                                    e_ty.clone()
                                } else {
                                    return syn::Error::new(
                                        ab.span(),
                                        "tool: unsupported Result error type",
                                    )
                                    .to_compile_error()
                                    .into();
                                }
                            } else {
                                return syn::Error::new(
                                    ab.span(),
                                    "tool: Result must have two type parameters",
                                )
                                .to_compile_error()
                                .into();
                            }
                        } else {
                            return syn::Error::new(seg.span(), "tool: unsupported Result type")
                                .to_compile_error()
                                .into();
                        }
                    } else {
                        return syn::Error::new(
                            seg.span(),
                            "tool: function must return Result<T, E>",
                        )
                        .to_compile_error()
                        .into();
                    }
                } else {
                    return syn::Error::new(tp.span(), "tool: invalid return type path")
                        .to_compile_error()
                        .into();
                }
            } else {
                return syn::Error::new(ty.span(), "tool: function must return Result<T, E>")
                    .to_compile_error()
                    .into();
            }
        }
        _ => {
            return syn::Error::new(func.sig.span(), "tool: function must return Result<T, E>")
                .to_compile_error()
                .into();
        }
    };

    let tool_name_lit = tool_name;
    let description_lit = description;

    // 4. 生成代码：保留原函数 + Args struct + *_tool 函数
    let vis = &func.vis;

    let expanded = quote! {
        #func

        #[derive(::serde::Deserialize, ::schemars::JsonSchema)]
        struct #args_struct_ident {
            #(#arg_fields),*
        }

        #vis fn #tool_fn_ident(
        ) -> langchain_core::state::RegisteredTool<#err_ty>
        where
            #err_ty: From<serde_json::Error> + Send + Sync + 'static,
        {
            langchain_core::state::RegisteredTool::from_typed(
                #tool_name_lit.to_string(),
                #description_lit.to_string(),
                |args: #args_struct_ident| async move {
                    let #args_struct_ident { #(#arg_bindings),* } = args;
                    #fn_name(#(#arg_pats),*).await
                },
            )
        }
    };

    expanded.into()
}

// 很简单的 snake_case -> CamelCase 辅助
fn to_camel_case(name: &str) -> String {
    let mut s = String::new();
    let mut upper = true;
    for c in name.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            s.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            s.push(c);
        }
    }
    s
}
