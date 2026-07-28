fn main() {
    for variable in [
        "OIKADE_BUILD_VERSION",
        "OIKADE_BUILD_COMMIT",
        "OIKADE_BUILD_DATE",
    ] {
        println!("cargo:rerun-if-env-changed={variable}");
    }
}
