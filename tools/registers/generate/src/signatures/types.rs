//! ABI shape from the compiler's resolved type graph. No typedef spelling
//! is guessed to mean an integer. Sizes are queried from the same RV32 TU.
use super::ast;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Class {
    Void,
    Integer,
    Pointer,
}

#[derive(Clone, Debug)]
pub(super) struct Type {
    pub class: Class,
    pub spelling: String,
}

impl Type {
    pub fn size_expression(&self) -> String {
        match self.class {
            Class::Pointer => "sizeof(void *)".into(),
            _ => format!("sizeof({})", self.spelling),
        }
    }
}

fn classify(node: &Value) -> std::result::Result<Class, String> {
    if node.get("containsErrors").and_then(Value::as_bool) == Some(true) {
        return Err("invalid type".into());
    }
    let kind = node.get("kind").and_then(Value::as_str).unwrap_or("");
    match kind {
        "PointerType" => Ok(Class::Pointer),
        "DecayedType" => {
            // Array and function parameters are adjusted to pointers by C.
            // The compiler retains the original type before the adjusted one.
            let adjusted = ast::children(node)
                .last()
                .ok_or("unresolved parameter adjustment")?;
            if adjusted["kind"] != "PointerType" {
                return Err("parameter adjustment is not a pointer".into());
            }
            Ok(Class::Pointer)
        }
        "EnumType" | "BitIntType" => Ok(Class::Integer),
        "BuiltinType" => {
            let name = node
                .pointer("/type/qualType")
                .and_then(Value::as_str)
                .unwrap_or("");
            match name {
                "void" => Ok(Class::Void),
                "_Bool" | "bool" | "char" | "signed char" | "unsigned char" | "short"
                | "unsigned short" | "int" | "unsigned int" | "long" | "unsigned long"
                | "long long" | "unsigned long long" | "__int128" | "unsigned __int128" => {
                    Ok(Class::Integer)
                }
                _ => Err(format!("floating or unsupported builtin {name}")),
            }
        }
        "TypedefType" | "ElaboratedType" | "ParenType" | "AttributedType" | "QualType" => {
            let inner = ast::children(node);
            let [child] = inner else {
                return Err(format!("unresolved {kind}"));
            };
            classify(child)
        }
        "RecordType" => Err("aggregate by value".into()),
        _ => Err(format!("unsupported type {kind}")),
    }
}

fn typed(node: &Value) -> std::result::Result<Type, String> {
    let class = classify(node)?;
    let spelling = node
        .pointer("/type/qualType")
        .and_then(Value::as_str)
        .ok_or("type without spelling")?
        .to_owned();
    if class == Class::Integer && (spelling.contains("unnamed") || spelling.contains("anonymous")) {
        return Err("anonymous integer type without a C type name".into());
    }
    Ok(Type { class, spelling })
}

#[derive(Clone, Debug)]
pub(super) struct Prototype {
    pub result: Type,
    pub parameters: Vec<Type>,
}

/// The generated typeof alias has one type-of-expression node, whose last
/// child is the resolved function type (the first child is its expression).
/// Looking only there avoids confusing a returned function pointer's
/// prototype with the function's own prototype.
pub(super) fn prototype(alias: &Value) -> std::result::Result<Prototype, String> {
    let outer = ast::children(alias);
    let [outer] = outer else {
        return Err("compiler did not resolve typeof(function)".into());
    };
    if outer["kind"] != "TypeOfExprType" {
        return Err("compiler did not resolve typeof(function)".into());
    }
    let function = ast::children(outer)
        .last()
        .ok_or("typeof without resolved type")?;
    if function["kind"] == "FunctionNoProtoType" {
        return Err("declaration has no parameter prototype".into());
    }
    if function["kind"] != "FunctionProtoType" {
        return Err("declaration is not a function".into());
    }
    if function.get("variadic").and_then(Value::as_bool) == Some(true) {
        return Err("variadic function".into());
    }
    if function["cc"] != "cdecl" {
        return Err("unsupported calling convention".into());
    }
    let parts = ast::children(function);
    let (result, parameters) = parts.split_first().ok_or("function without result type")?;
    let result = typed(result).map_err(|e| format!("result: {e}"))?;
    let parameters = parameters
        .iter()
        .map(typed)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if parameters.iter().any(|p| p.class == Class::Void) {
        return Err("void parameter in nonempty prototype".into());
    }
    Ok(Prototype { result, parameters })
}

/// Register words, not C parameter count: an RV32 64-bit scalar uses two
/// successive registers (or the last register plus the stack). Stack words
/// do not justify discarding any of a0..a7. Aggregates/variadics/floats never
/// reach this function. See the RISC-V psABI integer calling convention.
pub(super) fn contract(
    prototype: &Prototype,
    sizes: &[u64],
) -> std::result::Result<(u8, &'static str), String> {
    if sizes.len() != prototype.parameters.len() + 1 {
        return Err("incomplete compiler size proof".into());
    }
    let words = |bytes: u64| match bytes {
        1..=4 => Ok(1u64),
        8 => Ok(2),
        _ => Err(format!("integer size {bytes} is outside the supported ABI")),
    };
    let (result_size, parameter_sizes) = sizes.split_first().ok_or("missing result size proof")?;
    let result = match prototype.result.class {
        Class::Void => "void",
        _ => match words(*result_size)? {
            1 => "u32",
            _ => "u64",
        },
    };
    let mut arguments = 0;
    for size in parameter_sizes {
        arguments += words(*size)?;
    }
    Ok((arguments.min(8) as u8, result))
}
