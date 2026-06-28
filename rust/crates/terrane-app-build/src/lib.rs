use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Component, Path, PathBuf};

use nanoserde::DeJson;
use swc_core::common::{
    comments::SingleThreadedComments, sync::Lrc, FileName, Mark, SourceMap, GLOBALS,
};
use swc_core::ecma::ast::*;
use swc_core::ecma::codegen::to_code_default;
use swc_core::ecma::parser::{EsSyntax, Parser, StringInput, Syntax, TsSyntax};
use swc_core::ecma::transforms::base::{fixer::fixer, hygiene::hygiene, resolver};
use swc_core::ecma::transforms::react::{jsx, Options as ReactOptions, Runtime};
use swc_core::ecma::transforms::typescript::strip;
use swc_core::ecma::visit::{Visit, VisitMut, VisitMutWith, VisitWith};

const REACT_JS: &str = include_str!("../vendor/react/react.production.min.js");
const REACT_DOM_JS: &str = include_str!("../vendor/react/react-dom.production.min.js");
const REACT_LICENSE: &str = include_str!("../vendor/react/LICENSE.react.txt");
const REACT_DOM_LICENSE: &str = include_str!("../vendor/react/LICENSE.react-dom.txt");

const REACT_MODULE: &str = "assets/terrane-react.js";
const REACT_DOM_MODULE: &str = "assets/terrane-react-dom.js";
const REACT_DOM_CLIENT_MODULE: &str = "assets/terrane-react-dom-client.js";
const REACT_JSX_RUNTIME_MODULE: &str = "assets/terrane-react-jsx-runtime.js";

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub app_dir: PathBuf,
    pub check_only: bool,
}

#[derive(Debug, Clone)]
pub struct BuildResult {
    pub dist: PathBuf,
    pub files: Vec<BuiltFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltFile {
    pub path: String,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone, Default, DeJson)]
struct Manifest {
    #[nserde(default)]
    name: String,
    #[nserde(default)]
    frontend: Frontend,
}

#[derive(Debug, Clone, Default, DeJson)]
struct Frontend {
    #[nserde(default)]
    tool: String,
    #[nserde(default)]
    entry: String,
    #[nserde(default)]
    styles: Vec<String>,
}

#[derive(Debug, Clone)]
struct BuildConfig {
    app_dir: PathBuf,
    title: String,
    entry: String,
    styles: Vec<String>,
}

#[derive(Debug, Clone)]
struct SourceModule {
    logical_path: String,
    output_path: String,
    source: String,
}

#[derive(Debug, Clone)]
struct CompiledModule {
    output_path: String,
    code: String,
}

pub fn build_app(options: BuildOptions) -> Result<BuildResult, String> {
    let config = read_build_config(&options.app_dir)?;
    let mut compiler = ModuleCompiler::new(options.app_dir.clone());
    let modules = compiler.compile_entry(&config.entry)?;
    let files = render_output(&config, modules)?;
    let dist = options.app_dir.join("dist");

    if !options.check_only {
        write_dist(&dist, &files)?;
    }

    Ok(BuildResult { dist, files })
}

/// Compile one JS/TS/JSX/TSX module source string without reading or writing an
/// app directory. This is the sandbox helper exposed to generated backend code
/// through `ctx.resource.build.compileTs(path, source)`.
pub fn compile_script_source(logical_path: &str, source: &str) -> Result<String, String> {
    let logical_path = normalize_manifest_path("path", logical_path)?;
    if !is_supported_script(&logical_path) {
        return Err(format!(
            "path must be a .js, .jsx, .mjs, .ts, or .tsx file: {logical_path}"
        ));
    }
    let module = SourceModule {
        output_path: output_path_for(&logical_path),
        logical_path,
        source: source.to_string(),
    };
    let parsed = parse_module(&module)?;
    reject_unsupported_syntax(&module.logical_path, &parsed)?;
    compile_module(Path::new("."), &module)
}

fn read_build_config(app_dir: &Path) -> Result<BuildConfig, String> {
    let manifest_path = app_dir.join("manifest.json");
    let raw = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("read {}: {e}", manifest_path.display()))?;
    let manifest = Manifest::deserialize_json(&raw)
        .map_err(|e| format!("parse {}: {e}", manifest_path.display()))?;

    if manifest.frontend.tool.trim() != "terrane-app-build" {
        return Err("manifest.frontend.tool must be \"terrane-app-build\"".to_string());
    }
    let entry = normalize_manifest_path("manifest.frontend.entry", &manifest.frontend.entry)?;
    if !is_supported_script(&entry) {
        return Err(format!(
            "manifest.frontend.entry must be a .js, .jsx, .mjs, .ts, or .tsx file: {entry}"
        ));
    }

    let mut styles = Vec::new();
    for style in manifest.frontend.styles {
        let style = normalize_manifest_path("manifest.frontend.styles[]", &style)?;
        if !style.ends_with(".css") {
            return Err(format!("stylesheet must be a .css file: {style}"));
        }
        let path = app_dir.join(&style);
        fs::metadata(&path).map_err(|e| format!("read stylesheet {}: {e}", path.display()))?;
        styles.push(style);
    }

    Ok(BuildConfig {
        app_dir: app_dir.to_path_buf(),
        title: non_empty_or(manifest.name, "Terrane App"),
        entry,
        styles,
    })
}

struct ModuleCompiler {
    app_dir: PathBuf,
    seen: BTreeMap<String, SourceModule>,
}

impl ModuleCompiler {
    fn new(app_dir: PathBuf) -> Self {
        Self {
            app_dir,
            seen: BTreeMap::new(),
        }
    }

    fn compile_entry(&mut self, entry: &str) -> Result<Vec<CompiledModule>, String> {
        let mut queue = VecDeque::from([entry.to_string()]);
        while let Some(logical_path) = queue.pop_front() {
            if self.seen.contains_key(&logical_path) {
                continue;
            }
            let module = self.read_source_module(&logical_path)?;
            let parsed = parse_module(&module)?;
            reject_unsupported_syntax(&module.logical_path, &parsed)?;
            for specifier in collect_static_specifiers(&parsed)? {
                match resolve_specifier(&self.app_dir, &logical_path, &specifier)? {
                    ResolvedSpecifier::Local(dep) => queue.push_back(dep),
                    ResolvedSpecifier::External(_) => {}
                }
            }
            self.seen.insert(logical_path, module);
        }

        let mut used_outputs = BTreeMap::<String, String>::new();
        for module in self.seen.values() {
            if let Some(previous) =
                used_outputs.insert(module.output_path.clone(), module.logical_path.clone())
            {
                return Err(format!(
                    "module output collision: {} and {} both emit {}",
                    previous, module.logical_path, module.output_path
                ));
            }
        }

        let mut compiled = Vec::new();
        for module in self.seen.values() {
            compiled.push(CompiledModule {
                output_path: module.output_path.clone(),
                code: compile_module(&self.app_dir, module)?,
            });
        }
        Ok(compiled)
    }

    fn read_source_module(&self, logical_path: &str) -> Result<SourceModule, String> {
        let path = self.app_dir.join(logical_path);
        let source = fs::read_to_string(&path)
            .map_err(|e| format!("read frontend module {}: {e}", path.display()))?;
        Ok(SourceModule {
            logical_path: logical_path.to_string(),
            output_path: output_path_for(logical_path),
            source,
        })
    }
}

fn parse_module(module: &SourceModule) -> Result<Module, String> {
    let cm: Lrc<SourceMap> = Default::default();
    let comments = SingleThreadedComments::default();
    let file = cm.new_source_file(
        FileName::Real(PathBuf::from(&module.logical_path)).into(),
        module.source.clone(),
    );
    let syntax = syntax_for_path(&module.logical_path);
    let mut parser = Parser::new(syntax, StringInput::from(&*file), Some(&comments));
    let parsed = parser.parse_module().map_err(|e| {
        format!(
            "parse frontend module {}: {:?}",
            module.logical_path,
            e.into_kind()
        )
    })?;

    let parser_errors = parser.take_errors();
    if !parser_errors.is_empty() {
        return Err(format!(
            "parse frontend module {}: {:?}",
            module.logical_path, parser_errors
        ));
    }
    Ok(parsed)
}

fn compile_module(app_dir: &Path, module: &SourceModule) -> Result<String, String> {
    let cm: Lrc<SourceMap> = Default::default();
    let comments = SingleThreadedComments::default();
    let file = cm.new_source_file(
        FileName::Real(PathBuf::from(&module.logical_path)).into(),
        module.source.clone(),
    );
    let syntax = syntax_for_path(&module.logical_path);
    let mut parser = Parser::new(syntax, StringInput::from(&*file), Some(&comments));
    let parsed = parser.parse_module().map_err(|e| {
        format!(
            "parse frontend module {}: {:?}",
            module.logical_path,
            e.into_kind()
        )
    })?;
    let parser_errors = parser.take_errors();
    if !parser_errors.is_empty() {
        return Err(format!(
            "parse frontend module {}: {:?}",
            module.logical_path, parser_errors
        ));
    }

    GLOBALS.set(&Default::default(), || {
        let top_level_mark = Mark::new();
        let unresolved_mark = Mark::new();
        let mut program = Program::Module(parsed);
        program.visit_mut_with(&mut resolver(unresolved_mark, top_level_mark, false));
        if is_typescript_script(&module.logical_path) {
            program.mutate(strip(unresolved_mark, top_level_mark));
        }
        program.visit_mut_with(&mut jsx(
            cm.clone(),
            Some(&comments),
            ReactOptions {
                runtime: Some(Runtime::Automatic),
                import_source: Some("react".into()),
                development: Some(false),
                ..Default::default()
            },
            top_level_mark,
            unresolved_mark,
        ));
        if let Program::Module(module_ast) = &mut program {
            let mut rewriter = ImportRewriter {
                app_dir: app_dir.to_path_buf(),
                from_logical: module.logical_path.clone(),
                from_output: module.output_path.clone(),
                errors: Vec::new(),
            };
            module_ast.visit_mut_with(&mut rewriter);
            if !rewriter.errors.is_empty() {
                return Err(format!(
                    "rewrite imports in {}: {}",
                    module.logical_path,
                    rewriter.errors.join("; ")
                ));
            }
        }
        program.visit_mut_with(&mut hygiene());
        program.visit_mut_with(&mut fixer(Some(&comments)));

        match program {
            Program::Module(module) => Ok(to_code_default(cm, Some(&comments), &module)),
            Program::Script(_) => Err("internal error: module transform emitted a script".into()),
        }
    })
}

fn collect_static_specifiers(module: &Module) -> Result<Vec<String>, String> {
    let mut specifiers = Vec::new();
    for item in &module.body {
        match item {
            ModuleItem::ModuleDecl(ModuleDecl::Import(import)) if !import.type_only => {
                specifiers.push(str_lit_value(&import.src)?);
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportNamed(export)) if !export.type_only => {
                if let Some(src) = &export.src {
                    specifiers.push(str_lit_value(src)?);
                }
            }
            ModuleItem::ModuleDecl(ModuleDecl::ExportAll(export)) if !export.type_only => {
                specifiers.push(str_lit_value(&export.src)?);
            }
            _ => {}
        }
    }
    Ok(specifiers)
}

fn reject_unsupported_syntax(logical_path: &str, module: &Module) -> Result<(), String> {
    let mut visitor = UnsupportedSyntax::default();
    module.visit_with(&mut visitor);
    if visitor.errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "unsupported syntax in {logical_path}: {}",
            visitor.errors.join("; ")
        ))
    }
}

#[derive(Default)]
struct UnsupportedSyntax {
    errors: Vec<String>,
}

impl Visit for UnsupportedSyntax {
    fn visit_call_expr(&mut self, node: &CallExpr) {
        if matches!(node.callee, Callee::Import(_)) {
            self.errors
                .push("dynamic import() is not supported by terrane-app-build yet".to_string());
        }
        node.visit_children_with(self);
    }
}

fn str_lit_value(value: &Str) -> Result<String, String> {
    value
        .value
        .as_str()
        .map(ToString::to_string)
        .ok_or_else(|| "module specifier must be valid UTF-8".to_string())
}

struct ImportRewriter {
    app_dir: PathBuf,
    from_logical: String,
    from_output: String,
    errors: Vec<String>,
}

impl VisitMut for ImportRewriter {
    fn visit_mut_import_decl(&mut self, import: &mut ImportDecl) {
        match str_lit_value(&import.src).and_then(|raw| {
            rewrite_specifier(&self.app_dir, &self.from_logical, &self.from_output, &raw)
        }) {
            Ok(next) => {
                import.src.value = next.into();
                import.src.raw = None;
            }
            Err(e) => self.errors.push(e),
        }
    }

    fn visit_mut_named_export(&mut self, export: &mut NamedExport) {
        if let Some(src) = &mut export.src {
            match str_lit_value(src).and_then(|raw| {
                rewrite_specifier(&self.app_dir, &self.from_logical, &self.from_output, &raw)
            }) {
                Ok(next) => {
                    src.value = next.into();
                    src.raw = None;
                }
                Err(e) => self.errors.push(e),
            }
        }
    }

    fn visit_mut_export_all(&mut self, export: &mut ExportAll) {
        match str_lit_value(&export.src).and_then(|raw| {
            rewrite_specifier(&self.app_dir, &self.from_logical, &self.from_output, &raw)
        }) {
            Ok(next) => {
                export.src.value = next.into();
                export.src.raw = None;
            }
            Err(e) => self.errors.push(e),
        }
    }
}

fn rewrite_specifier(
    app_dir: &Path,
    from_logical: &str,
    from_output: &str,
    raw: &str,
) -> Result<String, String> {
    match resolve_specifier(app_dir, from_logical, raw)? {
        ResolvedSpecifier::External(external) => {
            Ok(relative_url(from_output, external.module_path()))
        }
        ResolvedSpecifier::Local(logical) => {
            Ok(relative_url(from_output, &output_path_for(&logical)))
        }
    }
}

enum ResolvedSpecifier {
    Local(String),
    External(ExternalModule),
}

enum ExternalModule {
    React,
    ReactDom,
    ReactDomClient,
    ReactJsxRuntime,
}

impl ExternalModule {
    fn module_path(&self) -> &'static str {
        match self {
            Self::React => REACT_MODULE,
            Self::ReactDom => REACT_DOM_MODULE,
            Self::ReactDomClient => REACT_DOM_CLIENT_MODULE,
            Self::ReactJsxRuntime => REACT_JSX_RUNTIME_MODULE,
        }
    }
}

fn resolve_specifier(
    app_dir: &Path,
    from_logical: &str,
    raw: &str,
) -> Result<ResolvedSpecifier, String> {
    match raw {
        "react" => Ok(ResolvedSpecifier::External(ExternalModule::React)),
        "react-dom" => Ok(ResolvedSpecifier::External(ExternalModule::ReactDom)),
        "react-dom/client" => Ok(ResolvedSpecifier::External(ExternalModule::ReactDomClient)),
        "react/jsx-runtime" => Ok(ResolvedSpecifier::External(ExternalModule::ReactJsxRuntime)),
        spec if spec.starts_with("./") || spec.starts_with("../") => {
            let base = Path::new(from_logical)
                .parent()
                .unwrap_or_else(|| Path::new(""));
            Ok(ResolvedSpecifier::Local(resolve_relative_module(
                app_dir, base, spec,
            )?))
        }
        spec if spec.starts_with('/') => Err(format!("absolute import is not allowed: {spec}")),
        spec => Err(format!(
            "unsupported package import {spec:?}; first real terrane-app-build supports local relative imports plus react/react-dom externals"
        )),
    }
}

fn resolve_relative_module(app_dir: &Path, base: &Path, spec: &str) -> Result<String, String> {
    let raw = normalize_joined_path(base, Path::new(spec))?;
    let candidates = if extension(&raw).is_some() {
        vec![raw]
    } else {
        vec![
            format!("{raw}.tsx"),
            format!("{raw}.ts"),
            format!("{raw}.jsx"),
            format!("{raw}.js"),
            format!("{raw}.mjs"),
            format!("{raw}/index.tsx"),
            format!("{raw}/index.ts"),
            format!("{raw}/index.jsx"),
            format!("{raw}/index.js"),
            format!("{raw}/index.mjs"),
        ]
    };

    for candidate in candidates {
        if is_supported_script(&candidate) && app_dir.join(&candidate).is_file() {
            return Ok(candidate);
        }
    }
    Err(format!("cannot resolve local module import: {spec}"))
}

fn render_output(
    config: &BuildConfig,
    modules: Vec<CompiledModule>,
) -> Result<Vec<BuiltFile>, String> {
    let mut files = Vec::new();
    files.push(BuiltFile {
        path: "assets/react.production.min.js".to_string(),
        content: REACT_JS.as_bytes().to_vec(),
    });
    files.push(BuiltFile {
        path: "assets/react-dom.production.min.js".to_string(),
        content: REACT_DOM_JS.as_bytes().to_vec(),
    });
    files.push(BuiltFile {
        path: "assets/react-licenses.txt".to_string(),
        content: format!("React:\n{REACT_LICENSE}\n\nReact DOM:\n{REACT_DOM_LICENSE}").into_bytes(),
    });
    files.push(BuiltFile {
        path: REACT_MODULE.to_string(),
        content: react_wrapper().into_bytes(),
    });
    files.push(BuiltFile {
        path: REACT_DOM_MODULE.to_string(),
        content: react_dom_wrapper().into_bytes(),
    });
    files.push(BuiltFile {
        path: REACT_DOM_CLIENT_MODULE.to_string(),
        content: react_dom_client_wrapper().into_bytes(),
    });
    files.push(BuiltFile {
        path: REACT_JSX_RUNTIME_MODULE.to_string(),
        content: react_jsx_runtime_wrapper().into_bytes(),
    });

    let mut stylesheet_links = String::new();
    let mut copied_styles = BTreeSet::new();
    for (index, style) in config.styles.iter().enumerate() {
        let output = if config.styles.len() == 1 {
            "assets/app.css".to_string()
        } else {
            format!("assets/app-{index}.css")
        };
        if !copied_styles.insert(output.clone()) {
            return Err(format!("duplicate stylesheet output: {output}"));
        }
        stylesheet_links.push_str(&format!(
            "<link rel=\"stylesheet\" href=\"{}\">\n",
            html_attr(&output)
        ));
        files.push(BuiltFile {
            path: output,
            content: fs::read(config.app_dir.join(style))
                .map_err(|e| format!("read stylesheet {style}: {e}"))?,
        });
    }

    for module in modules {
        files.push(BuiltFile {
            path: module.output_path,
            content: module.code.into_bytes(),
        });
    }

    let entry_output = output_path_for(&config.entry);
    files.push(BuiltFile {
        path: "index.html".to_string(),
        content: format!(
            "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{}</title>\n{}</head>\n<body>\n<div id=\"root\"></div>\n<script src=\"assets/react.production.min.js\"></script>\n<script src=\"assets/react-dom.production.min.js\"></script>\n<script type=\"module\" src=\"{}\"></script>\n</body>\n</html>\n",
            html_text(&config.title),
            stylesheet_links,
            html_attr(&entry_output)
        )
        .into_bytes(),
    });

    Ok(files)
}

fn write_dist(dist: &Path, files: &[BuiltFile]) -> Result<(), String> {
    if dist.exists() {
        fs::remove_dir_all(dist).map_err(|e| format!("clear {}: {e}", dist.display()))?;
    }
    for file in files {
        validate_generated_path(&file.path)?;
        let path = dist.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        fs::write(&path, &file.content).map_err(|e| format!("write {}: {e}", path.display()))?;
    }
    Ok(())
}

fn validate_generated_path(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(format!("unsafe generated path: {}", path.display()));
    }
    Ok(())
}

fn output_path_for(logical_path: &str) -> String {
    let mut path = PathBuf::from("assets/modules");
    path.push(logical_path);
    path.set_extension("js");
    slash_path(&path)
}

fn relative_url(from_output: &str, target_output: &str) -> String {
    let from_parent = Path::new(from_output)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let from_parts = normal_components(from_parent);
    let target_parts = normal_components(Path::new(target_output));
    let common = from_parts
        .iter()
        .zip(&target_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut out = Vec::new();
    for _ in common..from_parts.len() {
        out.push("..".to_string());
    }
    out.extend(target_parts[common..].iter().cloned());
    let joined = out.join("/");
    if joined.starts_with('.') {
        joined
    } else {
        format!("./{joined}")
    }
}

fn normalize_manifest_path(label: &str, path: &str) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(format!("missing {label}"));
    }
    let normalized = normalize_rel_path(trimmed)?;
    if normalized.split('/').any(|part| part == "..") {
        return Err(format!("{label} must not contain parent-dir components"));
    }
    Ok(normalized)
}

fn normalize_joined_path(base: &Path, rel: &Path) -> Result<String, String> {
    if rel.is_absolute() {
        return Err(format!("absolute path is not allowed: {}", rel.display()));
    }
    let mut stack = normal_components(base);
    for component in rel.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => stack.push(part.to_string_lossy().into_owned()),
            Component::ParentDir => {
                if stack.pop().is_none() {
                    return Err(format!("path escapes app root: {}", rel.display()));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("absolute path is not allowed: {}", rel.display()));
            }
        }
    }
    Ok(stack.join("/"))
}

fn normalize_rel_path(path: &str) -> Result<String, String> {
    let raw = Path::new(path);
    if raw.is_absolute() {
        return Err(format!("absolute paths are not allowed: {path}"));
    }
    let mut parts = Vec::new();
    for component in raw.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::ParentDir => parts.push("..".to_string()),
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("absolute paths are not allowed: {path}"));
            }
        }
    }
    if parts.is_empty() {
        return Err("path must not be empty".to_string());
    }
    Ok(parts.join("/"))
}

fn normal_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn extension(path: &str) -> Option<&str> {
    Path::new(path).extension().and_then(|ext| ext.to_str())
}

fn is_supported_script(path: &str) -> bool {
    matches!(extension(path), Some("js" | "jsx" | "mjs" | "ts" | "tsx"))
}

fn is_typescript_script(path: &str) -> bool {
    matches!(extension(path), Some("ts" | "tsx"))
}

fn syntax_for_path(path: &str) -> Syntax {
    match extension(path) {
        Some("ts") => Syntax::Typescript(TsSyntax {
            tsx: false,
            disallow_ambiguous_jsx_like: true,
            ..Default::default()
        }),
        Some("tsx") => Syntax::Typescript(TsSyntax {
            tsx: true,
            ..Default::default()
        }),
        _ => Syntax::Es(EsSyntax {
            jsx: true,
            ..Default::default()
        }),
    }
}

fn slash_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn non_empty_or(input: String, fallback: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.to_string()
    }
}

fn html_text(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn html_attr(input: &str) -> String {
    html_text(input).replace('"', "&quot;")
}

fn react_wrapper() -> String {
    [
        "const ReactGlobal = window.React;",
        "if (!ReactGlobal) throw new Error('Terrane React runtime is missing');",
        "export default ReactGlobal;",
        "export const Children = ReactGlobal.Children;",
        "export const Component = ReactGlobal.Component;",
        "export const Fragment = ReactGlobal.Fragment;",
        "export const Profiler = ReactGlobal.Profiler;",
        "export const PureComponent = ReactGlobal.PureComponent;",
        "export const StrictMode = ReactGlobal.StrictMode;",
        "export const Suspense = ReactGlobal.Suspense;",
        "export const cloneElement = ReactGlobal.cloneElement;",
        "export const createContext = ReactGlobal.createContext;",
        "export const createElement = ReactGlobal.createElement;",
        "export const createFactory = ReactGlobal.createFactory;",
        "export const createRef = ReactGlobal.createRef;",
        "export const forwardRef = ReactGlobal.forwardRef;",
        "export const isValidElement = ReactGlobal.isValidElement;",
        "export const lazy = ReactGlobal.lazy;",
        "export const memo = ReactGlobal.memo;",
        "export const startTransition = ReactGlobal.startTransition;",
        "export const useCallback = ReactGlobal.useCallback;",
        "export const useContext = ReactGlobal.useContext;",
        "export const useDebugValue = ReactGlobal.useDebugValue;",
        "export const useDeferredValue = ReactGlobal.useDeferredValue;",
        "export const useEffect = ReactGlobal.useEffect;",
        "export const useId = ReactGlobal.useId;",
        "export const useImperativeHandle = ReactGlobal.useImperativeHandle;",
        "export const useInsertionEffect = ReactGlobal.useInsertionEffect;",
        "export const useLayoutEffect = ReactGlobal.useLayoutEffect;",
        "export const useMemo = ReactGlobal.useMemo;",
        "export const useReducer = ReactGlobal.useReducer;",
        "export const useRef = ReactGlobal.useRef;",
        "export const useState = ReactGlobal.useState;",
        "export const useSyncExternalStore = ReactGlobal.useSyncExternalStore;",
        "export const useTransition = ReactGlobal.useTransition;",
        "",
    ]
    .join("\n")
}

fn react_dom_wrapper() -> String {
    [
        "const ReactDOMGlobal = window.ReactDOM;",
        "if (!ReactDOMGlobal) throw new Error('Terrane ReactDOM runtime is missing');",
        "export default ReactDOMGlobal;",
        "export const createPortal = ReactDOMGlobal.createPortal;",
        "export const createRoot = ReactDOMGlobal.createRoot;",
        "export const findDOMNode = ReactDOMGlobal.findDOMNode;",
        "export const flushSync = ReactDOMGlobal.flushSync;",
        "export const hydrate = ReactDOMGlobal.hydrate;",
        "export const hydrateRoot = ReactDOMGlobal.hydrateRoot;",
        "export const render = ReactDOMGlobal.render;",
        "export const unmountComponentAtNode = ReactDOMGlobal.unmountComponentAtNode;",
        "export const unstable_batchedUpdates = ReactDOMGlobal.unstable_batchedUpdates;",
        "",
    ]
    .join("\n")
}

fn react_dom_client_wrapper() -> String {
    [
        "const ReactDOMGlobal = window.ReactDOM;",
        "if (!ReactDOMGlobal) throw new Error('Terrane ReactDOM runtime is missing');",
        "export default ReactDOMGlobal;",
        "export const createRoot = ReactDOMGlobal.createRoot;",
        "export const hydrateRoot = ReactDOMGlobal.hydrateRoot;",
        "",
    ]
    .join("\n")
}

fn react_jsx_runtime_wrapper() -> String {
    [
        "const ReactGlobal = window.React;",
        "if (!ReactGlobal) throw new Error('Terrane React runtime is missing');",
        "export const Fragment = ReactGlobal.Fragment;",
        "export function jsx(type, props, key) { return ReactGlobal.createElement(type, key == null ? props : Object.assign({}, props, { key })); }",
        "export const jsxs = jsx;",
        "export const jsxDEV = jsx;",
        "",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests;
