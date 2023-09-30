use std::collections::HashMap;

use anyhow::{bail, Result};
use indexmap::IndexMap;
use turbo_tasks::{
    graph::{GraphTraversal, NonDeterministic},
    TryFlatJoinIterExt, Value, ValueToString, Vc,
};
use turbopack_binding::{
    swc::core::{
        css::modules::imports,
        ecma::{
            ast::{
                CallExpr, Callee, Expr, Ident, ImportDefaultSpecifier, Lit, Program, Prop,
                PropName, PropOrSpread,
            },
            visit::{Visit, VisitWith},
        },
    },
    turbo::tasks_fs::FileSystemPath,
    turbopack::{
        core::{
            chunk::{
                ChunkData, ChunkableModule, ChunkingContext, ChunksData, EvaluatableAsset,
                EvaluatableAssets,
            },
            issue::{IssueSeverity, OptionIssueSource},
            module::Module,
            output::{OutputAsset, OutputAssets},
            reference::primary_referenced_modules,
            reference_type::EcmaScriptModulesReferenceSubType,
            resolve::{origin::PlainResolveOrigin, parse::Request, pattern::Pattern},
        },
        ecmascript::{
            chunk::{EcmascriptChunkPlaceable, EcmascriptChunkingContext},
            parse::ParseResult,
            resolve::esm_resolve,
            EcmascriptModuleAsset,
        },
    },
};

pub(crate) async fn create_react_lodable_manifest(
    entry: Vc<Box<dyn EcmascriptChunkPlaceable>>,
    client_chunking_context: Vc<Box<dyn EcmascriptChunkingContext>>,
    output_root: Vc<FileSystemPath>,
) -> Result<()> {
    /*
    let all_actions = NonDeterministic::new()
        .skip_duplicates()
        .visit([Vc::upcast(entry)], get_referenced_modules)
        .await
        .completed()?
        .into_inner()
        .into_iter()
        .map(parse_actions_filter_map)
        .try_flat_join()
        .await?
        .into_iter()
        .collect::<IndexMap<_, _>>(); */

    // Traverse referenced modules graph, and collect all of the dynamic imports:
    // - Read the Program AST of the Module, this is the origin (A)
    //  - If it contains `dynamic(import(B))`, then B is the module that is being
    //    imported
    // Returned import mappings are in the form of
    // Vec<(A, (B, Module<B>))> (where B is the raw import source string, and
    // Module<B> is the resolved Module)
    let import_mappings = NonDeterministic::new()
        .skip_duplicates()
        .visit([Vc::upcast(entry)], get_referenced_modules)
        .await
        .completed()?
        .into_inner()
        .into_iter()
        .map(collect_dynamic_imports)
        .try_flat_join()
        .await?
        .into_iter()
        .collect::<Vec<_>>();

    /*
    let mut chunks_hash: HashMap<String, Vec<(String, Vc<Box<dyn Module>>)>> = HashMap::new();

    import_mappings
        .into_iter()
        .for_each(|DynamicImportsMap((_, import_sources))| {
            for (import, _module) in &import_sources {
                if !chunks_hash.contains_key(import) {
                    chunks_hash.insert(import.to_string(), vec![]);
                }
            }
        });



    for module in i {
        let ident = module.ident().to_string().await?;
        let r = &module.ident().path().await?.to_string();

        let dynamic_imported_module = chunks_hash.keys().find(|module_path| {
            println!("{:#?} == {:#?}", module_path, r);
            module_path.eq(&r)
        });

        //println!("================= {:#?}", dynamic_imported_module);
        if let Some(dynamic_imported_module) = dynamic_imported_module {
            println!("================= {:#?}", r);
        }
    } */

    /*
    // Traverse all of the referenced modules, then find out corresponding module
    // imported by dynamic import.
    // This'll be the entry to the chunk for the actual manifest.
    let referenced_modules = NonDeterministic::new()
        .skip_duplicates()
        .visit([Vc::upcast(entry)], get_referenced_modules)
        .await
        .completed()?
        .into_inner()
        .into_iter();
    */

    for import_mapping in import_mappings {
        let DynamicImportsMap((module, import_sources)) = import_mapping;

        for (import, module) in import_sources {
            let Some(module) =
                Vc::try_resolve_sidecast::<Box<dyn EvaluatableAsset>>(module).await?
            else {
                bail!("module must be evaluatable");
            };

            let evaluated = client_chunking_context.evaluated_chunk_group(
                entry.as_root_chunk(Vc::upcast(client_chunking_context)),
                EvaluatableAssets::one(module),
            );

            let evaluated = &*evaluated.await?;
            for ee in evaluated {
                //.next/server/edge/chunks/edge
                // Why edge/chunks/edge?
                // entry.as_root_chunk maybe the culprit, should have real root?
                println!("evaluated {:#?}", ee.ident().path().await?);
            }

            let chunks_data =
                ChunkData::from_assets(output_root.clone(), Vc::cell(evaluated.clone()));

            //TODOHEREHERE: No CHUNK DATA
            let d = &*chunks_data.await?;
            println!("d: {:#?}", d);
            for dd in d {
                let ddd = dd.await?;
                println!(
                    "================= import: {:#?} -> chunk: {:#?}",
                    import, ddd.path
                );
            }

            /*
            for chunk in &*evaluated {
                let chunk_data = ChunkData::from_asset(entry.ident().path(), chunk.clone());
                let x = &*chunk_data.await?;

                if let Some(x) = x {
                    let ide = chunk.ident().path();
                    println!(
                        "================= import: {:#?} -> chunk: {:#?}",
                        import,
                        x.await?.path
                    );
                } else {
                    println!("================= import: {:#?} -> chunk: {:#?}", import, x);
                }
            } */
        }
    }

    Ok(())
}

async fn get_referenced_modules(
    parent: Vc<Box<dyn Module>>,
) -> Result<impl Iterator<Item = Vc<Box<dyn Module>>> + Send> {
    primary_referenced_modules(parent)
        .await
        .map(|modules| modules.clone_value().into_iter())
}

#[turbo_tasks::function]
async fn parse_imports(module: Vc<Box<dyn Module>>) -> Result<Vc<OptionDynamicImportsMap>> {
    let Some(ecmascript_asset) =
        Vc::try_resolve_downcast_type::<EcmascriptModuleAsset>(module).await?
    else {
        return Ok(OptionDynamicImportsMap::none());
    };

    let id = &*module.ident().to_string().await?;
    let ParseResult::Ok { program, .. } = &*ecmascript_asset.parse().await? else {
        bail!("failed to parse module '{id}'");
    };

    // Reading the program, collect to the import wrapped in dynamic import
    // id for the manifest is composed as
    // `{module.id} -> {import_source}`
    let mut visitor = LodableImportVisitor::new();
    program.visit_with(&mut visitor);

    if visitor.import_sources.is_empty() {
        return Ok(OptionDynamicImportsMap::none());
    }

    println!("import_sources {:#?}", visitor.import_sources);

    let mut import_sources = vec![];

    // Using the given `Module` which is the origin of the dynamic import, trying to
    // resolve the module that is being imported.
    for import in visitor.import_sources.drain(..) {
        let resolved = *esm_resolve(
            Vc::upcast(PlainResolveOrigin::new(
                ecmascript_asset.await?.asset_context,
                module.ident().path(),
            )),
            Request::parse(Value::new(Pattern::Constant(import.to_string()))),
            Value::new(EcmaScriptModulesReferenceSubType::Undefined),
            OptionIssueSource::none(),
            IssueSeverity::Error.cell(),
        )
        .first_module()
        .await?;

        if let Some(resolved_mod) = resolved {
            import_sources.push((import, resolved_mod));
        }
    }

    Ok(Vc::cell(Some(Vc::cell((module.clone(), import_sources)))))
}

struct LodableImportVisitor {
    //in_dynamic_call: bool,
    dynamic_ident: Option<Ident>,
    pub import_sources: Vec<String>,
}

impl LodableImportVisitor {
    fn new() -> Self {
        Self {
            //in_dynamic_call: false,
            import_sources: vec![],
            dynamic_ident: None,
        }
    }
}

struct CollectImportSourceVisitor {
    import_source: Option<String>,
}

impl CollectImportSourceVisitor {
    fn new() -> Self {
        Self {
            import_source: None,
        }
    }
}

impl Visit for CollectImportSourceVisitor {
    fn visit_call_expr(&mut self, call_expr: &CallExpr) {
        // find import source from import('path/to/module')
        // [NOTE]: Turbopack does not support webpack-specific comment directives, i.e
        // import(/* webpackChunkName: 'hello1' */ '../../components/hello3')
        // Renamed chunk in the comment will be ignored.
        if let Callee::Import(import) = call_expr.callee {
            if let Some(arg) = call_expr.args.first() {
                if let Expr::Lit(lit) = &*arg.expr {
                    if let Lit::Str(str_) = &lit {
                        self.import_source = Some(str_.value.to_string());
                    }
                }
            }
        }

        // Don't need to visit children, we expect import() won't have any
        // nested calls as dynamic() should be statically analyzable import.
    }
}

impl Visit for LodableImportVisitor {
    fn visit_import_decl(&mut self, decl: &turbopack_binding::swc::core::ecma::ast::ImportDecl) {
        // find import decl from next/dynamic, i.e import dynamic from 'next/dynamic'
        if decl.src.value == *"next/dynamic" {
            if let Some(specifier) = decl.specifiers.first().map(|s| s.as_default()).flatten() {
                self.dynamic_ident = Some(specifier.local.clone());
            }
        }
    }

    fn visit_call_expr(&mut self, call_expr: &CallExpr) {
        // Collect imports if the import call is wrapped in the call dynamic()
        if let Callee::Expr(ident) = &call_expr.callee {
            if let Expr::Ident(ident) = &**ident {
                if let Some(dynamic_ident) = &self.dynamic_ident {
                    if ident.sym == *dynamic_ident.sym {
                        let mut collect_import_source_visitor = CollectImportSourceVisitor::new();
                        call_expr.visit_children_with(&mut collect_import_source_visitor);

                        if let Some(import_source) = collect_import_source_visitor.import_source {
                            self.import_sources.push(import_source);
                        }
                    }
                }
            }
        }

        call_expr.visit_children_with(self);
    }
}

async fn collect_dynamic_imports(module: Vc<Box<dyn Module>>) -> Result<Option<DynamicImportsMap>> {
    let imports = parse_imports(module).await?;

    if let Some(imports) = &*imports {
        Ok(Some(DynamicImportsMap(imports.await?.clone_value())))
    } else {
        Ok(None)
    }
    /*
    parse_imports(module).await.map(|option_action_map| {
        option_action_map
            .clone_value()
            .map(|action_map| (module, action_map))
    }) */
}

pub type ActionsMap = HashMap<String, String>;

#[turbo_tasks::value(transparent)]
struct ActionMap(IndexMap<String, String>);

//Intermidiate struct contains dynamic import source and its corresponding
// module
#[turbo_tasks::value(transparent)]
struct DynamicImportsMap((Vc<Box<dyn Module>>, Vec<(String, Vc<Box<dyn Module>>)>));

/// An Option wrapper around [DynamicImportsMap].
#[turbo_tasks::value(transparent)]
struct OptionDynamicImportsMap(Option<Vc<DynamicImportsMap>>);

#[turbo_tasks::value_impl]
impl OptionDynamicImportsMap {
    #[turbo_tasks::function]
    pub fn none() -> Vc<Self> {
        Vc::cell(None)
    }
}

#[turbo_tasks::value(transparent)]
struct ModuleActionMap(IndexMap<Vc<Box<dyn Module>>, Vc<ActionMap>>);

#[turbo_tasks::value_impl]
impl ModuleActionMap {
    #[turbo_tasks::function]
    pub fn empty() -> Vc<Self> {
        Vc::cell(IndexMap::new())
    }
}

#[turbo_tasks::value(shared)]
pub struct ManifestLoaderAsset {}

#[turbo_tasks::value_impl]
impl ManifestLoaderAsset {
    #[turbo_tasks::function]
    pub fn new() -> Vc<Self> {
        Self {}.cell()
    }

    #[turbo_tasks::function]
    async fn get_asset_chunks(self: Vc<Self>) -> Result<Vc<OutputAssets>> {
        panic!("")
    }
}
