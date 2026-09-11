// I4 as a type-system property: `Secret` has no `Serialize` impl, so this must not compile.
fn main() {
    let secret = api2mcp::secret::Secret::load("A2M_UI_TEST_SECRET_SERIALIZE").unwrap();
    let _ = serde_json::to_string(&secret);
}
