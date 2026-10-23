//! tests/cases/password.json, which seimei/auth.py passes too: which new passwords `seimei passwd` takes.

use seimei_core::auth::password_problem;
use serde_json::Value;

#[test]
fn password_cases() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/cases/password.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for c in doc["cases"].as_array().unwrap() {
        let pw = c["password"].as_str().unwrap();
        assert_eq!(password_problem(pw).is_none(), c["ok"].as_bool().unwrap(), "{pw:?}");
    }
}
