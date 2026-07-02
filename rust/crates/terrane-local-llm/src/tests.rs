//! Unit tests for the subprocess-output parsers. Engine behaviour with real
//! weights lives in `tests/engine.rs` (`#[ignore]`d).

use crate::mlx::{extract_json_object, parse_generation_tokens};

#[test]
fn generation_tokens_parse_from_mlx_stats() {
    let stats = "\nPrompt: 23 tokens, 25.278 tokens-per-sec\n\
                 Generation: 64 tokens, 410.321 tokens-per-sec\n\
                 Peak memory: 0.514 GB\n";
    assert_eq!(parse_generation_tokens(stats), Some(64));
    assert_eq!(parse_generation_tokens("no stats here"), None);
}

#[test]
fn json_object_extraction_survives_prose_and_fences() {
    let wrapped = "Thinking about it...\n```json\n{\"answer\": \"Paris\"}\n```\nHope that helps!";
    assert_eq!(
        extract_json_object(wrapped).as_deref(),
        Some("{\"answer\": \"Paris\"}")
    );

    assert_eq!(extract_json_object("no json at all"), None);
    assert_eq!(extract_json_object("{broken"), None);
    // An array is not the requested object shape.
    assert_eq!(extract_json_object("[1, 2]"), None);
}
