use terrane_cap_interface::CapabilityDoc;

pub fn all(include_internal: bool) -> Vec<CapabilityDoc> {
    let _ = include_internal;
    Vec::new()
}

pub fn get(namespace: &str, include_internal: bool) -> Option<CapabilityDoc> {
    let _ = namespace;
    let _ = include_internal;
    None
}
