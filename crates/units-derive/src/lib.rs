//! `#[derive(Params)]`: the millimetre wire form of a stored record.
//!
//! The document stores lengths as whole nanometres (`units::Nm`, `PointNm`,
//! `Angle`, `ArcNm`) under `_nm` and `_mdeg` field names. MCP and CLI
//! payloads carry the same records with millimetres and degrees
//! (`units::Length`, `PointMm`, `Degrees`, `ArcMm`), because `0.25` is
//! natural where `250000` invites ×1000 mistakes. This derive writes that
//! wire form once per record instead of by hand: for `T` it generates
//! `TParams` with
//!
//! - every `Nm`, `PointNm`, `Angle` and `ArcNm` (also inside `Option`,
//!   `Vec`, `Box` and arrays) replaced by its wire type, and a `_nm` or
//!   `_mdeg` suffix dropped from the field name;
//! - every field marked `#[params]` (a nested record that derives `Params`)
//!   replaced by its `…Params` type, keeping the name;
//! - the record's derives, doc comments and serde and schemars attributes,
//!   except those that name the stored types (`skip_serializing_if` helpers,
//!   `with`, a `default` function, which is converted);
//! - `From` both ways, which cannot fail: the wire types hold nanometres.
//!
//! `#[params(default)]` on the record also implements `Default` for
//! `TParams` from `T::default()`, for a record whose `Default` is written
//! by hand.
//!
//! The generated code names `crate::units`, so the derive is for
//! `sparksmith-core`.
use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{
    Attribute, Data, DeriveInput, Expr, ExprLit, Field, Fields, GenericArgument, Ident, Lit, Meta,
    Path, PathArguments, Token, Type,
};

/// The stored unit types and their wire forms in `crate::units`.
const UNITS: &[(&str, &str)] = &[
    ("Nm", "Length"),
    ("PointNm", "PointMm"),
    ("Angle", "Degrees"),
    ("ArcNm", "ArcMm"),
];

/// Field name suffixes the wire form drops.
const SUFFIXES: &[&str] = &["_nm", "_mdeg"];

#[proc_macro_derive(Params, attributes(params))]
pub fn derive_params(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand(input: &DeriveInput) -> syn::Result<Tokens> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(
            input.generics.span(),
            "#[derive(Params)] supports records without generics",
        ));
    }
    let stored = &input.ident;
    let wire = format_ident!("{}Params", stored);
    let vis = &input.vis;
    let derives = derives(&input.attrs)?;
    let attrs: Vec<&Attribute> = input.attrs.iter().filter(|a| keep_attr(a)).collect();
    let mut helpers = Vec::new();
    let (body, to_wire, to_stored) = match &input.data {
        Data::Struct(data) => {
            let fields = wire_fields(&data.fields, &wire, "", &mut helpers)?;
            let semi = matches!(data.fields, Fields::Unnamed(_) | Fields::Unit).then(|| quote!(;));
            let c = conversions(&data.fields)?;
            let (sp, wp, bw, bs) = (
                c.stored_pattern,
                c.wire_pattern,
                c.build_wire,
                c.build_stored,
            );
            (
                quote!(#vis struct #wire #fields #semi),
                quote!({ let #stored #sp = v; #wire #bw }),
                quote!({ let #wire #wp = v; #stored #bs }),
            )
        }
        Data::Enum(data) => {
            let mut variants = Vec::new();
            let mut to_wire = Vec::new();
            let mut to_stored = Vec::new();
            for variant in &data.variants {
                let name = &variant.ident;
                let vattrs: Vec<&Attribute> =
                    variant.attrs.iter().filter(|a| keep_attr(a)).collect();
                let label = name.to_string().to_lowercase();
                let fields = wire_fields(&variant.fields, &wire, &label, &mut helpers)?;
                variants.push(quote!(#(#vattrs)* #name #fields));
                let c = conversions(&variant.fields)?;
                let (sp, wp, bw, bs) = (
                    c.stored_pattern,
                    c.wire_pattern,
                    c.build_wire,
                    c.build_stored,
                );
                to_wire.push(quote!(#stored::#name #sp => #wire::#name #bw));
                to_stored.push(quote!(#wire::#name #wp => #stored::#name #bs));
            }
            (
                quote!(#vis enum #wire { #(#variants),* }),
                quote!(match v { #(#to_wire),* }),
                quote!(match v { #(#to_stored),* }),
            )
        }
        Data::Union(_) => {
            return Err(syn::Error::new(
                input.span(),
                "#[derive(Params)] supports structs and enums",
            ));
        }
    };
    let default = if wants_default(&input.attrs)? {
        quote! {
            impl ::core::default::Default for #wire {
                fn default() -> Self {
                    <Self as ::core::convert::From<#stored>>::from(
                        <#stored as ::core::default::Default>::default(),
                    )
                }
            }
        }
    } else {
        quote!()
    };
    let doc =
        format!(" The wire form of [`{stored}`]: millimetres and degrees (`#[derive(Params)]`).");
    let helpers = if helpers.is_empty() {
        quote!()
    } else {
        quote!(impl #wire { #(#helpers)* })
    };
    Ok(quote! {
        #[doc = #doc]
        #[derive(#(#derives),*)]
        #(#attrs)*
        #body

        #helpers
        #default

        #[allow(clippy::redundant_closure, clippy::useless_conversion)]
        impl ::core::convert::From<#stored> for #wire {
            fn from(v: #stored) -> Self {
                #to_wire
            }
        }

        #[allow(clippy::redundant_closure, clippy::useless_conversion)]
        impl ::core::convert::From<#wire> for #stored {
            fn from(v: #wire) -> Self {
                #to_stored
            }
        }
    })
}

/// The record's derives, without `Params`, with the ones the wire form
/// needs. A derive macro does not see the `#[derive]` attributes of its
/// item, so `Clone`, `Debug` and `PartialEq` (which every wire type has,
/// and which a command payload holding the wire form derives) are always
/// added too.
fn derives(attrs: &[Attribute]) -> syn::Result<Vec<Path>> {
    let mut out: Vec<Path> = Vec::new();
    for attr in attrs.iter().filter(|a| a.path().is_ident("derive")) {
        let paths = attr.parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)?;
        out.extend(
            paths
                .into_iter()
                .filter(|p| p.segments.last().is_none_or(|s| s.ident != "Params")),
        );
    }
    for (name, path) in [
        ("Clone", quote!(::core::clone::Clone)),
        ("Debug", quote!(::core::fmt::Debug)),
        ("PartialEq", quote!(::core::cmp::PartialEq)),
        ("Serialize", quote!(::serde::Serialize)),
        ("Deserialize", quote!(::serde::Deserialize)),
        ("JsonSchema", quote!(::schemars::JsonSchema)),
    ] {
        if !out
            .iter()
            .any(|p| p.segments.last().is_some_and(|s| s.ident == name))
        {
            out.push(syn::parse2(path)?);
        }
    }
    Ok(out)
}

fn keep_attr(attr: &Attribute) -> bool {
    ["doc", "serde", "schemars", "allow"]
        .iter()
        .any(|name| attr.path().is_ident(name))
}

/// `#[params(default)]`.
fn wants_default(attrs: &[Attribute]) -> syn::Result<bool> {
    let mut default = false;
    for attr in attrs.iter().filter(|a| a.path().is_ident("params")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("default") {
                default = true;
                Ok(())
            } else {
                Err(meta.error("expected `default`"))
            }
        })?;
    }
    Ok(default)
}

/// Whether the field is marked `#[params]`.
fn nested(field: &Field) -> bool {
    field.attrs.iter().any(|a| a.path().is_ident("params"))
}

/// The wire type of `ty`, or `None` when it is the same.
fn wire_type(ty: &Type, nested: bool) -> Option<Type> {
    match ty {
        Type::Array(array) => {
            let inner = wire_type(&array.elem, nested)?;
            let mut out = array.clone();
            out.elem = Box::new(inner);
            Some(Type::Array(out))
        }
        Type::Path(path) if path.qself.is_none() => {
            let last = path.path.segments.last()?;
            if let Some(inner) = wrapped(last) {
                let wire = wire_type(inner, nested)?;
                let mut out = path.clone();
                if let Some(segment) = out.path.segments.last_mut()
                    && let PathArguments::AngleBracketed(args) = &mut segment.arguments
                    && let Some(GenericArgument::Type(t)) = args.args.first_mut()
                {
                    *t = wire;
                }
                return Some(Type::Path(out));
            }
            if let Some((_, wire)) = UNITS.iter().find(|(s, _)| last.ident == s) {
                let wire = format_ident!("{}", wire);
                return Some(syn::parse_quote!(crate::units::#wire));
            }
            if nested {
                let mut out = path.clone();
                if let Some(segment) = out.path.segments.last_mut() {
                    segment.ident = format_ident!("{}Params", segment.ident);
                }
                return Some(Type::Path(out));
            }
            None
        }
        _ => None,
    }
}

/// The type inside `Option`, `Vec` or `Box`.
fn wrapped(segment: &syn::PathSegment) -> Option<&Type> {
    if !["Option", "Vec", "Box"].iter().any(|w| segment.ident == w) {
        return None;
    }
    match &segment.arguments {
        PathArguments::AngleBracketed(args) if args.args.len() == 1 => match args.args.first() {
            Some(GenericArgument::Type(t)) => Some(t),
            _ => None,
        },
        _ => None,
    }
}

/// `value` of `ty` converted to the other form. Both directions have the
/// same shape: `From` exists each way at the leaves.
fn convert(ty: &Type, value: Tokens) -> Tokens {
    match ty {
        Type::Array(array) => {
            let inner = convert(&array.elem, quote!(v));
            quote!((#value).map(|v| #inner))
        }
        Type::Path(path) => {
            let Some(last) = path.path.segments.last() else {
                return value;
            };
            if let Some(inner) = wrapped(last) {
                return match last.ident.to_string().as_str() {
                    "Option" => {
                        let inner = convert(inner, quote!(v));
                        quote!((#value).map(|v| #inner))
                    }
                    "Vec" => {
                        let inner = convert(inner, quote!(v));
                        quote!((#value).into_iter().map(|v| #inner).collect::<::std::vec::Vec<_>>())
                    }
                    _ => {
                        let inner = convert(inner, quote!(*(#value)));
                        quote!(::std::boxed::Box::new(#inner))
                    }
                };
            }
            quote!(::core::convert::From::from(#value))
        }
        _ => value,
    }
}

/// The field's name on the wire.
fn wire_name(ident: &Ident, changed: bool, nested: bool) -> Ident {
    if changed && !nested {
        let name = ident.to_string();
        for suffix in SUFFIXES {
            if let Some(base) = name.strip_suffix(suffix)
                && !base.is_empty()
            {
                return Ident::new(base, ident.span());
            }
        }
    }
    ident.clone()
}

/// The fields of the wire form, with their attributes.
fn wire_fields(
    fields: &Fields,
    wire: &Ident,
    variant: &str,
    helpers: &mut Vec<Tokens>,
) -> syn::Result<Tokens> {
    let mut out = Vec::new();
    for (i, field) in fields.iter().enumerate() {
        let nested = nested(field);
        let changed = wire_type(&field.ty, nested);
        let ty = changed.clone().unwrap_or_else(|| field.ty.clone());
        let mut attrs = Vec::new();
        for attr in field.attrs.iter().filter(|a| keep_attr(a)) {
            if changed.is_none() || attr.path().is_ident("doc") || attr.path().is_ident("allow") {
                attrs.push(quote!(#attr));
            } else if attr.path().is_ident("serde") {
                let name = field
                    .ident
                    .as_ref()
                    .map_or_else(|| i.to_string(), ToString::to_string);
                let label = if variant.is_empty() {
                    name
                } else {
                    format!("{variant}_{name}")
                };
                let site = Site {
                    stored_ty: &field.ty,
                    wire_ty: &ty,
                    wire,
                    label: &label,
                };
                if let Some(serde) = serde_attr(attr, &site, helpers)? {
                    attrs.push(serde);
                }
            }
            // A changed field's schemars attributes describe the stored
            // type; the wire type has its own schema.
        }
        let vis = &field.vis;
        match &field.ident {
            Some(ident) => {
                let name = wire_name(ident, changed.is_some(), nested);
                out.push(quote!(#(#attrs)* #vis #name: #ty));
            }
            None => out.push(quote!(#(#attrs)* #vis #ty)),
        }
    }
    Ok(match fields {
        Fields::Named(_) => quote!({ #(#out),* }),
        Fields::Unnamed(_) => quote!(( #(#out),* )),
        Fields::Unit => quote!(),
    })
}

/// A changed field, for its serde attribute.
struct Site<'a> {
    stored_ty: &'a Type,
    wire_ty: &'a Type,
    wire: &'a Ident,
    label: &'a str,
}

/// A changed field's serde attribute for the wire form: helpers typed for
/// the stored form are dropped, and a `default` function is converted.
fn serde_attr(
    attr: &Attribute,
    site: &Site,
    helpers: &mut Vec<Tokens>,
) -> syn::Result<Option<Tokens>> {
    let metas = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
    let mut kept = Vec::new();
    for meta in metas {
        let name = meta.path().get_ident().map(ToString::to_string);
        match (name.as_deref(), &meta) {
            (Some("with" | "serialize_with" | "deserialize_with"), _) => {}
            (Some("skip_serializing_if"), Meta::NameValue(nv)) => {
                // Generic helpers (`Option::is_none`, `Vec::is_empty`) fit
                // both forms; a local one is typed for the stored form.
                if string_lit(&nv.value).is_some_and(|p| p.contains("::")) {
                    kept.push(quote!(#meta));
                }
            }
            (Some("default"), Meta::NameValue(nv)) => {
                let Some(path) = string_lit(&nv.value) else {
                    return Err(syn::Error::new(nv.span(), "expected a function path"));
                };
                let function: Path = syn::parse_str(&path)?;
                let helper = format_ident!("__default_{}", site.label);
                let converted = convert(site.stored_ty, quote!(#function()));
                let wire_ty = site.wire_ty;
                helpers.push(quote! {
                    #[allow(clippy::redundant_closure, clippy::useless_conversion)]
                    fn #helper() -> #wire_ty {
                        #converted
                    }
                });
                let target = format!("{}::{helper}", site.wire);
                kept.push(quote!(default = #target));
            }
            _ => kept.push(quote!(#meta)),
        }
    }
    Ok((!kept.is_empty()).then(|| quote!(#[serde(#(#kept),*)])))
}

fn string_lit(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) => Some(s.value()),
        _ => None,
    }
}

/// How one variant's (or struct's) fields convert: the pattern binding
/// the stored form, the pattern binding the wire form, and the constructor
/// of each form from the other's bindings.
struct Conversions {
    stored_pattern: Tokens,
    wire_pattern: Tokens,
    build_wire: Tokens,
    build_stored: Tokens,
}

fn conversions(fields: &Fields) -> syn::Result<Conversions> {
    match fields {
        Fields::Unit => Ok(Conversions {
            stored_pattern: quote!(),
            wire_pattern: quote!(),
            build_wire: quote!(),
            build_stored: quote!(),
        }),
        Fields::Named(named) => {
            let mut stored_pattern = Vec::new();
            let mut wire_pattern = Vec::new();
            let mut build_wire = Vec::new();
            let mut build_stored = Vec::new();
            for field in &named.named {
                let Some(ident) = &field.ident else {
                    return Err(syn::Error::new(field.span(), "expected a named field"));
                };
                let nested = nested(field);
                let changed = wire_type(&field.ty, nested).is_some();
                let name = wire_name(ident, changed, nested);
                let binding = format_ident!("f_{}", ident);
                let value = if changed {
                    convert(&field.ty, quote!(#binding))
                } else {
                    quote!(#binding)
                };
                stored_pattern.push(quote!(#ident: #binding));
                wire_pattern.push(quote!(#name: #binding));
                build_wire.push(quote!(#name: #value));
                build_stored.push(quote!(#ident: #value));
            }
            Ok(Conversions {
                stored_pattern: quote!({ #(#stored_pattern),* }),
                wire_pattern: quote!({ #(#wire_pattern),* }),
                build_wire: quote!({ #(#build_wire),* }),
                build_stored: quote!({ #(#build_stored),* }),
            })
        }
        Fields::Unnamed(unnamed) => {
            let mut bindings = Vec::new();
            let mut values = Vec::new();
            for (i, field) in unnamed.unnamed.iter().enumerate() {
                let binding = format_ident!("f{}", i);
                let nested = nested(field);
                let value = if wire_type(&field.ty, nested).is_some() {
                    convert(&field.ty, quote!(#binding))
                } else {
                    quote!(#binding)
                };
                bindings.push(binding);
                values.push(value);
            }
            Ok(Conversions {
                stored_pattern: quote!(( #(#bindings),* )),
                wire_pattern: quote!(( #(#bindings),* )),
                build_wire: quote!(( #(#values),* )),
                build_stored: quote!(( #(#values),* )),
            })
        }
    }
}
