// This crate is used as a library by asw-cli.
// See asw-cli/src/main.rs for the binary entrypoint.
fn main() {
    eprintln!("Use the asw binary instead: asw serve");
    std::process::exit(1);
}
