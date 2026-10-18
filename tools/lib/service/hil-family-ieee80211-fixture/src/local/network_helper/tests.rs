use super::*;

#[test]
fn capability_contract_rejects_stale_installations() {
    assert!(require_capabilities(&format!("{REQUIRED_CAPABILITIES}\n")).is_ok());
    assert!(require_capabilities("schema=1 client=1 managed=1\n").is_err());
    assert!(
        require_capabilities("schema=5 station_ap=he20,ht40 client=1 observer=ht40 managed=1\n")
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn repository_helper_declares_the_current_contract() {
    let helper =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../linux-net/open-radio-net");
    let output = oer_process::command("sh")
        .arg(helper)
        .arg("capabilities")
        .supervised_output()
        .unwrap();
    assert!(output.status.success());
    require_capabilities(&String::from_utf8(output.stdout).unwrap()).unwrap();
}
