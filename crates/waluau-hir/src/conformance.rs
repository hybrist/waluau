//! Explicit interface conformance checking (`type Add = Op & { ... }`).
//!
//! A conformance declaration obligates the declaring type to satisfy the
//! named interface with `self` substituted by the declaring type. The check
//! runs after whole-program return-type inference, so implementations may be
//! `function Add:name(...)` method declarations anywhere in the program
//! (before or after the type declaration) or record fields of the declaring
//! type. Function types stay invariant beyond the `self` slot.
//!
//! Beyond validation, this module implements the bound-method coercion that
//! makes conformance useful: [`generate_conformance_wrappers`] emits one
//! ordinary free function per conformance pair that builds the interface
//! record from a receiver, and [`desugar_conformance_coercions`] rewrites
//! coercion sites (`local op: Op = add`, argument passing, returns, `::`
//! casts, ...) into calls to those constructors. Both run inside HIR, so IR
//! lowering and codegen only ever see ordinary calls, closures, and records.

use std::collections::{BTreeMap, HashMap, HashSet};

use waluau_ast::{
    BinaryOp, Expr, Function, FunctionExpr, FunctionName, NumberLiteral, NumericType, Param,
    Program, Rebindability, Stmt, TableField, Type, TypeDeclaration,
};
use waluau_diagnostics::Diagnostic;

use crate::expressions::{builtin_name, infer_expr, method_signature, type_method_signature};
use crate::signatures::{FnSignature, active_type_param_set};
use crate::statements::{checked_if_cast_scopes, narrowed_scopes};
use crate::{
    Binding, binding_for, method_signature_name, module_type_display, module_type_display_name,
};

/// The interfaces each type declaration conforms to, keyed by the
/// declaration's canonical name.
///
/// This is the lookup the bound-method coercion consumes: coercing a value of
/// nominal type `T` to interface type `I` is legal exactly when `table[T]`
/// contains `I`, and [`check_conformance_declarations`] has already
/// guaranteed that every interface field has a matching implementation on
/// `T`.
pub(crate) fn conformance_table(program: &Program) -> HashMap<String, Vec<String>> {
    program
        .type_declarations
        .iter()
        .filter(|decl| !decl.conforms.is_empty())
        .map(|decl| (decl.name.clone(), decl.conforms.clone()))
        .collect()
}

/// Verify every conformance declaration in the program.
///
/// For each `type T = I & { ... }`, every field of interface `I` must have a
/// matching implementation on `T`:
///
/// - a method field (`(self, ...) -> R`) is satisfied by a record field of
///   `T` or a `function T:name(...)` declaration whose type is the field
///   type with `self` substituted by `T`;
/// - a plain function field is satisfied the same way, with no substitution
///   (a method declaration never matches, because desugaring prepends its
///   receiver parameter);
/// - a non-function field is satisfied by a record field of `T` with the
///   same type.
///
/// Besides the diagnostics, returns the names of the generated conformance
/// wrapper functions belonging to failed pairs; their bodies would only
/// produce cascade errors, so the caller skips checking them.
pub(crate) fn check_conformance_declarations(
    program: &Program,
    fn_signatures: &HashMap<String, FnSignature>,
) -> (Vec<Diagnostic>, HashSet<String>) {
    let mut errors = Vec::new();
    let mut failed_wrappers = HashSet::new();
    let decls: HashMap<&str, &TypeDeclaration> = program
        .type_declarations
        .iter()
        .map(|decl| (decl.name.as_str(), decl))
        .collect();
    for decl in &program.type_declarations {
        for interface_name in &decl.conforms {
            let before = errors.len();
            check_conformance(decl, interface_name, &decls, fn_signatures, &mut errors);
            if errors.len() > before {
                failed_wrappers.insert(conformance_wrapper_name(&decl.name, interface_name));
                failed_wrappers.insert(conformance_check_name(&decl.name, interface_name));
                failed_wrappers.insert(conformance_cast_name(&decl.name, interface_name));
            }
        }
    }
    (errors, failed_wrappers)
}

fn check_conformance(
    decl: &TypeDeclaration,
    interface_name: &str,
    decls: &HashMap<&str, &TypeDeclaration>,
    fn_signatures: &HashMap<String, FnSignature>,
    errors: &mut Vec<Diagnostic>,
) {
    let type_name = &decl.source_name;
    let error = |message: String| {
        Diagnostic::new(message).with_file_path_if_missing(decl.file_path.clone())
    };
    let Some(interface) = decls.get(interface_name) else {
        errors.push(error(format!(
            "unknown interface '{}' in the conformance declaration of type '{type_name}'",
            module_type_display_name(interface_name),
        )));
        return;
    };
    let interface_display = &interface.source_name;
    if !interface.type_params.is_empty() {
        errors.push(error(format!(
            "type '{type_name}' cannot conform to '{interface_display}': a \
             generic type cannot be used as an interface"
        )));
        return;
    }
    // An interface may be reached through alias declarations
    // (`type I2 = I1`); the nominal wrappers do not change its fields.
    let mut interface_ty = &interface.ty;
    while let Type::Opaque { ty, .. } = interface_ty {
        interface_ty = ty;
    }
    let Type::Record(interface_fields) = interface_ty else {
        errors.push(error(format!(
            "type '{type_name}' cannot conform to '{interface_display}': an \
             interface must be a record type, got {}",
            module_type_display(&interface.ty),
        )));
        return;
    };
    let Type::Record(own_fields) = &decl.ty else {
        // The parser only records conformance on record-shaped declarations.
        return;
    };
    // The nominal type `self` substitutes to, spelled exactly like a
    // resolved `Type::Named` reference to this declaration.
    let self_ty = Type::Opaque {
        name: decl.name.clone(),
        ty: Box::new(decl.ty.clone()),
        generic_extern: None,
    };

    for (field_name, field_ty) in interface_fields {
        // The hidden identity field belongs to the wrapper machinery, not to
        // the conformance obligation.
        if field_name == META_FIELD {
            continue;
        }
        let expected = match field_ty {
            Type::Function {
                params,
                return_type,
                has_self: true,
            } => {
                let mut with_receiver = Vec::with_capacity(params.len() + 1);
                with_receiver.push(self_ty.clone());
                with_receiver.extend(params.iter().cloned());
                Type::Function {
                    params: with_receiver,
                    return_type: return_type.clone(),
                    has_self: false,
                }
            }
            other => other.clone(),
        };
        let expected_display = module_type_display(&expected);
        let is_function_field = matches!(field_ty, Type::Function { .. });

        if let Some(actual) = own_fields.get(field_name) {
            if !nominal_types_match(&expected, actual) {
                errors.push(error(format!(
                    "type '{type_name}' does not conform to interface \
                     '{interface_display}': field '{field_name}' has type {}, \
                     but the interface requires {expected_display}",
                    module_type_display(actual),
                )));
            }
            continue;
        }

        if is_function_field
            && let Some(signature) =
                fn_signatures.get(&method_signature_name(&decl.name, field_name))
        {
            match signature {
                FnSignature::Mono {
                    params,
                    vararg: None,
                    return_type,
                } => {
                    let actual = Type::Function {
                        params: params.clone(),
                        return_type: Box::new(return_type.clone()),
                        has_self: false,
                    };
                    if !nominal_types_match(&expected, &actual) {
                        errors.push(error(format!(
                            "type '{type_name}' does not conform to interface \
                             '{interface_display}': method '{field_name}' has \
                             type {}, but the interface requires \
                             {expected_display}",
                            module_type_display(&actual),
                        )));
                    }
                }
                _ => {
                    errors.push(error(format!(
                        "type '{type_name}' does not conform to interface \
                         '{interface_display}': method '{field_name}' must be \
                         a plain (non-variadic, non-generic) function with \
                         type {expected_display}"
                    )));
                }
            }
            continue;
        }

        if is_function_field {
            errors.push(error(format!(
                "type '{type_name}' does not conform to interface \
                 '{interface_display}': missing method '{field_name}'; \
                 declare 'function {type_name}:{field_name}(...)' or a record \
                 field with type {expected_display}"
            )));
        } else {
            errors.push(error(format!(
                "type '{type_name}' does not conform to interface \
                 '{interface_display}': missing field '{field_name}' with \
                 type {expected_display}"
            )));
        }
    }
}

/// Structural type equality that identifies nominal aliases by name alone.
///
/// The resolver may expand two mentions of the same alias to different
/// depths (recursion anchors keep cycles finite), so an exact `Type`
/// comparison can distinguish types that are the same. Mirroring
/// `coerce_type`'s nominal rule, two `Opaque` types match exactly when their
/// canonical names match; everything else compares structurally.
fn nominal_types_match(left: &Type, right: &Type) -> bool {
    match (left, right) {
        (Type::Opaque { name: left, .. }, Type::Opaque { name: right, .. }) => left == right,
        (
            Type::Function {
                params: left_params,
                return_type: left_return,
                has_self: left_self,
            },
            Type::Function {
                params: right_params,
                return_type: right_return,
                has_self: right_self,
            },
        ) => {
            left_self == right_self
                && left_params.len() == right_params.len()
                && left_params
                    .iter()
                    .zip(right_params)
                    .all(|(left, right)| nominal_types_match(left, right))
                && nominal_types_match(left_return, right_return)
        }
        (Type::Record(left), Type::Record(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|((left_name, left_ty), (right_name, right_ty))| {
                        left_name == right_name && nominal_types_match(left_ty, right_ty)
                    })
        }
        (Type::Nullable(left), Type::Nullable(right))
        | (Type::Array(left), Type::Array(right))
        | (Type::Variadic(left), Type::Variadic(right))
        | (Type::ExternSubtype(left), Type::ExternSubtype(right)) => {
            nominal_types_match(left, right)
        }
        (Type::Multi(left), Type::Multi(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| nominal_types_match(left, right))
        }
        (Type::TaggedVariant(left), Type::TaggedVariant(right)) => {
            left.tag == right.tag && nominal_types_match(&left.payload, &right.payload)
        }
        (Type::TaggedUnion(left), Type::TaggedUnion(right)) => {
            left.len() == right.len()
                && left.iter().zip(right).all(|(left, right)| {
                    left.tag == right.tag && nominal_types_match(&left.payload, &right.payload)
                })
        }
        _ => left == right,
    }
}

// ---------------------------------------------------------------------------
// Wrapper identity: hidden brand + receiver field
// ---------------------------------------------------------------------------

/// Hidden interface-record field carrying wrapper identity. On a plain
/// interface literal the field is nil (nullable record fields may be omitted
/// from literals and lower to nil); on a conformance wrapper it holds the
/// concrete type's brand and a reference to the original receiver. The `$`
/// keeps the name outside the identifier space of user programs, and record
/// display skips `$` fields, so the type system carries the field without
/// surfacing it.
pub(crate) const META_FIELD: &str = "__conform$meta";
const META_BRAND_FIELD: &str = "brand";
const META_RECEIVER_FIELD: &str = "receiver";

/// The type of the hidden identity field: `{ brand: i32, receiver: unknown }?`.
/// Nullable, so plain interface literals may omit it; the receiver is boxed
/// as `unknown` because concrete types with different shapes share one
/// interface.
fn meta_field_type() -> Type {
    Type::Nullable(Box::new(Type::Record(BTreeMap::from([
        (
            META_BRAND_FIELD.to_string(),
            Type::Numeric(NumericType::I32),
        ),
        (META_RECEIVER_FIELD.to_string(), Type::Unknown),
    ]))))
}

/// Deterministic i32 brand per conforming type: the 1-based rank of the
/// declaration's canonical name among all conforming declarations, in sorted
/// order. Canonical names are already link-time canonicalized (both linkers
/// round-trip `conforms` entries through their rewriters), so the assignment
/// depends only on the set of names in the linked program, not on module
/// order. Brands never escape the compiled program: they only exist as
/// constants inside generated wrapper and check functions.
pub(crate) fn conformance_brands(program: &Program) -> HashMap<String, i32> {
    let mut names: Vec<&str> = program
        .type_declarations
        .iter()
        .filter(|decl| !decl.conforms.is_empty())
        .map(|decl| decl.name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .enumerate()
        .map(|(index, name)| (name.to_string(), index as i32 + 1))
        .collect()
}

/// The declaration that owns the record type of an interface named in a
/// conformance declaration, following pre-resolution alias chains
/// (`type I2 = I1`). This is where the hidden identity field is injected.
fn interface_record_root(name: &str, decls: &HashMap<&str, &TypeDeclaration>) -> Option<String> {
    let mut seen = HashSet::new();
    let mut current = name;
    loop {
        if !seen.insert(current.to_string()) {
            return None;
        }
        let decl = decls.get(current)?;
        if !decl.type_params.is_empty() {
            return None;
        }
        match &decl.ty {
            Type::Record(_) => return Some(decl.name.clone()),
            Type::Named { name, type_args } if type_args.is_empty() => current = name,
            _ => return None,
        }
    }
}

// ---------------------------------------------------------------------------
// Bound-method coercion: wrapper generation
// ---------------------------------------------------------------------------

/// Name of the generated constructor that coerces a `type_name` value into an
/// `interface_name` record of bound methods. `$` keeps the name outside the
/// identifier space of user programs (mirroring overload variant names).
pub(crate) fn conformance_wrapper_name(type_name: &str, interface_name: &str) -> String {
    format!("__conform${type_name}${interface_name}")
}

/// Name of the generated brand check that recovers a `type_name` receiver
/// from an `interface_name` value, or nil when the value does not wrap one.
/// `if T(x) = op then` narrowing desugars onto this function.
pub(crate) fn conformance_check_name(type_name: &str, interface_name: &str) -> String {
    format!("__conformcheck${type_name}${interface_name}")
}

/// Name of the generated hard cast that recovers a `type_name` receiver from
/// an `interface_name` value or raises an error. `op :: T` desugars onto
/// this function.
pub(crate) fn conformance_cast_name(type_name: &str, interface_name: &str) -> String {
    format!("__conformcast${type_name}${interface_name}")
}

/// Receiver parameter name of a generated conformance wrapper. Out of the
/// user identifier space so wrapper bodies never shadow or capture a user
/// binding.
const RECEIVER_PARAM: &str = "__conform_receiver";

/// Generate one ordinary free function per conformance pair:
///
/// ```lua
/// function __conform$Add$Op(__conform_receiver: Add): Op
///     return {
///         exec = function(__conform_arg0: i32, __conform_arg1: i32): i32
///             return __conform_receiver:exec(__conform_arg0, __conform_arg1)
///         end,
///         -- plain function fields and data fields copy from the receiver:
///         name = __conform_receiver.name,
///     }
/// end
/// ```
///
/// Method slots become closures binding the receiver; the colon call inside
/// dispatches through the existing precedence, reaching either the
/// `function Add:exec(...)` desugared free function or `Add`'s own record
/// field. The interface record therefore *wraps* the receiver: mutations
/// through the original value stay visible to the bound methods, while data
/// fields are copied snapshots and the coerced record is a distinct object.
///
/// Runs before type resolution, so every annotation is written as a
/// `Type::Named` reference and resolves like hand-written code. The wrapper
/// takes the conforming declaration's `file_path`, so module privacy sees the
/// receiver's fields and methods exactly where the type lives. Pairs that
/// fail the conformance check produce compile errors anyway;
/// [`check_conformance_declarations`] reports the wrappers of failed pairs so
/// their bodies are excluded from body checking instead of cascading.
pub(crate) fn generate_conformance_wrappers(program: &mut Program) {
    if program
        .type_declarations
        .iter()
        .all(|decl| decl.conforms.is_empty())
    {
        return;
    }
    // Inject the hidden identity field into every interface's record type
    // before any wrapper is built, so wrappers, plain literals, and the
    // generated brand checks all agree on the interface shape.
    let interface_roots: HashSet<String> = {
        let decls: HashMap<&str, &TypeDeclaration> = program
            .type_declarations
            .iter()
            .map(|decl| (decl.name.as_str(), decl))
            .collect();
        program
            .type_declarations
            .iter()
            .flat_map(|decl| decl.conforms.iter())
            .filter_map(|interface_name| interface_record_root(interface_name, &decls))
            .collect()
    };
    for decl in &mut program.type_declarations {
        if interface_roots.contains(&decl.name)
            && let Type::Record(fields) = &mut decl.ty
        {
            fields.insert(META_FIELD.to_string(), meta_field_type());
        }
    }
    let brands = conformance_brands(program);
    let decls: HashMap<&str, &TypeDeclaration> = program
        .type_declarations
        .iter()
        .map(|decl| (decl.name.as_str(), decl))
        .collect();
    let mut generated = Vec::new();
    for decl in &program.type_declarations {
        if !decl.type_params.is_empty() {
            continue;
        }
        for interface_name in &decl.conforms {
            let Some(fields) = declared_interface_fields(interface_name, &decls) else {
                // Unknown or non-record interface: the conformance check
                // reports it; there is nothing to construct.
                continue;
            };
            let brand = brands[decl.name.as_str()];
            generated.push(conformance_wrapper(decl, interface_name, fields, brand));
            generated.push(conformance_check_fn(decl, interface_name, brand));
            generated.push(conformance_cast_fn(decl, interface_name));
        }
    }
    program.functions.append(&mut generated);
}

/// The record fields of an interface named in a conformance declaration,
/// following pre-resolution alias chains (`type I2 = I1`).
fn declared_interface_fields<'a>(
    name: &str,
    decls: &HashMap<&str, &'a TypeDeclaration>,
) -> Option<&'a BTreeMap<String, Type>> {
    let mut seen = HashSet::new();
    let mut current = name;
    loop {
        if !seen.insert(current.to_string()) {
            return None;
        }
        let decl = decls.get(current)?;
        if !decl.type_params.is_empty() {
            return None;
        }
        match &decl.ty {
            Type::Record(fields) => return Some(fields),
            Type::Named { name, type_args } if type_args.is_empty() => current = name,
            _ => return None,
        }
    }
}

fn conformance_wrapper(
    decl: &TypeDeclaration,
    interface_name: &str,
    fields: &BTreeMap<String, Type>,
    brand: i32,
) -> Function {
    let receiver = || Expr::Name(RECEIVER_PARAM.to_string(), None, None);
    let mut table_fields = Vec::with_capacity(fields.len());
    for (field_name, field_ty) in fields {
        // The hidden identity field records which concrete type built this
        // wrapper and keeps the original receiver recoverable. The receiver
        // reference is stored as-is (boxed to `unknown` by the field
        // coercion), so recovery returns the pre-coercion struct with
        // reference identity intact.
        if field_name == META_FIELD {
            table_fields.push(TableField {
                name: META_FIELD.to_string(),
                value: Expr::TableLiteral {
                    fields: vec![
                        TableField {
                            name: META_BRAND_FIELD.to_string(),
                            value: brand_literal(brand),
                        },
                        TableField {
                            name: META_RECEIVER_FIELD.to_string(),
                            value: receiver(),
                        },
                    ],
                    span: None,
                },
            });
            continue;
        }
        let value = match field_ty {
            Type::Function {
                params,
                return_type,
                has_self: true,
            } => {
                let arg_name = |index: usize| format!("__conform_arg{index}");
                let call = Expr::MethodCall {
                    receiver: Box::new(receiver()),
                    name: field_name.clone(),
                    resolved_name: None,
                    type_args: Vec::new(),
                    args: (0..params.len())
                        .map(|index| Expr::Name(arg_name(index), None, None))
                        .collect(),
                    span: None,
                };
                let body = if return_type.as_ref() == &Type::Unit {
                    vec![Stmt::Expr(call)]
                } else {
                    vec![Stmt::Return(call)]
                };
                Expr::Function(FunctionExpr {
                    name: None,
                    symbol_id: None,
                    implicit_self: None,
                    type_params: Vec::new(),
                    params: params
                        .iter()
                        .enumerate()
                        .map(|(index, ty)| Param {
                            name: arg_name(index),
                            symbol_id: None,
                            ty: ty.clone(),
                        })
                        .collect(),
                    vararg: None,
                    return_type: Some((**return_type).clone()),
                    body,
                    file_path: decl.file_path.clone(),
                    span: None,
                })
            }
            // Plain function fields and data fields copy from the receiver.
            _ => Expr::Field {
                base: Box::new(receiver()),
                name: field_name.clone(),
                resolved_name: None,
                span: None,
            },
        };
        table_fields.push(TableField {
            name: field_name.clone(),
            value,
        });
    }
    Function {
        name: FunctionName::Simple(conformance_wrapper_name(&decl.name, interface_name)),
        symbol_id: None,
        type_params: Vec::new(),
        params: vec![Param {
            name: RECEIVER_PARAM.to_string(),
            symbol_id: None,
            ty: Type::Named {
                name: decl.name.clone(),
                type_args: Vec::new(),
            },
        }],
        vararg: None,
        return_type: Some(Type::Named {
            name: interface_name.to_string(),
            type_args: Vec::new(),
        }),
        body: vec![Stmt::Return(Expr::TableLiteral {
            fields: table_fields,
            span: None,
        })],
        file_path: decl.file_path.clone(),
        span: None,
    }
}

/// Parameter name of the generated brand check and hard cast: the interface
/// value under scrutiny. Like [`RECEIVER_PARAM`], the name never meets user
/// code (generated bodies contain no user statements).
const VALUE_PARAM: &str = "__conform_value";

/// Local holding the identity field inside a generated brand check.
const META_LOCAL: &str = "__conform_meta";

/// Local bound to a recovered receiver. Also used by the `if T(x) = op`
/// statement desugar, where it is spliced into user function bodies: the `$`
/// keeps it out of the user identifier space, and local shadowing makes a
/// fixed name correct even when several narrowing statements share a scope.
const NARROWED_LOCAL: &str = "__conform$narrowed";

fn brand_literal(brand: i32) -> Expr {
    Expr::Number(
        NumberLiteral {
            raw: brand.to_string(),
        },
        None,
    )
}

fn nil_compare(op: BinaryOp, expr: Expr) -> Expr {
    Expr::Binary {
        op,
        left: Box::new(expr),
        right: Box::new(Expr::Nil(None)),
        resolved_name: None,
        span: None,
    }
}

/// Generate the brand check for a conformance pair:
///
/// ```lua
/// function __conformcheck$Add$Op(__conform_value: Op): Add?
///     local __conform_meta = __conform_value.__conform$meta
///     if __conform_meta ~= nil then
///         if __conform_meta.brand == 1 then
///             return __conform_meta.receiver :: Add
///         end
///     end
///     return nil
/// end
/// ```
///
/// A plain interface literal has a nil identity field and a wrapper built by
/// a different concrete type has a different brand; both return nil instead
/// of trapping. Once the brand matches, the `unknown -> Add` unbox is safe
/// even against layout-canonicalized sibling structs, because the brand has
/// already pinned the nominal type; the returned reference is the original
/// receiver.
fn conformance_check_fn(decl: &TypeDeclaration, interface_name: &str, brand: i32) -> Function {
    let meta = || Expr::Name(META_LOCAL.to_string(), None, None);
    let meta_field = |name: &str| Expr::Field {
        base: Box::new(meta()),
        name: name.to_string(),
        resolved_name: None,
        span: None,
    };
    let target_ty = Type::Named {
        name: decl.name.clone(),
        type_args: Vec::new(),
    };
    let body = vec![
        Stmt::Let {
            name: META_LOCAL.to_string(),
            symbol_id: None,
            rebindability: Rebindability::Const,
            ty: None,
            value: Expr::Field {
                base: Box::new(Expr::Name(VALUE_PARAM.to_string(), None, None)),
                name: META_FIELD.to_string(),
                resolved_name: None,
                span: None,
            },
        },
        Stmt::If {
            condition: nil_compare(BinaryOp::NotEq, meta()),
            then_body: vec![Stmt::If {
                condition: Expr::Binary {
                    op: BinaryOp::Eq,
                    left: Box::new(meta_field(META_BRAND_FIELD)),
                    right: Box::new(brand_literal(brand)),
                    resolved_name: None,
                    span: None,
                },
                then_body: vec![Stmt::Return(Expr::Cast {
                    expr: Box::new(meta_field(META_RECEIVER_FIELD)),
                    ty: target_ty.clone(),
                    span: None,
                })],
                else_body: Vec::new(),
            }],
            else_body: Vec::new(),
        },
        Stmt::Return(Expr::Nil(None)),
    ];
    Function {
        name: FunctionName::Simple(conformance_check_name(&decl.name, interface_name)),
        symbol_id: None,
        type_params: Vec::new(),
        params: vec![Param {
            name: VALUE_PARAM.to_string(),
            symbol_id: None,
            ty: Type::Named {
                name: interface_name.to_string(),
                type_args: Vec::new(),
            },
        }],
        vararg: None,
        return_type: Some(Type::Nullable(Box::new(target_ty))),
        body,
        file_path: decl.file_path.clone(),
        span: None,
    }
}

/// Generate the hard cast for a conformance pair:
///
/// ```lua
/// function __conformcast$Add$Op(__conform_value: Op): Add
///     local __conform$narrowed = __conformcheck$Add$Op(__conform_value)
///     if __conform$narrowed ~= nil then
///         return __conform$narrowed
///     end
///     error("interface cast failed: value is not 'Add'")
///     return (nil :: unknown) :: Add
/// end
/// ```
///
/// The mismatch path raises the catchable error tag with a message naming
/// the target type; the trailing cast only satisfies the checker's
/// all-paths-return rule and is unreachable.
fn conformance_cast_fn(decl: &TypeDeclaration, interface_name: &str) -> Function {
    let narrowed = || Expr::Name(NARROWED_LOCAL.to_string(), None, None);
    let target_ty = Type::Named {
        name: decl.name.clone(),
        type_args: Vec::new(),
    };
    let body = vec![
        Stmt::Let {
            name: NARROWED_LOCAL.to_string(),
            symbol_id: None,
            rebindability: Rebindability::Const,
            ty: None,
            value: Expr::Call {
                callee: Box::new(Expr::Name(
                    conformance_check_name(&decl.name, interface_name),
                    None,
                    None,
                )),
                type_args: Vec::new(),
                args: vec![Expr::Name(VALUE_PARAM.to_string(), None, None)],
                span: None,
                method_call_origin: None,
            },
        },
        Stmt::If {
            condition: nil_compare(BinaryOp::NotEq, narrowed()),
            then_body: vec![Stmt::Return(narrowed())],
            else_body: Vec::new(),
        },
        Stmt::Expr(Expr::Call {
            callee: Box::new(Expr::Name("error".to_string(), None, None)),
            type_args: Vec::new(),
            args: vec![Expr::String(
                format!("interface cast failed: value is not '{}'", decl.source_name),
                None,
            )],
            span: None,
            method_call_origin: None,
        }),
        Stmt::Return(Expr::Cast {
            expr: Box::new(Expr::Cast {
                expr: Box::new(Expr::Nil(None)),
                ty: Type::Unknown,
                span: None,
            }),
            ty: target_ty.clone(),
            span: None,
        }),
    ];
    Function {
        name: FunctionName::Simple(conformance_cast_name(&decl.name, interface_name)),
        symbol_id: None,
        type_params: Vec::new(),
        params: vec![Param {
            name: VALUE_PARAM.to_string(),
            symbol_id: None,
            ty: Type::Named {
                name: interface_name.to_string(),
                type_args: Vec::new(),
            },
        }],
        vararg: None,
        return_type: Some(target_ty),
        body,
        file_path: decl.file_path.clone(),
        span: None,
    }
}

// ---------------------------------------------------------------------------
// Bound-method coercion: rewriting coercion sites
// ---------------------------------------------------------------------------

/// Rewrite every coercion site where a value of a conforming nominal type
/// meets its interface type into a call to the pair's generated constructor:
/// `local op: Op = add` becomes `local op: Op = __conform$Add$Op(add)`.
///
/// Covered positions: `local`/`const` annotations, assignments, returns,
/// `::` casts, call and method-call arguments, field and index assignments,
/// record-literal fields, array-literal elements, and if-expression branches.
/// The pass also rewrites colon calls through bound-method fields
/// (`op:exec(a, b)`) into dot calls (`op.exec(a, b)`) — the receiver was
/// applied when the record was built, so the stored closure takes the
/// self-less parameters, and the `Expr::Field` base keeps single evaluation
/// of the receiver.
///
/// The pass is best-effort and never fails: sites it cannot type yet (for
/// example arguments to a function whose return type is still being
/// inferred) are simply left alone, to be either caught by a later run of
/// this pass or reported by the checker. It is idempotent, so running it
/// both before return-type inference (unannotated functions may return
/// coerced values) and again once every signature is known only widens
/// coverage.
pub(crate) fn desugar_conformance_coercions(
    program: &mut Program,
    fn_signatures: &HashMap<String, FnSignature>,
    module_bindings: &HashMap<String, Binding>,
    reusable: &[bool],
) {
    let conformance = conformance_table(program);
    if conformance.is_empty() {
        return;
    }
    // Parameter lists for functions that have no signature entry yet (their
    // return type is still uninferred); parameters are always annotated, so
    // argument positions can still rewrite against them.
    let fallback_params: HashMap<String, Vec<Type>> = program
        .functions
        .iter()
        .filter(|function| function.type_params.is_empty() && function.vararg.is_none())
        .map(|function| {
            (
                function.name.to_string(),
                function
                    .params
                    .iter()
                    .map(|param| param.ty.clone())
                    .collect(),
            )
        })
        .collect();
    let rewriter = CoercionRewriter {
        conformance,
        fn_signatures,
        fallback_params,
    };
    for (function, reusable) in program.functions.iter_mut().zip(reusable) {
        if *reusable {
            continue;
        }
        let mut vars = crate::function_module_bindings(function, module_bindings).clone();
        for param in &function.params {
            vars.insert(
                param.name.clone(),
                binding_for(param.ty.clone(), Rebindability::Rebindable),
            );
        }
        crate::bind_vararg(&mut vars, function.vararg.as_ref());
        let active = active_type_param_set(&function.type_params);
        let expected_return = function.return_type.clone();
        rewriter.rewrite_stmts(
            &mut function.body,
            &mut vars,
            &active,
            expected_return.as_ref(),
        );
    }
    // Top-level statements exist twice: in `program.top_level` (the copy
    // module-binding symbols are declared from) and cloned into the generated
    // `__waluau_top_level_init` function (rewritten in the loop above with an
    // empty binding seed, matching `function_module_bindings`). The
    // interface-narrowing desugar introduces statements, so rewrite the
    // top-level copy with the same context to keep both copies identical;
    // the pass is deterministic and idempotent, so identical inputs stay in
    // lockstep across both desugar runs. `top_level_file_paths` is parallel
    // to `top_level` (one path per statement), so the rewrite goes slice by
    // slice — sharing one scope, like the single pass over the init clone —
    // and repeats each slice's path for the statements it gains.
    if !program.top_level.is_empty() {
        let mut vars = HashMap::new();
        let active = HashSet::new();
        let stmts = std::mem::take(&mut program.top_level);
        let paths = std::mem::take(&mut program.top_level_file_paths);
        debug_assert_eq!(stmts.len(), paths.len());
        let mut new_stmts = Vec::with_capacity(stmts.len());
        let mut new_paths = Vec::with_capacity(paths.len());
        let mut stmts = stmts.into_iter();
        let mut start = 0;
        while start < paths.len() {
            let path = &paths[start];
            let mut end = start + 1;
            while end < paths.len() && paths[end] == *path {
                end += 1;
            }
            let mut slice: Vec<Stmt> = stmts.by_ref().take(end - start).collect();
            rewriter.rewrite_stmts(&mut slice, &mut vars, &active, None);
            new_paths.extend(std::iter::repeat_n(path.clone(), slice.len()));
            new_stmts.append(&mut slice);
            start = end;
        }
        program.top_level = new_stmts;
        program.top_level_file_paths = new_paths;
    }
}

struct CoercionRewriter<'a> {
    conformance: HashMap<String, Vec<String>>,
    fn_signatures: &'a HashMap<String, FnSignature>,
    fallback_params: HashMap<String, Vec<Type>>,
}

impl CoercionRewriter<'_> {
    /// The wrapper to call when a value of type `actual` flows into
    /// `expected`, or `None` when no conformance coercion applies. A
    /// nullable *value* never coerces (a nil check would be required); a nullable
    /// *expectation* accepts the coerced record like any other value.
    fn conforming_wrapper(&self, actual: &Type, expected: &Type) -> Option<String> {
        let expected = match expected {
            Type::Nullable(inner) => inner.as_ref(),
            other => other,
        };
        let (
            Type::Opaque {
                name: actual_name, ..
            },
            Type::Opaque {
                name: interface_name,
                ..
            },
        ) = (actual, expected)
        else {
            return None;
        };
        if actual_name == interface_name {
            return None;
        }
        self.conformance
            .get(actual_name)?
            .iter()
            .any(|name| name == interface_name)
            .then(|| conformance_wrapper_name(actual_name, interface_name))
    }

    /// The generated hard cast to call for `value :: T` when `value` is
    /// interface-typed and `T` is declared to conform to that interface, or
    /// `None` when the cast is not a conformance downcast. Casts to types
    /// that do not conform stay compile errors through the unchanged cast
    /// rules.
    fn conforming_downcast(&self, actual: &Type, target: &Type) -> Option<String> {
        let (
            Type::Opaque {
                name: interface_name,
                ..
            },
            Type::Opaque {
                name: target_name, ..
            },
        ) = (actual, target)
        else {
            return None;
        };
        if interface_name == target_name {
            return None;
        }
        self.conformance
            .get(target_name)?
            .iter()
            .any(|name| name == interface_name)
            .then(|| conformance_cast_name(target_name, interface_name))
    }

    /// The generated brand check backing an `if T(x) = value then` statement
    /// whose scrutinee is interface-typed and whose target conforms to that
    /// interface, or `None` when the statement is not interface narrowing
    /// (tagged-union and extern if-casts keep their existing lowering).
    fn interface_if_cast_check(
        &self,
        stmt: &Stmt,
        vars: &HashMap<String, Binding>,
        active: &HashSet<String>,
    ) -> Option<String> {
        let Stmt::IfCast {
            target_ty, value, ..
        } = stmt
        else {
            return None;
        };
        let value_ty = infer_expr(value, vars, self.fn_signatures, active, None).ok()?;
        let (
            Type::Opaque {
                name: interface_name,
                ..
            },
            Type::Opaque {
                name: target_name, ..
            },
        ) = (&value_ty, target_ty)
        else {
            return None;
        };
        if interface_name == target_name {
            return None;
        }
        self.conformance
            .get(target_name.as_str())?
            .iter()
            .any(|name| name == interface_name)
            .then(|| conformance_check_name(target_name, interface_name))
    }

    /// Parameter types for arguments of a `Expr::Call`, when the callee is a
    /// plain (non-generic, non-overloaded, non-variadic) function reachable
    /// by name.
    fn call_param_types(&self, callee: &Expr) -> Option<Vec<Type>> {
        let name = builtin_name(callee)?;
        match self.fn_signatures.get(&name) {
            Some(FnSignature::Mono {
                params,
                vararg: None,
                ..
            }) => Some(params.clone()),
            Some(_) => None,
            None => self.fallback_params.get(&name).cloned(),
        }
    }

    fn rewrite_stmts(
        &self,
        stmts: &mut Vec<Stmt>,
        vars: &mut HashMap<String, Binding>,
        active: &HashSet<String>,
        expected_return: Option<&Type>,
    ) {
        let mut index = 0;
        while index < stmts.len() {
            // Interface narrowing desugars a whole statement into two:
            //
            //     if Add(a) = op then BODY else EBODY end
            // ==>
            //     local __conform$narrowed = __conformcheck$Add$Op(op)
            //     if __conform$narrowed ~= nil then
            //         local a = __conform$narrowed
            //         BODY
            //     else
            //         EBODY
            //     end
            //
            // The scrutinee is evaluated once, the binding is visible in the
            // then-branch only (matching the tagged-union and extern forms),
            // and existing nullable narrowing types the fresh local. The two
            // replacement statements are then rewritten by the ordinary loop
            // below, which also recurses into the branch bodies.
            if let Some(check_fn) = self.interface_if_cast_check(&stmts[index], vars, active) {
                let Stmt::IfCast {
                    binding,
                    binding_symbol_id,
                    value,
                    then_body,
                    else_body,
                    ..
                } = std::mem::replace(&mut stmts[index], Stmt::Break)
                else {
                    unreachable!("interface_if_cast_check matched an IfCast");
                };
                let replacement = interface_if_cast_statements(
                    check_fn,
                    binding,
                    binding_symbol_id,
                    value,
                    then_body,
                    else_body,
                );
                stmts.splice(index..=index, replacement);
                continue;
            }
            self.rewrite_stmt(&mut stmts[index], vars, active, expected_return);
            index += 1;
        }
    }

    fn rewrite_stmt(
        &self,
        stmt: &mut Stmt,
        vars: &mut HashMap<String, Binding>,
        active: &HashSet<String>,
        expected_return: Option<&Type>,
    ) {
        match stmt {
            Stmt::Let {
                name,
                rebindability,
                ty,
                value,
                ..
            } => {
                self.rewrite_expr(value, ty.as_ref(), vars, active);
                // Mirror the binding the checker will create, so later
                // statements see this local's type.
                let inferred_ty = if let Some(expected_ty) = ty {
                    infer_expr(
                        value,
                        vars,
                        self.fn_signatures,
                        active,
                        Some(expected_ty.clone()),
                    )
                    .unwrap_or_else(|_| expected_ty.clone())
                } else if matches!(value, Expr::ArrayLiteral { elements, .. } if elements.is_empty())
                {
                    Type::Record(BTreeMap::new())
                } else {
                    infer_expr(value, vars, self.fn_signatures, active, None)
                        .unwrap_or(Type::Unknown)
                };
                vars.insert(name.clone(), binding_for(inferred_ty, *rebindability));
            }
            Stmt::Assign {
                op, name, value, ..
            } => {
                let expected = match op {
                    waluau_ast::AssignOp::Set => vars.get(name.as_str()).map(|b| b.ty.clone()),
                    waluau_ast::AssignOp::Compound(_) => None,
                };
                self.rewrite_expr(value, expected.as_ref(), vars, active);
            }
            Stmt::Return(value) => {
                self.rewrite_expr(value, expected_return, vars, active);
            }
            Stmt::Expr(value) => {
                self.rewrite_expr(value, None, vars, active);
            }
            Stmt::IndexAssign {
                base, index, value, ..
            } => {
                self.rewrite_expr(base, None, vars, active);
                self.rewrite_expr(index, None, vars, active);
                let element_ty = infer_expr(base, vars, self.fn_signatures, active, None)
                    .ok()
                    .and_then(|base_ty| array_element_type(&base_ty).cloned());
                self.rewrite_expr(value, element_ty.as_ref(), vars, active);
            }
            Stmt::FieldAssign {
                base, name, value, ..
            } => {
                self.rewrite_expr(base, None, vars, active);
                let field_ty = infer_expr(base, vars, self.fn_signatures, active, None)
                    .ok()
                    .and_then(|base_ty| base_ty.record_field(name));
                self.rewrite_expr(value, field_ty.as_ref(), vars, active);
            }
            Stmt::Match { value, arms, .. } => {
                self.rewrite_expr(value, None, vars, active);
                for arm in arms {
                    self.rewrite_stmts(&mut arm.body, &mut vars.clone(), active, expected_return);
                }
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                self.rewrite_expr(condition, None, vars, active);
                let (mut then_scope, mut else_scope) = narrowed_scopes(condition, vars);
                self.rewrite_stmts(then_body, &mut then_scope, active, expected_return);
                self.rewrite_stmts(else_body, &mut else_scope, active, expected_return);
            }
            Stmt::IfCast {
                target_name,
                target_ty,
                binding,
                value,
                then_body,
                else_body,
                ..
            } => {
                self.rewrite_expr(value, None, vars, active);
                let (mut then_scope, mut else_scope) = match checked_if_cast_scopes(
                    target_name,
                    target_ty,
                    binding,
                    value,
                    vars,
                    self.fn_signatures,
                    active,
                ) {
                    Ok(scopes) => (scopes.then_scope, scopes.else_scope),
                    Err(_) => (vars.clone(), vars.clone()),
                };
                self.rewrite_stmts(then_body, &mut then_scope, active, expected_return);
                self.rewrite_stmts(else_body, &mut else_scope, active, expected_return);
            }
            Stmt::While { condition, body } => {
                self.rewrite_expr(condition, None, vars, active);
                let mut loop_scope = vars.clone();
                self.rewrite_stmts(body, &mut loop_scope, active, expected_return);
            }
            Stmt::Repeat { body, condition } => {
                let mut loop_scope = vars.clone();
                self.rewrite_stmts(body, &mut loop_scope, active, expected_return);
                self.rewrite_expr(condition, None, vars, active);
            }
            Stmt::NumericFor {
                name,
                start,
                stop,
                step,
                body,
                ..
            } => {
                self.rewrite_expr(start, None, vars, active);
                self.rewrite_expr(stop, None, vars, active);
                if let Some(step) = step {
                    self.rewrite_expr(step, None, vars, active);
                }
                let mut loop_scope = vars.clone();
                let mut bounds = vec![&*start, &*stop];
                if let Some(step) = step {
                    bounds.push(step);
                }
                if let Ok(loop_ty) =
                    crate::numeric::infer_numeric_for_loop_type(&bounds, |expr, expected| {
                        infer_expr(expr, vars, self.fn_signatures, active, expected)
                    })
                {
                    loop_scope.insert(name.clone(), binding_for(loop_ty, Rebindability::Const));
                }
                self.rewrite_stmts(body, &mut loop_scope, active, expected_return);
            }
            Stmt::ForIn {
                names,
                iterators,
                body,
                ..
            } => {
                for iterator in iterators.iter_mut() {
                    self.rewrite_expr(iterator, None, vars, active);
                }
                let mut loop_scope = vars.clone();
                if let Ok(loop_types) = crate::statements::for_in_loop_value_types(
                    iterators,
                    names.len(),
                    vars,
                    self.fn_signatures,
                    active,
                ) {
                    for (name, ty) in names.iter().zip(loop_types) {
                        loop_scope.insert(name.clone(), binding_for(ty, Rebindability::Const));
                    }
                }
                self.rewrite_stmts(body, &mut loop_scope, active, expected_return);
            }
            Stmt::ReturnMulti(values) => {
                let expected_parts = match expected_return {
                    Some(Type::Multi(parts)) if parts.len() == values.len() => Some(parts.clone()),
                    _ => None,
                };
                for (index, value) in values.iter_mut().enumerate() {
                    let expected = expected_parts.as_ref().map(|parts| &parts[index]);
                    self.rewrite_expr(value, expected, vars, active);
                }
            }
            Stmt::LetMulti { bindings, values } => {
                let one_to_one = bindings.len() == values.len();
                for (index, value) in values.iter_mut().enumerate() {
                    let expected = if one_to_one {
                        bindings[index].ty.clone()
                    } else {
                        None
                    };
                    self.rewrite_expr(value, expected.as_ref(), vars, active);
                }
                for (index, binding) in bindings.iter().enumerate() {
                    let ty = if let Some(ty) = &binding.ty {
                        ty.clone()
                    } else if one_to_one {
                        infer_expr(&values[index], vars, self.fn_signatures, active, None)
                            .unwrap_or(Type::Unknown)
                    } else {
                        Type::Unknown
                    };
                    vars.insert(binding.name.clone(), binding_for(ty, binding.rebindability));
                }
            }
            Stmt::AssignMulti {
                targets, values, ..
            } => {
                let one_to_one = targets.len() == values.len();
                for (index, value) in values.iter_mut().enumerate() {
                    let expected = if one_to_one {
                        vars.get(targets[index].as_str()).map(|b| b.ty.clone())
                    } else {
                        None
                    };
                    self.rewrite_expr(value, expected.as_ref(), vars, active);
                }
            }
            Stmt::Break | Stmt::Continue => {}
        }
    }

    fn rewrite_expr(
        &self,
        expr: &mut Expr,
        expected: Option<&Type>,
        vars: &HashMap<String, Binding>,
        active: &HashSet<String>,
    ) {
        match expr {
            Expr::Call { callee, args, .. } => {
                self.rewrite_expr(callee, None, vars, active);
                let params = self.call_param_types(callee);
                for (index, arg) in args.iter_mut().enumerate() {
                    let expected = params.as_ref().and_then(|params| params.get(index));
                    self.rewrite_expr(arg, expected, vars, active);
                }
            }
            Expr::MethodCall { receiver, name, .. } => {
                self.rewrite_expr(receiver, None, vars, active);
                let receiver_ty = infer_expr(receiver, vars, self.fn_signatures, active, None).ok();
                // Mirror the checker's precedence: an explicit `T.name`
                // method signature wins over a record field.
                let signature = method_signature(receiver, name, self.fn_signatures)
                    .or_else(|| {
                        receiver_ty.as_ref().and_then(|receiver_ty| {
                            type_method_signature(receiver_ty, name, self.fn_signatures)
                        })
                    })
                    .map(|(signature, _)| signature.clone());
                let field_ty = if signature.is_none() {
                    receiver_ty
                        .as_ref()
                        .and_then(|receiver_ty| receiver_ty.record_field(name))
                } else {
                    None
                };
                let Expr::MethodCall { args, .. } = expr else {
                    unreachable!("matched above");
                };
                match (&signature, &field_ty) {
                    // Explicit method declaration: the receiver occupies the
                    // first parameter slot.
                    (Some(FnSignature::Mono { params, .. }), _) => {
                        for (index, arg) in args.iter_mut().enumerate() {
                            self.rewrite_expr(arg, params.get(index + 1), vars, active);
                        }
                    }
                    // Bound-method field: arguments are the self-less
                    // parameters; rewrite the colon call into a dot call so
                    // lowering uses the plain record-field call path.
                    (
                        None,
                        Some(Type::Function {
                            params,
                            has_self: true,
                            ..
                        }),
                    ) => {
                        for (index, arg) in args.iter_mut().enumerate() {
                            self.rewrite_expr(arg, params.get(index), vars, active);
                        }
                        let Expr::MethodCall {
                            receiver,
                            name,
                            args,
                            span,
                            ..
                        } = std::mem::replace(expr, Expr::Nil(None))
                        else {
                            unreachable!("matched above");
                        };
                        *expr = Expr::Call {
                            callee: Box::new(Expr::Field {
                                base: receiver,
                                name,
                                resolved_name: None,
                                span,
                            }),
                            type_args: Vec::new(),
                            args,
                            span,
                            method_call_origin: None,
                        };
                    }
                    // Explicit-receiver convention field (`(T, ...) -> R`):
                    // the receiver occupies the first parameter slot.
                    (
                        None,
                        Some(Type::Function {
                            params,
                            has_self: false,
                            ..
                        }),
                    ) => {
                        for (index, arg) in args.iter_mut().enumerate() {
                            self.rewrite_expr(arg, params.get(index + 1), vars, active);
                        }
                    }
                    _ => {
                        for arg in args {
                            self.rewrite_expr(arg, None, vars, active);
                        }
                    }
                }
            }
            Expr::Cast {
                expr: inner, ty, ..
            } => {
                let target = ty.clone();
                self.rewrite_expr(inner, Some(&target), vars, active);
                // `op :: Add` on an interface-typed value is the hard
                // conformance downcast: brand check, then the original
                // receiver, or a raised error on mismatch.
                let downcast = infer_expr(inner, vars, self.fn_signatures, active, None)
                    .ok()
                    .and_then(|actual| self.conforming_downcast(&actual, &target));
                if let Some(cast_fn) = downcast {
                    let span = expr.span();
                    let Expr::Cast { expr: inner, .. } = std::mem::replace(expr, Expr::Nil(None))
                    else {
                        unreachable!("matched above");
                    };
                    *expr = Expr::Call {
                        callee: Box::new(Expr::Name(cast_fn, None, span)),
                        type_args: Vec::new(),
                        args: vec![*inner],
                        span,
                        method_call_origin: None,
                    };
                }
            }
            Expr::TableLiteral { fields, .. } => {
                let expected_fields = expected.and_then(record_field_types).cloned();
                for field in fields {
                    let expected = expected_fields
                        .as_ref()
                        .and_then(|fields| fields.get(&field.name));
                    self.rewrite_expr(&mut field.value, expected, vars, active);
                }
            }
            Expr::ArrayLiteral { elements, .. } => {
                let element_ty = expected.and_then(array_element_type).cloned();
                for element in elements {
                    self.rewrite_expr(element, element_ty.as_ref(), vars, active);
                }
            }
            Expr::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => {
                self.rewrite_expr(condition, None, vars, active);
                self.rewrite_expr(then_expr, expected, vars, active);
                self.rewrite_expr(else_expr, expected, vars, active);
            }
            Expr::Unary { expr, .. } | Expr::IsVariant { expr, .. } => {
                self.rewrite_expr(expr, None, vars, active);
            }
            Expr::Binary { left, right, .. } => {
                self.rewrite_expr(left, None, vars, active);
                self.rewrite_expr(right, None, vars, active);
            }
            Expr::Field { base, .. } => {
                self.rewrite_expr(base, None, vars, active);
            }
            Expr::Index { base, index, .. } => {
                self.rewrite_expr(base, None, vars, active);
                self.rewrite_expr(index, None, vars, active);
            }
            Expr::Function(function) => {
                let mut inner_vars = vars.clone();
                for param in &function.params {
                    inner_vars.insert(
                        param.name.clone(),
                        binding_for(param.ty.clone(), Rebindability::Rebindable),
                    );
                }
                crate::bind_vararg(&mut inner_vars, function.vararg.as_ref());
                let mut inner_active = active.clone();
                inner_active.extend(active_type_param_set(&function.type_params));
                // An unannotated lambda adopts the return type of a
                // function-typed expectation, mirroring the checker.
                let expected_return = function.return_type.clone().or_else(|| match expected {
                    Some(Type::Function { return_type, .. }) => Some((**return_type).clone()),
                    _ => None,
                });
                self.rewrite_stmts(
                    &mut function.body,
                    &mut inner_vars,
                    &inner_active,
                    expected_return.as_ref(),
                );
            }
            Expr::Number(..)
            | Expr::Bool(..)
            | Expr::Nil(..)
            | Expr::String(..)
            | Expr::Bytes(..)
            | Expr::Name(..)
            | Expr::Vararg(..)
            | Expr::Require(..) => {}
        }
        let Some(expected) = expected else {
            return;
        };
        let Ok(actual) = infer_expr(expr, vars, self.fn_signatures, active, None) else {
            return;
        };
        let Some(wrapper) = self.conforming_wrapper(&actual, expected) else {
            return;
        };
        let span = expr.span();
        let original = std::mem::replace(expr, Expr::Nil(None));
        *expr = Expr::Call {
            callee: Box::new(Expr::Name(wrapper, None, span)),
            type_args: Vec::new(),
            args: vec![original],
            span,
            method_call_origin: None,
        };
    }
}

/// The two statements replacing an interface-narrowing `if T(x) = value`:
/// a fresh local calling the generated brand check, and a nil test that
/// binds the recovered receiver at the head of the then-branch. See
/// [`CoercionRewriter::rewrite_stmts`] for the shape.
fn interface_if_cast_statements(
    check_fn: String,
    binding: String,
    binding_symbol_id: Option<waluau_ast::SymbolId>,
    value: Expr,
    mut then_body: Vec<Stmt>,
    else_body: Vec<Stmt>,
) -> [Stmt; 2] {
    let span = value.span();
    let narrowed = || Expr::Name(NARROWED_LOCAL.to_string(), None, span);
    let check_let = Stmt::Let {
        name: NARROWED_LOCAL.to_string(),
        symbol_id: None,
        rebindability: Rebindability::Const,
        ty: None,
        value: Expr::Call {
            callee: Box::new(Expr::Name(check_fn, None, span)),
            type_args: Vec::new(),
            args: vec![value],
            span,
            method_call_origin: None,
        },
    };
    then_body.insert(
        0,
        Stmt::Let {
            name: binding,
            symbol_id: binding_symbol_id,
            rebindability: Rebindability::Const,
            ty: None,
            value: narrowed(),
        },
    );
    let check_if = Stmt::If {
        condition: Expr::Binary {
            op: BinaryOp::NotEq,
            left: Box::new(narrowed()),
            right: Box::new(Expr::Nil(None)),
            resolved_name: None,
            span,
        },
        then_body,
        else_body,
    };
    [check_let, check_if]
}

/// Record fields behind nominal or nullable wrappers.
fn record_field_types(ty: &Type) -> Option<&BTreeMap<String, Type>> {
    match ty {
        Type::Record(fields) => Some(fields),
        Type::Opaque { ty, .. } | Type::Nullable(ty) => record_field_types(ty),
        _ => None,
    }
}

/// Array element type behind nominal or nullable wrappers.
fn array_element_type(ty: &Type) -> Option<&Type> {
    match ty {
        Type::Array(element) => Some(element),
        Type::Opaque { ty, .. } | Type::Nullable(ty) => array_element_type(ty),
        _ => None,
    }
}
