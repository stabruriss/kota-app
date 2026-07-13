fn main() {
    if let Err(err) = kota_v2_lib::bartender::run_cli() {
        eprintln!("kota-bartender: {err}");
        std::process::exit(1);
    }
}
