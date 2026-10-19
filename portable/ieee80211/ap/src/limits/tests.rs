use super::*;

#[test]
fn client_limit_rejects_zero_and_values_above_the_owned_tables() {
    assert_eq!(AccessPointClientLimit::new(0).unwrap_err().value(), 0,);
    assert_eq!(AccessPointClientLimit::new(16).unwrap_err().value(), 16,);
    assert_eq!(AccessPointClientLimit::new(15).unwrap().get(), 15);
}
