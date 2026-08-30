pub use waluau_span::Span;

mod function_declarations;
mod lua_pattern;
pub mod metrics;
mod module_constants;
pub use function_declarations::{
    FunctionBindingClass, FunctionDeclarationClass, FunctionDeclarationFacts, FunctionExposure,
    LexicalFunctionDeclaration, ModuleInterface,
};
pub use lua_pattern::{
    LuaCaptureKind, lua_pattern_captures, lua_pattern_is_plain, string_find_result_types,
    string_match_result_types,
};
pub use module_constants::{ModuleConstantError, collect_module_constants};

/// Capture kinds for a string builtin's pattern argument. Only literal
/// patterns can be analyzed statically; non-literal patterns and malformed
/// literals fall back to "no captures" (the runtime pattern engine raises
/// the Lua error for malformed patterns when the call actually runs).
pub fn expr_pattern_captures(pattern_arg: &Expr) -> Vec<LuaCaptureKind> {
    match pattern_arg {
        Expr::String(literal, _) => lua_pattern_captures(literal).unwrap_or_default(),
        _ => Vec::new(),
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SymbolId(pub usize);

/// Separator used to build unique internal names for overloaded declared
/// host functions. `declare function abs(x: f32): f32` and
/// `declare function abs(x: f64): f64` are renamed to `abs$overload0` and
/// `abs$overload1` during type checking; `$` cannot appear in source
/// identifiers, so the mangled names never collide with user code.
pub const OVERLOAD_SEPARATOR: &str = "$overload";

/// The unique internal name for overload `index` of declared function `base`.
pub fn overload_variant_name(base: &str, index: usize) -> String {
    format!("{base}{OVERLOAD_SEPARATOR}{index}")
}

/// The base (source-level) name of a mangled overload name, or `None` when
/// `name` is not an overload variant name.
pub fn overload_base_name(name: &str) -> Option<&str> {
    let (base, suffix) = name.rsplit_once(OVERLOAD_SEPARATOR)?;
    if base.is_empty() || suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(base)
}

use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub functions: Vec<Function>,
    pub declared_imports: Vec<DeclaredImport>,
    pub declared_constants: Vec<DeclaredConstant>,
    pub type_declarations: Vec<TypeDeclaration>,
    pub top_level: Vec<Stmt>,
    /// Source owner for each top-level statement. Linkers preserve this
    /// parallel list so module initializers can be checked with the same
    /// private-type visibility as ordinary functions.
    pub top_level_file_paths: Vec<String>,
    /// The legacy value a module exports through a trailing top-level `return`.
    ///
    /// The value is a function name or a table of functions. Module linkers
    /// consume dependency exports while resolving `require`. Explicit named
    /// function exports use [`FunctionDeclarationClass::Export`] and cannot be
    /// combined with this value. A trailing return in the linked entry file does not
    /// define the Wasm export surface, so linkers discard this metadata after
    /// hoisting any inline exported functions.
    pub export: Option<Expr>,
    pub sources: BTreeMap<String, String>,
    pub entry_file_path: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeDeclaration {
    pub name: String,
    /// Source spelling retained when a linker gives the declaration a
    /// module-unique canonical name. Diagnostics and source-defined type
    /// capabilities use this name; nominal identity uses `name`.
    pub source_name: String,
    pub type_params: Vec<String>,
    pub ty: Type,
    /// Whether this declaration is part of its source module's interface.
    /// Plain `type`/`enum` declarations remain private to their file; only an
    /// explicit `export` makes the source name available through `require`.
    pub exported: bool,
    /// Declaration-order variants for a nominal enum. `None` identifies an
    /// ordinary type alias. Variants remain compile-time metadata and do not
    /// create a runtime table.
    pub enum_variants: Option<Vec<String>>,
    /// Whether importing modules see this alias as an opaque nominal handle.
    /// The declaring file still type-checks against `ty`; lowering always uses
    /// `ty`, so this changes no runtime representation.
    pub module_opaque: bool,
    /// Source file that owns the representation of a module-opaque alias.
    pub file_path: String,
    /// Interfaces this type declares conformance to via
    /// `type Name = Interface & { ... }`. The conformance checker verifies
    /// every function-typed field of each interface has a matching
    /// implementation with `self` substituted by this type; the bound-method
    /// coercion consumes this list to build interface records.
    ///
    /// The parser currently accepts at most one interface per declaration;
    /// the field is a list so the coercion side never needs an AST change if
    /// that restriction is lifted.
    pub conforms: Vec<String>,
}

/// Resolved source metadata for an application of a generic extern type
/// constructor. The constructor is canonical and therefore participates in
/// nominal identity; `source_name` exists only for diagnostics and
/// source-defined capabilities such as `Promise<T>` await typing.
///
/// This metadata has no runtime representation. IR erases the containing
/// [`Type::Opaque`] to its extern representation before lowering.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct GenericExternType {
    pub constructor: String,
    pub source_name: String,
    pub type_args: Vec<Type>,
}

impl GenericExternType {
    /// Transform every resolved argument while preserving constructor identity.
    pub fn map_type_args(&self, mut map: impl FnMut(&Type) -> Type) -> Self {
        Self {
            constructor: self.constructor.clone(),
            source_name: self.source_name.clone(),
            type_args: self.type_args.iter().map(&mut map).collect(),
        }
    }

    /// Fallible form of [`Self::map_type_args`].
    pub fn try_map_type_args<E>(
        &self,
        mut map: impl FnMut(&Type) -> Result<Type, E>,
    ) -> Result<Self, E> {
        Ok(Self {
            constructor: self.constructor.clone(),
            source_name: self.source_name.clone(),
            type_args: self
                .type_args
                .iter()
                .map(&mut map)
                .collect::<Result<_, _>>()?,
        })
    }

    /// Canonical nominal name used by internal type maps after substitution.
    pub fn canonical_name(&self) -> String {
        let args = self
            .type_args
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}<{args}>", self.constructor)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeclaredImport {
    pub name: String,
    pub host_name: String,
    pub symbol_id: Option<SymbolId>,
    pub params: Vec<Param>,
    pub return_type: Type,
}

/// A named compile-time constant on a builtin namespace, declared as
/// `declare const math.pi: f64 = 3.141592653589793`. Reads fold to the
/// literal during lowering; nothing is imported from the host.
#[derive(Clone, Debug, PartialEq)]
pub struct DeclaredConstant {
    /// Qualified name, e.g. `math.pi`.
    pub name: String,
    pub ty: Type,
    pub value: NumberLiteral,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableField {
    pub name: String,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub name: FunctionName,
    /// Canonical authored declaration semantics. Functions stored directly on
    /// a program are `Module` or `Export`; lexical declarations use
    /// [`FunctionExpr::declaration_class`] instead.
    pub declaration_class: FunctionDeclarationClass,
    pub symbol_id: Option<SymbolId>,
    pub type_params: Vec<String>,
    pub params: Vec<Param>,
    /// `Some(element)` when the parameter list ends in `...`. The type is the
    /// element type of the pack (`...: number` accepts numbers), never a list
    /// type; an unannotated `...` carries `Type::Unknown` like an unannotated
    /// parameter.
    pub vararg: Option<Type>,
    pub return_type: Option<Type>,
    pub body: Vec<Stmt>,
    pub file_path: String,
    /// Full authored declaration span. Synthetic functions have no span.
    pub span: Option<Span>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum FunctionName {
    Simple(String),
    Method { table: String, method: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct FunctionExpr {
    pub name: Option<String>,
    /// Authored function-declaration form when this expression represents a
    /// lexical declaration. Plain function literals, including explicitly
    /// named literals, use `None`.
    pub declaration_class: Option<FunctionDeclarationClass>,
    pub symbol_id: Option<SymbolId>,
    pub implicit_self: Option<String>,
    pub type_params: Vec<String>,
    pub params: Vec<Param>,
    /// `Some(element)` when the parameter list ends in `...`; see
    /// [`Function::vararg`].
    pub vararg: Option<Type>,
    pub return_type: Option<Type>,
    pub body: Vec<Stmt>,
    pub file_path: String,
    pub span: Option<Span>,
}

impl FunctionName {
    pub fn simple_name(&self) -> Option<&str> {
        match self {
            Self::Simple(name) => Some(name),
            Self::Method { .. } => None,
        }
    }

    /// Source name of an unqualified module function. Dot-named statics are
    /// represented by `Simple` internally, so callers deciding authored
    /// export eligibility must use this narrower query.
    pub fn unqualified_name(&self) -> Option<&str> {
        match self {
            Self::Simple(name) if !name.contains('.') => Some(name),
            Self::Simple(_) | Self::Method { .. } => None,
        }
    }
}

impl std::fmt::Display for FunctionName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Simple(name) => f.write_str(name),
            Self::Method { table, method } => write!(f, "{table}:{method}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub symbol_id: Option<SymbolId>,
    pub ty: Type,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TaggedVariant {
    pub tag: String,
    pub payload: Arc<Type>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MethodCallOrigin {
    /// The original receiver expression from the method call
    pub original_receiver: Box<Expr>,
    /// The method name that was called
    pub method_name: String,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NumericType {
    U32,
    U64,
    I32,
    I64,
    F32,
    F64,
}

/// Element kind of a linear-memory typed array (`Float32Array` & friends).
/// Values are pointers into the module's linear memory; the element count
/// lives in an 8-byte header preceding the data.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TypedArrayKind {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl TypedArrayKind {
    /// The surface type name (`Float32Array` etc.), mirroring JS typed arrays.
    pub const fn type_name(self) -> &'static str {
        match self {
            Self::I8 => "Int8Array",
            Self::U8 => "Uint8Array",
            Self::I16 => "Int16Array",
            Self::U16 => "Uint16Array",
            Self::I32 => "Int32Array",
            Self::U32 => "Uint32Array",
            Self::F32 => "Float32Array",
            Self::F64 => "Float64Array",
        }
    }

    pub const fn from_type_name(name: &str) -> Option<Self> {
        match name.as_bytes() {
            b"Int8Array" => Some(Self::I8),
            b"Uint8Array" => Some(Self::U8),
            b"Int16Array" => Some(Self::I16),
            b"Uint16Array" => Some(Self::U16),
            b"Int32Array" => Some(Self::I32),
            b"Uint32Array" => Some(Self::U32),
            b"Float32Array" => Some(Self::F32),
            b"Float64Array" => Some(Self::F64),
            _ => None,
        }
    }

    /// Size of one element in bytes.
    pub const fn element_size(self) -> u32 {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }

    /// The waluau numeric type produced by reads (and expected by writes,
    /// modulo implicit numeric coercion). Sub-word integer kinds widen to
    /// their 32-bit signedness on read; writes truncate.
    pub const fn element_numeric_type(self) -> NumericType {
        match self {
            Self::I8 | Self::I16 | Self::I32 => NumericType::I32,
            Self::U8 | Self::U16 | Self::U32 => NumericType::U32,
            Self::F32 => NumericType::F32,
            Self::F64 => NumericType::F64,
        }
    }

    pub const ALL: [Self; 8] = [
        Self::I8,
        Self::U8,
        Self::I16,
        Self::U16,
        Self::I32,
        Self::U32,
        Self::F32,
        Self::F64,
    ];
}

/// Shared representation of a resolved opaque alias.
///
/// Named aliases form a graph: a large record can mention the same nested
/// alias from many fields and signatures. Sharing the resolved payload keeps
/// that graph a graph instead of cloning it into an exponentially larger
/// tree. Equality remains structural across independently-created payloads,
/// while the common shared case exits by identity.
#[derive(Clone, Debug)]
pub struct OpaquePayload(std::sync::Arc<Type>);

impl OpaquePayload {
    pub fn new(ty: Type) -> Self {
        Self(std::sync::Arc::new(ty))
    }

    pub fn as_ptr(&self) -> *const Type {
        std::sync::Arc::as_ptr(&self.0)
    }

    pub fn ptr_eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }

    pub fn make_mut(&mut self) -> &mut Type {
        std::sync::Arc::make_mut(&mut self.0)
    }
}

impl std::ops::Deref for OpaquePayload {
    type Target = Type;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<Type> for OpaquePayload {
    fn as_ref(&self) -> &Type {
        &self.0
    }
}

impl PartialEq for OpaquePayload {
    fn eq(&self, other: &Self) -> bool {
        self.ptr_eq(other) || self.0 == other.0
    }
}

impl Eq for OpaquePayload {}

impl std::hash::Hash for OpaquePayload {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&self.0, state);
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Type {
    Numeric(NumericType),
    Unit,
    Bool,
    String,
    Bytes,
    Extern,
    ExternSubtype(Arc<Type>),
    Nil,
    Nullable(Arc<Type>),
    TaggedVariant(TaggedVariant),
    TaggedUnion(Vec<TaggedVariant>),
    /// A closed set of string constants (`type CardColor = "red" | "black"`).
    /// Semantically an enum whose spelling happens to be string literals:
    /// values exist only where a member literal meets the union type, and the
    /// type never converts to or from plain `string`, even with a cast. The
    /// runtime representation is the member string itself.
    StringLiteralUnion(Vec<String>),
    Named {
        name: String,
        type_args: Vec<Type>,
    },
    Opaque {
        name: String,
        ty: OpaquePayload,
        generic_extern: Option<Arc<GenericExternType>>,
    },
    Array(Arc<Type>),
    /// A fixed-length numeric array in linear memory (`Float32Array` etc.).
    /// Runtime value: i32 pointer to the element data; see [`TypedArrayKind`].
    TypedArray(TypedArrayKind),
    /// A dynamically sized sequence of values produced by `...`.
    ///
    /// Variadic packs use the same runtime representation as `Array`, but keep
    /// their expansion semantics across call and return boundaries.
    Variadic(Arc<Type>),
    Multi(Vec<Type>),
    Function {
        params: Vec<Type>,
        return_type: Arc<Type>,
        /// Whether the function type opens with the contextual `self` receiver
        /// placeholder (`(self, a: i32) -> i32`). Only legal as the immediate
        /// type of a record field, where it marks an interface method whose
        /// receiver type is substituted at conformance-check time. `self` is
        /// not included in `params`. Participates in type equality: a method
        /// type never unifies with a plain function type of the same shape.
        has_self: bool,
    },
    /// A fixed-shape record used for module namespaces (`require` results).
    ///
    /// The field map is shared: a record type describes an entire module or
    /// record surface, and cloning `Type` values must stay O(1) rather than
    /// O(exported surface). Use [`Type::record`] to build one and
    /// `Arc::make_mut` to rewrite fields in place.
    Record(Arc<BTreeMap<String, Type>>),
    /// Reference to an in-scope generic type parameter (e.g. `T` in `function f<T>(x: T)`).
    TypeParam(String),
    /// A coroutine handle. Yield/resume values are always `i32` (see design 0007).
    Thread,
    Unknown,
}

impl Type {
    pub const fn number() -> Self {
        Self::Numeric(NumericType::F64)
    }

    /// Build a record type from an owned field map.
    pub fn record(fields: BTreeMap<String, Type>) -> Self {
        Self::Record(Arc::new(fields))
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Self::Numeric(_))
    }

    pub fn is_array(&self) -> bool {
        match self {
            Self::Array(_) | Self::Variadic(_) => true,
            Self::Opaque { ty: inner, .. } => inner.is_array(),
            _ => false,
        }
    }

    pub fn is_record(&self) -> bool {
        match self {
            Self::Record(_) => true,
            Self::Opaque { ty: inner, .. } => inner.is_record(),
            _ => false,
        }
    }

    pub fn is_typed_array(&self) -> bool {
        match self {
            Self::TypedArray(_) => true,
            Self::Opaque { ty: inner, .. } => inner.is_typed_array(),
            _ => false,
        }
    }

    /// The resolved generic extern application represented by this nominal
    /// type, if any. Consumers inspect constructor identity and arguments
    /// through this interface; rendered type names are diagnostics only.
    pub fn generic_extern(&self) -> Option<&GenericExternType> {
        match self {
            Self::Opaque {
                generic_extern: Some(generic),
                ..
            } => Some(generic),
            Self::Opaque { ty, .. } => ty.generic_extern(),
            _ => None,
        }
    }

    pub fn element_type(&self) -> Option<Type> {
        match self {
            Self::Array(element) | Self::Variadic(element) => Some((**element).clone()),
            Self::TypedArray(kind) => Some(Self::Numeric(kind.element_numeric_type())),
            Self::Nullable(inner) => inner.element_type(),
            Self::Opaque { ty: inner, .. } => inner.element_type(),
            _ => None,
        }
    }

    pub fn record_field(&self, name: &str) -> Option<Type> {
        match self {
            Self::Record(fields) => fields.get(name).cloned(),
            Self::Opaque { ty, .. } => ty.record_field(name),
            Self::TaggedVariant(variant) if name == "value" => Some((*variant.payload).clone()),
            // Both are the canonical `{ tag, value }` record at runtime, so the
            // discriminant reads off either one. A variant answers its own payload
            // type for `value` above, because which variant it is is already known.
            Self::TaggedVariant(_) | Self::TaggedUnion(_) if name == "tag" => {
                Some(Self::Numeric(NumericType::I32))
            }
            Self::TaggedUnion(_) if name == "value" => Some(Self::Unknown),
            _ => None,
        }
    }

    pub fn nullable_inner(&self) -> Option<Type> {
        match self {
            Self::Nullable(inner) => Some(inner.as_ref().clone()),
            _ => None,
        }
    }

    /// Whether a value of this type may be represented by `nil`.
    pub fn accepts_nil(&self) -> bool {
        matches!(self, Self::Nullable(_))
    }

    /// Nullable types whose inner value type has no null representation in
    /// wasm (numerics, bools, and typed-array pointers). These lower to typed nullable box refs
    /// (`ref null $nullable_box_K`): null stands for nil and a one-field GC
    /// struct holds the payload, so conversions to/from the inner type must
    /// wrap/unwrap the box.
    pub fn is_boxed_nullable(&self) -> bool {
        matches!(self, Self::Nullable(inner) if matches!(**inner, Self::Numeric(_) | Self::Bool | Self::TypedArray(_)))
    }

    /// The string literal union this type represents, seen through nominal
    /// alias wrappers (`type CardColor = "red" | "black"` resolves to an
    /// `Opaque` around the union).
    pub fn string_literal_union(&self) -> Option<&[String]> {
        match self {
            Self::StringLiteralUnion(members) => Some(members),
            Self::Opaque { ty, .. } => ty.string_literal_union(),
            _ => None,
        }
    }

    pub fn tagged_variant(&self, tag: &str) -> Option<TaggedVariant> {
        match self {
            Self::TaggedVariant(variant) if variant.tag == tag => Some(variant.clone()),
            Self::TaggedUnion(variants) => {
                variants.iter().find(|variant| variant.tag == tag).cloned()
            }
            Self::Opaque { ty, .. } => ty.tagged_variant(tag),
            _ => None,
        }
    }

    /// The variant a constructor call `Tag(payload)` builds when this type is
    /// the expectation. A nullable is transparent here and nowhere else:
    /// constructing is the one direction in which `nil` cannot be the answer,
    /// so `Goods?` names the same variants as `Goods` and the constructed value
    /// simply widens into the nullable. Reading a nullable union — `is`,
    /// pattern matching, narrowing — must prove the value non-nil first, so
    /// those paths keep using [`Type::tagged_variant`], which stops at the
    /// `Nullable`.
    pub fn constructed_tagged_variant(&self, tag: &str) -> Option<TaggedVariant> {
        match self {
            Self::Nullable(inner) => inner.constructed_tagged_variant(tag),
            Self::Opaque { ty, .. } => ty.constructed_tagged_variant(tag),
            other => other.tagged_variant(tag),
        }
    }

    /// The canonical GC record used at runtime to represent any tagged-union value.
    /// Layout: `{ tag: i32, value: unknown }` where `tag` is the variant discriminant
    /// and `value` holds the boxed payload (anyref / i31ref).
    pub fn canonical_tagged_union_record() -> Self {
        let mut fields = BTreeMap::new();
        fields.insert("tag".to_string(), Type::Numeric(NumericType::I32));
        fields.insert("value".to_string(), Type::Unknown);
        Type::record(fields)
    }

    /// This type as the runtime represents it: every tagged union and variant,
    /// however deeply nested, replaced by the canonical `{ tag, value }` record
    /// it is stored as. Source-level types and the annotations on IR
    /// instructions disagree about which name to use for the same value —
    /// `{Goods}` against `{{tag, value}}` — so both sides normalize through this
    /// before being compared. Nothing but tagged types is rewritten, which is
    /// what makes agreement here mean "the same value, named differently".
    /// Whether [`Self::runtime_representation`] would change this type.
    fn has_tagged_types(&self) -> bool {
        match self {
            Self::TaggedUnion(_) | Self::TaggedVariant(_) => true,
            Self::Record(fields) => fields.values().any(Self::has_tagged_types),
            Self::Array(inner) | Self::Variadic(inner) | Self::Nullable(inner) => {
                inner.has_tagged_types()
            }
            Self::Multi(types) => types.iter().any(Self::has_tagged_types),
            _ => false,
        }
    }

    pub fn runtime_representation(&self) -> Type {
        // Most types are already in runtime form; returning the same shared
        // value keeps this allocation-free and pointer-stable for them.
        if !self.has_tagged_types() {
            return self.clone();
        }
        match self {
            Self::TaggedUnion(_) | Self::TaggedVariant(_) => Self::canonical_tagged_union_record(),
            Self::Record(fields) => Self::record(
                fields
                    .iter()
                    .map(|(name, field_ty)| (name.clone(), field_ty.runtime_representation()))
                    .collect(),
            ),
            Self::Array(element_ty) => Self::Array(Arc::new(element_ty.runtime_representation())),
            Self::Variadic(element_ty) => {
                Self::Variadic(Arc::new(element_ty.runtime_representation()))
            }
            Self::Multi(types) => {
                Self::Multi(types.iter().map(Self::runtime_representation).collect())
            }
            Self::Nullable(inner) => Self::Nullable(Arc::new(inner.runtime_representation())),
            other => other.clone(),
        }
    }

    pub fn remove_tagged_variant(&self, tag: &str) -> Option<Type> {
        match self {
            Self::TaggedVariant(variant) if variant.tag == tag => None,
            Self::TaggedUnion(variants) => {
                let remaining = variants
                    .iter()
                    .filter(|variant| variant.tag != tag)
                    .cloned()
                    .collect::<Vec<_>>();
                match remaining.len() {
                    0 => None,
                    1 => Some(Self::TaggedVariant(
                        remaining.into_iter().next().expect("len checked"),
                    )),
                    _ => Some(Self::TaggedUnion(remaining)),
                }
            }
            Self::Opaque {
                name,
                ty,
                generic_extern,
            } => ty.remove_tagged_variant(tag).map(|inner| Self::Opaque {
                name: name.clone(),
                ty: OpaquePayload::new(inner),
                generic_extern: generic_extern.clone(),
            }),
            _ => None,
        }
    }
}

impl NumericType {
    pub fn can_implicitly_widen_to(self, target: Self) -> bool {
        use NumericType::{F32, F64, I32, I64, U32, U64};

        match (self, target) {
            (from, to) if from == to => true,
            (U32, U64 | I64 | F64) => true,
            (I32, I64 | F64) => true,
            (F32, F64) => true,
            _ => false,
        }
    }

    pub fn common(self, other: Self) -> Option<Self> {
        if self.can_implicitly_widen_to(other) {
            Some(other)
        } else if other.can_implicitly_widen_to(self) {
            Some(self)
        } else {
            None
        }
    }
}

impl std::fmt::Display for NumericType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::F32 => "f32",
            Self::F64 => "f64",
        };
        f.write_str(name)
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Numeric(ty) => ty.fmt(f),
            Self::Unit => f.write_str("unit"),
            Self::Bool => f.write_str("bool"),
            Self::Unknown => f.write_str("unknown"),
            Self::String => f.write_str("string"),
            Self::Bytes => f.write_str("bytes"),
            Self::Extern => f.write_str("extern"),
            Self::ExternSubtype(parent) => write!(f, "extern extends {parent}"),
            Self::Nil => f.write_str("nil"),
            Self::Nullable(inner) if matches!(inner.as_ref(), Self::Function { .. }) => {
                write!(f, "({inner})?")
            }
            Self::Nullable(inner) => write!(f, "{inner}?"),
            Self::TaggedVariant(variant) => write!(f, "{}({})", variant.tag, variant.payload),
            Self::TaggedUnion(variants) => {
                for (index, variant) in variants.iter().enumerate() {
                    if index > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "{}({})", variant.tag, variant.payload)?;
                }
                Ok(())
            }
            Self::StringLiteralUnion(members) => {
                for (index, member) in members.iter().enumerate() {
                    if index > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "\"{member}\"")?;
                }
                Ok(())
            }
            Self::Named { name, type_args } => {
                f.write_str(name)?;
                if !type_args.is_empty() {
                    write!(f, "<")?;
                    for (index, ty) in type_args.iter().enumerate() {
                        if index > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{ty}")?;
                    }
                    write!(f, ">")?;
                }
                Ok(())
            }
            Self::Opaque {
                name,
                generic_extern,
                ..
            } => {
                let Some(generic) = generic_extern else {
                    return f.write_str(name);
                };
                f.write_str(&generic.source_name)?;
                write!(f, "<")?;
                for (index, ty) in generic.type_args.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{ty}")?;
                }
                write!(f, ">")
            }
            Self::Array(element) => write!(f, "{{{element}}}"),
            Self::Variadic(element) => write!(f, "{element}..."),
            Self::TypedArray(kind) => f.write_str(kind.type_name()),
            Self::Multi(types) => {
                for (index, ty) in types.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{ty}")?;
                }
                Ok(())
            }
            Self::Function {
                params,
                return_type,
                has_self,
            } => {
                write!(f, "(")?;
                if *has_self {
                    write!(f, "self")?;
                    if !params.is_empty() {
                        write!(f, ", ")?;
                    }
                }
                for (index, param) in params.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{param}")?;
                }
                write!(f, ") -> {return_type}")
            }
            Self::Record(fields) => {
                write!(f, "{{")?;
                let mut first = true;
                for (name, ty) in fields.iter() {
                    // `$` never appears in user-written field names; fields
                    // carrying it are compiler-internal (the conformance
                    // wrapper identity field) and stay out of display.
                    if name.contains('$') {
                        continue;
                    }
                    if !first {
                        write!(f, ", ")?;
                    }
                    first = false;
                    write!(f, "{name}: {ty}")?;
                }
                write!(f, "}}")
            }
            Self::TypeParam(name) => f.write_str(name),
            Self::Thread => f.write_str("thread"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Stmt {
    Let {
        name: String,
        symbol_id: Option<SymbolId>,
        rebindability: Rebindability,
        ty: Option<Type>,
        value: Expr,
    },
    Assign {
        op: AssignOp,
        name: String,
        symbol_id: Option<SymbolId>,
        value: Expr,
    },
    IndexAssign {
        op: AssignOp,
        base: Box<Expr>,
        index: Box<Expr>,
        value: Expr,
    },
    FieldAssign {
        op: AssignOp,
        base: Box<Expr>,
        name: String,
        resolved_name: Option<String>,
        value: Expr,
    },
    If {
        condition: Expr,
        then_body: Vec<Stmt>,
        else_body: Vec<Stmt>,
    },
    IfCast {
        target_name: String,
        target_ty: Type,
        binding: String,
        binding_symbol_id: Option<SymbolId>,
        value: Expr,
        then_body: Vec<Stmt>,
        else_body: Vec<Stmt>,
    },
    Match {
        value: Expr,
        enum_ty: Type,
        arms: Vec<EnumMatchArm>,
    },
    While {
        condition: Expr,
        body: Vec<Stmt>,
    },
    Repeat {
        body: Vec<Stmt>,
        condition: Expr,
    },
    NumericFor {
        name: String,
        symbol_id: Option<SymbolId>,
        start: Expr,
        stop: Expr,
        step: Option<Expr>,
        body: Vec<Stmt>,
    },
    ForIn {
        names: Vec<String>,
        symbol_ids: Option<Vec<SymbolId>>,
        /// The iterator expression list after `in` — one to three expressions.
        /// A single expression is an array, a parameterless closure, a
        /// compile-time special form (`pairs(...)`, `string.gmatch(...)`), or
        /// a call returning an `(iterator, state, control)` triple. Two or
        /// three expressions are the explicit Lua generic-for protocol:
        /// `iterator, state[, control]`, where the loop calls
        /// `iterator(state, control)` until the first result is nil.
        iterators: Vec<Expr>,
        body: Vec<Stmt>,
    },
    Break,
    Continue,
    Return(Expr),
    ReturnMulti(Vec<Expr>),
    LetMulti {
        bindings: Vec<Binding>,
        values: Vec<Expr>,
    },
    AssignMulti {
        targets: Vec<String>,
        symbol_ids: Option<Vec<SymbolId>>,
        values: Vec<Expr>,
    },
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumMatchArm {
    pub variant: String,
    pub ordinal: i32,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    pub name: String,
    pub symbol_id: Option<SymbolId>,
    pub rebindability: Rebindability,
    pub ty: Option<Type>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignOp {
    Set,
    /// A compound assignment `target op= value`, desugaring to
    /// `target = target op value` while evaluating `target` only once.
    Compound(BinaryOp),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rebindability {
    Rebindable,
    Const,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Number(NumberLiteral, Option<Span>),
    Bool(bool, Option<Span>),
    Nil(Option<Span>),
    String(String, Option<Span>),
    Bytes(Vec<u8>, Option<Span>),
    Name(String, Option<SymbolId>, Option<Span>),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
        resolved_name: Option<String>,
        span: Option<Span>,
    },
    Cast {
        expr: Box<Expr>,
        ty: Type,
        span: Option<Span>,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expr>,
        right: Box<Expr>,
        resolved_name: Option<String>,
        span: Option<Span>,
    },
    IsVariant {
        expr: Box<Expr>,
        tag: String,
        span: Option<Span>,
    },
    If {
        condition: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
        span: Option<Span>,
    },
    Call {
        callee: Box<Expr>,
        type_args: Vec<Type>,
        args: Vec<Expr>,
        span: Option<Span>,
        /// If this call originated from a generic method call, this contains
        /// information needed to perform receiver mutation writeback.
        /// The receiver is always the first argument when this is Some.
        method_call_origin: Option<MethodCallOrigin>,
    },
    Vararg(Option<Span>),
    MethodCall {
        receiver: Box<Expr>,
        name: String,
        resolved_name: Option<String>,
        type_args: Vec<Type>,
        args: Vec<Expr>,
        span: Option<Span>,
    },
    Function(FunctionExpr),
    /// A relative module import, e.g. `require("./math")`.
    ///
    /// The string is the raw path as written in source. The module linker in
    /// `waluau-driver` resolves it and replaces this node with a reference to
    /// the imported module's exported function, so later compiler stages never
    /// observe a `Require` node.
    Require(String, Option<Span>),
    ArrayLiteral {
        elements: Vec<Expr>,
        span: Option<Span>,
    },
    /// A table literal with named fields, e.g. `{ add = fn, sub = other }`.
    TableLiteral {
        fields: Vec<TableField>,
        span: Option<Span>,
    },
    /// Field access on a namespace value, e.g. `m.add`.
    Field {
        base: Box<Expr>,
        name: String,
        resolved_name: Option<String>,
        span: Option<Span>,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
        span: Option<Span>,
    },
}

impl Expr {
    pub fn span(&self) -> Option<Span> {
        match self {
            Expr::Number(_, span) => *span,
            Expr::Bool(_, span) => *span,
            Expr::Nil(span) => *span,
            Expr::String(_, span) => *span,
            Expr::Bytes(_, span) => *span,
            Expr::Name(_, _, span) => *span,
            Expr::Vararg(span) => *span,
            Expr::Unary { span, .. } => *span,
            Expr::Cast { span, .. } => *span,
            Expr::Binary { span, .. } => *span,
            Expr::IsVariant { span, .. } => *span,
            Expr::If { span, .. } => *span,
            Expr::Call { span, .. } => *span,
            Expr::MethodCall { span, .. } => *span,
            Expr::Function(f) => f.span,
            Expr::Require(_, span) => *span,
            Expr::ArrayLiteral { span, .. } => *span,
            Expr::TableLiteral { span, .. } => *span,
            Expr::Field { span, .. } => *span,
            Expr::Index { span, .. } => *span,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NumberLiteral {
    pub raw: String,
}

impl NumberLiteral {
    /// The literal's value when written in integer form (decimal or hex,
    /// no fraction or exponent). `None` for float-form or malformed literals.
    pub fn int_value(&self) -> Option<i128> {
        let raw = self.raw.replace('_', "");
        if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
            return u128::from_str_radix(hex, 16)
                .ok()
                .map(|value| value as i128);
        }
        raw.parse::<i128>().ok()
    }

    /// The literal's value as an f64. `None` for malformed literals.
    pub fn float_value(&self) -> Option<f64> {
        let raw = self.raw.replace('_', "");
        if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
            return u128::from_str_radix(hex, 16).ok().map(|value| value as f64);
        }
        raw.parse::<f64>().ok()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOp {
    Add,
    Concat,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    Eq,
    NotEq,
    Less,
    LessEq,
    Greater,
    GreaterEq,
    And,
    Or,
}

impl BinaryOp {
    /// Whether `target op= value` is a legal compound assignment for a target of
    /// type `ty`. Arithmetic ops require a numeric target; `..` requires a
    /// string target. Other operators are never used in compound assignment.
    pub fn compound_target_ok(self, ty: &Type) -> bool {
        match self {
            BinaryOp::Concat => matches!(ty, Type::String),
            BinaryOp::Add
            | BinaryOp::Sub
            | BinaryOp::Mul
            | BinaryOp::Div
            | BinaryOp::FloorDiv
            | BinaryOp::Mod
            | BinaryOp::Pow => ty.is_numeric(),
            BinaryOp::Eq
            | BinaryOp::NotEq
            | BinaryOp::Less
            | BinaryOp::LessEq
            | BinaryOp::Greater
            | BinaryOp::GreaterEq
            | BinaryOp::And
            | BinaryOp::Or => false,
        }
    }

    /// Human-readable description of the target type a compound assignment with
    /// this operator requires, used in diagnostics.
    pub fn compound_target_kind(self) -> &'static str {
        match self {
            BinaryOp::Concat => "string",
            _ => "numeric",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
    Len,
}

use std::collections::{HashMap, HashSet};
use waluau_diagnostics::Diagnostic;

struct Resolver {
    scopes: Vec<HashMap<String, SymbolId>>,
    next_symbol_id: usize,
    /// Source name of every declared symbol, for Wasm debug-name emission.
    symbol_names: std::collections::BTreeMap<SymbolId, String>,
    /// Hoisted authored module function bindings cannot be rebound.
    non_rebindable_module_functions: HashSet<SymbolId>,
}

impl Resolver {
    fn new() -> Self {
        let mut global_bindings = HashMap::new();
        let mut resolver = Self {
            scopes: Vec::new(),
            next_symbol_id: 1,
            symbol_names: std::collections::BTreeMap::new(),
            non_rebindable_module_functions: HashSet::new(),
        };

        // Populate builtins
        for builtin in &[
            "print",
            "assert",
            "error",
            "pcall",
            "tostring",
            "select",
            "math",
            "coroutine",
            "promise",
            "json",
            "table",
            "string",
            "bit32",
        ] {
            let id = resolver.next_id();
            global_bindings.insert(builtin.to_string(), id);
        }
        // Typed-array constructor namespaces (`Float32Array.create(n)` etc.).
        for kind in TypedArrayKind::ALL {
            let id = resolver.next_id();
            global_bindings.insert(kind.type_name().to_string(), id);
        }

        resolver.scopes.push(global_bindings);
        resolver
    }

    fn next_id(&mut self) -> SymbolId {
        let id = SymbolId(self.next_symbol_id);
        self.next_symbol_id += 1;
        id
    }

    fn enter_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn exit_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, name: &str) -> SymbolId {
        let id = self.next_id();
        if let Some(current) = self.scopes.last_mut() {
            current.insert(name.to_string(), id);
        }
        self.symbol_names.insert(id, name.to_string());
        id
    }

    /// Bind `name` to an existing symbol in the current scope without
    /// allocating a new id.
    fn bind_existing(&mut self, name: &str, id: SymbolId) {
        if let Some(current) = self.scopes.last_mut() {
            current.insert(name.to_string(), id);
        }
    }

    fn lookup(&self, name: &str) -> Option<SymbolId> {
        for scope in self.scopes.iter().rev() {
            if let Some(id) = scope.get(name) {
                return Some(*id);
            }
        }
        None
    }

    fn reject_module_function_rebinding(
        &self,
        id: SymbolId,
        name: &str,
        span: Option<Span>,
    ) -> Result<(), Diagnostic> {
        if self.non_rebindable_module_functions.contains(&id) {
            let mut diagnostic = Diagnostic::new_with_code(
                "binding/module-function-rebind",
                format!("cannot rebind module function '{name}'"),
            );
            if let Some(span) = span {
                diagnostic = diagnostic.with_span(span);
            }
            return Err(diagnostic);
        }
        Ok(())
    }

    fn resolve_function(&mut self, function: &mut Function) -> Result<(), Diagnostic> {
        self.enter_scope();
        for param in &mut function.params {
            let id = self.declare(&param.name);
            param.symbol_id = Some(id);
        }
        for stmt in &mut function.body {
            self.resolve_stmt(stmt)?;
        }
        self.exit_scope();
        Ok(())
    }

    fn resolve_top_level_init_function(
        &mut self,
        function: &mut Function,
    ) -> Result<(), Diagnostic> {
        for stmt in &mut function.body {
            let lexical_function_declaration = stmt.lexical_function_declaration().is_some();
            match stmt {
                Stmt::Let {
                    name,
                    symbol_id,
                    value,
                    ..
                } => {
                    if lexical_function_declaration && self.lookup(name).is_none() {
                        self.declare(name);
                    }
                    self.resolve_expr(value)?;
                    *symbol_id = Some(self.lookup(name).ok_or_else(|| {
                        Diagnostic::new(format!("unknown module binding '{name}'"))
                    })?);
                }
                Stmt::LetMulti { bindings, values } => {
                    for value in values {
                        self.resolve_expr(value)?;
                    }
                    for binding in bindings {
                        binding.symbol_id = Some(self.lookup(&binding.name).ok_or_else(|| {
                            Diagnostic::new(format!("unknown module binding '{}'", binding.name))
                        })?);
                    }
                }
                _ => self.resolve_stmt(stmt)?,
            }
        }
        Ok(())
    }

    fn resolve_stmt(&mut self, stmt: &mut Stmt) -> Result<(), Diagnostic> {
        let lexical_function_class = stmt
            .lexical_function_declaration()
            .map(|declaration| declaration.class);
        match stmt {
            Stmt::Let {
                name,
                symbol_id,
                value,
                ..
            } => {
                let id = if lexical_function_class.is_some() {
                    // A lexical function declaration introduces one binding:
                    // the declaration and references from its body share this
                    // identity. It is visible to the function body, but no
                    // earlier statement can see it because resolution remains
                    // ordered.
                    let id = self.declare(name);
                    self.resolve_expr(value)?;
                    id
                } else {
                    self.resolve_expr(value)?;
                    self.declare(name)
                };
                *symbol_id = Some(id);
            }
            Stmt::Assign {
                name,
                symbol_id,
                value,
                ..
            } => {
                self.resolve_expr(value)?;
                let id = self.lookup(name).ok_or_else(|| {
                    Diagnostic::new(format!("unknown lexical or module binding '{name}'"))
                })?;
                self.reject_module_function_rebinding(id, name, value.span())?;
                *symbol_id = Some(id);
            }
            Stmt::LetMulti { bindings, values } => {
                for value in values {
                    self.resolve_expr(value)?;
                }
                for binding in bindings {
                    let id = self.declare(&binding.name);
                    binding.symbol_id = Some(id);
                }
            }
            Stmt::AssignMulti {
                targets,
                symbol_ids,
                values,
            } => {
                for value in values.iter_mut() {
                    self.resolve_expr(value)?;
                }
                let mut ids = Vec::new();
                for target in targets {
                    let id = self.lookup(target).ok_or_else(|| {
                        Diagnostic::new(format!("unknown lexical or module binding '{target}'"))
                    })?;
                    self.reject_module_function_rebinding(
                        id,
                        target,
                        values.first().and_then(Expr::span),
                    )?;
                    ids.push(id);
                }
                *symbol_ids = Some(ids);
            }
            Stmt::If {
                condition,
                then_body,
                else_body,
            } => {
                self.resolve_expr(condition)?;
                self.enter_scope();
                for s in then_body {
                    self.resolve_stmt(s)?;
                }
                self.exit_scope();
                self.enter_scope();
                for s in else_body {
                    self.resolve_stmt(s)?;
                }
                self.exit_scope();
            }
            Stmt::IfCast {
                binding,
                binding_symbol_id,
                value,
                then_body,
                else_body,
                ..
            } => {
                self.resolve_expr(value)?;
                self.enter_scope();
                let id = self.declare(binding);
                *binding_symbol_id = Some(id);
                for s in then_body {
                    self.resolve_stmt(s)?;
                }
                self.exit_scope();
                self.enter_scope();
                for s in else_body {
                    self.resolve_stmt(s)?;
                }
                self.exit_scope();
            }
            Stmt::Match { value, arms, .. } => {
                self.resolve_expr(value)?;
                for arm in arms {
                    self.enter_scope();
                    for stmt in &mut arm.body {
                        self.resolve_stmt(stmt)?;
                    }
                    self.exit_scope();
                }
            }
            Stmt::While { condition, body } => {
                self.resolve_expr(condition)?;
                self.enter_scope();
                for s in body {
                    self.resolve_stmt(s)?;
                }
                self.exit_scope();
            }
            Stmt::Repeat { body, condition } => {
                self.enter_scope();
                for s in body {
                    self.resolve_stmt(s)?;
                }
                self.resolve_expr(condition)?;
                self.exit_scope();
            }
            Stmt::NumericFor {
                name,
                symbol_id,
                start,
                stop,
                step,
                body,
            } => {
                self.resolve_expr(start)?;
                self.resolve_expr(stop)?;
                if let Some(s) = step {
                    self.resolve_expr(s)?;
                }
                self.enter_scope();
                let id = self.declare(name);
                *symbol_id = Some(id);
                for s in body {
                    self.resolve_stmt(s)?;
                }
                self.exit_scope();
            }
            Stmt::ForIn {
                names,
                symbol_ids,
                iterators,
                body,
            } => {
                // A bare unbound `next` heading a multi-expression iterator
                // list is the compile-time `next` builtin, not a value; a
                // local named `next` still shadows it and binds normally.
                let skip_builtin_next_head = iterators.len() >= 2
                    && matches!(&iterators[0], Expr::Name(name, _, _) if name == "next")
                    && self.lookup("next").is_none();
                for (index, iterator) in iterators.iter_mut().enumerate() {
                    if index == 0 && skip_builtin_next_head {
                        continue;
                    }
                    self.resolve_expr(iterator)?;
                }
                self.enter_scope();
                let mut ids = Vec::new();
                for name in names {
                    let id = self.declare(name);
                    ids.push(id);
                }
                *symbol_ids = Some(ids);
                for s in body {
                    self.resolve_stmt(s)?;
                }
                self.exit_scope();
            }
            Stmt::Return(expr) => {
                self.resolve_expr(expr)?;
            }
            Stmt::ReturnMulti(exprs) => {
                for expr in exprs {
                    self.resolve_expr(expr)?;
                }
            }
            Stmt::Expr(expr) => {
                self.resolve_expr(expr)?;
            }
            Stmt::IndexAssign {
                base, index, value, ..
            } => {
                self.resolve_expr(base)?;
                self.resolve_expr(index)?;
                self.resolve_expr(value)?;
            }
            Stmt::FieldAssign { base, value, .. } => {
                self.resolve_expr(base)?;
                self.resolve_expr(value)?;
            }
            Stmt::Break | Stmt::Continue => {}
        }
        Ok(())
    }

    fn resolve_top_level_stmt(&mut self, stmt: &mut Stmt) -> Result<(), Diagnostic> {
        match stmt {
            Stmt::Assign {
                op,
                name,
                symbol_id,
                value,
            } => {
                self.resolve_expr(value)?;
                let id = match self.lookup(name) {
                    Some(id) => id,
                    None if *op == AssignOp::Set => self.declare(name),
                    None => {
                        return Err(Diagnostic::new(format!(
                            "unknown lexical or module binding '{name}'"
                        )));
                    }
                };
                self.reject_module_function_rebinding(id, name, value.span())?;
                *symbol_id = Some(id);
                Ok(())
            }
            Stmt::AssignMulti {
                targets,
                symbol_ids,
                values,
            } => {
                for value in values.iter_mut() {
                    self.resolve_expr(value)?;
                }
                let mut ids = Vec::new();
                for target in targets {
                    let id = self.lookup(target).unwrap_or_else(|| self.declare(target));
                    self.reject_module_function_rebinding(
                        id,
                        target,
                        values.first().and_then(Expr::span),
                    )?;
                    ids.push(id);
                }
                *symbol_ids = Some(ids);
                Ok(())
            }
            _ => self.resolve_stmt(stmt),
        }
    }

    fn resolve_expr(&mut self, expr: &mut Expr) -> Result<(), Diagnostic> {
        match expr {
            Expr::Name(name, symbol_id, _) => {
                let id = self.lookup(name).ok_or_else(|| {
                    Diagnostic::new(format!("unknown lexical or module binding '{name}'"))
                })?;
                *symbol_id = Some(id);
            }
            Expr::Unary { expr, .. } | Expr::Cast { expr, .. } | Expr::IsVariant { expr, .. } => {
                self.resolve_expr(expr)?;
            }
            Expr::Binary { left, right, .. }
            | Expr::Index {
                base: left,
                index: right,
                ..
            } => {
                self.resolve_expr(left)?;
                self.resolve_expr(right)?;
            }
            Expr::If {
                condition,
                then_expr,
                else_expr,
                ..
            } => {
                self.resolve_expr(condition)?;
                self.resolve_expr(then_expr)?;
                self.resolve_expr(else_expr)?;
            }
            Expr::Call {
                callee,
                args,
                method_call_origin,
                ..
            } => {
                // `Tag(expr)` may be a tagged-union constructor rather than a call to a
                // known function/local. HIR has already validated such names (rejecting
                // genuinely unknown names with "unknown name '...'"), so here we simply
                // leave the callee's symbol_id as `None` and let IR lowering recognize it
                // as a constructor via the expected tagged-union type.
                let is_potential_constructor = matches!(
                    (callee.as_ref(), args.as_slice()),
                    (Expr::Name(name, _, _), [_]) if self.lookup(name).is_none()
                );
                if !is_potential_constructor {
                    self.resolve_expr(callee)?;
                }
                for arg in args {
                    self.resolve_expr(arg)?;
                }
                if let Some(origin) = method_call_origin {
                    self.resolve_expr(&mut origin.original_receiver)?;
                }
            }
            Expr::MethodCall { receiver, args, .. } => {
                self.resolve_expr(receiver)?;
                for arg in args {
                    self.resolve_expr(arg)?;
                }
            }
            Expr::Function(function) => {
                self.enter_scope();
                if let Some(name) = &function.name {
                    let lexical_declaration = matches!(
                        function.declaration_class,
                        Some(FunctionDeclarationClass::Local | FunctionDeclarationClass::Const)
                    );
                    let id = if lexical_declaration {
                        let id = self.lookup(name).ok_or_else(|| {
                            Diagnostic::new(format!("unknown lexical function binding '{name}'"))
                        })?;
                        self.bind_existing(name, id);
                        id
                    } else {
                        self.declare(name)
                    };
                    function.symbol_id = Some(id);
                }
                for param in &mut function.params {
                    let id = self.declare(&param.name);
                    param.symbol_id = Some(id);
                }
                for s in &mut function.body {
                    self.resolve_stmt(s)?;
                }
                self.exit_scope();
            }
            Expr::ArrayLiteral { elements, .. } => {
                for element in elements {
                    self.resolve_expr(element)?;
                }
            }
            Expr::TableLiteral { fields, .. } => {
                for field in fields {
                    self.resolve_expr(&mut field.value)?;
                }
            }
            Expr::Field { base, .. } => {
                self.resolve_expr(base)?;
            }
            Expr::Number(..)
            | Expr::Bool(..)
            | Expr::Nil(..)
            | Expr::String(..)
            | Expr::Bytes(..)
            | Expr::Vararg(..)
            | Expr::Require(..) => {}
        }
        Ok(())
    }
}

/// The argument of a `pairs(x)` call, when `expr` has exactly that shape.
/// `pairs` is not a runtime value: it only has meaning in for-in iterator
/// position, where the parser, the linkers, and the type checker each give it
/// a compile-time expansion.
pub fn pairs_call_arg(expr: &Expr) -> Option<&Expr> {
    let Expr::Call {
        callee,
        type_args,
        args,
        ..
    } = expr
    else {
        return None;
    };
    if !type_args.is_empty() || args.len() != 1 {
        return None;
    }
    match callee.as_ref() {
        Expr::Name(name, _, _) if name == "pairs" => Some(&args[0]),
        _ => None,
    }
}

/// The field map behind a record value iterated with `pairs`, looking through
/// nominal aliases.
pub fn pairs_record_fields(ty: &Type) -> Option<&BTreeMap<String, Type>> {
    match ty {
        Type::Record(fields) => Some(fields),
        Type::Opaque { ty, .. } => pairs_record_fields(ty),
        _ => None,
    }
}

/// The shared field type of a record iterated with `pairs`. `None` when the
/// type is not a record or has no fields. The type checker rejects mixed
/// field types before lowering, so the first field's type is authoritative.
pub fn pairs_record_value_type(ty: &Type) -> Option<Type> {
    pairs_record_fields(ty).and_then(|fields| fields.values().next().cloned())
}

/// The loop-index temporary introduced when `for name, value in pairs(Enum)`
/// is desugared into an array loop over the variant names.
pub const ENUM_PAIRS_ORDINAL: &str = "__enum_pairs_ordinal";

/// Desugars `for <names> in pairs(<Enum>) do <body> end` into an array for-in
/// over the variant-name strings. Authored array-loop indices are 1-based,
/// while variant ordinals are their 0-based declaration positions, so a
/// two-variable loop rebuilds the enum value by subtracting one before the
/// cast:
///
/// ```text
/// for __enum_pairs_ordinal, name in {"A", "B"} do
///     local value = (__enum_pairs_ordinal - 1) :: Enum
///     <body>
/// end
/// ```
///
/// `type_name` is the enum type to cast ordinals back into: the source name
/// for a local enum, the canonical linked name for an imported one.
pub fn enum_pairs_for_in(
    enum_display_name: &str,
    type_name: &str,
    variants: &[String],
    names: Vec<String>,
    body: Vec<Stmt>,
    span: Option<Span>,
) -> Result<Stmt, Diagnostic> {
    if names.is_empty() || names.len() > 2 {
        return Err(Diagnostic::new(format!(
            "pairs over enum '{enum_display_name}' yields a variant name and value; expected 1 or 2 loop variables, got {}",
            names.len()
        )));
    }
    let iterator = Expr::ArrayLiteral {
        elements: variants
            .iter()
            .map(|variant| Expr::String(variant.clone(), span))
            .collect(),
        span,
    };
    let (loop_names, body) = if let [name, value] = names.as_slice() {
        let value_binding = Stmt::Let {
            name: value.clone(),
            symbol_id: None,
            rebindability: Rebindability::Const,
            ty: None,
            value: Expr::Cast {
                expr: Box::new(Expr::Binary {
                    op: BinaryOp::Sub,
                    left: Box::new(Expr::Name(ENUM_PAIRS_ORDINAL.to_string(), None, span)),
                    right: Box::new(Expr::Number(NumberLiteral { raw: "1".into() }, span)),
                    resolved_name: None,
                    span,
                }),
                ty: Type::Named {
                    name: type_name.to_string(),
                    type_args: Vec::new(),
                },
                span,
            },
        };
        let mut new_body = Vec::with_capacity(body.len() + 1);
        new_body.push(value_binding);
        new_body.extend(body);
        (vec![ENUM_PAIRS_ORDINAL.to_string(), name.clone()], new_body)
    } else {
        (names, body)
    };
    Ok(Stmt::ForIn {
        names: loop_names,
        symbol_ids: None,
        iterators: vec![iterator],
        body,
    })
}

/// The target of a `for ... in next, t[, nil]` loop: the builtin `next`
/// iterating a record's fields or an array's elements. Only fires for a bare
/// unresolved `next` heading a multi-expression iterator list — a local named
/// `next` resolves to a symbol and is treated as an ordinary iterator value.
/// An explicit third expression must be the literal `nil` (Lua's `pairs(t)`
/// expansion); any other control start has no meaning for `next`.
pub fn for_in_builtin_next_target(iterators: &[Expr]) -> Option<Result<&Expr, Diagnostic>> {
    let (head, target) = match iterators {
        [head, target] => (head, target),
        [head, target, control] => {
            if !matches!(head, Expr::Name(name, None, _) if name == "next") {
                return None;
            }
            if !matches!(control, Expr::Nil(_)) {
                return Some(Err(Diagnostic::new(
                    "the control start for a `next` iterator must be nil",
                )));
            }
            (head, target)
        }
        _ => return None,
    };
    matches!(head, Expr::Name(name, None, _) if name == "next").then_some(Ok(target))
}

/// The pieces of a PIL-style stateful iterator function type
/// `(S, C) -> (K?, V...)`, driving `for k, v... in f, s, c0` loops: the loop
/// calls `f(s, control)` each iteration, stops when the first result is nil,
/// and otherwise binds `k: K` (plus the values) and rebinds the control.
pub struct IteratorProtocol {
    /// `S`: the invariant state parameter.
    pub state_ty: Type,
    /// `C`: the control parameter, fed `c0` (or nil) then each `K`.
    pub control_param_ty: Type,
    /// `K`: the non-nil control result bound to the first loop variable.
    pub control_ty: Type,
    /// `V...`: the remaining loop-variable types.
    pub value_types: Vec<Type>,
    /// `[K?, V...]` as declared — the call's result slots.
    pub return_slots: Vec<Type>,
}

/// Reads `f_ty` as an iterator-protocol function. `None` when the type is not
/// a two-parameter function or its first return value is not nullable (the
/// nil result is what ends the loop).
pub fn iterator_protocol(f_ty: &Type) -> Option<IteratorProtocol> {
    let Type::Function {
        params,
        return_type,
        has_self: false,
    } = f_ty
    else {
        return None;
    };
    let [state_ty, control_param_ty] = params.as_slice() else {
        return None;
    };
    let return_slots = match return_type.as_ref() {
        Type::Multi(slots) => slots.clone(),
        other => vec![other.clone()],
    };
    let control_ty = return_slots.first()?.nullable_inner()?;
    Some(IteratorProtocol {
        state_ty: state_ty.clone(),
        control_param_ty: control_param_ty.clone(),
        control_ty,
        value_types: return_slots[1..].to_vec(),
        return_slots,
    })
}

/// Resolve every name in `program` to a [`SymbolId`], stamping the ids into
/// the AST in place. Returns the source name of each declared symbol so later
/// stages can label Wasm locals in the emitted `name` section.
pub fn resolve_symbols(
    program: &mut Program,
) -> Result<std::collections::BTreeMap<SymbolId, String>, Diagnostic> {
    let mut resolver = Resolver::new();

    // Hoisted declarations enter module scope before any body or initializer
    // is resolved. The declaration facts own that semantic decision; the
    // existing `Program::functions` storage remains behavior-preserving.
    for function in &mut program.functions {
        let facts = function.declaration_class().facts();
        if !facts.hoisted || facts.binding != FunctionBindingClass::Module {
            continue;
        }
        if let FunctionName::Simple(name) = &function.name {
            let id = resolver.declare(name);
            resolver.non_rebindable_module_functions.insert(id);
            function.symbol_id = Some(id);
        }
    }
    for declared in &mut program.declared_imports {
        let id = resolver.declare(&declared.name);
        declared.symbol_id = Some(id);
    }
    // Overloaded declared imports carry unique internal names after type
    // checking (`name$overloadN`). Keep the shared base name resolvable so
    // builtin-intercepted references (e.g. `tonumber`) still bind; type
    // checking rewrites all other call sites to the mangled names.
    let mut overload_bases: Vec<(String, SymbolId)> = Vec::new();
    for declared in &program.declared_imports {
        if let (Some(base), Some(id)) = (overload_base_name(&declared.name), declared.symbol_id) {
            match overload_bases.iter_mut().find(|(name, _)| name == base) {
                Some(entry) => entry.1 = id,
                None => overload_bases.push((base.to_string(), id)),
            }
        }
    }
    for (base, id) in overload_bases {
        resolver.bind_existing(&base, id);
    }

    // Resolve top-level statements
    for stmt in &mut program.top_level {
        resolver.resolve_top_level_stmt(stmt)?;
    }

    // Resolve export expression
    if let Some(export) = &mut program.export {
        resolver.resolve_expr(export)?;
    }

    // Resolve each function's body
    for function in &mut program.functions {
        if function.name.to_string() == "__waluau_top_level_init" {
            resolver.resolve_top_level_init_function(function)?;
        } else {
            resolver.resolve_function(function)?;
        }
    }

    Ok(resolver.symbol_names)
}

#[cfg(test)]
mod type_tests {
    use std::hash::{DefaultHasher, Hash, Hasher};

    use super::{Arc, NumericType, OpaquePayload, TaggedVariant, Type};

    fn goods() -> Type {
        Type::TaggedUnion(vec![
            TaggedVariant {
                tag: "Upgrade".to_string(),
                payload: Arc::new(Type::Numeric(NumericType::I32)),
            },
            TaggedVariant {
                tag: "Spell".to_string(),
                payload: Arc::new(Type::Numeric(NumericType::I32)),
            },
        ])
    }

    #[test]
    fn tagged_variant_stops_at_a_nullable() {
        let nullable = Type::Nullable(Arc::new(goods()));
        assert!(nullable.tagged_variant("Upgrade").is_none());
        assert!(nullable.remove_tagged_variant("Upgrade").is_none());
    }

    #[test]
    fn constructed_tagged_variant_looks_through_a_nullable() {
        let nullable = Type::Nullable(Arc::new(goods()));
        let variant = nullable
            .constructed_tagged_variant("Upgrade")
            .expect("a nullable union constructs the same variants as the union");
        assert_eq!(variant.tag, "Upgrade");
        assert_eq!(*variant.payload, Type::Numeric(NumericType::I32));
    }

    #[test]
    fn constructed_tagged_variant_looks_through_an_aliased_nullable() {
        let aliased = Type::Opaque {
            name: "MaybeGoods".to_string(),
            ty: OpaquePayload::new(Type::Nullable(Arc::new(Type::Opaque {
                name: "Goods".to_string(),
                ty: OpaquePayload::new(goods()),
                generic_extern: None,
            }))),
            generic_extern: None,
        };
        assert_eq!(
            aliased
                .constructed_tagged_variant("Spell")
                .expect("alias wrappers are transparent")
                .tag,
            "Spell"
        );
    }

    #[test]
    fn constructed_tagged_variant_rejects_an_unknown_tag() {
        let nullable = Type::Nullable(Arc::new(goods()));
        assert!(nullable.constructed_tagged_variant("Trinket").is_none());
    }

    #[test]
    fn opaque_payload_identity_and_structural_hash_agree() {
        let shared = OpaquePayload::new(Type::Numeric(NumericType::I32));
        let shared_clone = shared.clone();
        let independent = OpaquePayload::new(Type::Numeric(NumericType::I32));

        assert!(shared.ptr_eq(&shared_clone));
        assert!(!shared.ptr_eq(&independent));
        assert_eq!(shared, independent);

        let hash = |payload: &OpaquePayload| {
            let mut hasher = DefaultHasher::new();
            payload.hash(&mut hasher);
            hasher.finish()
        };
        assert_eq!(hash(&shared), hash(&independent));

        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OpaquePayload>();
    }
}
