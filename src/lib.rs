//! Extract generated pallet code

use inflector::Inflector;
use proc_macro2::{Span, TokenStream};
use quote::quote;
use std::collections::BTreeMap;
use syn::spanned::Spanned;
use syn::{
    parse::ParseStream, punctuated::Punctuated, Attribute, Fields, FieldsUnnamed, Ident, Item,
    Path, PathSegment, Type, TypePath, Variant,
};
use synstructure::{MacroResult, Structure};

/// Additional parameters to configure the pallet expansion
#[derive(Default)]
pub struct PalletCallConfig {
    /// Use this name for the Call enum, by default `Call` will be used
    name: Option<String>,
    /// Use this variant conversion function, by default `CamelCase` will be
    /// used even if the pallet call variants are snake case
    variant_name_conversion: Option<Box<dyn Fn(&str) -> String>>,
    /// Use this generic conversion function to modify The generic name
    /// by default the last type path segment is used: `T::Balance` -> `Balance`
    generic_name_conversion: Option<Box<dyn Fn(&TypePath) -> String>>,
    /// How to expand call parameters to variant fields
    call_parameter_style: ParameterStyle,
    /// Whether to keep original comments
    keep_comments: bool,
    /// The name fo the scale codec crate by default it's `codec`
    codec_crate: Option<String>,
    /// Whether to derive runtime debug
    runtime_debug: Option<String>,
    /// Additional attributes
    additional_attr: Vec<Attribute>,
    /// Additional derives
    additional_derives: Vec<Path>,
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

    /// Set the conversion function to use when determine the variant names
    pub fn generic_name<F>(mut self, convert: F) -> Self
    where
        F: Fn(&TypePath) -> String + 'static,
    {
        self.generic_name_conversion = Some(Box::new(convert));
        self
    }

    /// Keep original comments, otherwise they're stripped
    pub fn keep_comments<F>(mut self) -> Self {
        self.keep_comments = true;
        self
    }

    /// The Name of the `codec` crate
    pub fn codec_crate(mut self, codec: impl Into<String>) -> Self {
        self.codec_crate = Some(codec.into());
        self
    }

    /// The path to the module where the `RuntimeDebug` trait can be found
    /// Such as `frame_support`
    pub fn use_runtime_debug_from_crate(mut self, debug: impl Into<String>) -> Self {
        self.runtime_debug = Some(debug.into());
        self
    }

    /// Derive `frame_support::RuntimeDebug`
    pub fn frame_support_runtime_debug(mut self) -> Self {
        self.use_runtime_debug_from_crate("frame_support")
    }

    /// Push an additional derive such as "Debug" to add to the `Call`
    pub fn push_derive_str(&mut self, attr: impl AsRef<str>) -> syn::Result<&mut Self> {
        let derive = syn::parse_str(attr.as_ref())?;
        self.additional_derives.push(derive);
        Ok(self)
    }

    /// Push an additional derive such as "Debug" to add to the `Call`
    pub fn push_derive(mut self, derive: Path) -> Self {
        self.additional_derives.push(derive);
        self
    }

    /// Push an additional attribute to add to the `Call`
    pub fn push_attr(mut self, attr: Attribute) -> Self {
        self.additional_attr.push(attr);
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
    /// Expands the pallet call as configured in the `PalletCallConfig`
    ///
    /// The returned `TokenStream` will be a call enum in which any unique type bound to the `T:Config` trait of `pallet::Call` (like `T::Balance`) will be replaced by a generic type
    ///
    /// Example
    ///
    /// Let this be an excerpt of the generated `pallet::Call`
    /// ```ignore
    /// pub enum Call<T: Config> {
    ///     set_balance(
    ///         <T::Lookup as StaticLookup>::Source,
    ///         #[codec(compact)] T::Balance,
    ///         #[codec(compact)] T::Balance,
    ///     ),
    /// }
    /// ```
    ///
    /// calling `expand` will return the corresponding generic enum:
    ///
    /// ```ignore
    /// pub enum Call<Source, Balance> {
    ///     SetBalance(Source, #[codec(compact)] Balance, #[codec(compact)] Balance),
    /// }
    /// ```
    pub fn expand(&self) -> syn::Result<TokenStream> {
        let structure = synstructure::Structure::new(&self.input);

        // the name of the final call enum
        let name = self.config.name.as_deref().unwrap_or("Call");
        let name = syn::parse_str::<Ident>(name)?;

        // the name of the final call enum
        let codec_crate = self.config.name.as_deref().unwrap_or("codec");
        let codec_crate = syn::parse_str::<Ident>(codec_crate)?;

        let runtime_dbg = self
            .config
            .runtime_debug
            .as_ref()
            .map(|s| syn::parse_str::<Path>(&format!("{}::RuntimeDebug", s)))
            .transpose()?
            .map(|p| quote! { #[derive(#p)]})
            .unwrap_or_else(|| quote! {});

        // all unique `Config` trait generics used for call parameters
        let mut generics = BTreeMap::new();
        let mut variants = Vec::with_capacity(structure.variants().len());

        for variant in structure.variants().into_iter().skip_while(|v| {
            let ast = v.ast();
            // skip the `__ignore` variant, which is also marked `[codec(skip)]`
            ast.ident.to_string().to_lowercase() == "__ignore"
        }) {
            let ast = variant.ast();

            let variant_name = self
                .config
                .variant_name_conversion
                .as_ref()
                .map(|c| (c)(&ast.ident.to_string()))
                .unwrap_or_else(|| ast.ident.to_string().to_pascal_case());
            let variant_name = syn::parse_str::<Ident>(&variant_name)?;

            let mut fields = Vec::with_capacity(variant.bindings().len());

            for binding in variant.bindings() {
                let mut field = binding.ast().clone();
                if let Type::Path(ref mut path) = field.ty {
                    if !binding.referenced_ty_params().is_empty() {
                        // generic type
                        let ty_str = quote!(#path).to_string();
                        let generic_ty = generics.entry(ty_str).or_insert_with(|| {
                            self.config
                                .generic_name_conversion
                                .as_ref()
                                .map(|c| (c)(path))
                                .unwrap_or_else(|| {
                                    let ty = path.path.segments.last().unwrap();
                                    quote!(#ty).to_string()
                                })
                        });
                        // create a new field with the generic as type
                        let ident = syn::parse_str::<Ident>(&generic_ty)?;
                        let mut segments = Punctuated::new();
                        segments.push(PathSegment::from(ident));
                        *path = TypePath {
                            qself: None,
                            path: Path {
                                leading_colon: None,
                                segments,
                            },
                        };
                    }
                } else {
                    return Err(syn::Error::new(
                        field.span(),
                        "Only TypePaths are supported currently",
                    ));
                }
                fields.push(field);
            }

            // parse as fields unnamed
            // TODO support named fields as well
            let fields = Fields::Unnamed(syn::parse_str::<FieldsUnnamed>(
                &quote! {( #(#fields ),* )}.to_string(),
            )?);

            let mut attrs = ast.attrs.to_vec();
            if !self.config.keep_comments {
                remove_doc_attributes(&mut attrs);
            }

            variants.push(Variant {
                attrs,
                ident: variant_name,
                fields,
                discriminant: ast.discriminant.clone(),
            });
        }

        let mut call_enum_attrs = structure.ast().attrs.clone();
        if !self.config.keep_comments {
            remove_doc_attributes(&mut call_enum_attrs);
        }

        let generics = if generics.is_empty() {
            quote! {}
        } else {
            let generics = generics
                .values()
                .map(|gen| syn::parse_str::<Ident>(&gen))
                .collect::<Result<Vec<_>, _>>()?;
            quote! {< #( #generics), * > }
        };

        let additional_attr = &self.config.additional_attr;
        let additional_derives = &self.config.additional_derives;
        let call_enum = quote! {
            #[derive(
                Clone, PartialEq, Eq,
                #codec_crate::Encode,
                #codec_crate::Decode,
                #( #additional_derives ), *
            )]
            #runtime_dbg
            #(#additional_attr)*
            pub enum #name # generics {
                #( #variants ),*
            }
        };

        Ok(call_enum)
    }
}

fn remove_doc_attributes(attrs: &mut Vec<Attribute>) {
    attrs.retain(|attr| !attr.path.is_ident("doc"));
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

fn x() {
    let call = PalletCallConfig::default().variant_name(|s| s.to_string());
}
