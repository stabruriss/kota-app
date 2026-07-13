fn main() {
    if let Err(err) = kota_v2_lib::ember::run_cli() {
        eprintln!("kota-ember: {err}");
        std::process::exit(1);
    }
}
