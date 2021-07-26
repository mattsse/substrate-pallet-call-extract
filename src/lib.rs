//! Extract generated pallet code

use inflector::Inflector;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{parse::ParseStream, Fields};
use synstructure::Structure;

/// Additional parameters to configure the pallet expansion
pub struct PalletCallConfig {
    /// Use this name for the Call enum, by default `Call` will be used
    name: Option<String>,
    /// Use this variant conversion function, by default `CamelCase` will be
    /// used even if the pallet call variants are snake case
    variant_name_conversion: Option<Box<dyn Fn(&str) -> String>>,
    /// How to expand call parameters to variant fields
    call_parameter_style: ParameterStyle,
}

impl PalletCallConfig {
    /// Set the name of the generated `Call` enum explicitly
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Set the conversion function to use when determine the variant names
    pub fn variant_name<F>(mut self, convert: F) -> Self
    where
        F: Fn(&str) -> String + 'static,
    {
        self.variant_name_conversion = Some(Box::new(convert));
        self
    }

    /// Parse the previously extracted `pallet::Call` ast
    pub fn parse(self, content: impl AsRef<str>) -> syn::Result<PalletCall> {
        let input = syn::parse_str::<syn::DeriveInput>(content.as_ref())?;
        Ok(PalletCall {
            config: self,
            input,
        })
    }
}

/// Represents a `pallet::Call`
pub struct PalletCall {
    /// Parameters for how to modify expansion
    config: PalletCallConfig,
    /// The parsed `Call` ast
    pub input: syn::DeriveInput,
}

impl PalletCall {
    pub fn expand(&self) -> syn::Result<TokenStream> {
        let structure = synstructure::Structure::new(&self.input);

        // all unique `Config` trait generics used for call parameters
        let mut generics = vec![()];

        for variant in structure.variants() {
            let ast = variant.ast();
            let variant_name = self
                .config
                .variant_name_conversion
                .as_ref()
                .map(|c| (c)(&ast.ident.to_string()))
                .unwrap_or_else(|| ast.ident.to_string().to_pascal_case());

            // pallet calls are unnamed
            if let Fields::Unnamed(fields) = ast.fields {
                for f in fields.unnamed {
                    // replace the `T` type paths with unique generics but keep everything else
                    let mut field = f.clone();
                }
            }
        }

        // remove the `__ignore` variant and adjust the codec index

        Ok(quote! {})
    }
}

/// How to expand the call parameters as enum variant fields
pub enum ParameterStyle {
    /// Use default `(ty,ty)` unnamed fields
    Unnamed,
    /// Expand call parameters as named fields
    // TODO add convert type for determine the name, allow extracting it from the ast of the actual
    // function fn(call_name, index)
    Named(Option<Box<dyn Fn(&str) -> String>>),
}

impl Default for ParameterStyle {
    fn default() -> Self {
        ParameterStyle::Unnamed
    }
}
