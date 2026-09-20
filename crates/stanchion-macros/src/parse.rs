//! Attribute parsing for `#[lua_class]` and its `#[lua(..)]` method options.

use proc_macro2::{Span, TokenStream as TokenStream2};
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Attribute, Error, Expr, ExprLit, Ident, Lit, Meta, Token};

/// Options on the `#[lua_class(..)]` attribute itself.
#[derive(Default)]
pub struct ClassArgs {
    /// Name of the generated class-table handle. Defaults to `{Trait}Class`.
    pub class: Option<Ident>,
    /// Name of the generated instance handle. Defaults to `{Trait}Handle`.
    pub handle: Option<Ident>,
    /// Class name used in conversion errors. Defaults to the trait name.
    pub lua_name: Option<String>,
}

impl ClassArgs {
    pub fn parse(tokens: TokenStream2) -> syn::Result<Self> {
        let mut args = ClassArgs::default();
        if tokens.is_empty() {
            return Ok(args);
        }
        let metas = Punctuated::<Meta, Token![,]>::parse_terminated.parse2(tokens)?;
        for meta in metas {
            let Meta::NameValue(nv) = &meta else {
                return Err(Error::new(
                    meta.span(),
                    "expected `class = \"..\"`, `handle = \"..\"` or `name = \"..\"`",
                ));
            };
            let value = string_value(&nv.value)?;
            let key = nv
                .path
                .get_ident()
                .map(Ident::to_string)
                .unwrap_or_default();
            match key.as_str() {
                "class" => args.class = Some(Ident::new(&value, nv.value.span())),
                "handle" => args.handle = Some(Ident::new(&value, nv.value.span())),
                "name" => args.lua_name = Some(value),
                _ => {
                    return Err(Error::new(
                        nv.path.span(),
                        "unknown option: expected `class`, `handle` or `name`",
                    ));
                }
            }
        }
        Ok(args)
    }
}

/// Options on a `#[lua(..)]` method attribute.
#[derive(Default)]
pub struct MethodOpts {
    /// Overrides the Lua key this method resolves to.
    pub name: Option<String>,
    /// A missing Lua method yields `Ok(None)` instead of an error.
    pub optional: bool,
    /// Read or write a field rather than calling a function.
    pub field: bool,
    /// Call with dot syntax (`obj.f(..)`) rather than colon syntax (`obj:f(..)`).
    pub function: bool,
    /// Span of the `#[lua(..)]` attribute, for diagnostics about its contents.
    pub span: Option<Span>,
}

impl MethodOpts {
    /// Splits `#[lua(..)]` off the attribute list, returning the rest untouched.
    pub fn parse(attrs: &[Attribute]) -> syn::Result<(Self, Vec<Attribute>)> {
        let mut opts = MethodOpts::default();
        let mut kept = Vec::new();

        for attr in attrs {
            if !attr.path().is_ident("lua") {
                kept.push(attr.clone());
                continue;
            }
            opts.span = Some(attr.span());
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    let value: Lit = meta.value()?.parse()?;
                    let Lit::Str(value) = value else {
                        return Err(meta.error("`name` takes a string literal"));
                    };
                    opts.name = Some(value.value());
                } else if meta.path.is_ident("optional") {
                    opts.optional = true;
                } else if meta.path.is_ident("field") {
                    opts.field = true;
                } else if meta.path.is_ident("function") {
                    opts.function = true;
                } else {
                    return Err(meta.error(
                        "unknown option: expected `name`, `optional`, `field` or `function`",
                    ));
                }
                Ok(())
            })?;
        }

        if opts.field && opts.function {
            return Err(Error::new(
                opts.span.unwrap_or_else(Span::call_site),
                "`field` and `function` are mutually exclusive",
            ));
        }
        Ok((opts, kept))
    }
}

/// How a method reaches into the Lua object.
#[derive(Clone, Copy)]
pub enum MethodKind {
    /// `obj:name(..)` — the object is passed as the first argument.
    Method { optional: bool },
    /// `obj.name(..)` — nothing implicit is passed.
    Function { optional: bool },
    /// `obj.name` read.
    FieldGet,
    /// `obj.name = value` write.
    FieldSet,
}

impl MethodKind {
    pub fn classify(
        opts: &MethodOpts,
        takes_self: bool,
        arg_count: usize,
        ident: &Ident,
        span: Span,
    ) -> syn::Result<Self> {
        if opts.field {
            if !takes_self {
                return Err(Error::new(
                    span,
                    format!("`{ident}` is marked `field` but takes no `&self`: fields live on instances"),
                ));
            }
            if opts.optional {
                return Err(Error::new(
                    span,
                    "`optional` applies to methods, not fields: read a field as `Result<Option<T>>` instead",
                ));
            }
            return match arg_count {
                0 => Ok(MethodKind::FieldGet),
                1 => Ok(MethodKind::FieldSet),
                _ => Err(Error::new(
                    span,
                    "a `field` method takes no arguments (get) or exactly one (set)",
                )),
            };
        }

        // Class-level functions have no receiver to pass, so they are always dot calls.
        if !takes_self || opts.function {
            return Ok(MethodKind::Function { optional: opts.optional });
        }
        Ok(MethodKind::Method { optional: opts.optional })
    }

    /// Whether the Lua table must define this key for the handle to be valid.
    pub fn is_required(&self) -> bool {
        matches!(
            self,
            MethodKind::Method { optional: false } | MethodKind::Function { optional: false }
        )
    }

    /// The Lua key this method resolves to when `#[lua(name = ..)]` is absent.
    pub fn default_name(&self, ident: &Ident) -> String {
        let name = ident.to_string();
        match self {
            MethodKind::FieldSet => name.strip_prefix("set_").unwrap_or(&name).to_string(),
            _ => name,
        }
    }
}

fn string_value(expr: &Expr) -> syn::Result<String> {
    match expr {
        Expr::Lit(ExprLit { lit: Lit::Str(value), .. }) => Ok(value.value()),
        other => Err(Error::new(other.span(), "expected a string literal")),
    }
}
