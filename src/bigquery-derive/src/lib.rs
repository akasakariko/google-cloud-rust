// Copyright 2026 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Derive macros for the Google Cloud BigQuery client.
//!
//! Each named field must map to a distinct BigQuery field name. The
//! `#[bigquery(rename = "...")]` option can appear only once per named field
//! and is not supported on the container or on tuple fields.
//!
//! # Compatibility
//!
//! `cargo-semver-checks` cannot verify this crate, and it does not see these
//! macros through the re-exports in `google-cloud-bigquery` either. Any change
//! that does one of the following is a **breaking change**, even if all tests
//! pass:
//!
//! - renames or removes a derive, the `#[bigquery(...)]` helper attribute, or
//!   any option it accepts (e.g. `rename`),
//! - rejects input that used to compile,
//! - changes the behavior of the generated code (e.g. which column a field
//!   reads).
//!
//! Such changes require a breaking version bump (major, or minor for `0.x`)
//! of this crate **and** of `google-cloud-bigquery`, which re-exports these
//! derives and depends on this crate. Update the workspace dependency versions
//! in the root `Cargo.toml` accordingly. Librarian defaults to non-breaking
//! version bumps, so make these bumps by hand.
//!
//! The generated code may only reference `google_cloud_bigquery` items that
//! exist in every version of `google-cloud-bigquery` compatible with this
//! crate. When `google-cloud-bigquery` takes a breaking version bump, also take
//! a breaking version bump of this crate.
//!
//! ## Contract tests
//!
//! The contract tests pin the input these macros **accept**. They are
//! compile-only: each case file is one supported input shape, and the test
//! target fails to build if a case stops compiling. They do not check the
//! behavior of the generated code. Behavior changes are still breaking, but
//! the unit tests in `src/bigquery/src/query/` (`row.rs`, `from_sql.rs`) cover
//! them. Code review owns everything the tests do not catch.
//!
//! Every published proc-macro crate in this repository has a contract test
//! directory under any crate's tests directory (`src/*/tests/`). The directory
//! name is the package name without the `google-cloud-` prefix, with `-`
//! replaced by `_`, plus `_contract`. For example,
//! `google-cloud-bigquery-derive` uses
//! `src/bigquery/tests/bigquery_derive_contract/`. The directory is one test
//! target: its `main.rs` declares one `mod <case>;` per case file.
//!
//! - New accepted input gets a new case file and a new `mod` line.
//! - Never modify, rename, or delete an existing case file, or remove its
//!   `mod` line.
//!
//! `.github/scripts/proc-macro-contract-check.sh` fails if an existing case
//! file is modified, renamed, or deleted on any pull request (see
//! `.github/workflows/proc-macro-contract.yaml`). It also fails if a published
//! proc-macro crate has no contract tests. Removing a `mod` line is left to
//! code review.
//!
//! To change an existing case on purpose, apply the
//! `rust: proc-macro-contract-change` label to the pull request. The label
//! skips only the check for changes to existing cases; the coverage check
//! always runs. Changes that only touch whitespace or
//! blank lines are ignored. Comment-only or other formatting changes still need
//! the label, but no version bump. If the change affects accepted input, it is
//! very likely a breaking change, and the pull request (or the release) must
//! take the breaking version bumps of both crates described above.

extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use std::collections::HashSet;
use syn::ext::IdentExt;
use syn::{Data, DeriveInput, Fields, parse_macro_input};

/// Derives standard library [TryFrom] for converting a BigQuery `Row` into a struct.
///
/// Structs with named fields match columns by field name (or via `#[bigquery(rename = "new_name")]`).
/// Tuple structs match columns positionally by 0-based index.
///
/// [TryFrom]: std::convert::TryFrom
#[proc_macro_derive(FromRow, attributes(bigquery))]
pub fn derive_from_row(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    derive_from_row_impl(input).into()
}

fn derive_from_row_impl(input: DeriveInput) -> proc_macro2::TokenStream {
    let body = match struct_initializer(&input, "FromRow", quote!(row)) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error(),
    };

    // TODO(#5592): check that the schema and this struct have same columns/attributes count.

    let name = input.ident;
    let generics = add_trait_bounds(input.generics);
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    quote! {
        impl #impl_generics std::convert::TryFrom<google_cloud_bigquery::query::Row> for #name #ty_generics #where_clause {
            type Error = google_cloud_bigquery::error::RowError;

            fn try_from(mut row: google_cloud_bigquery::query::Row) -> std::result::Result<Self, Self::Error> {
                std::result::Result::Ok(#body)
            }
        }
    }
}

/// Derives `FromSql` for converting a BigQuery `STRUCT` value into a Rust struct.
///
/// Structs with named fields match `STRUCT` fields by field name (or via `#[bigquery(rename = "new_name")]`).
/// Tuple structs match `STRUCT` fields positionally by 0-based index, supporting anonymous
/// `STRUCT(1, 'a')` fields and ordered positional extraction.
#[proc_macro_derive(FromSql, attributes(bigquery))]
pub fn derive_from_sql(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    derive_from_sql_impl(input).into()
}

fn derive_from_sql_impl(input: DeriveInput) -> proc_macro2::TokenStream {
    let body = match struct_initializer(&input, "FromSql", quote!(value)) {
        Ok(body) => body,
        Err(err) => return err.to_compile_error(),
    };

    let name = input.ident;
    let generics = add_trait_bounds(input.generics);
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    quote! {
        impl #impl_generics google_cloud_bigquery::query::FromSql for #name #ty_generics #where_clause {
            fn from_value(mut value: google_cloud_bigquery::query::SqlValue) -> std::result::Result<Self, google_cloud_bigquery::error::ConvertError> {
                std::result::Result::Ok(#body)
            }
        }
    }
}

fn struct_initializer(
    input: &DeriveInput,
    derive_name: &str,
    source: proc_macro2::TokenStream,
) -> syn::Result<proc_macro2::TokenStream> {
    if let Some(attr) = input
        .attrs
        .iter()
        .find(|attr| attr.path().is_ident("bigquery"))
    {
        return Err(syn::Error::new_spanned(
            attr,
            "bigquery attributes are not supported on the container",
        ));
    }
    let unsupported = || {
        syn::Error::new_spanned(
            &input.ident,
            format!("{derive_name} can only be derived for non-empty structs"),
        )
    };
    let Data::Struct(data) = &input.data else {
        return Err(unsupported());
    };
    match &data.fields {
        Fields::Named(fields) if !fields.named.is_empty() => {
            let mut names = HashSet::new();
            let mut initializers = Vec::new();
            for field in &fields.named {
                let ident = field
                    .ident
                    .as_ref()
                    .expect("named field must have identifier");
                let (name, span) = get_field_name(ident, &field.attrs)?;
                if !names.insert(name.clone()) {
                    return Err(syn::Error::new(
                        span,
                        format!("duplicate BigQuery field name `{name}`"),
                    ));
                }
                initializers.push(quote!(#ident: #source.take(#name)?));
            }
            Ok(quote!(Self { #(#initializers,)* }))
        }
        Fields::Unnamed(fields) if !fields.unnamed.is_empty() => {
            reject_bigquery_attrs(&fields.unnamed)?;
            let indices = 0..fields.unnamed.len();
            Ok(quote!(Self(#(#source.take(#indices)?,)*)))
        }
        _ => Err(unsupported()),
    }
}

fn reject_bigquery_attrs<'a>(fields: impl IntoIterator<Item = &'a syn::Field>) -> syn::Result<()> {
    for field in fields {
        if let Some(attr) = field.attrs.iter().find(|a| a.path().is_ident("bigquery")) {
            return Err(syn::Error::new_spanned(
                attr,
                "bigquery attributes are not supported on tuple struct fields",
            ));
        }
    }
    Ok(())
}

/// Adds a `FromSql` bound for each generic parameter.
fn add_trait_bounds(mut generics: syn::Generics) -> syn::Generics {
    for param in &mut generics.params {
        if let syn::GenericParam::Type(type_param) = param {
            type_param
                .bounds
                .push(syn::parse_quote!(google_cloud_bigquery::query::FromSql));
        }
    }
    generics
}

fn get_field_name(
    ident: &syn::Ident,
    attrs: &[syn::Attribute],
) -> syn::Result<(String, proc_macro2::Span)> {
    let mut renamed: Option<syn::LitStr> = None;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("bigquery")) {
        attr.parse_nested_meta(|meta| {
            if !meta.path.is_ident("rename") {
                return Err(meta.error("unsupported bigquery attribute"));
            }
            if renamed.is_some() {
                return Err(meta.error("duplicate `rename` option"));
            }
            renamed = Some(meta.value()?.parse()?);
            Ok(())
        })?;
    }
    Ok(match renamed {
        Some(lit) => (lit.value(), lit.span()),
        None => (ident.unraw().to_string(), ident.span()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case("struct Empty {}"; "empty named struct")]
    #[test_case("struct EmptyTuple();"; "empty tuple struct")]
    #[test_case("struct Unit;"; "unit struct")]
    fn test_rejects_empty_structs(def: &str) -> Result<(), syn::Error> {
        let row_err = derive_from_row_impl(syn::parse_str(def)?).to_string();
        assert!(
            row_err.contains("FromRow can only be derived for non-empty structs"),
            "unexpected expansion for {def}: {row_err}"
        );

        let sql_err = derive_from_sql_impl(syn::parse_str(def)?).to_string();
        assert!(
            sql_err.contains("FromSql can only be derived for non-empty structs"),
            "unexpected expansion for {def}: {sql_err}"
        );
        Ok(())
    }

    #[test_case(
        r#"struct Row(#[bigquery(rename = "custom")] i64);"#,
        "bigquery attributes are not supported on tuple struct fields"; "tuple field"
    )]
    #[test_case(
        r#"struct Row { r#type: i64, #[bigquery(rename = "type")] other: i64 }"#,
        "duplicate BigQuery field name `type`"; "duplicate field name"
    )]
    #[test_case(
        r#"struct Row { #[bigquery(rename = "first", rename = "second")] field: i64 }"#,
        "duplicate `rename` option"; "duplicate rename in one attribute"
    )]
    #[test_case(
        r#"struct Row { #[bigquery(rename = "first")] #[bigquery(rename = "second")] field: i64 }"#,
        "duplicate `rename` option"; "duplicate rename across attributes"
    )]
    #[test_case(
        r#"struct Row { #[bigquery(rename = "valid")] #[bigquery(renam = "typo")] field: i64 }"#,
        "unsupported bigquery attribute"; "unknown option after rename"
    )]
    #[test_case(
        "struct Row { #[bigquery(rename = 123)] field: i64 }",
        "expected string literal"; "non-string rename"
    )]
    #[test_case(
        r#"#[bigquery(rename_all = "camelCase")] struct Row { field: i64 }"#,
        "bigquery attributes are not supported on the container"; "container attribute"
    )]
    fn test_rejects_bigquery_attributes(def: &str, message: &str) -> syn::Result<()> {
        for derive in [derive_from_row_impl, derive_from_sql_impl] {
            let tokens = derive(syn::parse_str(def)?).to_string();
            assert!(tokens.contains(message), "unexpected expansion: {tokens}");
        }
        Ok(())
    }

    #[test]
    fn test_rejects_non_structs() -> Result<(), syn::Error> {
        let row_err = derive_from_row_impl(syn::parse_str("enum Foo {}")?).to_string();
        assert!(
            row_err.contains("FromRow can only be derived for non-empty structs"),
            "unexpected expansion: {row_err}"
        );

        let sql_err = derive_from_sql_impl(syn::parse_str("enum Foo {}")?).to_string();
        assert!(
            sql_err.contains("FromSql can only be derived for non-empty structs"),
            "unexpected expansion: {sql_err}"
        );
        Ok(())
    }

    #[test]
    fn test_generics_expansion() -> Result<(), syn::Error> {
        let input1: DeriveInput = syn::parse_str("struct Wrapper<T> { val: T }")?;
        let row_tokens = derive_from_row_impl(input1).to_string();
        assert!(
            row_tokens.contains("impl < T : google_cloud_bigquery :: query :: FromSql > std :: convert :: TryFrom < google_cloud_bigquery :: query :: Row > for Wrapper < T >"),
            "unexpected row expansion: {row_tokens}"
        );

        let input2: DeriveInput = syn::parse_str("struct Wrapper<T> { val: T }")?;
        let sql_tokens = derive_from_sql_impl(input2).to_string();
        assert!(
            sql_tokens.contains("impl < T : google_cloud_bigquery :: query :: FromSql > google_cloud_bigquery :: query :: FromSql for Wrapper < T >"),
            "unexpected sql expansion: {sql_tokens}"
        );
        Ok(())
    }

    #[test]
    fn test_generics_expansion_with_where_clause() -> Result<(), syn::Error> {
        let input1: DeriveInput =
            syn::parse_str("struct Wrapper<T> where T: std::fmt::Debug { val: T }")?;
        let row_tokens = derive_from_row_impl(input1).to_string();
        assert!(
            row_tokens.contains("impl < T : google_cloud_bigquery :: query :: FromSql > std :: convert :: TryFrom < google_cloud_bigquery :: query :: Row > for Wrapper < T > where T : std :: fmt :: Debug"),
            "unexpected row expansion: {row_tokens}"
        );

        let input2: DeriveInput =
            syn::parse_str("struct Wrapper<T> where T: std::fmt::Debug { val: T }")?;
        let sql_tokens = derive_from_sql_impl(input2).to_string();
        assert!(
            sql_tokens.contains("impl < T : google_cloud_bigquery :: query :: FromSql > google_cloud_bigquery :: query :: FromSql for Wrapper < T > where T : std :: fmt :: Debug"),
            "unexpected sql expansion: {sql_tokens}"
        );
        Ok(())
    }

    #[test]
    fn test_generics_expansion_with_default_type_param() -> Result<(), syn::Error> {
        let input1: DeriveInput = syn::parse_str("struct Wrapper<T = i64> { val: T }")?;
        let row_tokens = derive_from_row_impl(input1).to_string();
        assert!(
            row_tokens.contains("impl < T : google_cloud_bigquery :: query :: FromSql > std :: convert :: TryFrom < google_cloud_bigquery :: query :: Row > for Wrapper < T >"),
            "unexpected row expansion: {row_tokens}"
        );

        let input2: DeriveInput = syn::parse_str("struct Wrapper<T = i64> { val: T }")?;
        let sql_tokens = derive_from_sql_impl(input2).to_string();
        assert!(
            sql_tokens.contains("impl < T : google_cloud_bigquery :: query :: FromSql > google_cloud_bigquery :: query :: FromSql for Wrapper < T >"),
            "unexpected sql expansion: {sql_tokens}"
        );
        Ok(())
    }

    #[test]
    fn test_generics_expansion_multiple_params() -> Result<(), syn::Error> {
        let input1: DeriveInput = syn::parse_str("struct Pair<A, B> { a: A, b: B }")?;
        let row_tokens = derive_from_row_impl(input1).to_string();
        assert!(
            row_tokens.contains("impl < A : google_cloud_bigquery :: query :: FromSql , B : google_cloud_bigquery :: query :: FromSql > std :: convert :: TryFrom < google_cloud_bigquery :: query :: Row > for Pair < A , B >"),
            "unexpected row expansion: {row_tokens}"
        );

        let input2: DeriveInput = syn::parse_str("struct Pair<A, B> { a: A, b: B }")?;
        let sql_tokens = derive_from_sql_impl(input2).to_string();
        assert!(
            sql_tokens.contains("impl < A : google_cloud_bigquery :: query :: FromSql , B : google_cloud_bigquery :: query :: FromSql > google_cloud_bigquery :: query :: FromSql for Pair < A , B >"),
            "unexpected sql expansion: {sql_tokens}"
        );
        Ok(())
    }

    #[test]
    fn test_generics_tuple_struct_expansion() -> Result<(), syn::Error> {
        let input1: DeriveInput = syn::parse_str("struct TupleWrapper<T, U>(T, U);")?;
        let row_tokens = derive_from_row_impl(input1).to_string();
        assert!(
            row_tokens.contains("impl < T : google_cloud_bigquery :: query :: FromSql , U : google_cloud_bigquery :: query :: FromSql > std :: convert :: TryFrom < google_cloud_bigquery :: query :: Row > for TupleWrapper < T , U >"),
            "unexpected row expansion: {row_tokens}"
        );

        let input2: DeriveInput = syn::parse_str("struct TupleWrapper<T, U>(T, U);")?;
        let sql_tokens = derive_from_sql_impl(input2).to_string();
        assert!(
            sql_tokens.contains("impl < T : google_cloud_bigquery :: query :: FromSql , U : google_cloud_bigquery :: query :: FromSql > google_cloud_bigquery :: query :: FromSql for TupleWrapper < T , U >"),
            "unexpected sql expansion: {sql_tokens}"
        );
        Ok(())
    }
}
