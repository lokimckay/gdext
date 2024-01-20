/*
 * Copyright (c) godot-rust; Bromeon and contributors.
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use proc_macro2::{Ident, Literal, TokenStream};
use quote::{format_ident, quote, ToTokens, TokenStreamExt};
use std::path::Path;

use crate::domain_models::{
    BuiltinMethod, BuiltinVariant, Class, ClassLike, ClassMethod, Constructor, Enumerator,
    ExtensionApi, FnDirection, Function, GodotApiVersion, Operator,
};
use crate::util::{make_table_accessor_name, ClassCodegenLevel, MethodTableKey};
use crate::{conv, ident, special_cases, util, Context, SubmitFn, TyName};

struct CentralItems {
    opaque_types: [Vec<TokenStream>; 2],
    variant_ty_enumerators_pascal: Vec<Ident>,
    variant_ty_enumerators_rust: Vec<TokenStream>,
    variant_ty_enumerators_ord: Vec<Literal>,
    variant_op_enumerators_pascal: Vec<Ident>,
    variant_op_enumerators_ord: Vec<Literal>,
    global_enum_defs: Vec<TokenStream>,
    godot_version: GodotApiVersion,
}

struct NamedMethodTable {
    table_name: Ident,
    imports: TokenStream,
    ctor_parameters: TokenStream,
    pre_init_code: TokenStream,
    method_decls: Vec<TokenStream>,
    method_inits: Vec<TokenStream>,
    class_count: usize,
    method_count: usize,
}

#[allow(dead_code)] // for lazy feature
struct IndexedMethodTable {
    table_name: Ident,
    imports: TokenStream,
    ctor_parameters: TokenStream,
    pre_init_code: TokenStream,
    fptr_type: TokenStream,
    fetch_fptr_type: TokenStream,
    method_init_groups: Vec<MethodInitGroup>,
    lazy_key_type: TokenStream,
    lazy_method_init: TokenStream,
    named_accessors: Vec<AccessorMethod>,
    class_count: usize,
    method_count: usize,
}

// ----------------------------------------------------------------------------------------------------------------------------------------------

#[cfg_attr(feature = "codegen-lazy-fptrs", allow(dead_code))]
struct MethodInit {
    method_init: TokenStream,
    index: usize,
}

impl ToTokens for MethodInit {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        self.method_init.to_tokens(tokens);
    }
}

// ----------------------------------------------------------------------------------------------------------------------------------------------

#[cfg_attr(feature = "codegen-lazy-fptrs", allow(dead_code))]
struct MethodInitGroup {
    class_name: Ident,
    class_var_init: Option<TokenStream>,
    method_inits: Vec<MethodInit>,
}

impl MethodInitGroup {
    fn new(
        godot_class_name: &str,
        class_var: Option<Ident>,
        method_inits: Vec<MethodInit>,
    ) -> Self {
        Self {
            class_name: ident(godot_class_name),
            // Only create class variable if any methods have been added.
            class_var_init: if class_var.is_none() || method_inits.is_empty() {
                None
            } else {
                let initializer_expr = util::make_sname_ptr(godot_class_name);
                Some(quote! {
                    let #class_var = #initializer_expr;
                })
            },
            method_inits,
        }
    }

    #[cfg(not(feature = "codegen-lazy-fptrs"))]
    fn function_name(&self) -> Ident {
        format_ident!("load_{}_methods", self.class_name)
    }
}

// ----------------------------------------------------------------------------------------------------------------------------------------------

struct AccessorMethod {
    name: Ident,
    index: usize,
    lazy_key: TokenStream,
}

// ----------------------------------------------------------------------------------------------------------------------------------------------

pub(crate) fn generate_sys_central_file(
    api: &ExtensionApi,
    ctx: &mut Context,
    sys_gen_path: &Path,
    submit_fn: &mut SubmitFn,
) {
    let central_items = make_central_items(api, ctx);
    let sys_code = make_sys_code(central_items);

    submit_fn(sys_gen_path.join("central.rs"), sys_code);
}

pub(crate) fn generate_sys_classes_file(
    api: &ExtensionApi,
    sys_gen_path: &Path,
    watch: &mut godot_bindings::StopWatch,
    ctx: &mut Context,
    submit_fn: &mut SubmitFn,
) {
    for api_level in ClassCodegenLevel::with_tables() {
        let code = make_class_method_table(api, api_level, ctx);
        let filename = api_level.table_file();

        submit_fn(sys_gen_path.join(filename), code);
        watch.record(format!("generate_classes_{}_file", api_level.lower()));
    }
}

pub(crate) fn generate_sys_utilities_file(
    api: &ExtensionApi,
    sys_gen_path: &Path,
    submit_fn: &mut SubmitFn,
) {
    let mut table = NamedMethodTable {
        table_name: ident("UtilityFunctionTable"),
        imports: quote! {},
        ctor_parameters: quote! {
            interface: &crate::GDExtensionInterface,
            string_names: &mut crate::StringCache,
        },
        pre_init_code: quote! {
            let get_utility_fn = interface.variant_get_ptr_utility_function
                .expect("variant_get_ptr_utility_function absent");
        },
        method_decls: vec![],
        method_inits: vec![],
        class_count: 0,
        method_count: 0,
    };

    for function in api.utility_functions.iter() {
        let fn_name_str = function.name();
        let field = util::make_utility_function_ptr_name(fn_name_str);
        let hash = function.hash();

        table.method_decls.push(quote! {
            pub #field: crate::UtilityFunctionBind,
        });

        table.method_inits.push(quote! {
            #field: crate::load_utility_function(get_utility_fn, string_names, #fn_name_str, #hash),
        });

        table.method_count += 1;
    }

    let code = make_named_method_table(table);

    submit_fn(sys_gen_path.join("table_utilities.rs"), code);
}

/// Generate code for a method table based on shared layout.
fn make_named_method_table(info: NamedMethodTable) -> TokenStream {
    let NamedMethodTable {
        table_name,
        imports,
        ctor_parameters,
        pre_init_code,
        method_decls,
        method_inits,
        class_count,
        method_count,
    } = info;

    // Assumes that both decls and inits already have a trailing comma.
    // This is necessary because some generators emit multiple lines (statements) per element.
    quote! {
        #imports

        #[allow(non_snake_case)]
        pub struct #table_name {
            #( #method_decls )*
        }

        impl #table_name {
            pub const CLASS_COUNT: usize = #class_count;
            pub const METHOD_COUNT: usize = #method_count;

            pub fn load(
                #ctor_parameters
            ) -> Self {
                #pre_init_code

                Self {
                    #( #method_inits )*
                }
            }
        }
    }
}

#[cfg(not(feature = "codegen-lazy-fptrs"))]
fn make_method_table(info: IndexedMethodTable) -> TokenStream {
    let IndexedMethodTable {
        table_name,
        imports,
        ctor_parameters,
        pre_init_code,
        fptr_type,
        fetch_fptr_type,
        method_init_groups,
        lazy_key_type: _,
        lazy_method_init: _,
        named_accessors,
        class_count,
        method_count,
    } = info;

    // Editor table can be empty, if the Godot binary is compiled without editor.
    let unused_attr = (method_count == 0).then(|| quote! { #[allow(unused_variables)] });
    let named_method_api = make_named_accessors(&named_accessors, &fptr_type);

    // Make sure methods are complete and in order of index.
    assert_eq!(
        method_init_groups
            .iter()
            .map(|group| group.method_inits.len())
            .sum::<usize>(),
        method_count,
        "number of methods does not match count"
    );

    if let Some(last) = method_init_groups.last() {
        assert_eq!(
            last.method_inits.last().unwrap().index,
            method_count - 1,
            "last method should have highest index (table {})",
            table_name
        );
    } else {
        assert_eq!(method_count, 0, "empty method table should have count 0");
    }

    let method_load_inits = method_init_groups.iter().map(|group| {
        let func = group.function_name();
        quote! {
            #func(&mut function_pointers, string_names, fetch_fptr);
        }
    });

    let method_load_decls = method_init_groups.iter().map(|group| {
        let func = group.function_name();
        let method_inits = &group.method_inits;
        let class_var_init = &group.class_var_init;

        quote! {
            fn #func(
                function_pointers: &mut Vec<#fptr_type>,
                string_names: &mut crate::StringCache,
                fetch_fptr: FetchFn,
            ) {
                #class_var_init

                #(
                    function_pointers.push(#method_inits);
                )*
            }
        }
    });

    // Assumes that inits already have a trailing comma.
    // This is necessary because some generators emit multiple lines (statements) per element.
    quote! {
        #imports

        type FetchFn = <#fetch_fptr_type as crate::Inner>::FnPtr;

        pub struct #table_name {
            function_pointers: Vec<#fptr_type>,
        }

        impl #table_name {
            pub const CLASS_COUNT: usize = #class_count;
            pub const METHOD_COUNT: usize = #method_count;

            #unused_attr
            pub fn load(
                #ctor_parameters
            ) -> Self {
                #pre_init_code

                let mut function_pointers = Vec::with_capacity(#method_count);
                #( #method_load_inits )*

                Self { function_pointers }
            }

            #[inline(always)]
            pub fn fptr_by_index(&self, index: usize) -> #fptr_type {
                // SAFETY: indices are statically generated and guaranteed to be in range.
                unsafe {
                    *self.function_pointers.get_unchecked(index)
                }
            }

            #named_method_api
        }

        #( #method_load_decls )*
    }
}

#[cfg(feature = "codegen-lazy-fptrs")]
fn make_method_table(info: IndexedMethodTable) -> TokenStream {
    let IndexedMethodTable {
        table_name,
        imports,
        ctor_parameters: _,
        pre_init_code: _,
        fptr_type,
        fetch_fptr_type: _,
        method_init_groups: _,
        lazy_key_type,
        lazy_method_init,
        named_accessors,
        class_count,
        method_count,
    } = info;

    // Editor table can be empty, if the Godot binary is compiled without editor.
    let unused_attr = (method_count == 0).then(|| quote! { #[allow(unused_variables)] });
    let named_method_api = make_named_accessors(&named_accessors, &fptr_type);

    // Assumes that inits already have a trailing comma.
    // This is necessary because some generators emit multiple lines (statements) per element.
    quote! {
        #imports
        use crate::StringCache;
        use std::collections::HashMap;
        use std::cell::RefCell;

        // Exists to be stored inside RefCell.
        struct InnerTable {
            // 'static because at this point, the interface and lifecycle tables are globally available.
            string_cache: StringCache<'static>,
            function_pointers: HashMap<#lazy_key_type, #fptr_type>,
        }

        // Note: get_method_bind and other function pointers could potentially be stored as fields in table, to avoid interface_fn!.
        pub struct #table_name {
            inner: RefCell<InnerTable>,
        }

        impl #table_name {
            pub const CLASS_COUNT: usize = #class_count;
            pub const METHOD_COUNT: usize = #method_count;

            #unused_attr
            pub fn load() -> Self {
                // SAFETY: interface and lifecycle tables are initialized at this point, so we can get 'static references to them.
                let (interface, lifecycle_table) = unsafe {
                    (crate::get_interface(), crate::builtin_lifecycle_api())
                };

                Self {
                    inner: RefCell::new(InnerTable {
                        string_cache: StringCache::new(interface, lifecycle_table),
                        function_pointers: HashMap::new(),
                    }),
                }
            }

            #[inline(always)]
            pub fn fptr_by_key(&self, key: #lazy_key_type) -> #fptr_type {
                let mut guard = self.inner.borrow_mut();
                let inner = &mut *guard;
                *inner.function_pointers.entry(key.clone()).or_insert_with(|| {
                    #lazy_method_init
                })
            }

            #named_method_api
        }
    }
}

pub(crate) fn generate_sys_builtin_methods_file(
    api: &ExtensionApi,
    sys_gen_path: &Path,
    ctx: &mut Context,
    submit_fn: &mut SubmitFn,
) {
    let code = make_builtin_method_table(api, ctx);
    submit_fn(sys_gen_path.join("table_builtins.rs"), code);
}

pub(crate) fn generate_sys_builtin_lifecycle_file(
    api: &ExtensionApi,
    sys_gen_path: &Path,
    submit_fn: &mut SubmitFn,
) {
    let code = make_builtin_lifecycle_table(api);
    submit_fn(sys_gen_path.join("table_builtins_lifecycle.rs"), code);
}

pub(crate) fn generate_core_mod_file(gen_path: &Path, submit_fn: &mut SubmitFn) {
    // When invoked by another crate during unit-test (not integration test), don't run generator.
    let code = quote! {
        pub mod central;
        pub mod classes;
        pub mod builtin_classes;
        pub mod utilities;
        pub mod native;
    };

    submit_fn(gen_path.join("mod.rs"), code);
}

pub(crate) fn generate_core_central_file(
    api: &ExtensionApi,
    ctx: &mut Context,
    gen_path: &Path,
    submit_fn: &mut SubmitFn,
) {
    let central_items = make_central_items(api, ctx);
    let core_code = make_core_code(&central_items);

    submit_fn(gen_path.join("central.rs"), core_code);
}

fn make_sys_code(central_items: CentralItems) -> TokenStream {
    let CentralItems {
        opaque_types,
        variant_ty_enumerators_pascal,
        variant_ty_enumerators_ord,
        variant_op_enumerators_pascal,
        variant_op_enumerators_ord,
        godot_version,
        ..
    } = central_items;

    let build_config_struct = make_build_config(&godot_version);
    let [opaque_32bit, opaque_64bit] = opaque_types;

    quote! {
        use crate::{GDExtensionVariantOperator, GDExtensionVariantType};

        #[cfg(target_pointer_width = "32")]
        pub mod types {
            #(#opaque_32bit)*
        }
        #[cfg(target_pointer_width = "64")]
        pub mod types {
            #(#opaque_64bit)*
        }


        // ----------------------------------------------------------------------------------------------------------------------------------------------

        #build_config_struct

        // ----------------------------------------------------------------------------------------------------------------------------------------------

        #[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
        #[repr(i32)]
        pub enum VariantType {
            Nil = 0,
            #(
                #variant_ty_enumerators_pascal = #variant_ty_enumerators_ord,
            )*
        }

        impl VariantType {
            #[doc(hidden)]
            pub fn from_sys(enumerator: GDExtensionVariantType) -> Self {
                // Annoying, but only stable alternative is transmute(), which dictates enum size.
                match enumerator {
                    0 => Self::Nil,
                    #(
                        #variant_ty_enumerators_ord => Self::#variant_ty_enumerators_pascal,
                    )*
                    _ => unreachable!("invalid variant type {}", enumerator)
                }
            }

            #[doc(hidden)]
            pub fn sys(self) -> GDExtensionVariantType {
                self as _
            }
        }

        // ----------------------------------------------------------------------------------------------------------------------------------------------

        #[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
        #[repr(i32)]
        pub enum VariantOperator {
            #(
                #variant_op_enumerators_pascal = #variant_op_enumerators_ord,
            )*
        }

        impl VariantOperator {
            #[doc(hidden)]
            pub fn from_sys(enumerator: GDExtensionVariantOperator) -> Self {
                match enumerator {
                    #(
                        #variant_op_enumerators_ord => Self::#variant_op_enumerators_pascal,
                    )*
                    _ => unreachable!("invalid variant operator {}", enumerator)
                }
            }

            #[doc(hidden)]
            pub fn sys(self) -> GDExtensionVariantOperator {
                self as _
            }
        }
    }
}

fn make_build_config(header: &GodotApiVersion) -> TokenStream {
    let GodotApiVersion {
        major,
        minor,
        patch,
        version_string,
    } = header;

    // Should this be mod?
    quote! {
        /// Provides meta-information about the library and the Godot version in use.
        pub struct GdextBuild;

        impl GdextBuild {
            /// Godot version against which gdext was compiled.
            ///
            /// Example format: `v4.0.stable.official`
            pub const fn godot_static_version_string() -> &'static str {
                #version_string
            }

            /// Godot version against which gdext was compiled, as `(major, minor, patch)` triple.
            pub const fn godot_static_version_triple() -> (u8, u8, u8) {
                (#major, #minor, #patch)
            }

            /// Version of the Godot engine which loaded gdext via GDExtension binding.
            pub fn godot_runtime_version_string() -> String {
                unsafe {
                    let char_ptr = crate::runtime_metadata().godot_version.string;
                    let c_str = std::ffi::CStr::from_ptr(char_ptr);
                    String::from_utf8_lossy(c_str.to_bytes()).to_string()
                }
            }

            /// Version of the Godot engine which loaded gdext via GDExtension binding, as
            /// `(major, minor, patch)` triple.
            pub fn godot_runtime_version_triple() -> (u8, u8, u8) {
                let version = unsafe {
                    crate::runtime_metadata().godot_version
                };
                (version.major as u8, version.minor as u8, version.patch as u8)
            }

            /// For a string "4.x", returns `true` if the current Godot version is strictly less than 4.x.
            ///
            /// Runtime equivalent of `#[cfg(before_api = "4.x")]`.
            ///
            /// # Panics
            /// On bad input.
            pub fn before_api(major_minor: &str) -> bool {
                let mut parts = major_minor.split('.');
                let queried_major = parts.next().unwrap().parse::<u8>().expect("invalid major version");
                let queried_minor = parts.next().unwrap().parse::<u8>().expect("invalid minor version");
                assert_eq!(queried_major, 4, "major version must be 4");

                let (_, minor, _) = Self::godot_runtime_version_triple();
                minor < queried_minor
            }

            /// For a string "4.x", returns `true` if the current Godot version is equal or greater to 4.x.
            ///
            /// Runtime equivalent of `#[cfg(since_api = "4.x")]`.
            ///
            /// # Panics
            /// On bad input.
            pub fn since_api(major_minor: &str) -> bool {
                !Self::before_api(major_minor)
            }
        }
    }
}

fn make_core_code(central_items: &CentralItems) -> TokenStream {
    let CentralItems {
        variant_ty_enumerators_pascal,
        variant_ty_enumerators_rust,
        global_enum_defs,
        ..
    } = central_items;

    // TODO impl Clone, Debug, PartialEq, PartialOrd, Hash for VariantDispatch
    // TODO could use try_to().unwrap_unchecked(), since type is already verified. Also directly overload from_variant().
    // But this requires that all the variant types support this.
    quote! {
        use crate::builtin::*;
        use crate::engine::Object;
        use crate::obj::Gd;

        #[allow(dead_code)]
        pub enum VariantDispatch {
            Nil,
            #(
                #variant_ty_enumerators_pascal(#variant_ty_enumerators_rust),
            )*
        }

        #[cfg(FALSE)]
        impl FromVariant for VariantDispatch {
            fn try_from_variant(variant: &Variant) -> Result<Self, VariantConversionError> {
                let dispatch = match variant.get_type() {
                    VariantType::Nil => Self::Nil,
                    #(
                        VariantType::#variant_ty_enumerators_pascal
                            => Self::#variant_ty_enumerators_pascal(variant.to::<#variant_ty_enumerators_rust>()),
                    )*
                };

                Ok(dispatch)
            }
        }

        /// Global enums and constants.
        ///
        /// A list of global-scope enumerated constants.
        /// For global built-in functions, check out the [`utilities` module][crate::engine::utilities].
        ///
        /// See also [Godot docs for `@GlobalScope`](https://docs.godotengine.org/en/stable/classes/class_@globalscope.html#enumerations).
        pub mod global {
            use crate::sys;
            #( #global_enum_defs )*
        }
    }
}

fn make_central_items(api: &ExtensionApi, ctx: &mut Context) -> CentralItems {
    let mut opaque_types = [Vec::new(), Vec::new()];

    for b in api.builtin_sizes.iter() {
        let index = b.config.is_64bit() as usize;

        opaque_types[index].push(make_opaque_type(&b.builtin_original_name, b.size));
    }

    let variant_operators = collect_variant_operators(api);

    // Generate builtin methods, now with info for all types available.
    // Separate vectors because that makes usage in quote! easier.
    let len = api.builtins.len();

    let mut result = CentralItems {
        opaque_types,
        variant_ty_enumerators_pascal: Vec::with_capacity(len),
        variant_ty_enumerators_rust: Vec::with_capacity(len),
        variant_ty_enumerators_ord: Vec::with_capacity(len),
        variant_op_enumerators_pascal: Vec::new(),
        variant_op_enumerators_ord: Vec::new(),
        global_enum_defs: Vec::new(),
        godot_version: api.godot_version.clone(),
    };

    // Note: NIL is not part of this iteration, it will be added manually.
    for builtin in api.builtins.iter() {
        let original_name = builtin.godot_original_name();
        let rust_ty = conv::to_rust_type(original_name, None, ctx);
        let pascal_case = conv::to_pascal_case(original_name);
        let ord = builtin.unsuffixed_ord_lit();

        result
            .variant_ty_enumerators_pascal
            .push(ident(&pascal_case));
        result
            .variant_ty_enumerators_rust
            .push(rust_ty.to_token_stream());
        result.variant_ty_enumerators_ord.push(ord);
    }

    for op in variant_operators {
        let pascal_name = conv::to_pascal_case(&op.name.to_string());

        let enumerator_name = if pascal_name == "Module" {
            ident("Modulo")
        } else {
            ident(&pascal_name)
        };

        result.variant_op_enumerators_pascal.push(enumerator_name);
        result
            .variant_op_enumerators_ord
            .push(op.value.unsuffixed_lit());
    }

    for enum_ in api.global_enums.iter() {
        // Skip those enums which are already manually handled.
        if enum_.name == "VariantType" || enum_.name == "VariantOperator" {
            continue;
        }

        let def = util::make_enum_definition(enum_);
        result.global_enum_defs.push(def);
    }

    result
}

fn make_builtin_lifecycle_table(api: &ExtensionApi) -> TokenStream {
    let builtins = &api.builtins;
    let len = builtins.len();

    let mut table = NamedMethodTable {
        table_name: ident("BuiltinLifecycleTable"),
        imports: quote! {
            use crate::{
                GDExtensionConstTypePtr, GDExtensionTypePtr, GDExtensionUninitializedTypePtr,
                GDExtensionUninitializedVariantPtr, GDExtensionVariantPtr,
            };
        },
        ctor_parameters: quote! {
            interface: &crate::GDExtensionInterface,
        },
        pre_init_code: quote! {
            let get_construct_fn = interface.variant_get_ptr_constructor.unwrap();
            let get_destroy_fn = interface.variant_get_ptr_destructor.unwrap();
            let get_operator_fn = interface.variant_get_ptr_operator_evaluator.unwrap();

            let get_to_variant_fn = interface.get_variant_from_type_constructor.unwrap();
            let get_from_variant_fn = interface.get_variant_to_type_constructor.unwrap();
        },
        method_decls: Vec::with_capacity(len),
        method_inits: Vec::with_capacity(len),
        class_count: len,
        method_count: 0,
    };

    // Note: NIL is not part of this iteration, it will be added manually.
    for variant in builtins.iter() {
        let (decls, inits) = make_variant_fns(api, variant);

        table.method_decls.push(decls);
        table.method_inits.push(inits);
    }

    make_named_method_table(table)
}

fn make_class_method_table(
    api: &ExtensionApi,
    api_level: ClassCodegenLevel,
    ctx: &mut Context,
) -> TokenStream {
    let mut table = IndexedMethodTable {
        table_name: api_level.table_struct(),
        imports: TokenStream::new(),
        ctor_parameters: quote! {
            interface: &crate::GDExtensionInterface,
            string_names: &mut crate::StringCache,
        },
        pre_init_code: TokenStream::new(), // late-init, depends on class string names
        fptr_type: quote! { crate::ClassMethodBind },
        fetch_fptr_type: quote! { crate::GDExtensionInterfaceClassdbGetMethodBind },
        method_init_groups: vec![],
        lazy_key_type: quote! { crate::lazy_keys::ClassMethodKey },
        lazy_method_init: quote! {
            let get_method_bind = crate::interface_fn!(classdb_get_method_bind);
            crate::load_class_method(
                get_method_bind,
                &mut inner.string_cache,
                None,
                key.class_name,
                key.method_name,
                key.hash
            )
        },
        named_accessors: vec![],
        class_count: 0,
        method_count: 0,
    };

    api.classes
        .iter()
        .filter(|c| c.api_level == api_level)
        .for_each(|c| populate_class_methods(&mut table, c, ctx));

    table.pre_init_code = quote! {
        let fetch_fptr = interface.classdb_get_method_bind.expect("classdb_get_method_bind absent");
    };

    make_method_table(table)
}

/// For index-based method tables, have select methods exposed by name for internal use.
fn make_named_accessors(accessors: &[AccessorMethod], fptr: &TokenStream) -> TokenStream {
    let mut result_api = TokenStream::new();

    for accessor in accessors {
        let AccessorMethod {
            name,
            index,
            lazy_key,
        } = accessor;

        let code = if cfg!(feature = "codegen-lazy-fptrs") {
            quote! {
                #[inline(always)]
                pub fn #name(&self) -> #fptr {
                    self.fptr_by_key(#lazy_key)
                }
            }
        } else {
            quote! {
                #[inline(always)]
                pub fn #name(&self) -> #fptr {
                    self.fptr_by_index(#index)
                }
            }
        };

        result_api.append_all(code.into_iter());
    }

    result_api
}

fn make_builtin_method_table(api: &ExtensionApi, ctx: &mut Context) -> TokenStream {
    let mut table = IndexedMethodTable {
        table_name: ident("BuiltinMethodTable"),
        imports: TokenStream::new(),
        ctor_parameters: quote! {
            interface: &crate::GDExtensionInterface,
            string_names: &mut crate::StringCache,
        },
        pre_init_code: quote! {
            let fetch_fptr = interface.variant_get_ptr_builtin_method.expect("variant_get_ptr_builtin_method absent");
        },
        fptr_type: quote! { crate::BuiltinMethodBind },
        fetch_fptr_type: quote! { crate::GDExtensionInterfaceVariantGetPtrBuiltinMethod },
        method_init_groups: vec![],
        lazy_key_type: quote! { crate::lazy_keys::BuiltinMethodKey },
        lazy_method_init: quote! {
            let fetch_fptr = crate::interface_fn!(variant_get_ptr_builtin_method);
            crate::load_builtin_method(
                fetch_fptr,
                &mut inner.string_cache,
                key.variant_type.sys(),
                key.variant_type_str,
                key.method_name,
                key.hash
            )
        },
        named_accessors: vec![],
        class_count: 0,
        method_count: 0,
    };

    for builtin in api.builtins.iter() {
        populate_builtin_methods(&mut table, builtin, ctx);
    }

    make_method_table(table)
}

fn populate_class_methods(table: &mut IndexedMethodTable, class: &Class, ctx: &mut Context) {
    // Note: already checked outside whether class is active in codegen.

    let class_ty = class.name();
    let class_var = format_ident!("sname_{}", class_ty.godot_ty);
    let mut method_inits = vec![];

    for method in class.methods.iter() {
        // Virtual methods are not part of the class API itself, but exposed as an accompanying trait.
        let FnDirection::Outbound { hash } = method.direction() else {
            continue;
        };

        // Note: varcall/ptrcall is only decided at call time; the method bind is the same for both.
        let index = ctx.get_table_index(&MethodTableKey::from_class(class, method));

        let method_init = make_class_method_init(method, hash, &class_var, class_ty);
        method_inits.push(MethodInit { method_init, index });
        table.method_count += 1;

        // If requested, add a named accessor for this method.
        if special_cases::is_named_accessor_in_table(class_ty, method.godot_name()) {
            let class_name_str = class_ty.godot_ty.as_str();
            let method_name_str = method.name();

            table.named_accessors.push(AccessorMethod {
                name: make_table_accessor_name(class_ty, method),
                index,
                lazy_key: quote! {
                    crate::lazy_keys::ClassMethodKey {
                        class_name: #class_name_str,
                        method_name: #method_name_str,
                        hash: #hash,
                    }
                },
            });
        }
    }

    // No methods available, or all excluded (e.g. virtual ones) -> no group needed.
    if !method_inits.is_empty() {
        table.method_init_groups.push(MethodInitGroup::new(
            &class_ty.godot_ty,
            Some(class_var),
            method_inits,
        ));

        table.class_count += 1;
    }
}

fn populate_builtin_methods(
    table: &mut IndexedMethodTable,
    builtin: &BuiltinVariant,
    ctx: &mut Context,
) {
    let Some(builtin_class) = builtin.associated_builtin_class() else {
        // Ignore those where no class is generated (Object, int, bool etc.).
        return;
    };

    let builtin_ty = builtin_class.name();

    let mut method_inits = vec![];
    for method in builtin_class.methods.iter() {
        let index = ctx.get_table_index(&MethodTableKey::from_builtin(builtin_class, method));

        let method_init = make_builtin_method_init(builtin, method, index);
        method_inits.push(MethodInit { method_init, index });
        table.method_count += 1;

        // If requested, add a named accessor for this method.
        if special_cases::is_named_accessor_in_table(builtin_ty, method.godot_name()) {
            let variant_type = builtin.sys_variant_type();
            let variant_type_str = builtin.godot_original_name();
            let method_name_str = method.name();
            let hash = method.hash();

            table.named_accessors.push(AccessorMethod {
                name: make_table_accessor_name(builtin_ty, method),
                index,
                lazy_key: quote! {
                    crate::lazy_keys::BuiltinMethodKey {
                        variant_type: #variant_type,
                        variant_type_str: #variant_type_str,
                        method_name: #method_name_str,
                        hash: #hash,
                    }
                },
            });
        }
    }

    table.method_init_groups.push(MethodInitGroup::new(
        &builtin_class.name().godot_ty,
        None, // load_builtin_method() doesn't need a StringName for the class, as it accepts the VariantType enum.
        method_inits,
    ));
    table.class_count += 1;
}

fn make_class_method_init(
    method: &ClassMethod,
    hash: i64,
    class_var: &Ident,
    class_ty: &TyName,
) -> TokenStream {
    let class_name_str = class_ty.godot_ty.as_str();
    let method_name_str = method.godot_name();

    // Could reuse lazy key, but less code like this -> faster parsing.
    quote! {
        crate::load_class_method(
            fetch_fptr,
            string_names,
            Some(#class_var),
            #class_name_str,
            #method_name_str,
            #hash
        ),
    }
}

fn make_builtin_method_init(
    builtin: &BuiltinVariant,
    method: &BuiltinMethod,
    index: usize,
) -> TokenStream {
    let method_name_str = method.name();

    let variant_type = builtin.sys_variant_type();
    let variant_type_str = builtin.godot_original_name();

    let hash = method.hash();

    // Could reuse lazy key, but less code like this -> faster parsing.
    quote! {
        {
            let _ = #index;
            crate::load_builtin_method(
                fetch_fptr,
                string_names,
                crate::#variant_type,
                #variant_type_str,
                #method_name_str,
                #hash
            )
        },
    }
}

fn collect_variant_operators(api: &ExtensionApi) -> Vec<&Enumerator> {
    let variant_operator_enum = api
        .global_enums
        .iter()
        .find(|e| &e.name == "VariantOperator") // in JSON: "Variant.Operator"
        .expect("missing enum for VariantOperator in JSON");

    variant_operator_enum.enumerators.iter().collect()
}

fn make_opaque_type(godot_original_name: &str, size: usize) -> TokenStream {
    let name = conv::to_pascal_case(godot_original_name);
    let (first, rest) = name.split_at(1);

    // Capitalize: "int" -> "Int".
    let ident = format_ident!("Opaque{}{}", first.to_ascii_uppercase(), rest);
    quote! {
        pub type #ident = crate::opaque::Opaque<#size>;
    }
}

fn make_variant_fns(api: &ExtensionApi, builtin: &BuiltinVariant) -> (TokenStream, TokenStream) {
    let (special_decls, special_inits);
    if let Some(builtin_class) = builtin.associated_builtin_class() {
        let (construct_decls, construct_inits) =
            make_construct_fns(api, builtin, &builtin_class.constructors);

        let (destroy_decls, destroy_inits) =
            make_destroy_fns(builtin, builtin_class.has_destructor);

        let (op_eq_decls, op_eq_inits) =
            make_operator_fns(builtin, &builtin_class.operators, "==", "EQUAL");

        let (op_lt_decls, op_lt_inits) =
            make_operator_fns(builtin, &builtin_class.operators, "<", "LESS");

        special_decls = quote! {
            #op_eq_decls
            #op_lt_decls
            #construct_decls
            #destroy_decls
        };
        special_inits = quote! {
            #op_eq_inits
            #op_lt_inits
            #construct_inits
            #destroy_inits
        };
    } else {
        special_decls = TokenStream::new();
        special_inits = TokenStream::new();
    };

    let snake_case = builtin.snake_name();
    let to_variant = format_ident!("{}_to_variant", snake_case);
    let from_variant = format_ident!("{}_from_variant", snake_case);

    let to_variant_str = to_variant.to_string();
    let from_variant_str = from_variant.to_string();

    let variant_type = builtin.sys_variant_type();
    let variant_type = quote! { crate::#variant_type };

    // Field declaration.
    // The target types are uninitialized-ptrs, because Godot performs placement new on those:
    // https://github.com/godotengine/godot/blob/b40b35fb39f0d0768d7ec2976135adffdce1b96d/core/variant/variant_internal.h#L1535-L1535

    let decl = quote! {
        pub #to_variant: unsafe extern "C" fn(GDExtensionUninitializedVariantPtr, GDExtensionTypePtr),
        pub #from_variant: unsafe extern "C" fn(GDExtensionUninitializedTypePtr, GDExtensionVariantPtr),
        #special_decls
    };

    // Field initialization in new().
    let init = quote! {
        #to_variant: {
            let fptr = unsafe { get_to_variant_fn(#variant_type) };
            crate::validate_builtin_lifecycle(fptr, #to_variant_str)
        },
        #from_variant: {
            let fptr = unsafe { get_from_variant_fn(#variant_type) };
            crate::validate_builtin_lifecycle(fptr, #from_variant_str)
        },
        #special_inits
    };

    (decl, init)
}

fn make_construct_fns(
    api: &ExtensionApi,
    builtin: &BuiltinVariant,
    constructors: &[Constructor],
) -> (TokenStream, TokenStream) {
    if constructors.is_empty() {
        return (TokenStream::new(), TokenStream::new());
    };

    // Constructor vec layout:
    //   [0]: default constructor
    //   [1]: copy constructor
    //   [2]: (optional) typically the most common conversion constructor (e.g. StringName -> String)
    //  rest: (optional) other conversion constructors and multi-arg constructors (e.g. Vector3(x, y, z))

    // Sanity checks -- ensure format is as expected.
    for (i, c) in constructors.iter().enumerate() {
        assert_eq!(i, c.index);
    }

    assert!(
        constructors[0].raw_parameters.is_empty(),
        "default constructor at index 0 must have no parameters"
    );

    let args = &constructors[1].raw_parameters;
    assert_eq!(args.len(), 1);
    assert_eq!(args[0].name, "from");
    assert_eq!(args[0].type_, builtin.godot_original_name());

    let builtin_snake_name = builtin.snake_name();
    let variant_type = builtin.sys_variant_type();

    let construct_default = format_ident!("{builtin_snake_name}_construct_default");
    let construct_copy = format_ident!("{builtin_snake_name}_construct_copy");
    let construct_default_str = construct_default.to_string();
    let construct_copy_str = construct_copy.to_string();

    let (construct_extra_decls, construct_extra_inits) =
        make_extra_constructors(api, builtin, constructors);

    // Target types are uninitialized pointers, because Godot uses placement-new for raw pointer constructions. Callstack:
    // https://github.com/godotengine/godot/blob/b40b35fb39f0d0768d7ec2976135adffdce1b96d/core/extension/gdextension_interface.cpp#L511
    // https://github.com/godotengine/godot/blob/b40b35fb39f0d0768d7ec2976135adffdce1b96d/core/variant/variant_construct.cpp#L299
    // https://github.com/godotengine/godot/blob/b40b35fb39f0d0768d7ec2976135adffdce1b96d/core/variant/variant_construct.cpp#L36
    // https://github.com/godotengine/godot/blob/b40b35fb39f0d0768d7ec2976135adffdce1b96d/core/variant/variant_construct.h#L267
    // https://github.com/godotengine/godot/blob/b40b35fb39f0d0768d7ec2976135adffdce1b96d/core/variant/variant_construct.h#L50
    let decls = quote! {
        pub #construct_default: unsafe extern "C" fn(GDExtensionUninitializedTypePtr, *const GDExtensionConstTypePtr),
        pub #construct_copy: unsafe extern "C" fn(GDExtensionUninitializedTypePtr, *const GDExtensionConstTypePtr),
        #(
            #construct_extra_decls
        )*
    };

    let inits = quote! {
        #construct_default: {
            let fptr = unsafe { get_construct_fn(crate::#variant_type, 0i32) };
            crate::validate_builtin_lifecycle(fptr, #construct_default_str)
        },
        #construct_copy: {
            let fptr = unsafe { get_construct_fn(crate::#variant_type, 1i32) };
            crate::validate_builtin_lifecycle(fptr, #construct_copy_str)
        },
        #(
            #construct_extra_inits
        )*
    };

    (decls, inits)
}

/// Lists special cases for useful constructors
fn make_extra_constructors(
    api: &ExtensionApi,
    builtin: &BuiltinVariant,
    constructors: &[Constructor],
) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let mut extra_decls = Vec::with_capacity(constructors.len() - 2);
    let mut extra_inits = Vec::with_capacity(constructors.len() - 2);
    let variant_type = builtin.sys_variant_type();

    for (i, ctor) in constructors.iter().enumerate().skip(2) {
        let args = &ctor.raw_parameters;
        assert!(
            !args.is_empty(),
            "custom constructors must have at least 1 parameter"
        );

        let type_name = builtin.snake_name();
        let construct_custom = if args.len() == 1 && args[0].name == "from" {
            // Conversion constructor is named according to the source type:
            // String(NodePath from) => string_from_node_path

            let arg_type = api.builtin_by_original_name(&args[0].type_).snake_name();
            format_ident!("{type_name}_from_{arg_type}")
        } else {
            // Type-specific constructor is named according to the argument names:
            // Vector3(float x, float y, float z) => vector3_from_x_y_z
            let mut arg_names = args
                .iter()
                .fold(String::new(), |acc, arg| acc + &arg.name + "_");
            arg_names.pop(); // remove trailing '_'
            format_ident!("{type_name}_from_{arg_names}")
        };

        let construct_custom_str = construct_custom.to_string();
        extra_decls.push(quote! {
                pub #construct_custom: unsafe extern "C" fn(GDExtensionUninitializedTypePtr, *const GDExtensionConstTypePtr),
            });

        let i = i as i32;
        extra_inits.push(quote! {
            #construct_custom: {
                let fptr = unsafe { get_construct_fn(crate::#variant_type, #i) };
                crate::validate_builtin_lifecycle(fptr, #construct_custom_str)
            },
        });
    }

    (extra_decls, extra_inits)
}

fn make_destroy_fns(builtin: &BuiltinVariant, has_destructor: bool) -> (TokenStream, TokenStream) {
    if !has_destructor {
        return (TokenStream::new(), TokenStream::new());
    }

    let destroy = format_ident!("{}_destroy", builtin.snake_name());
    let destroy_str = destroy.to_string();
    let variant_type = builtin.sys_variant_type();

    let decls = quote! {
        pub #destroy: unsafe extern "C" fn(GDExtensionTypePtr),
    };

    let inits = quote! {
        #destroy: {
            let fptr = unsafe { get_destroy_fn(crate::#variant_type) };
            crate::validate_builtin_lifecycle(fptr, #destroy_str)
        },
    };

    (decls, inits)
}

fn make_operator_fns(
    builtin: &BuiltinVariant,
    operators: &[Operator],
    json_symbol: &str,
    sys_name: &str,
) -> (TokenStream, TokenStream) {
    // If there are no operators for that builtin type, or none of the operator matches symbol, then don't generate function.
    if operators.is_empty() || !operators.iter().any(|op| op.symbol == json_symbol) {
        return (TokenStream::new(), TokenStream::new());
    }

    let operator = format_ident!(
        "{}_operator_{}",
        builtin.snake_name(),
        sys_name.to_ascii_lowercase()
    );
    let operator_str = operator.to_string();

    let variant_type = builtin.sys_variant_type();
    let variant_type = quote! { crate::#variant_type };
    let sys_ident = format_ident!("GDEXTENSION_VARIANT_OP_{}", sys_name);

    // Field declaration.
    let decl = quote! {
        pub #operator: unsafe extern "C" fn(GDExtensionConstTypePtr, GDExtensionConstTypePtr, GDExtensionTypePtr),
    };

    // Field initialization in new().
    let init = quote! {
        #operator: {
            let fptr = unsafe { get_operator_fn(crate::#sys_ident, #variant_type, #variant_type) };
            crate::validate_builtin_lifecycle(fptr, #operator_str)
        },
    };

    (decl, init)
}
