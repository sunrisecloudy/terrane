//! Parse + bookend passes: SWC parse, the static-import rejection that gates the
//! whole scan, and the comment/string-aware surface-text backstop that catches
//! the highest-severity code-eval spellings a parser could normalise away.

use forge_domain::{CoreError, Result};
use swc_core::common::{sync::Lrc, FileName, SourceMap};
use swc_core::ecma::ast::{EsVersion, ModuleItem};
use swc_core::ecma::parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};

use super::models::ScanFinding;

pub(crate) fn parse(src: &str) -> Result<swc_core::ecma::ast::Module> {
    let cm: Lrc<SourceMap> = Lrc::default();
    let fm = cm.new_source_file(Lrc::new(FileName::Custom("scan.ts".into())), src.to_string());
    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax { tsx: false, ..Default::default() }),
        EsVersion::Es2022,
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    parser
        .parse_module()
        .map_err(|e| crate::parse_error(e.kind().msg()))
}

/// Reject any static `import ... from ...` (review 010 P2). Type-only imports
/// (`import type {...}`) are erased before runtime and carry no runtime
/// specifier, so they are *not* rejected.
pub(crate) fn reject_static_imports(body: &[ModuleItem]) -> Result<()> {
    use swc_core::ecma::ast::ModuleDecl;
    for item in body {
        if let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item {
            if import.type_only {
                continue;
            }
            return Err(CoreError::ValidationError(
                "static imports not supported in M0a; use a single entry module".to_string(),
            ));
        }
    }
    Ok(())
}

/// Comment- and string-aware surface-text backstop for the highest-severity
/// code-eval spellings, matched at an identifier boundary so a benign name like
/// `retrieval(` or `myFunction` is never flagged. The AST pass is authoritative
/// and catches all of these already on parseable input; this is
/// belt-and-suspenders for any edge the parser could normalise away.
///
/// Review 010 P3: the backstop must NOT fire inside string literals or comments
/// — `const msg = "eval("` and `// Function(` must pass. We therefore scan a
/// *masked* copy of the source where the bytes of every line/block comment and
/// every string/template literal are blanked out, so only real code text is
/// matched. Adds a finding only if not already reported by the AST walk.
pub(crate) fn text_backstop(src: &str, findings: &mut Vec<ScanFinding>) {
    let code = mask_comments_and_strings(src);
    let checks: &[(&str, &str, &str)] = &[
        // (identifier, construct, reason)
        ("eval", "eval", "arbitrary code evaluation is forbidden"),
        (
            "Function",
            "Function constructor",
            "constructing code from strings is forbidden",
        ),
        (
            "XMLHttpRequest",
            "XMLHttpRequest",
            "raw network is forbidden; use the ctx.* host API",
        ),
    ];
    for (ident, construct, reason) in checks {
        if contains_ident_call(&code, ident) {
            let f = ScanFinding::new(construct, reason);
            if !findings.contains(&f) {
                findings.push(f);
            }
        }
    }
}

/// Replace the bytes of every comment (line `//…`, block `/*…*/`) and every
/// string/template literal with spaces, preserving length and newlines, so a
/// text scan over the result only sees real code. Best-effort lexing that is
/// good enough for the narrow backstop (the AST pass is authoritative).
pub(crate) fn mask_comments_and_strings(src: &str) -> String {
    #[derive(PartialEq)]
    enum St {
        Code,
        Line,           // // comment
        Block,          // /* */ comment
        Str(char),      // '...' or "..."
        Template,       // `...`
    }
    let bytes = src.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut st = St::Code;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let next = bytes.get(i + 1).copied();
        match st {
            St::Code => {
                if b == b'/' && next == Some(b'/') {
                    st = St::Line;
                    out.push(b' ');
                    out.push(b' ');
                    i += 2;
                    continue;
                }
                if b == b'/' && next == Some(b'*') {
                    st = St::Block;
                    out.push(b' ');
                    out.push(b' ');
                    i += 2;
                    continue;
                }
                if b == b'\'' || b == b'"' {
                    st = St::Str(b as char);
                    out.push(b' ');
                    i += 1;
                    continue;
                }
                if b == b'`' {
                    st = St::Template;
                    out.push(b' ');
                    i += 1;
                    continue;
                }
                out.push(b);
            }
            St::Line => {
                if b == b'\n' {
                    st = St::Code;
                    out.push(b'\n');
                } else {
                    out.push(if b.is_ascii_whitespace() { b } else { b' ' });
                }
            }
            St::Block => {
                if b == b'*' && next == Some(b'/') {
                    st = St::Code;
                    out.push(b' ');
                    out.push(b' ');
                    i += 2;
                    continue;
                }
                out.push(if b.is_ascii_whitespace() { b } else { b' ' });
            }
            St::Str(q) => {
                if b == b'\\' {
                    // Skip the escaped char too.
                    out.push(b' ');
                    if next.is_some() {
                        out.push(b' ');
                        i += 2;
                        continue;
                    }
                } else if b as char == q {
                    st = St::Code;
                    out.push(b' ');
                } else {
                    out.push(if b == b'\n' { b'\n' } else { b' ' });
                }
            }
            St::Template => {
                // Note: template `${...}` substitutions are masked too. The AST
                // pass remains authoritative for code inside substitutions.
                if b == b'\\' {
                    out.push(b' ');
                    if next.is_some() {
                        out.push(b' ');
                        i += 2;
                        continue;
                    }
                } else if b == b'`' {
                    st = St::Code;
                    out.push(b' ');
                } else {
                    out.push(if b == b'\n' { b'\n' } else { b' ' });
                }
            }
        }
        i += 1;
    }
    // The masked buffer is byte-for-byte length-preserving over ASCII control
    // characters; any multi-byte UTF-8 in code text is preserved as-is, and in
    // masked regions each byte became a single space, which is still valid
    // UTF-8. So `from_utf8` cannot fail, but fall back defensively.
    String::from_utf8(out).unwrap_or_else(|_| src.to_string())
}

/// True if `src` contains `ident` as a whole identifier (not a substring of a
/// longer name) used as a constructor/callee — i.e. immediately preceded by a
/// non-identifier char and followed (after optional whitespace) by `(`.
pub(crate) fn contains_ident_call(src: &str, ident: &str) -> bool {
    let bytes = src.as_bytes();
    let mut from = 0;
    while let Some(rel) = src[from..].find(ident) {
        let start = from + rel;
        let end = start + ident.len();
        let prev_ok = start == 0 || !is_ident_byte(bytes[start - 1]);
        // The match must not be a prefix of a longer identifier (e.g. `evalX`).
        let boundary_after = end >= bytes.len() || !is_ident_byte(bytes[end]);
        // After the identifier, skip whitespace, then require an opening paren.
        let mut i = end;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let next_ok = i < bytes.len() && bytes[i] == b'(';
        if prev_ok && boundary_after && next_ok {
            return true;
        }
        from = end;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}
