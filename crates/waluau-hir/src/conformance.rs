//! Explicit interface conformance checking (`type Add = Op & { ... }`).
//!
//! A conformance declaration obligates the declaring type to satisfy the
//! named interface with `self` substituted by the declaring type. The check
//! runs after whole-program return-type inference, so implementations may be
//! `function Add:name(...)` method declarations anywhere in the program
//! (before or after the type declaration) or record fields of the declaring
//! type. Function types stay invariant beyond the `self` slot.
//!
//! The check only validates declarations; coercing a conforming value to its
//! interface type (building a record of bound-method closures) is the
//! follow-up issue waluau-trbt.4, which consumes [`conformance_table`].

use std::collections::HashMap;

use waluau_ast::{Program, Type, TypeDeclaration};
use waluau_diagnostics::Diagnostic;

use crate::signatures::FnSignature;
use crate::{method_signature_name, module_type_display, module_type_display_name};

/// The interfaces each type declaration conforms to, keyed by the
/// declaration's canonical name.
///
/// This is the lookup the bound-method coercion (waluau-trbt.4) consumes:
/// coercing a value of nominal type `T` to interface type `I` is legal
/// exactly when `table[T]` contains `I`, and
/// [`check_conformance_declarations`] has already guaranteed that every
/// interface field has a matching implementation on `T`.
#[allow(dead_code)] // consumed by the bound-method coercion (waluau-trbt.4)
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
pub(crate) fn check_conformance_declarations(
    program: &Program,
    fn_signatures: &HashMap<String, FnSignature>,
) -> Vec<Diagnostic> {
    let mut errors = Vec::new();
    let decls: HashMap<&str, &TypeDeclaration> = program
        .type_declarations
        .iter()
        .map(|decl| (decl.name.as_str(), decl))
        .collect();
    for decl in &program.type_declarations {
        for interface_name in &decl.conforms {
            check_conformance(decl, interface_name, &decls, fn_signatures, &mut errors);
        }
    }
    errors
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
                    vararg: false,
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
        | (Type::Readonly(left), Type::Readonly(right))
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
