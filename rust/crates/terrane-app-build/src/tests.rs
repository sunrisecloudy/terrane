use tempfile::TempDir;

use super::*;

#[test]
fn parses_nested_frontend_config() {
    let dir = fixture_app(
        r#"{
              "name": "Demo",
              "frontend": {
                "tool": "terrane-app-build",
                "entry": "src/main.tsx",
                "styles": ["src/app.css"]
              }
            }"#,
        &[
            ("src/main.tsx", "console.log('ok');"),
            ("src/app.css", "body {}"),
        ],
    );

    let config = read_build_config(dir.path()).unwrap();

    assert_eq!(config.title, "Demo");
    assert_eq!(config.entry, "src/main.tsx");
    assert_eq!(config.styles, vec!["src/app.css"]);
}

#[test]
fn rejects_missing_frontend_tool() {
    let dir = fixture_app(r#"{"name":"Demo"}"#, &[]);

    let err = read_build_config(dir.path()).unwrap_err();

    assert!(err.contains("manifest.frontend.tool"));
}

#[test]
fn swc_compiles_jsx_fragments_and_expression_jsx() {
    let module = SourceModule {
        logical_path: "src/main.jsx".into(),
        output_path: output_path_for("src/main.jsx"),
        source: r#"
                import { useMemo } from "react";
                export function App({ items }) {
                  const rows = useMemo(() => items.map((item) => <li key={item.id}>{item.text}</li>), [items]);
                  return <><ul>{rows}</ul><input disabled {...{ className: "field" }} /></>;
                }
            "#
        .into(),
    };

    let dir = fixture_app(
        r#"{
              "name": "Inline",
              "frontend": {
                "tool": "terrane-app-build",
                "entry": "src/main.jsx",
                "styles": []
              }
            }"#,
        &[("src/main.jsx", &module.source)],
    );

    let out = compile_module(dir.path(), &module).unwrap();

    assert!(out.contains("terrane-react-jsx-runtime"));
    assert!(!out.contains("\"react/jsx-runtime\""));
    assert!(out.contains("items.map"));
    assert!(!out.contains("<li"));
}

#[test]
fn swc_compiles_tsx_and_strips_type_only_imports() {
    let module = SourceModule {
        logical_path: "src/main.tsx".into(),
        output_path: output_path_for("src/main.tsx"),
        source: r#"
                import type { RemoteShape } from "external-types";
                import type { LocalShape } from "./types";
                import { useMemo } from "react";

                type Item = LocalShape & RemoteShape & { id: string; text: string };

                export function App({ items }: { items: Item[] }) {
                  const rows = useMemo(() => items.map((item) => <li key={item.id}>{item.text}</li>), [items]);
                  return <ul>{rows}</ul>;
                }
            "#
        .into(),
    };

    let dir = fixture_app(
        r#"{
              "name": "Typed",
              "frontend": {
                "tool": "terrane-app-build",
                "entry": "src/main.tsx",
                "styles": []
              }
            }"#,
        &[
            ("src/main.tsx", &module.source),
            ("src/types.ts", "export type LocalShape = { local: true };"),
        ],
    );

    let out = compile_module(dir.path(), &module).unwrap();

    assert!(out.contains("terrane-react-jsx-runtime"));
    assert!(!out.contains("external-types"));
    assert!(!out.contains("./types"));
    assert!(!out.contains("type Item"));
    assert!(!out.contains(": Item"));
    assert!(!out.contains("<li"));

    let result = build_app(BuildOptions {
        app_dir: dir.path().to_path_buf(),
        check_only: true,
    })
    .unwrap();
    let names = result
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"assets/modules/src/main.js"));
    assert!(!names.contains(&"assets/modules/src/types.js"));
}

#[test]
fn compile_script_source_compiles_typescript_without_writing_dist() {
    let out = compile_script_source(
        "main.ts",
        r#"
                type Count = number;
                const value: Count = 1;
                export const next = value + 1;
            "#,
    )
    .unwrap();

    assert!(out.contains("const value = 1"));
    assert!(out.contains("export const next = value + 1"));
    assert!(!out.contains("type Count"));
}

#[test]
fn builds_split_local_import_graph() {
    let dir = fixture_app(
        r#"{
              "name": "Split",
              "frontend": {
                "tool": "terrane-app-build",
                "entry": "src/main.tsx",
                "styles": ["src/app.css"]
              }
            }"#,
        &[
            (
                "src/main.tsx",
                r#"import { createRoot } from "react-dom/client";
                       import { App } from "./components/App";
                       import { title } from "./state";
                       createRoot(document.getElementById("root")).render(<App title={title} />);"#,
            ),
            (
                "src/components/App.tsx",
                r#"export function App({ title }: { title: string }) { return <main>{title}</main>; }"#,
            ),
            ("src/state.ts", r#"export const title: string = "BMI";"#),
            ("src/app.css", "main { color: red; }"),
        ],
    );

    let result = build_app(BuildOptions {
        app_dir: dir.path().to_path_buf(),
        check_only: true,
    })
    .unwrap();

    let names = result
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"assets/modules/src/main.js"));
    assert!(names.contains(&"assets/modules/src/components/App.js"));
    assert!(names.contains(&"assets/modules/src/state.js"));
    assert!(names.contains(&REACT_DOM_CLIENT_MODULE));
    let main = text_file(&result, "assets/modules/src/main.js");
    assert!(main.contains("./components/App.js"));
    assert!(main.contains("./state.js"));
    assert!(main.contains("../../terrane-react-dom-client.js"));
}

#[test]
fn check_mode_writes_nothing() {
    let dir = fixture_app(
        r#"{
              "name": "Check",
              "frontend": {
                "tool": "terrane-app-build",
                "entry": "src/main.jsx",
                "styles": []
              }
            }"#,
        &[("src/main.jsx", "console.log('check');")],
    );

    build_app(BuildOptions {
        app_dir: dir.path().to_path_buf(),
        check_only: true,
    })
    .unwrap();

    assert!(!dir.path().join("dist").exists());
}

#[test]
fn rejects_unsupported_bare_imports() {
    let dir = fixture_app(
        r#"{
              "name": "Bare",
              "frontend": {
                "tool": "terrane-app-build",
                "entry": "src/main.jsx",
                "styles": []
              }
            }"#,
        &[(
            "src/main.jsx",
            r#"import thing from "left-pad"; console.log(thing);"#,
        )],
    );

    let err = build_app(BuildOptions {
        app_dir: dir.path().to_path_buf(),
        check_only: true,
    })
    .unwrap_err();

    assert!(err.contains("unsupported package import"));
}

#[test]
fn rejects_dynamic_imports() {
    let dir = fixture_app(
        r#"{
              "name": "Dynamic",
              "frontend": {
                "tool": "terrane-app-build",
                "entry": "src/main.jsx",
                "styles": []
              }
            }"#,
        &[("src/main.jsx", r#"import("./chunk.js");"#)],
    );

    let err = build_app(BuildOptions {
        app_dir: dir.path().to_path_buf(),
        check_only: true,
    })
    .unwrap_err();

    assert!(err.contains("dynamic import()"));
}

fn fixture_app(manifest: &str, files: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("manifest.json"), manifest).unwrap();
    for (path, content) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    dir
}

fn text_file<'a>(result: &'a BuildResult, path: &str) -> &'a str {
    result
        .files
        .iter()
        .find(|file| file.path == path)
        .and_then(|file| std::str::from_utf8(&file.content).ok())
        .unwrap()
}
