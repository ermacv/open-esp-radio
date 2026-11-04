extern crate std;

#[allow(
    unsafe_code,
    reason = "the local macro invocation claims its private static"
)]
mod declarations {
    crate::declare_installations!(pub first);
    crate::declare_installations!(pub second);
}

#[test]
fn each_nominal_static_declaration_can_be_taken_exactly_once() {
    let mut first = declarations::first::take_static::<()>().expect("first take");
    let mut second = declarations::second::take_static::<()>().expect("independent declaration");
    assert!(declarations::first::take_static::<()>().is_none());
    assert!(declarations::second::take_static::<()>().is_none());
    assert!(
        first.first().is_some(),
        "the authority mints its first installation"
    );
    assert!(first.first().is_none(), "and only once");
    assert!(second.first().is_some(), "each declaration mints its own");
    assert!(declarations::second::take_static::<()>().is_none());
}
