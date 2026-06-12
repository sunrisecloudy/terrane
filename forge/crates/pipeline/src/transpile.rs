//! TypeScript -> ES-module JS type-strip (prd-merged/01 CR-14).
//!
//! Pure, offline, deterministic. We parse with SWC's TS parser, run the
//! `resolver` + `strip` passes (the type-erasure transform) under a fresh
//! `GLOBALS`, and emit JS. No bundling/minification; output is a 1:1
//! type-stripped module.

use crate::{parse_error, Result};
use swc_core::common::{sync::Lrc, FileName, Globals, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::{EsVersion, Program};
use swc_core::ecma::codegen::{text_writer::JsWriter, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::transforms::typescript::strip;

/// Strip TypeScript types and return `(js_code, source_map)`.
///
/// `source_map` is `None` in M0a — the return shape keeps the seam so a later
/// milestone can wire SWC's source-map output without an API change.
pub(crate) fn strip_types(ts: &str) -> Result<(String, Option<String>)> {
    let cm: Lrc<SourceMap> = Lrc::default();
    let fm = cm.new_source_file(Lrc::new(FileName::Custom("applet.ts".into())), ts.to_string());

    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax { tsx: false, ..Default::default() }),
        EsVersion::Es2022,
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);

    // A fatal parse error (cannot build a module at all).
    let module = parser.parse_module().map_err(|e| parse_error(e.kind().msg()))?;
    // Recoverable errors the parser accumulated (e.g. partial syntax). Strict:
    // any recoverable error means the source is not a valid applet module.
    let errs = parser.take_errors();
    if let Some(first) = errs.first() {
        return Err(parse_error(first.kind().msg()));
    }

    let mut program = Program::Module(module);
    // SWC's resolver/strip use thread-local syntax-context state guarded by
    // GLOBALS; a fresh `Globals` per call keeps transpiles independent and the
    // output deterministic (marks are allocated from a clean counter each time).
    GLOBALS.set(&Globals::new(), || {
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        program.mutate(resolver(unresolved_mark, top_level_mark, true));
        program.mutate(strip(unresolved_mark, top_level_mark));
    });

    let mut buf = Vec::new();
    {
        let writer = JsWriter::new(cm.clone(), "\n", &mut buf, None);
        let mut emitter =
            Emitter { cfg: Default::default(), cm: cm.clone(), comments: None, wr: writer };
        emitter
            .emit_program(&program)
            .map_err(|e| parse_error(format!("codegen: {e}")))?;
    }

    let js_code = String::from_utf8(buf)
        .map_err(|e| parse_error(format!("non-utf8 codegen output: {e}")))?;
    Ok((js_code, None))
}
