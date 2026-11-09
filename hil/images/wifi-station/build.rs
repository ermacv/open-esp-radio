//! Measurement only: the network composition the image builds, by
//! `OER_MEASURE_NETWORK` (`direct-copying`, `direct-lending`,
//! `queued-copying`, `queued-lending`); unset, the image's own
//! (`queued-lending`).

fn main() {
    println!("cargo::rerun-if-env-changed=OER_MEASURE_NETWORK");
    println!("cargo::rustc-check-cfg=cfg(oer_measure_direct)");
    println!("cargo::rustc-check-cfg=cfg(oer_measure_copying)");
    let (direct, copying) = match std::env::var("OER_MEASURE_NETWORK").as_deref() {
        Err(_) | Ok("queued-lending") => (false, false),
        Ok("queued-copying") => (false, true),
        Ok("direct-lending") => (true, false),
        Ok("direct-copying") => (true, true),
        Ok(other) => panic!("OER_MEASURE_NETWORK=`{other}` names no composition"),
    };
    if direct {
        println!("cargo::rustc-cfg=oer_measure_direct");
    }
    if copying {
        println!("cargo::rustc-cfg=oer_measure_copying");
    }
}
