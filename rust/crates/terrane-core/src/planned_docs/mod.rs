mod document;
mod relational_db;

use terrane_cap_interface::CapabilityDoc;

pub fn all(include_internal: bool) -> Vec<CapabilityDoc> {
    vec![
        document::document_doc(include_internal),
        relational_db::relational_db_doc(include_internal),
    ]
}

pub fn get(namespace: &str, include_internal: bool) -> Option<CapabilityDoc> {
    match namespace {
        "document" => Some(document::document_doc(include_internal)),
        "relational_db" => Some(relational_db::relational_db_doc(include_internal)),
        _ => None,
    }
}
