mod document;

use terrane_cap_interface::CapabilityDoc;

pub fn all(include_internal: bool) -> Vec<CapabilityDoc> {
    vec![document::document_doc(include_internal)]
}

pub fn get(namespace: &str, include_internal: bool) -> Option<CapabilityDoc> {
    match namespace {
        "document" => Some(document::document_doc(include_internal)),
        _ => None,
    }
}
