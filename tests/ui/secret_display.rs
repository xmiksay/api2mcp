// I4 as a type-system property: `Secret` has no `Display` impl, so this must not compile.
fn main() {
    let secret = api2mcp::secret::Secret::load("A2M_UI_TEST_SECRET_DISPLAY").unwrap();
    println!("{}", secret);
}
